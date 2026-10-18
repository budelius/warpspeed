use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
#[cfg(feature = "local_fs")]
use std::sync::OnceLock;
use std::time::SystemTime;

#[cfg(feature = "local_fs")]
use anyhow::bail;
use anyhow::{Context, Result};
#[cfg(feature = "local_fs")]
use instant::Instant;
use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_bytes_repr::ByteFmtSerializer;
use sha2::{Digest, Sha256};

use crate::ai::blocklist::SerializedBlockListItem;
#[cfg(feature = "local_fs")]
use crate::app_state::LeafSnapshot;
use crate::app_state::{
    CliAgentResumeDescriptor, LeafContents, PaneNodeSnapshot, PaneUuid, TabSnapshot,
    TerminalPaneSnapshot, WindowSnapshot,
};
use crate::launch_configs::launch_config::LaunchConfigWindowSource;
use crate::terminal::CLIAgent;
use crate::terminal::model::block::MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES;

const LEGACY_SESSION_FORMAT_VERSION: u32 = 1;
const SESSION_FORMAT_VERSION: u32 = 2;
const CATALOG_FORMAT_VERSION: u32 = 1;
const MAX_PANE_STYLIZED_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_WINDOW_STYLIZED_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESTORED_BLOCKS_PER_PANE: usize = 2_048;
const MAX_RESTORED_BLOCKS_PER_WINDOW: usize = 4_096;
const MAX_STORED_PANE_BLOCK_LISTS: usize = 256;
const MAX_CHECKPOINT_FILE_BYTES: u64 = 512 * 1024 * 1024;
const CATALOG_FILE_NAME: &str = "stored-sessions.json";
const HISTORY_DIRECTORY_NAME: &str = "history";

#[derive(Serialize, Deserialize)]
struct StoredPaneBlocks {
    uuid: Vec<u8>,
    #[serde(deserialize_with = "deserialize_pane_blocks")]
    blocks: Vec<SerializedBlockListItem>,
}

#[derive(Serialize, Deserialize)]
struct StoredWindowSession {
    version: u32,
    source: LaunchConfigWindowSource,
    #[serde(default)]
    profile: Option<String>,
    window: WindowSnapshot,
    #[serde(default, deserialize_with = "deserialize_window_block_lists")]
    block_lists: Vec<StoredPaneBlocks>,
    #[serde(default)]
    saved_at_unix_seconds: i64,
}

pub(crate) struct RestoredWindowSession {
    pub window: WindowSnapshot,
    pub block_lists: Arc<HashMap<PaneUuid, Vec<SerializedBlockListItem>>>,
}

#[derive(Default)]
struct CheckpointBudgetStats {
    original_output_bytes: usize,
    retained_output_bytes: usize,
    truncated_blocks: usize,
}

fn command_block_mut(
    item: &mut SerializedBlockListItem,
) -> &mut crate::terminal::model::block::SerializedBlock {
    match item {
        SerializedBlockListItem::Command { block } => block,
    }
}

fn enforce_block_list_output_budget(blocks: &mut [SerializedBlockListItem], max_bytes: usize) {
    let mut remaining = max_bytes;
    for item in blocks.iter_mut().rev() {
        let block = command_block_mut(item);
        block.enforce_command_budget();
        block.enforce_output_budget(remaining.min(MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES));
        remaining = remaining.saturating_sub(block.stylized_output.len());
    }
}

struct PaneBlocksVisitor;

impl<'de> Visitor<'de> for PaneBlocksVisitor {
    type Value = Vec<SerializedBlockListItem>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a sequence of serialized terminal blocks")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut blocks = VecDeque::new();
        let mut retained_bytes = 0usize;
        let mut oldest_output_index = 0usize;
        while let Some(mut item) = sequence.next_element()? {
            let block = command_block_mut(&mut item);
            block.enforce_command_budget();
            block.enforce_output_budget(MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES);
            retained_bytes = retained_bytes.saturating_add(block.stylized_output.len());
            blocks.push_back(item);

            if blocks.len() > MAX_RESTORED_BLOCKS_PER_PANE
                && let Some(removed) = blocks.pop_front()
            {
                let SerializedBlockListItem::Command { block } = removed;
                retained_bytes = retained_bytes.saturating_sub(block.stylized_output.len());
                oldest_output_index = oldest_output_index.saturating_sub(1);
            }

            while retained_bytes > MAX_PANE_STYLIZED_OUTPUT_BYTES
                && oldest_output_index < blocks.len()
            {
                let block = command_block_mut(&mut blocks[oldest_output_index]);
                let previous_len = block.stylized_output.len();
                let overflow = retained_bytes - MAX_PANE_STYLIZED_OUTPUT_BYTES;
                block.enforce_output_budget(previous_len.saturating_sub(overflow));
                retained_bytes = retained_bytes
                    .saturating_sub(previous_len)
                    .saturating_add(block.stylized_output.len());
                if block.stylized_output.is_empty()
                    || retained_bytes > MAX_PANE_STYLIZED_OUTPUT_BYTES
                {
                    oldest_output_index += 1;
                }
            }
        }
        Ok(blocks.into())
    }
}

fn deserialize_pane_blocks<'de, D>(
    deserializer: D,
) -> Result<Vec<SerializedBlockListItem>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_seq(PaneBlocksVisitor)
}

struct WindowBlockListsVisitor;

impl<'de> Visitor<'de> for WindowBlockListsVisitor {
    type Value = Vec<StoredPaneBlocks>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a sequence of stored terminal pane blocks")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut block_lists = Vec::new();
        while let Some(pane) = sequence.next_element()? {
            block_lists.push(pane);
            enforce_checkpoint_budgets(&mut block_lists);
        }
        Ok(block_lists)
    }
}

fn deserialize_window_block_lists<'de, D>(
    deserializer: D,
) -> Result<Vec<StoredPaneBlocks>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_seq(WindowBlockListsVisitor)
}

