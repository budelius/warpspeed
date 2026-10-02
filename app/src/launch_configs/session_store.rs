#[cfg(feature = "local_fs")]
use std::collections::HashSet;
use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
#[cfg(feature = "local_fs")]
use std::sync::OnceLock;
use std::time::SystemTime;

#[cfg(feature = "local_fs")]
use anyhow::bail;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ai::blocklist::SerializedBlockListItem;
#[cfg(feature = "local_fs")]
use crate::app_state::LeafSnapshot;
use crate::app_state::{
    LeafContents, PaneNodeSnapshot, PaneUuid, TabSnapshot, TerminalPaneSnapshot, WindowSnapshot,
};
use crate::launch_configs::launch_config::LaunchConfigWindowSource;
use crate::terminal::CLIAgent;

const SESSION_FORMAT_VERSION: u32 = 1;
const CATALOG_FORMAT_VERSION: u32 = 1;
const CATALOG_FILE_NAME: &str = "stored-sessions.json";
const HISTORY_DIRECTORY_NAME: &str = "history";

#[derive(Serialize, Deserialize)]
struct StoredPaneBlocks {
    uuid: Vec<u8>,
    blocks: Vec<SerializedBlockListItem>,
}

#[derive(Serialize, Deserialize)]
struct StoredWindowSession {
    version: u32,
    source: LaunchConfigWindowSource,
    #[serde(default)]
    profile: Option<String>,
    window: WindowSnapshot,
    #[serde(default)]
    block_lists: Vec<StoredPaneBlocks>,
    #[serde(default)]
    saved_at_unix_seconds: i64,
}

pub(crate) struct RestoredWindowSession {
    pub window: WindowSnapshot,
    pub block_lists: Arc<HashMap<PaneUuid, Vec<SerializedBlockListItem>>>,
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

fn collect_agent_session_ids(node: &PaneNodeSnapshot, ids: &mut HashSet<String>) {
    match node {
        PaneNodeSnapshot::Branch(branch) => {
            for (_, child) in &branch.children {
                collect_agent_session_ids(child, ids);
            }
        }
        PaneNodeSnapshot::Leaf(leaf) => {
            if let LeafContents::Terminal(terminal) = &leaf.contents
                && let Some(session_id) = terminal
                    .cli_agent_resume_descriptor
                    .as_ref()
                    .and_then(|descriptor| descriptor.session_id.as_ref())
            {
                ids.insert(session_id.clone());
            }
        }
    }
}

fn normalized_cwd(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn recover_agent_session_ids(
    node: &mut PaneNodeSnapshot,
    used_ids: &mut HashSet<String>,
    candidates: &mut HashMap<
        CLIAgent,
        Vec<crate::terminal::agent_session_history::PastAgentSession>,
    >,
) {
    match node {
        PaneNodeSnapshot::Branch(branch) => {
            for (_, child) in &mut branch.children {
                recover_agent_session_ids(child, used_ids, candidates);
            }
        }
        PaneNodeSnapshot::Leaf(leaf) => {
            let LeafContents::Terminal(terminal) = &mut leaf.contents else {
                return;
            };
            let Some(descriptor) = terminal.cli_agent_resume_descriptor.as_mut() else {
                return;
            };
            if descriptor.session_id.is_some() {
                return;
            }
            let cwd_key = normalized_cwd(&descriptor.cwd);
            let sessions = candidates.entry(descriptor.agent).or_insert_with(|| {
                crate::terminal::agent_session_history::find_all(descriptor.agent)
            });
            let Some(session) = sessions.iter().find(|session| {
                normalized_cwd(&session.cwd) == cwd_key && !used_ids.contains(&session.session_id)
            }) else {
                return;
            };
            descriptor.session_id = Some(session.session_id.clone());
            if descriptor.title.is_none() {
                descriptor.title = session.title.clone();
            }
            used_ids.insert(session.session_id.clone());
        }
    }
}

pub(crate) fn recover_missing_agent_session_ids(windows: &mut [WindowSnapshot]) {
    let mut used_ids = HashSet::new();
    for window in windows.iter() {
        for tab in &window.tabs {
            collect_agent_session_ids(&tab.root, &mut used_ids);
        }
    }
    let mut candidates = HashMap::new();
    for window in windows {
        for tab in &mut window.tabs {
            recover_agent_session_ids(&mut tab.root, &mut used_ids, &mut candidates);
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

#[cfg(feature = "local_fs")]
fn write_json_atomically(path: &Path, value: &impl Serialize) -> Result<()> {
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
    temp.persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("Failed to replace stored session at {path:?}"))?;
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
    let mut window = window.clone();
    window.launch_config_source = Some(source.clone());
    let pane_uuids = terminal_pane_uuids(&window);
    let path = session_path(dir, source);
    let stored_blocks = if preserve_existing_blocks {
        std::fs::File::open(&path)
            .ok()
            .and_then(|file| {
                serde_json::from_reader::<_, StoredWindowSession>(std::io::BufReader::new(file))
                    .ok()
            })
            .map(|stored| stored.block_lists)
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

    write_json_atomically(&path, &stored)?;
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
fn read_stored_window(path: &Path) -> Result<StoredWindowSession> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open launch config session at {path:?}"))?;
    let stored: StoredWindowSession = serde_json::from_reader(std::io::BufReader::new(file))
        .with_context(|| format!("Failed to parse launch config session at {path:?}"))?;
    if stored.version != SESSION_FORMAT_VERSION {
        bail!(
            "Unsupported launch config session version {} at {path:?}",
            stored.version
        );
    }
    Ok(stored)
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
    write_json_atomically(&archive_path, &stored)?;
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
fn load_from_path(
    path: &Path,
    source: &LaunchConfigWindowSource,
    recover_missing_agent_ids: bool,
) -> Result<RestoredWindowSession> {
    let stored = read_stored_window(path)?;
    if !sources_match(&stored.source, source) {
        bail!("Session config snapshot source does not match {path:?}");
    }
    let mut window = stored.window;
    if recover_missing_agent_ids {
        recover_missing_agent_session_ids(std::slice::from_mut(&mut window));
    }
    Ok(RestoredWindowSession {
        window,
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
    match load_from_path(&path, source, true) {
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
            load_from_path(
                &dir.join(&window.checkpoint_path),
                &window.source,
                summary.is_current(),
            )
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
        write_json_atomically(&path, &stored)?;
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