fn enforce_checkpoint_budgets(block_lists: &mut Vec<StoredPaneBlocks>) -> CheckpointBudgetStats {
    for pane in block_lists.iter_mut() {
        if pane.blocks.len() > MAX_RESTORED_BLOCKS_PER_PANE {
            pane.blocks
                .drain(..pane.blocks.len() - MAX_RESTORED_BLOCKS_PER_PANE);
        }
    }

    if block_lists.len() > MAX_STORED_PANE_BLOCK_LISTS {
        block_lists.sort_by_key(|pane| {
            std::cmp::Reverse(
                pane.blocks
                    .iter()
                    .filter_map(SerializedBlockListItem::start_ts)
                    .max(),
            )
        });
        block_lists.truncate(MAX_STORED_PANE_BLOCK_LISTS);
    }

    let block_count = block_lists
        .iter()
        .map(|pane| pane.blocks.len())
        .sum::<usize>();
    if block_count > MAX_RESTORED_BLOCKS_PER_WINDOW {
        let mut locations = block_lists
            .iter()
            .enumerate()
            .flat_map(|(pane_index, pane)| {
                pane.blocks
                    .iter()
                    .enumerate()
                    .map(move |(block_index, item)| (pane_index, block_index, item.start_ts()))
            })
            .collect::<Vec<_>>();
        locations.sort_by_key(|(pane_index, block_index, timestamp)| {
            std::cmp::Reverse((*timestamp, *pane_index, *block_index))
        });
        let retained = locations
            .into_iter()
            .take(MAX_RESTORED_BLOCKS_PER_WINDOW)
            .map(|(pane_index, block_index, _)| (pane_index, block_index))
            .collect::<HashSet<_>>();
        for (pane_index, pane) in block_lists.iter_mut().enumerate() {
            let mut block_index = 0usize;
            pane.blocks.retain(|_| {
                let keep = retained.contains(&(pane_index, block_index));
                block_index += 1;
                keep
            });
        }
    }

    for pane in block_lists.iter_mut() {
        enforce_block_list_output_budget(&mut pane.blocks, MAX_PANE_STYLIZED_OUTPUT_BYTES);
    }

    let retained_after_pane_limits = block_lists
        .iter()
        .flat_map(|pane| &pane.blocks)
        .map(|item| match item {
            SerializedBlockListItem::Command { block } => block.stylized_output.len(),
        })
        .sum::<usize>();
    if retained_after_pane_limits > MAX_WINDOW_STYLIZED_OUTPUT_BYTES {
        let mut locations = block_lists
            .iter()
            .enumerate()
            .flat_map(|(pane_index, pane)| {
                pane.blocks
                    .iter()
                    .enumerate()
                    .map(move |(block_index, item)| {
                        (
                            pane_index,
                            block_index,
                            item.start_ts()
                                .map(|timestamp| timestamp.timestamp_millis()),
                        )
                    })
            })
            .collect::<Vec<_>>();
        locations.sort_by_key(|(_, _, timestamp)| std::cmp::Reverse(*timestamp));

        let mut remaining = MAX_WINDOW_STYLIZED_OUTPUT_BYTES;
        for (pane_index, block_index, _) in locations {
            let block = command_block_mut(&mut block_lists[pane_index].blocks[block_index]);
            block.enforce_output_budget(remaining);
            remaining = remaining.saturating_sub(block.stylized_output.len());
        }
    }

    block_lists.iter().flat_map(|pane| &pane.blocks).fold(
        CheckpointBudgetStats::default(),
        |mut stats, item| {
            let SerializedBlockListItem::Command { block } = item;
            stats.original_output_bytes = stats.original_output_bytes.saturating_add(
                block
                    .stylized_output_original_bytes
                    .unwrap_or(block.stylized_output.len()),
            );
            stats.retained_output_bytes = stats
                .retained_output_bytes
                .saturating_add(block.stylized_output.len());
            stats.truncated_blocks += usize::from(block.stylized_output_truncated);
            stats
        },
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredAgentSummary {
    pub agent: CLIAgent,
    pub session_id: Option<String>,
    pub resume_command: Option<String>,
    pub title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredPaneSummary {
    pub pane_number: usize,
    pub pane_uuid: String,
    pub cwd: Option<String>,
    pub shell: Option<String>,
    pub agent: Option<StoredAgentSummary>,
    pub has_persistent_process: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredTabSummary {
    pub tab_index: usize,
    pub title: Option<String>,
    pub panes: Vec<StoredPaneSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredWindowSummary {
    source: LaunchConfigWindowSource,
    checkpoint_path: PathBuf,
    #[serde(default)]
    profile: Option<String>,
    saved_at_unix_seconds: i64,
    history_id: Option<String>,
    tabs: Vec<StoredTabSummary>,
}

impl StoredWindowSummary {
    pub fn window_index(&self) -> usize {
        self.source.window_index
    }

    pub fn tabs(&self) -> &[StoredTabSummary] {
        &self.tabs
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredSessionSummary {
    id: String,
    name: String,
    saved_at_unix_seconds: i64,
    is_current: bool,
    profile: Option<String>,
    windows: Vec<StoredWindowSummary>,
}

impl StoredSessionSummary {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn saved_at_unix_seconds(&self) -> i64 {
        self.saved_at_unix_seconds
    }

    pub(crate) fn is_current(&self) -> bool {
        self.is_current
    }

    pub(crate) fn windows(&self) -> &[StoredWindowSummary] {
        &self.windows
    }

    pub(crate) fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    pub(crate) fn source_path(&self) -> Option<&Path> {
        self.first_source().map(|source| source.path.as_path())
    }

    pub(crate) fn tabs(&self) -> impl Iterator<Item = &StoredTabSummary> {
        self.windows.iter().flat_map(|window| &window.tabs)
    }

    pub(crate) fn pane_count(&self) -> usize {
        self.tabs().map(|tab| tab.panes.len()).sum()
    }

    pub(crate) fn agent_count(&self) -> usize {
        self.tabs()
            .flat_map(|tab| &tab.panes)
            .filter(|pane| pane.agent.is_some())
            .count()
    }

    pub(crate) fn searchable_text(&self) -> String {
        let mut values = vec![self.name.clone()];
        if let Some(profile) = &self.profile {
            values.push(profile.clone());
        }
        if let Some(source_path) = self.source_path() {
            values.push(source_path.to_string_lossy().into_owned());
        }
        for tab in self.tabs() {
            if let Some(title) = &tab.title {
                values.push(title.clone());
            }
            for pane in &tab.panes {
                if let Some(cwd) = &pane.cwd {
                    values.push(cwd.clone());
                }
                if let Some(shell) = &pane.shell {
                    values.push(shell.clone());
                }
                if let Some(agent) = &pane.agent {
                    values.push(agent.agent.display_name().to_owned());
                    if let Some(title) = &agent.title {
                        values.push(title.clone());
                    }
                }
            }
        }
        values.join(" ")
    }

    fn first_source(&self) -> Option<&LaunchConfigWindowSource> {
        self.windows.first().map(|window| &window.source)
    }

    pub(crate) fn contains_source(&self, source: &LaunchConfigWindowSource) -> bool {
        self.windows.iter().any(|window| &window.source == source)
    }
}

#[derive(Serialize, Deserialize)]
struct StoredSessionCatalog {
    version: u32,
    windows: Vec<StoredWindowSummary>,
}

pub(crate) fn sessions_dir() -> PathBuf {
    warp_core::paths::state_dir().join("session_config_sessions")
}

fn catalog_path(dir: &Path) -> PathBuf {
    dir.join(CATALOG_FILE_NAME)
}

pub(crate) fn catalog_revision() -> Option<SystemTime> {
    #[cfg(feature = "local_fs")]
    {
        std::fs::metadata(catalog_path(&sessions_dir()))
            .and_then(|metadata| metadata.modified())
            .ok()
    }
    #[cfg(not(feature = "local_fs"))]
    {
        None
    }
}

fn source_hash(source: &LaunchConfigWindowSource, include_window_index: bool) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.path.to_string_lossy().as_bytes());
    hasher.update(source.document_index.to_le_bytes());
    if include_window_index {
        hasher.update(source.window_index.to_le_bytes());
    }
    hex::encode(hasher.finalize())
}

fn session_file_name(source: &LaunchConfigWindowSource) -> String {
    format!("{}.json", source_hash(source, true))
}

fn session_path(dir: &Path, source: &LaunchConfigWindowSource) -> PathBuf {
    match (source.session_id(), source.session_generation()) {
        (Some(session_id), Some(generation)) => dir
            .join(session_id)
            .join(generation.to_string())
            .join(format!("window-{}.json", source.window_index)),
        _ => dir.join(session_file_name(source)),
    }
}

fn history_path(dir: &Path, source: &LaunchConfigWindowSource, history_id: &str) -> PathBuf {
    dir.join(HISTORY_DIRECTORY_NAME)
        .join(source_hash(source, false))
        .join(history_id)
        .join(format!("window-{}.json", source.window_index))
}

fn sources_match(stored: &LaunchConfigWindowSource, expected: &LaunchConfigWindowSource) -> bool {
    if stored.is_session_config() && expected.is_session_config() {
        stored.session_id() == expected.session_id()
            && stored.session_generation() == expected.session_generation()
            && stored.window_index == expected.window_index
    } else {
        stored == expected
    }
}

fn collect_panes(node: &PaneNodeSnapshot, panes: &mut Vec<StoredPaneSummary>) {
    match node {
        PaneNodeSnapshot::Branch(branch) => {
            for (_, child) in &branch.children {
                collect_panes(child, panes);
            }
        }
        PaneNodeSnapshot::Leaf(leaf) => {
            let LeafContents::Terminal(terminal) = &leaf.contents else {
                return;
            };
            panes.push(pane_summary(panes.len() + 1, terminal));
        }
    }
}

#[cfg(feature = "local_fs")]
fn unidentified_agents(node: &mut PaneNodeSnapshot) -> Vec<&mut CliAgentResumeDescriptor> {
    match node {
        PaneNodeSnapshot::Branch(branch) => branch
            .children
            .iter_mut()
            .flat_map(|(_, child)| unidentified_agents(child))
            .collect(),
        PaneNodeSnapshot::Leaf(leaf) => match &mut leaf.contents {
            LeafContents::Terminal(terminal) => terminal
                .cli_agent_resume_descriptor
                .as_mut()
                .filter(|descriptor| descriptor.session_id.is_none())
                .into_iter()
                .collect(),
            _ => Vec::new(),
        },
    }
}

#[cfg(feature = "local_fs")]
fn identified_agent_session_ids(node: &PaneNodeSnapshot, ids: &mut HashSet<String>) {
    match node {
        PaneNodeSnapshot::Branch(branch) => {
            for (_, child) in &branch.children {
                identified_agent_session_ids(child, ids);
            }
        }
        PaneNodeSnapshot::Leaf(leaf) => {
            if let LeafContents::Terminal(terminal) = &leaf.contents
                && let Some(session_id) = terminal
                    .cli_agent_resume_descriptor
                    .as_ref()
                    .and_then(|descriptor| descriptor.session_id.clone())
            {
                ids.insert(session_id);
            }
        }
    }
}

/// Fills in the session of each agent pane whose snapshot predates session IDs being recorded,
/// from the agents' own conversation history, so that reopening the window resumes them.
#[cfg(feature = "local_fs")]
fn identify_agent_sessions(window: &mut WindowSnapshot, saved_at_unix_seconds: i64) {
    use crate::terminal::agent_session_history::{UnidentifiedAgent, find_all, match_for_restore};

    let mut excluded = crate::terminal::cli_agent_sessions::process_registry::running_session_ids();
    for tab in &window.tabs {
        identified_agent_session_ids(&tab.root, &mut excluded);
    }
    let mut descriptors = window
        .tabs
        .iter_mut()
        .flat_map(|tab| unidentified_agents(&mut tab.root))
        .collect::<Vec<_>>();
    if descriptors.is_empty() {
        return;
    }
    let wanted = descriptors
        .iter()
        .map(|descriptor| UnidentifiedAgent {
            agent: descriptor.agent,
            cwd: descriptor.cwd.clone(),
            title: descriptor.title.clone(),
        })
        .collect::<Vec<_>>();
    let mut agents = wanted.iter().map(|wanted| wanted.agent).collect::<Vec<_>>();
    agents.sort_by_key(|agent| agent.display_name());
    agents.dedup();
    let candidates = agents.into_iter().flat_map(find_all).collect::<Vec<_>>();
    let saved_at = SystemTime::UNIX_EPOCH
        + std::time::Duration::from_secs(u64::try_from(saved_at_unix_seconds).unwrap_or(0));
    for (descriptor, session_id) in
        descriptors
            .iter_mut()
            .zip(match_for_restore(&wanted, saved_at, &candidates, &excluded))
    {
        if let Some(session_id) = session_id {
            log::info!(
                "Resuming {} in {} as session {session_id}, matched from its history",
                descriptor.agent.display_name(),
                descriptor.cwd.display()
            );
            descriptor.session_id = Some(session_id);
        }
    }
}

fn pane_summary(pane_number: usize, terminal: &TerminalPaneSnapshot) -> StoredPaneSummary {
    let descriptor = terminal.cli_agent_resume_descriptor.as_ref();
    StoredPaneSummary {
        pane_number,
        pane_uuid: hex::encode(&terminal.uuid),
        cwd: descriptor
            .map(|descriptor| descriptor.cwd.to_string_lossy().into_owned())
            .or_else(|| terminal.cwd.clone()),
        shell: terminal
            .shell_launch_data
            .as_ref()
            .map(|shell| shell.shell_detail()),
        agent: descriptor.map(|descriptor| StoredAgentSummary {
            agent: descriptor.agent,
            session_id: descriptor.session_id.clone(),
            resume_command: descriptor.resume_command(),
            title: descriptor.title.clone(),
        }),
        has_persistent_process: terminal.persistent_session_id.is_some(),
    }
}

fn tab_summary(tab_index: usize, tab: &TabSnapshot) -> StoredTabSummary {
    let mut panes = Vec::new();
    collect_panes(&tab.root, &mut panes);
    StoredTabSummary {
        tab_index,
        title: tab.custom_title.clone(),
        panes,
    }
}

fn window_summary(
    dir: &Path,
    checkpoint_path: &Path,
    stored: &StoredWindowSession,
    history_id: Option<String>,
) -> Result<StoredWindowSummary> {
    let checkpoint_path = checkpoint_path
        .strip_prefix(dir)
        .context("Stored session checkpoint is outside its session directory")?
        .to_path_buf();
    Ok(StoredWindowSummary {
        source: stored.source.clone(),
        checkpoint_path,
        profile: stored.profile.clone(),
        saved_at_unix_seconds: stored.saved_at_unix_seconds,
        history_id,
        tabs: stored
            .window
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| tab_summary(index, tab))
            .collect(),
    })
}

#[cfg(all(feature = "local_fs", windows))]
fn persist_temp_file(mut temp: tempfile::NamedTempFile, path: &Path) -> std::io::Result<()> {
    use std::thread;
    use std::time::Duration;

    use windows::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION,
    };

    // Windows replacement fails while any short-lived reader denies delete sharing.
    let mut retry_delays = [10, 20, 40, 80, 160].into_iter();
    loop {
        match temp.persist(path) {
            Ok(_) => return Ok(()),
            Err(error) => {
                let tempfile::PersistError { error, file } = error;
                let retryable = matches!(
                    error.raw_os_error(),
                    Some(code)
                        if code == ERROR_ACCESS_DENIED.0 as i32
                            || code == ERROR_SHARING_VIOLATION.0 as i32
                            || code == ERROR_LOCK_VIOLATION.0 as i32
                );
                if !retryable {
                    return Err(error);
                }
                let Some(delay_ms) = retry_delays.next() else {
                    return Err(error);
                };
                temp = file;
                thread::sleep(Duration::from_millis(delay_ms));
            }
        }
    }
}

#[cfg(all(feature = "local_fs", not(windows)))]
fn persist_temp_file(temp: tempfile::NamedTempFile, path: &Path) -> std::io::Result<()> {
    temp.persist(path).map(|_| ()).map_err(|error| error.error)
}

#[cfg(feature = "local_fs")]
fn deserialize_checkpoint(reader: impl std::io::Read) -> Result<StoredWindowSession> {
    let mut stored =
        StoredWindowSession::deserialize(&mut serde_json::Deserializer::from_reader(reader))?;
    if !matches!(
        stored.version,
        LEGACY_SESSION_FORMAT_VERSION | SESSION_FORMAT_VERSION
    ) {
        bail!(
            "Unsupported launch config session version {}",
            stored.version
        );
    }
    enforce_checkpoint_budgets(&mut stored.block_lists);
    Ok(stored)
}

#[cfg(all(feature = "local_fs", unix))]
fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .context("Stored session path has no parent directory")?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(all(feature = "local_fs", not(unix)))]
fn sync_parent_directory(path: &Path) -> Result<()> {
    path.parent()
        .context("Stored session path has no parent directory")?;
    Ok(())
}

#[cfg(feature = "local_fs")]
fn write_checkpoint_atomically(path: &Path, value: &StoredWindowSession) -> Result<()> {
    write_checkpoint_atomically_before_replace(path, value, || Ok(()))
}

#[cfg(feature = "local_fs")]
fn write_checkpoint_atomically_before_replace(
    path: &Path,
    value: &StoredWindowSession,
    before_replace: impl FnOnce() -> Result<()>,
) -> Result<()> {
    use std::io::{BufReader, BufWriter, Write};

    let parent = path
        .parent()
        .context("Stored session path has no parent directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create stored session directory {parent:?}"))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("Failed to create temporary session file in {parent:?}"))?;
    {
        let mut writer = BufWriter::new(temp.as_file_mut());
        let mut serializer = serde_json::Serializer::new(&mut writer);
        let config = base64::engine::GeneralPurposeConfig::new();
        let serializer =
            ByteFmtSerializer::base64(&mut serializer, base64::alphabet::STANDARD, config);
        value.serialize(serializer)?;
        writer.flush()?;
    }
    temp.as_file().sync_all()?;
    let checkpoint_bytes = temp.as_file().metadata()?.len();
    if checkpoint_bytes > MAX_CHECKPOINT_FILE_BYTES {
        bail!(
            "New checkpoint is {checkpoint_bytes} bytes, exceeding the {MAX_CHECKPOINT_FILE_BYTES}-byte limit"
        );
    }
    let validated = deserialize_checkpoint(BufReader::new(temp.reopen()?))?;
    if validated.version != SESSION_FORMAT_VERSION {
        bail!("New checkpoint did not validate as version {SESSION_FORMAT_VERSION}");
    }
    before_replace()?;
    temp.persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("Failed to replace stored session at {path:?}"))?;
    sync_parent_directory(path)?;
    Ok(())
}

#[cfg(feature = "local_fs")]
pub(crate) fn write_json_atomically(path: &Path, value: &impl Serialize) -> Result<()> {
    use std::io::{BufWriter, Write};

    let parent = path
        .parent()
        .context("Stored session path has no parent directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create stored session directory {parent:?}"))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("Failed to create temporary session file in {parent:?}"))?;
    {
        let mut writer = BufWriter::new(temp.as_file_mut());
        serde_json::to_writer(&mut writer, value)?;
        writer.flush()?;
    }
    temp.as_file().sync_all()?;
    persist_temp_file(temp, path)
        .with_context(|| format!("Failed to replace stored session at {path:?}"))?;
    sync_parent_directory(path)?;
    Ok(())
}

#[cfg(feature = "local_fs")]
fn save_to_dir(
    dir: &Path,
    source: &LaunchConfigWindowSource,
    window: &WindowSnapshot,
    block_lists: &HashMap<PaneUuid, Vec<SerializedBlockListItem>>,
    preserve_existing_blocks: bool,
    saved_at_unix_seconds: i64,
) -> Result<StoredWindowSummary> {
    let checkpoint_started = Instant::now();
    let mut window = window.clone();
    window.launch_config_source = Some(source.clone());
    let pane_uuids = terminal_pane_uuids(&window);
    let path = session_path(dir, source);
    let mut stored_blocks: Vec<StoredPaneBlocks> = if preserve_existing_blocks {
        read_stored_window(&path)
            .ok()
            .map(|mut stored| {
                if let Err(error) = migrate_legacy_checkpoint(&path, &mut stored) {
                    warp_core::safe_warn!(
                        safe: ("Failed to migrate a stored terminal session before updating it"),
                        full: ("Failed to migrate stored terminal session at {path:?}: {error:#}")
                    );
                }
                stored.block_lists
            })
            .unwrap_or_default()
            .into_iter()
            .filter(|pane| pane_uuids.contains(&pane.uuid))
            .collect()
    } else {
        block_lists
            .iter()
            .filter(|(uuid, _)| pane_uuids.contains(&uuid.0))
            .map(|(uuid, blocks)| StoredPaneBlocks {
                uuid: uuid.0.clone(),
                blocks: blocks.clone(),
            })
            .collect()
    };
    let budget_stats = enforce_checkpoint_budgets(&mut stored_blocks);
    log::debug!(
        "Stored session output budget original_bytes={} retained_bytes={} truncated_blocks={}",
        budget_stats.original_output_bytes,
        budget_stats.retained_output_bytes,
        budget_stats.truncated_blocks
    );
    let stored = StoredWindowSession {
        version: SESSION_FORMAT_VERSION,
        source: source.clone(),
        profile: Some(
            warp_core::channel::ChannelState::app_id()
                .application_name()
                .to_owned(),
        ),
        window,
        block_lists: stored_blocks,
        saved_at_unix_seconds,
    };

    write_checkpoint_atomically(&path, &stored)?;
    log::debug!(
        "Stored session checkpoint file_bytes={} duration_ms={} truncated_blocks={}",
        std::fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or(0),
        checkpoint_started.elapsed().as_millis(),
        budget_stats.truncated_blocks
    );
    window_summary(dir, &path, &stored, None)
}

#[cfg(not(feature = "local_fs"))]
fn save_to_dir(
    _dir: &Path,
    _source: &LaunchConfigWindowSource,
    _window: &WindowSnapshot,
    _block_lists: &HashMap<PaneUuid, Vec<SerializedBlockListItem>>,
    _preserve_existing_blocks: bool,
    _saved_at_unix_seconds: i64,
) -> Result<StoredWindowSummary> {
    unreachable!("stored sessions are unavailable without local filesystem support")
}

#[cfg(feature = "local_fs")]
fn read_checkpoint_file(path: &Path) -> Result<StoredWindowSession> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open launch config session at {path:?}"))?;
    let checkpoint_bytes = file.metadata()?.len();
    if checkpoint_bytes > MAX_CHECKPOINT_FILE_BYTES {
        bail!(
            "Launch config session is {checkpoint_bytes} bytes, exceeding the {MAX_CHECKPOINT_FILE_BYTES}-byte limit"
        );
    }
    deserialize_checkpoint(std::io::BufReader::new(file))
        .with_context(|| format!("Failed to parse launch config session at {path:?}"))
}

#[cfg(feature = "local_fs")]
fn read_stored_window(path: &Path) -> Result<StoredWindowSession> {
    match read_checkpoint_file(path) {
        Ok(stored) => Ok(stored),
        Err(active_error) => {
            let backup_path = legacy_backup_path(path);
            if !backup_path.is_file() {
                return Err(active_error);
            }
            let stored = validate_legacy_backup(&backup_path).with_context(|| {
                format!("Active checkpoint {path:?} is unreadable and its legacy backup is invalid")
            })?;
            warp_core::safe_warn!(
                safe: ("Recovering an unreadable terminal session from its legacy backup"),
                full: (
                    "Recovering unreadable terminal session at {path:?} from {backup_path:?}: {active_error:#}"
                )
            );
            Ok(stored)
        }
    }
}

#[cfg(feature = "local_fs")]
fn legacy_backup_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "session.json".to_owned());
    path.with_file_name(format!("{file_name}.v1.bak"))
}

#[cfg(feature = "local_fs")]
fn legacy_backup_checksum_path(backup_path: &Path) -> PathBuf {
    let mut path = backup_path.as_os_str().to_os_string();
    path.push(".sha256");
    path.into()
}

#[cfg(feature = "local_fs")]
fn file_sha256(path: &Path) -> Result<String> {
    use std::io::{BufReader, Read};

    let mut reader = BufReader::new(std::fs::File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(feature = "local_fs")]
fn write_checksum_atomically(path: &Path, checksum: &str) -> Result<()> {
    use std::io::Write;

    let parent = path
        .parent()
        .context("Stored session checksum path has no parent directory")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    writeln!(temp, "{checksum}")?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    sync_parent_directory(path)?;
    Ok(())
}

#[cfg(feature = "local_fs")]
fn validate_legacy_backup(backup_path: &Path) -> Result<StoredWindowSession> {
    let stored = read_checkpoint_file(backup_path)?;
    if stored.version != LEGACY_SESSION_FORMAT_VERSION {
        bail!("Legacy rollback checkpoint is not version 1");
    }
    let actual_checksum = file_sha256(backup_path)?;
    let checksum_path = legacy_backup_checksum_path(backup_path);
    if checksum_path.exists() {
        let expected_checksum = std::fs::read_to_string(&checksum_path)?;
        if expected_checksum.trim() != actual_checksum {
            bail!("Legacy rollback checkpoint checksum does not match");
        }
    } else {
        write_checksum_atomically(&checksum_path, &actual_checksum)?;
    }
    Ok(stored)
}

#[cfg(feature = "local_fs")]
fn ensure_legacy_backup(path: &Path) -> Result<PathBuf> {
    use std::io::{BufReader, BufWriter, Read, Write};

    let backup_path = legacy_backup_path(path);
    if backup_path.exists() {
        validate_legacy_backup(&backup_path)?;
        return Ok(backup_path);
    }

    let parent = path
        .parent()
        .context("Stored session path has no parent directory")?;
    let mut backup = tempfile::NamedTempFile::new_in(parent)?;
    let mut source = BufReader::new(std::fs::File::open(path)?);
    let mut hasher = Sha256::new();
    {
        let mut writer = BufWriter::new(backup.as_file_mut());
        let mut buffer = [0; 64 * 1024];
        loop {
            let read = source.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            writer.write_all(&buffer[..read])?;
        }
        writer.flush()?;
    }
    backup.as_file().sync_all()?;
    let checksum = hex::encode(hasher.finalize());
    backup
        .persist_noclobber(&backup_path)
        .map_err(|error| error.error)?;
    sync_parent_directory(&backup_path)?;
    write_checksum_atomically(&legacy_backup_checksum_path(&backup_path), &checksum)?;
    Ok(backup_path)
}

#[cfg(feature = "local_fs")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MigrationBoundary {
    RollbackPrepared,
    TemporaryValidated,
    ActiveReplaced,
}

#[cfg(feature = "local_fs")]
fn migrate_legacy_checkpoint(path: &Path, stored: &mut StoredWindowSession) -> Result<()> {
    migrate_legacy_checkpoint_with_hook(path, stored, |_| Ok(()))
}

#[cfg(feature = "local_fs")]
fn migrate_legacy_checkpoint_with_hook(
    path: &Path,
    stored: &mut StoredWindowSession,
    mut boundary_hook: impl FnMut(MigrationBoundary) -> Result<()>,
) -> Result<()> {
    if stored.version != LEGACY_SESSION_FORMAT_VERSION {
        return Ok(());
    }

    let migration_started = Instant::now();
    let source_bytes = std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    ensure_legacy_backup(path)?;
    boundary_hook(MigrationBoundary::RollbackPrepared)?;

    stored.version = SESSION_FORMAT_VERSION;
    if let Err(error) = write_checkpoint_atomically_before_replace(path, stored, || {
        boundary_hook(MigrationBoundary::TemporaryValidated)
    }) {
        stored.version = LEGACY_SESSION_FORMAT_VERSION;
        return Err(error).context("Failed to migrate stored terminal session");
    }
    boundary_hook(MigrationBoundary::ActiveReplaced)?;
    log::info!(
        "Migrated stored terminal session source_bytes={} destination_bytes={} duration_ms={} rollback_retained=true",
        source_bytes,
        std::fs::metadata(path)
            .map(|metadata| metadata.len())
            .unwrap_or(0),
        migration_started.elapsed().as_millis()
    );
    Ok(())
}

#[cfg(feature = "local_fs")]
fn scan_catalog(dir: &Path) -> Vec<StoredWindowSummary> {
    use walkdir::WalkDir;
    use warp_core::safe_warn;

    if !dir.is_dir() {
        return Vec::new();
    }

    WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
                && entry.path() != catalog_path(dir)
                && entry.file_name() != crate::session_configs::OWNER_FILE_NAME
        })
        .filter_map(|entry| {
            let path = entry.path();
            let stored = match read_stored_window(path) {
                Ok(stored) => stored,
                Err(error) => {
                    safe_warn!(
                        safe: ("Skipping an unreadable stored terminal session"),
                        full: ("Skipping unreadable stored terminal session at {path:?}: {error:#}")
                    );
                    return None;
                }
            };
            let history_id = path
                .strip_prefix(dir.join(HISTORY_DIRECTORY_NAME))
                .ok()
                .and_then(|relative| relative.parent())
                .and_then(Path::file_name)
                .map(|value| value.to_string_lossy().into_owned());
            let saved_at_unix_seconds = if stored.saved_at_unix_seconds == 0 {
                path.metadata()
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|modified| modified.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_secs() as i64)
                    .unwrap_or_default()
            } else {
                stored.saved_at_unix_seconds
            };
            let stored = StoredWindowSession {
                saved_at_unix_seconds,
                ..stored
            };
            match window_summary(dir, path, &stored, history_id) {
                Ok(summary) => Some(summary),
                Err(error) => {
                    safe_warn!(
                        safe: ("Skipping a stored terminal session outside its state directory"),
                        full: ("Skipping stored terminal session at {path:?}: {error:#}")
                    );
                    None
                }
            }
        })
        .collect()
}

#[cfg(feature = "local_fs")]
pub(crate) fn read_catalog(dir: &Path) -> Result<Vec<StoredWindowSummary>> {
    let path = catalog_path(dir);
    let file = std::fs::File::open(&path)
        .with_context(|| format!("Failed to open stored session catalog at {path:?}"))?;
    let catalog: StoredSessionCatalog = serde_json::from_reader(std::io::BufReader::new(file))
        .with_context(|| format!("Failed to parse stored session catalog at {path:?}"))?;
    if catalog.version != CATALOG_FORMAT_VERSION {
        bail!(
            "Unsupported stored session catalog version {} at {path:?}",
            catalog.version
        );
    }
    Ok(catalog.windows)
}

#[cfg(feature = "local_fs")]
fn write_catalog(dir: &Path, windows: Vec<StoredWindowSummary>) -> Result<()> {
    write_json_atomically(
        &catalog_path(dir),
        &StoredSessionCatalog {
            version: CATALOG_FORMAT_VERSION,
            windows,
        },
    )
}

#[cfg(feature = "local_fs")]
fn load_or_rebuild_catalog(dir: &Path) -> Vec<StoredWindowSummary> {
    match read_catalog(dir) {
        Ok(windows) => windows,
        Err(_) => {
            let windows = scan_catalog(dir);
            let _ = write_catalog(dir, windows.clone());
            windows
        }
    }
}

#[cfg(feature = "local_fs")]
fn update_catalog(
    dir: &Path,
    current: Vec<StoredWindowSummary>,
    archived: Vec<StoredWindowSummary>,
) -> Result<()> {
    let current_sources = current
        .iter()
        .map(|window| window.source.clone())
        .collect::<HashSet<_>>();
    let archived_keys = archived
        .iter()
        .map(|window| (window.source.clone(), window.history_id.clone()))
        .collect::<HashSet<_>>();
    let mut windows = load_or_rebuild_catalog(dir);
    windows.retain(|window| {
        (window.history_id.is_some() || !current_sources.contains(&window.source))
            && !archived_keys.contains(&(window.source.clone(), window.history_id.clone()))
    });
    windows.extend(current);
    windows.extend(archived);
    write_catalog(dir, windows)
}

#[cfg(feature = "local_fs")]
fn archive_window(
    dir: &Path,
    summary: &StoredWindowSummary,
    history_id: &str,
) -> Result<StoredWindowSummary> {
    let source_path = dir.join(&summary.checkpoint_path);
    let stored = read_stored_window(&source_path)?;
    let archive_path = history_path(dir, &stored.source, history_id);
    let mut stored = stored;
    stored.version = SESSION_FORMAT_VERSION;
    enforce_checkpoint_budgets(&mut stored.block_lists);
    write_checkpoint_atomically(&archive_path, &stored)?;
    window_summary(dir, &archive_path, &stored, Some(history_id.to_owned()))
}

#[cfg(feature = "local_fs")]
fn save_windows_to_dir(
    dir: &Path,
    windows: &[WindowSnapshot],
    block_lists: &HashMap<PaneUuid, Vec<SerializedBlockListItem>>,
    preserve_existing_blocks: bool,
    saved_at_unix_seconds: i64,
    history_id: Option<&str>,
) -> Result<()> {
    let mut current = Vec::new();
    for window in windows {
        if let Some(source) = window.launch_config_source.as_ref() {
            current.push(save_to_dir(
                dir,
                source,
                window,
                block_lists,
                preserve_existing_blocks,
                saved_at_unix_seconds,
            )?);
        }
    }

    if current.is_empty() {
        return Ok(());
    }

    let archived = if let Some(history_id) = history_id {
        let mut session_windows = scan_catalog(dir)
            .into_iter()
            .filter(|summary| {
                summary.history_id.is_none()
                    && current.iter().any(|current| {
                        summary.source.path == current.source.path
                            && summary.source.document_index == current.source.document_index
                            && summary.source.config_name == current.source.config_name
                    })
            })
            .collect::<Vec<_>>();
        for latest in &current {
            session_windows.retain(|summary| summary.source != latest.source);
            session_windows.push(latest.clone());
        }
        session_windows
            .iter()
            .map(|summary| archive_window(dir, summary, history_id))
            .collect::<Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    update_catalog(dir, current, archived)
}

#[cfg(feature = "local_fs")]
fn configuration_source_key(source: &LaunchConfigWindowSource) -> LaunchConfigWindowSource {
    LaunchConfigWindowSource {
        window_index: 0,
        ..source.clone()
    }
}

#[cfg(feature = "local_fs")]
fn history_ids() -> &'static parking_lot::Mutex<HashMap<LaunchConfigWindowSource, String>> {
    static HISTORY_IDS: OnceLock<parking_lot::Mutex<HashMap<LaunchConfigWindowSource, String>>> =
        OnceLock::new();
    HISTORY_IDS.get_or_init(Default::default)
}

pub(crate) fn begin_run(source: &LaunchConfigWindowSource) {
    #[cfg(feature = "local_fs")]
    history_ids().lock().insert(
        configuration_source_key(source),
        uuid::Uuid::new_v4().to_string(),
    );
    #[cfg(not(feature = "local_fs"))]
    let _ = source;
}

#[cfg(feature = "local_fs")]
fn active_history_id(windows: &[WindowSnapshot]) -> Option<String> {
    let source = windows
        .iter()
        .find_map(|window| window.launch_config_source.as_ref())?;
    Some(
        history_ids()
            .lock()
            .entry(configuration_source_key(source))
            .or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone(),
    )
}

pub(crate) fn history_id_for_windows(windows: &[WindowSnapshot]) -> Option<String> {
    #[cfg(feature = "local_fs")]
    {
        active_history_id(windows)
    }
    #[cfg(not(feature = "local_fs"))]
    {
        let _ = windows;
        None
    }
}

pub(crate) fn save_windows(
    windows: &[WindowSnapshot],
    block_lists: &HashMap<PaneUuid, Vec<SerializedBlockListItem>>,
    preserve_existing_blocks: bool,
    history_id: Option<&str>,
) -> Result<()> {
    #[cfg(feature = "local_fs")]
    {
        save_windows_to_dir(
            &sessions_dir(),
            windows,
            block_lists,
            preserve_existing_blocks,
            chrono::Utc::now().timestamp(),
            history_id,
        )
    }
    #[cfg(not(feature = "local_fs"))]
    {
        let _ = (windows, block_lists, preserve_existing_blocks, history_id);
        Ok(())
    }
}

fn configuration_key(source: &LaunchConfigWindowSource) -> String {
    source_hash(source, false)
}

fn group_key(window: &StoredWindowSummary) -> String {
    let configuration_key = configuration_key(&window.source);
    match &window.history_id {
        Some(history_id) => format!("history:{configuration_key}:{history_id}"),
        None => format!("current:{configuration_key}"),
    }
}

fn group_summaries(windows: Vec<StoredWindowSummary>) -> Vec<StoredSessionSummary> {
    let mut groups = BTreeMap::<String, Vec<StoredWindowSummary>>::new();
    for window in windows {
        groups.entry(group_key(&window)).or_default().push(window);
    }

    let mut sessions = groups
        .into_iter()
        .filter_map(|(id, mut windows)| {
            windows.sort_by_key(|window| window.source.window_index);
            let first = windows.first()?;
            let profile = windows
                .iter()
                .find_map(|window| window.profile.clone())
                .or_else(|| {
                    Some(
                        warp_core::channel::ChannelState::app_id()
                            .application_name()
                            .to_owned(),
                    )
                });
            Some(StoredSessionSummary {
                id,
                name: first.source.config_name.clone(),
                saved_at_unix_seconds: windows
                    .iter()
                    .map(|window| window.saved_at_unix_seconds)
                    .max()
                    .unwrap_or_default(),
                is_current: first.history_id.is_none(),
                profile,
                windows,
            })
        })
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| {
        right
            .saved_at_unix_seconds
            .cmp(&left.saved_at_unix_seconds)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    sessions
}

pub(crate) fn list_summaries() -> Vec<StoredSessionSummary> {
    #[cfg(feature = "local_fs")]
    {
        group_summaries(load_or_rebuild_catalog(&sessions_dir()))
    }
    #[cfg(not(feature = "local_fs"))]
    {
        Vec::new()
    }
}

pub(crate) fn list_interrupted_summaries() -> Vec<StoredSessionSummary> {
    list_summaries()
        .into_iter()
        .filter(|summary| summary.is_current())
        .filter(|summary| {
            summary
                .first_source()
                .is_some_and(crate::session_configs::was_open_when_process_ended)
        })
        .collect()
}

fn is_safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

#[cfg(feature = "local_fs")]
fn load_from_path(path: &Path, source: &LaunchConfigWindowSource) -> Result<RestoredWindowSession> {
    let restoration_started = Instant::now();
    let mut stored = read_stored_window(path)?;
    if !sources_match(&stored.source, source) {
        bail!("Session config snapshot source does not match {path:?}");
    }
    if let Err(error) = migrate_legacy_checkpoint(path, &mut stored) {
        warp_core::safe_warn!(
            safe: ("Restoring a legacy terminal session without migrating it"),
            full: ("Restoring legacy terminal session at {path:?} without migration: {error:#}")
        );
    }
    identify_agent_sessions(&mut stored.window, stored.saved_at_unix_seconds);
    let restored_block_count = stored
        .block_lists
        .iter()
        .map(|pane| pane.blocks.len())
        .sum::<usize>();
    log::debug!(
        "Loaded stored terminal session file_bytes={} block_count={} duration_ms={}",
        std::fs::metadata(path)
            .map(|metadata| metadata.len())
            .unwrap_or(0),
        restored_block_count,
        restoration_started.elapsed().as_millis()
    );
    Ok(RestoredWindowSession {
        window: stored.window,
        block_lists: Arc::new(
            stored
                .block_lists
                .into_iter()
                .map(|pane| (PaneUuid(pane.uuid), pane.blocks))
                .collect(),
        ),
    })
}

#[cfg(feature = "local_fs")]
fn load_from_dir(
    dir: &Path,
    source: &LaunchConfigWindowSource,
) -> Result<Option<RestoredWindowSession>> {
    let path = session_path(dir, source);
    match load_from_path(&path, source) {
        Ok(session) => Ok(Some(session)),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

#[cfg(not(feature = "local_fs"))]
fn load_from_dir(
    _dir: &Path,
    _source: &LaunchConfigWindowSource,
) -> Result<Option<RestoredWindowSession>> {
    Ok(None)
}

pub(crate) fn load(source: &LaunchConfigWindowSource) -> Result<Option<RestoredWindowSession>> {
    load_from_dir(&sessions_dir(), source)
}

#[cfg(feature = "local_fs")]
fn load_stored_session_from_dir(
    dir: &Path,
    summary: &StoredSessionSummary,
) -> Result<Vec<RestoredWindowSession>> {
    summary
        .windows
        .iter()
        .map(|window| {
            if !is_safe_relative_path(&window.checkpoint_path) {
                bail!("Stored session checkpoint path is invalid");
            }
            load_from_path(&dir.join(&window.checkpoint_path), &window.source)
        })
        .collect()
}

pub(crate) fn load_stored_session(
    summary: &StoredSessionSummary,
) -> Result<Vec<RestoredWindowSession>> {
    #[cfg(feature = "local_fs")]
    {
        load_stored_session_from_dir(&sessions_dir(), summary)
    }
    #[cfg(not(feature = "local_fs"))]
    {
        let _ = summary;
        Ok(Vec::new())
    }
}

/// Frees every identity the live window owns so the clone cannot contend with
/// it: a persistent process has exactly one owner, restoring a window detaches
/// its conversations from the previous holder, and block lists are keyed by
/// pane uuid.
#[cfg(feature = "local_fs")]
fn release_owned_identities(node: &mut PaneNodeSnapshot) {
    match node {
        PaneNodeSnapshot::Branch(branch) => {
            for (_, child) in &mut branch.children {
                release_owned_identities(child);
            }
        }
        PaneNodeSnapshot::Leaf(LeafSnapshot {
            contents: LeafContents::Terminal(terminal),
            ..
        }) => {
            terminal.uuid = uuid::Uuid::new_v4().as_bytes().to_vec();
            terminal.persistent_session_id = None;
            terminal.cli_agent_resume_descriptor = None;
            terminal.conversation_ids_to_restore.clear();
            terminal.active_conversation_id = None;
        }
        PaneNodeSnapshot::Leaf(LeafSnapshot {
            contents: LeafContents::AmbientAgent(ambient),
            ..
        }) => {
            ambient.uuid = uuid::Uuid::new_v4().as_bytes().to_vec();
            // A restored ambient pane with a task id attaches to that task's
            // existing conversation; `None` restores as a new cloud
            // conversation, which is what the clone dialog promises.
            ambient.task_id = None;
        }
        // Every other leaf is content the clone may copy verbatim. A future
        // leaf variant that carries an id the live window owns -- a pane uuid,
        // a session, a conversation -- must be given its own arm above
        // deliberately: falling through here hands the clone the original's
        // identity, and persisted blocks are keyed by pane uuid.
        //
        // `LeafContents::CodeReview`'s `CodeReviewPaneSnapshot::Local` already
        // falls through here carrying an owned `terminal_uuid`. That is safe
        // only by accident: `pane_group::mod.rs` rejects code-review panes on
        // restore, so the copied uuid never binds a live pane. Restoring them
        // again would make this arm a bug.
        PaneNodeSnapshot::Leaf(_) => {}
    }
}

/// Distinguishes "nothing to copy yet" from every other way cloning can fail,
/// so the dialog can show an actionable message instead of a generic one.
#[cfg(feature = "local_fs")]
#[derive(Debug, thiserror::Error)]
#[error("Session {0:?} has not been checkpointed yet")]
pub(crate) struct NoCheckpointsToClone(pub String);

#[cfg(feature = "local_fs")]
fn clone_checkpoints_in_dir(
    dir: &Path,
    from: &LaunchConfigWindowSource,
    to_path: &Path,
    to_config_name: &str,
) -> Result<Vec<LaunchConfigWindowSource>> {
    let session_id = from
        .session_id()
        .context("Cloning requires a session config source")?;
    let generation = from
        .session_generation()
        .context("Cloning requires a session config generation")?;
    let source_dir = dir.join(&session_id).join(generation.to_string());

    let mut window_indexes = std::fs::read_dir(&source_dir)
        .with_context(|| format!("Failed to read session checkpoints in {source_dir:?}"))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            name.strip_prefix("window-")?
                .strip_suffix(".json")?
                .parse::<usize>()
                .ok()
        })
        .collect::<Vec<_>>();
    window_indexes.sort_unstable();
    if window_indexes.is_empty() {
        return Err(NoCheckpointsToClone(session_id).into());
    }

    let saved_at_unix_seconds = chrono::Utc::now().timestamp();
    let mut sources = Vec::with_capacity(window_indexes.len());
    let mut summaries = Vec::with_capacity(window_indexes.len());
    for window_index in window_indexes {
        let from_window = LaunchConfigWindowSource {
            window_index,
            ..from.clone()
        };
        let mut stored = read_stored_window(&session_path(dir, &from_window))?;
        let to_window = LaunchConfigWindowSource {
            path: to_path.to_path_buf(),
            document_index: 1,
            window_index,
            config_name: to_config_name.to_owned(),
        };

        for tab in &mut stored.window.tabs {
            release_owned_identities(&mut tab.root);
        }
        stored.window.launch_config_source = Some(to_window.clone());
        // Inherited bounds would place the clone exactly on top of the
        // original, which on a single monitor reads as nothing having
        // happened at all -- the complaint this feature exists to fix.
        stored.window.bounds = None;
        stored.source = to_window.clone();
        stored.block_lists.clear();
        stored.saved_at_unix_seconds = saved_at_unix_seconds;

        let path = session_path(dir, &to_window);
        stored.version = SESSION_FORMAT_VERSION;
        write_checkpoint_atomically(&path, &stored)?;
        summaries.push(window_summary(dir, &path, &stored, None)?);
        sources.push(to_window);
    }

    update_catalog(dir, summaries, Vec::new())?;
    Ok(sources)
}

// The sole caller lives in `clone_session`, which is itself `local_fs`-only, so
// a `not(local_fs)` arm here would be unreachable rather than merely unused.
#[cfg(feature = "local_fs")]
pub(crate) fn clone_checkpoints(
    from: &LaunchConfigWindowSource,
    to_path: &Path,
    to_config_name: &str,
) -> Result<Vec<LaunchConfigWindowSource>> {
    clone_checkpoints_in_dir(&sessions_dir(), from, to_path, to_config_name)
}

pub(crate) fn first_source(summary: &StoredSessionSummary) -> Option<&LaunchConfigWindowSource> {
    summary.first_source()
}

/// The uuids of every terminal pane in a window snapshot.
///
/// Callers restoring a window use these to read only the blocks they need.
pub(crate) fn terminal_pane_uuids(window: &WindowSnapshot) -> Vec<Vec<u8>> {
    fn collect(node: &PaneNodeSnapshot, uuids: &mut Vec<Vec<u8>>) {
        match node {
            PaneNodeSnapshot::Branch(branch) => {
                for (_, child) in &branch.children {
                    collect(child, uuids);
                }
            }
            PaneNodeSnapshot::Leaf(leaf) => {
                if let LeafContents::Terminal(terminal) = &leaf.contents {
                    uuids.push(terminal.uuid.clone());
                }
            }
        }
    }

    let mut uuids = Vec::new();
    for tab in &window.tabs {
        collect(&tab.root, &mut uuids);
    }
    uuids
}

#[cfg(test)]
#[path = "session_store_tests.rs"]
mod tests;
