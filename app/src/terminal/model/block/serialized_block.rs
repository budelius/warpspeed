use std::collections::{HashSet, VecDeque};
use std::fmt;

use base64::Engine as _;
use chrono::{DateTime, Local, TimeZone as _};
use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_bytes_repr::ByteFmtSerializer;
use warp_core::command::ExitCode;

use super::AgentInteractionMetadata;
use crate::ai::agent::AIAgentActionId;
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::agent::task::TaskId;
use crate::ai::blocklist::block::cli_controller::LongRunningCommandControlState;
use crate::terminal::ShellHost;
use crate::terminal::model::BlockId;
use crate::terminal::model::block::{
    AgentViewVisibility, Block, BlockState, MAX_SERIALIZED_STYLIZED_COMMAND_BYTES,
    MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES, MAX_SERIALIZED_STYLIZED_OUTPUT_LINES, PromptInfo,
    has_block_failed,
};
use crate::terminal::model::session::SessionId;
use crate::util::extensions::TrimStringExt;

/// Serialization-stable representation of [`AgentViewVisibility`].
///
/// This type decouples the persisted format from the in-app format, allowing
/// internal changes to `AgentViewVisibility` without breaking serialization.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SerializedAgentViewVisibility {
    Terminal {
        #[serde(default)]
        pending_conversation_ids: HashSet<AIConversationId>,
        conversation_ids: HashSet<AIConversationId>,
    },
    Agent {
        #[serde(alias = "conversation_id")]
        origin_conversation_id: AIConversationId,
        #[serde(default)]
        pending_other_conversation_ids: HashSet<AIConversationId>,
        #[serde(default)]
        other_conversation_ids: HashSet<AIConversationId>,
    },
}

impl From<AgentViewVisibility> for SerializedAgentViewVisibility {
    fn from(value: AgentViewVisibility) -> Self {
        match value {
            AgentViewVisibility::Terminal {
                pending_conversation_ids,
                conversation_ids,
            } => SerializedAgentViewVisibility::Terminal {
                pending_conversation_ids,
                conversation_ids,
            },
            AgentViewVisibility::Agent {
                origin_conversation_id,
                pending_other_conversation_ids,
                other_conversation_ids,
            } => SerializedAgentViewVisibility::Agent {
                origin_conversation_id,
                pending_other_conversation_ids,
                other_conversation_ids,
            },
        }
    }
}

impl From<SerializedAgentViewVisibility> for AgentViewVisibility {
    fn from(value: SerializedAgentViewVisibility) -> Self {
        match value {
            SerializedAgentViewVisibility::Terminal {
                pending_conversation_ids,
                conversation_ids,
            } => AgentViewVisibility::Terminal {
                pending_conversation_ids,
                conversation_ids,
            },
            SerializedAgentViewVisibility::Agent {
                origin_conversation_id,
                pending_other_conversation_ids,
                other_conversation_ids,
            } => AgentViewVisibility::Agent {
                origin_conversation_id,
                pending_other_conversation_ids,
                other_conversation_ids,
            },
        }
    }
}

fn default_as_true() -> bool {
    true
}

/// Blocklist AI metadata associated with this block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SerializedAIMetadata {
    /// The ID of the `AIAgentAction` associated with this block's requested command execution.
    /// This is optional because not all AI-related blocks are associated with a requested command.
    #[serde(alias = "action_id")]
    requested_command_action_id: Option<AIAgentActionId>,

    /// The ID of the conversation to which this action belongs.
    conversation_id: AIConversationId,

    subagent_task_id: Option<TaskId>,

    /// State governing user/agent interaction with the command in this block.
    long_running_control_state: Option<LongRunningCommandControlState>,

    /// `true` if the agent has previously written to this block.
    has_agent_written_to_block: bool,

    /// `true` if this block should be hidden from the user (as is the case with AI-requested
    /// commands, for example).
    #[serde(default = "default_as_true", skip)]
    should_hide_block: bool,
}

impl From<AgentInteractionMetadata> for SerializedAIMetadata {
    fn from(value: AgentInteractionMetadata) -> Self {
        SerializedAIMetadata {
            requested_command_action_id: value.requested_command_action_id().cloned(),
            conversation_id: *value.conversation_id(),
            subagent_task_id: value.subagent_task_id().cloned(),
            long_running_control_state: value.long_running_control_state().cloned(),
            has_agent_written_to_block: value.has_agent_written_to_block(),
            should_hide_block: value.should_hide_block(),
        }
    }
}

impl From<SerializedAIMetadata> for AgentInteractionMetadata {
    fn from(value: SerializedAIMetadata) -> Self {
        AgentInteractionMetadata::new(
            value.requested_command_action_id,
            value.conversation_id,
            value.subagent_task_id,
            value.long_running_control_state,
            value.has_agent_written_to_block,
            value.should_hide_block,
        )
    }
}

#[derive(Clone, Debug, Serialize, Default, PartialEq)]
pub struct SerializedBlock {
    pub id: BlockId,
    /// The input lines with their corresponding escape sequences so it can be rendered outside of
    /// the terminal.
    #[serde(serialize_with = "serialize_stylized_bytes")]
    pub stylized_command: Vec<u8>,

    /// The output lines with their corresponding escape sequences so it can be rendered outside of
    /// the terminal.
    /// They are truncated to MAX_SERIALIZED_STYLIZED_OUTPUT_LINES lines.
    #[serde(serialize_with = "serialize_stylized_bytes")]
    pub stylized_output: Vec<u8>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stylized_output_original_bytes: Option<usize>,

    #[serde(default)]
    pub stylized_output_truncated: bool,

    /// The current working directory of the block.
    pub pwd: Option<String>,

    #[serde(alias = "git_branch")]
    pub git_head: Option<String>,

    #[serde(default)]
    pub git_branch_name: Option<String>,

    pub virtual_env: Option<String>,

    pub conda_env: Option<String>,

    pub node_version: Option<String>,

    pub exit_code: ExitCode,

    /// True iff the block _started_ executing (i.e. preexec was received) or it's a static block.
    pub did_execute: bool,

    pub completed_ts: Option<DateTime<Local>>,

    pub start_ts: Option<DateTime<Local>>,

    pub ps1: Option<String>,

    pub rprompt: Option<String>,

    pub honor_ps1: bool,

    pub is_background: bool,

    pub session_id: Option<SessionId>,

    pub shell_host: Option<ShellHost>,

    /// JSON-serialized representation of the Warp prompt snapshot (Context Chips). Note that this
    /// is different from PS1 and RPROMPT1
    pub prompt_snapshot: Option<String>,

    /// JSON-serialized representation of [`SerializedAIMetadata`].
    pub ai_metadata: Option<String>,

    /// Whether this block was created locally (true) or remotely (false)
    #[serde(default)]
    pub is_local: Option<bool>,

    /// Tracks which views (terminal and/or agent conversations) this block should be visible in.
    #[serde(default)]
    pub agent_view_visibility: Option<SerializedAgentViewVisibility>,
}

#[derive(Deserialize)]
struct SerializedBlockWire {
    id: BlockId,
    #[serde(deserialize_with = "deserialize_stylized_command")]
    stylized_command: BoundedStylizedBytes,
    #[serde(deserialize_with = "deserialize_stylized_output")]
    stylized_output: BoundedStylizedBytes,
    #[serde(default)]
    stylized_output_original_bytes: Option<usize>,
    #[serde(default)]
    stylized_output_truncated: bool,
    pwd: Option<String>,
    #[serde(alias = "git_branch")]
    git_head: Option<String>,
    #[serde(default)]
    git_branch_name: Option<String>,
    virtual_env: Option<String>,
    conda_env: Option<String>,
    node_version: Option<String>,
    exit_code: ExitCode,
    did_execute: bool,
    completed_ts: Option<DateTime<Local>>,
    start_ts: Option<DateTime<Local>>,
    ps1: Option<String>,
    rprompt: Option<String>,
    honor_ps1: bool,
    is_background: bool,
    session_id: Option<SessionId>,
    shell_host: Option<ShellHost>,
    prompt_snapshot: Option<String>,
    ai_metadata: Option<String>,
    #[serde(default)]
    is_local: Option<bool>,
    #[serde(default)]
    agent_view_visibility: Option<SerializedAgentViewVisibility>,
}

impl<'de> Deserialize<'de> for SerializedBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = SerializedBlockWire::deserialize(deserializer)?;
        let decoded_original_bytes = wire
            .stylized_output
            .truncated
            .then_some(wire.stylized_output.original_len)
            .filter(|original_bytes| *original_bytes > 0);
        let stylized_output_original_bytes =
            match (wire.stylized_output_original_bytes, decoded_original_bytes) {
                (Some(persisted), Some(decoded)) => Some(persisted.max(decoded)),
                (persisted, decoded) => persisted.or(decoded),
            };

        Ok(Self {
            id: wire.id,
            stylized_command: wire.stylized_command.bytes,
            stylized_output: wire.stylized_output.bytes,
            stylized_output_original_bytes,
            stylized_output_truncated: wire.stylized_output_truncated
                || wire.stylized_output.truncated,
            pwd: wire.pwd,
            git_head: wire.git_head,
            git_branch_name: wire.git_branch_name,
            virtual_env: wire.virtual_env,
            conda_env: wire.conda_env,
            node_version: wire.node_version,
            exit_code: wire.exit_code,
            did_execute: wire.did_execute,
            completed_ts: wire.completed_ts,
            start_ts: wire.start_ts,
            ps1: wire.ps1,
            rprompt: wire.rprompt,
            honor_ps1: wire.honor_ps1,
            is_background: wire.is_background,
            session_id: wire.session_id,
            shell_host: wire.shell_host,
            prompt_snapshot: wire.prompt_snapshot,
            ai_metadata: wire.ai_metadata,
            is_local: wire.is_local,
            agent_view_visibility: wire.agent_view_visibility,
        })
    }
}

const TRUNCATED_OUTPUT_PREFIX: &[u8] = b"\x1b[0m[older output truncated]\r\n";
const UNAVAILABLE_OUTPUT: &[u8] = b"\x1b[0m[stored output unavailable]\r\n\x18\x1b[0m";
const TERMINAL_STYLE_RESET: &[u8] = b"\x18\x1b[0m";

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BoundedStylizedBytes {
    pub bytes: Vec<u8>,
    pub original_len: usize,
    pub truncated: bool,
}

pub(crate) fn bound_stylized_output(bytes: Vec<u8>, max_bytes: usize) -> BoundedStylizedBytes {
    let original_len = bytes.len();
    bound_retained_stylized_output(bytes, original_len, max_bytes)
}

fn bound_retained_stylized_output(
    bytes: Vec<u8>,
    original_len: usize,
    max_bytes: usize,
) -> BoundedStylizedBytes {
    if original_len <= max_bytes {
        return BoundedStylizedBytes {
            bytes,
            original_len,
            truncated: false,
        };
    }

    if max_bytes <= TRUNCATED_OUTPUT_PREFIX.len() + TERMINAL_STYLE_RESET.len() {
        return BoundedStylizedBytes {
            bytes: if max_bytes >= TERMINAL_STYLE_RESET.len() {
                TERMINAL_STYLE_RESET.to_vec()
            } else {
                Vec::new()
            },
            original_len,
            truncated: true,
        };
    }

    let retained_bytes = max_bytes - TRUNCATED_OUTPUT_PREFIX.len() - TERMINAL_STYLE_RESET.len();
    let initial_start = bytes.len().saturating_sub(retained_bytes);
    let search_end = (initial_start + retained_bytes / 4).min(bytes.len());
    let start = bytes[initial_start..search_end]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(initial_start, |offset| initial_start + offset + 1);
    let mut bounded = Vec::with_capacity(max_bytes);
    bounded.extend_from_slice(TRUNCATED_OUTPUT_PREFIX);
    bounded.extend_from_slice(&bytes[start..]);
    if bounded.len() + TERMINAL_STYLE_RESET.len() > max_bytes {
        bounded.truncate(max_bytes - TERMINAL_STYLE_RESET.len());
    }
    bounded.extend_from_slice(TERMINAL_STYLE_RESET);

    BoundedStylizedBytes {
        bytes: bounded,
        original_len,
        truncated: true,
    }
}

fn bound_stylized_command(mut bytes: Vec<u8>) -> Vec<u8> {
    if bytes.len() <= MAX_SERIALIZED_STYLIZED_COMMAND_BYTES {
        return bytes;
    }
    bytes.truncate(MAX_SERIALIZED_STYLIZED_COMMAND_BYTES - TERMINAL_STYLE_RESET.len());
    bytes.extend_from_slice(TERMINAL_STYLE_RESET);
    bytes
}

fn serialize_stylized_bytes<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_bytes(bytes)
}

#[derive(Clone, Copy)]
enum StylizedByteKind {
    Command,
    Output,
}

struct BoundedByteCollector {
    kind: StylizedByteKind,
    bytes: VecDeque<u8>,
    original_len: usize,
}

impl BoundedByteCollector {
    fn new(kind: StylizedByteKind) -> Self {
        Self {
            kind,
            bytes: VecDeque::new(),
            original_len: 0,
        }
    }

    fn max_bytes(&self) -> usize {
        match self.kind {
            StylizedByteKind::Command => MAX_SERIALIZED_STYLIZED_COMMAND_BYTES,
            StylizedByteKind::Output => MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES,
        }
    }

    fn push(&mut self, byte: u8) {
        self.original_len = self.original_len.saturating_add(1);
        let max_bytes = self.max_bytes();
        match self.kind {
            StylizedByteKind::Command if self.bytes.len() < max_bytes => {
                self.bytes.push_back(byte);
            }
            StylizedByteKind::Command => {}
            StylizedByteKind::Output => {
                if self.bytes.len() == max_bytes {
                    self.bytes.pop_front();
                }
                if max_bytes > 0 {
                    self.bytes.push_back(byte);
                }
            }
        }
    }

    fn extend(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.push(*byte);
        }
    }

    fn finish(self) -> BoundedStylizedBytes {
        let bytes = self.bytes.into_iter().collect::<Vec<_>>();
        match self.kind {
            StylizedByteKind::Command if self.original_len > bytes.len() => {
                let mut bytes = bytes;
                bytes.truncate(MAX_SERIALIZED_STYLIZED_COMMAND_BYTES - TERMINAL_STYLE_RESET.len());
                bytes.extend_from_slice(TERMINAL_STYLE_RESET);
                BoundedStylizedBytes {
                    bytes,
                    original_len: self.original_len,
                    truncated: true,
                }
            }
            StylizedByteKind::Command => BoundedStylizedBytes {
                bytes,
                original_len: self.original_len,
                truncated: false,
            },
            StylizedByteKind::Output => bound_retained_stylized_output(
                bytes,
                self.original_len,
                MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES,
            ),
        }
    }
}

struct LenientByte(Option<u8>);

struct LenientByteVisitor;

impl<'de> Visitor<'de> for LenientByteVisitor {
    type Value = LenientByte;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an integer byte")
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(LenientByte(u8::try_from(value).ok()))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(LenientByte(u8::try_from(value).ok()))
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(LenientByte(None))
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(LenientByte(None))
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(LenientByte(None))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(LenientByte(None))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(LenientByte(None))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence.next_element::<IgnoredAny>()?.is_some() {}
        Ok(LenientByte(None))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(LenientByte(None))
    }
}

impl<'de> Deserialize<'de> for LenientByte {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(LenientByteVisitor)
    }
}

struct BoundedBytesVisitor {
    kind: StylizedByteKind,
}

impl BoundedBytesVisitor {
    fn collect_base64<E>(self, value: &str) -> Result<BoundedStylizedBytes, E>
    where
        E: serde::de::Error,
    {
        if !value.len().is_multiple_of(4) {
            return match self.kind {
                StylizedByteKind::Output => Ok(BoundedStylizedBytes {
                    bytes: UNAVAILABLE_OUTPUT.to_vec(),
                    original_len: 0,
                    truncated: true,
                }),
                StylizedByteKind::Command => Err(E::custom("base64 byte field has invalid length")),
            };
        }
        let mut collector = BoundedByteCollector::new(self.kind);
        let mut decoded = [0; 3];
        for chunk in value.as_bytes().chunks_exact(4) {
            let written =
                match base64::engine::general_purpose::STANDARD.decode_slice(chunk, &mut decoded) {
                    Ok(written) => written,
                    Err(_) if matches!(self.kind, StylizedByteKind::Output) => {
                        return Ok(BoundedStylizedBytes {
                            bytes: UNAVAILABLE_OUTPUT.to_vec(),
                            original_len: 0,
                            truncated: true,
                        });
                    }
                    Err(error) => return Err(E::custom(error)),
                };
            collector.extend(&decoded[..written]);
        }
        Ok(collector.finish())
    }
}

impl<'de> Visitor<'de> for BoundedBytesVisitor {
    type Value = BoundedStylizedBytes;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a base64 string or byte array")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.collect_base64(value)
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.collect_base64(&value)
    }

    fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        let mut collector = BoundedByteCollector::new(self.kind);
        collector.extend(value);
        Ok(collector.finish())
    }

    fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_bytes(&value)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut collector = BoundedByteCollector::new(self.kind);
        let mut malformed = false;
        while let Some(LenientByte(byte)) = sequence.next_element()? {
            match byte {
                Some(byte) => collector.push(byte),
                None => {
                    collector.original_len = collector.original_len.saturating_add(1);
                    malformed = true;
                }
            }
        }
        if !malformed {
            return Ok(collector.finish());
        }
        match self.kind {
            StylizedByteKind::Output => Ok(BoundedStylizedBytes {
                bytes: UNAVAILABLE_OUTPUT.to_vec(),
                original_len: collector.original_len,
                truncated: true,
            }),
            StylizedByteKind::Command => Err(serde::de::Error::custom(
                "command byte array contains a non-byte value",
            )),
        }
    }
}

fn deserialize_stylized_command<'de, D>(deserializer: D) -> Result<BoundedStylizedBytes, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(BoundedBytesVisitor {
        kind: StylizedByteKind::Command,
    })
}

fn deserialize_stylized_output<'de, D>(deserializer: D) -> Result<BoundedStylizedBytes, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(BoundedBytesVisitor {
        kind: StylizedByteKind::Output,
    })
}

impl SerializedBlock {
    pub(crate) fn enforce_command_budget(&mut self) {
        self.stylized_command = bound_stylized_command(std::mem::take(&mut self.stylized_command));
    }

    pub(crate) fn enforce_output_budget(&mut self, max_bytes: usize) -> (usize, bool) {
        let was_truncated = self.stylized_output_truncated
            || self.stylized_output.starts_with(TRUNCATED_OUTPUT_PREFIX)
            || self.stylized_output == UNAVAILABLE_OUTPUT;
        let bounded = bound_stylized_output(std::mem::take(&mut self.stylized_output), max_bytes);
        let original_len = self
            .stylized_output_original_bytes
            .unwrap_or(bounded.original_len)
            .max(bounded.original_len);
        self.stylized_output_truncated = was_truncated || bounded.truncated;
        if bounded.truncated || (was_truncated && self.stylized_output_original_bytes.is_some()) {
            self.stylized_output_original_bytes = Some(original_len);
        }
        self.stylized_output = bounded.bytes;
        (original_len, self.stylized_output_truncated)
    }

    /// Sets the command & output and `did_execute` to true.
    /// Everything else is a default value.
    #[cfg(any(test, feature = "test-util"))]
    pub fn new_for_test(stylized_command: Vec<u8>, stylized_output: Vec<u8>) -> SerializedBlock {
        SerializedBlock {
            stylized_command,
            stylized_output,
            did_execute: true,
            start_ts: Some(Local::now()),
            completed_ts: Some(Local::now()),
            ..Default::default()
        }
    }

    /// Sets only the command with no output, and `did_execute` to false.
    /// Everything else is a default value.
    #[cfg(test)]
    pub fn new_active_block_for_test() -> SerializedBlock {
        SerializedBlock::default()
    }

    /// Serialize this block to JSON bytes.
    ///
    /// The command and output contents are base64-encoded. This is *not* the default serde behavior,
    /// and blocks encoded this way must be deserialized with [`Self::from_json`].
    pub fn to_json(&self) -> anyhow::Result<Vec<u8>> {
        let mut buf = Vec::new();
        let mut ser = serde_json::Serializer::new(&mut buf);
        let base64_config = base64::engine::GeneralPurposeConfig::new();
        let base64_ser =
            ByteFmtSerializer::base64(&mut ser, base64::alphabet::STANDARD, base64_config);
        self.serialize(base64_ser)
            .map_err(|e| anyhow::anyhow!("Failed to serialize block to JSON: {e}"))?;
        Ok(buf)
    }

    /// Deserialize a block from JSON bytes produced by [`Self::to_json`]
    /// or [`serde_json`].
    pub fn from_json(json: &[u8]) -> anyhow::Result<Self> {
        serde_json::from_slice(json)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize block from JSON: {e}"))
    }

    pub fn has_failed(&self) -> bool {
        let block_state = match self.did_execute {
            true => BlockState::DoneWithExecution,
            false => BlockState::DoneWithNoExecution,
        };
        has_block_failed(self.exit_code, block_state)
    }
}

/// We should only be serializing a block that has finished.
impl From<&Block> for SerializedBlock {
    fn from(block: &Block) -> Self {
        let stylized_command = bound_stylized_command(
            block
                .command_with_secrets_unobfuscated(true /*include_escape_sequences*/)
                .into_bytes(),
        );
        let stylized_output = bound_stylized_output(
            block
                .output_grid()
                .contents_to_string_with_secrets_unobfuscated(
                    true, /*include_escape_sequences*/
                    Some(MAX_SERIALIZED_STYLIZED_OUTPUT_LINES),
                )
                .into_bytes(),
            MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES,
        );
        let ps1 =
            (!block.is_prompt_empty()).then(|| hex::encode(block.prompt_contents_to_string(true)));
        let rprompt = (!block.rprompt_grid().is_empty()).then(|| {
            let mut grid_content = block.rprompt_grid().contents_to_string(true, None);
            // We mustn't allow trailing newlines in the rprompt grid. This is b/c a trailing
            // newline will cause the Grid::max_cursor::column to go to 0. That is a problem b/c we
            // assume that the Grid::max_cursor::column is the length of the rprompt, and that
            // value is used to calculate the left-alignment position when painting the block.
            grid_content.trim_trailing_newline();
            hex::encode(grid_content)
        });
        let prompt_snapshot = block
            .prompt_snapshot
            .as_ref()
            .and_then(|prompt_snapshot| serde_json::to_string(prompt_snapshot).ok());
        let prompt_info = PromptInfo {
            pwd: block.pwd().map(String::from),
            git_branch: block.git_branch.clone(),
            git_branch_name: block.git_branch_name.clone(),
            virtual_env: block.virtual_env.clone(),
            conda_env: block.conda_env.clone(),
            node_version: block.node_version.clone(),
            ps1,
            rprompt,
            honor_ps1: block.honor_ps1(),
            prompt_snapshot,
        };

        let ai_metadata = block
            .agent_interaction_metadata()
            .cloned()
            .map(Into::<SerializedAIMetadata>::into)
            .and_then(|metadata| serde_json::to_string(&metadata).ok());

        SerializedBlock {
            id: block.id.clone(),
            stylized_command,
            stylized_output_original_bytes: stylized_output
                .truncated
                .then_some(stylized_output.original_len),
            stylized_output_truncated: stylized_output.truncated,
            stylized_output: stylized_output.bytes,
            pwd: prompt_info.pwd,
            git_head: prompt_info.git_branch,
            git_branch_name: prompt_info.git_branch_name,
            virtual_env: prompt_info.virtual_env,
            conda_env: prompt_info.conda_env,
            node_version: prompt_info.node_version,
            exit_code: block.exit_code,
            did_execute: block.state == BlockState::Executing
                || block.state == BlockState::DoneWithExecution
                || block.state == BlockState::Static,
            completed_ts: block.completed_ts,
            start_ts: block.start_ts,
            is_background: block.is_background(),
            ps1: prompt_info.ps1,
            rprompt: prompt_info.rprompt,
            honor_ps1: prompt_info.honor_ps1,
            session_id: block.session_id,
            shell_host: block.shell_host.clone(),
            prompt_snapshot: prompt_info.prompt_snapshot,
            ai_metadata,
            is_local: None,
            agent_view_visibility: Some(block.agent_view_visibility().clone().into()),
        }
    }
}

impl From<crate::persistence::model::Block> for SerializedBlock {
    fn from(block: crate::persistence::model::Block) -> Self {
        let exit_code = ExitCode::from(block.exit_code);
        let shell_host = ShellHost::try_from_persisted_block(&block);
        let stylized_command = bound_stylized_command(block.stylized_command);
        let stylized_output =
            bound_stylized_output(block.stylized_output, MAX_SERIALIZED_STYLIZED_OUTPUT_BYTES);
        Self {
            shell_host,
            id: block.block_id.into(),
            stylized_command,
            stylized_output_original_bytes: stylized_output
                .truncated
                .then_some(stylized_output.original_len),
            stylized_output_truncated: stylized_output.truncated,
            stylized_output: stylized_output.bytes,
            pwd: block.pwd,
            git_head: block.git_branch,
            git_branch_name: block.git_branch_name,
            virtual_env: block.virtual_env,
            conda_env: block.conda_env,
            node_version: None, // Database does not store node_version yet
            exit_code,
            did_execute: block.did_execute,
            completed_ts: block
                .completed_ts
                .map(|naive_ts| Local.from_utc_datetime(&naive_ts)),
            start_ts: block
                .start_ts
                .map(|naive_ts| Local.from_utc_datetime(&naive_ts)),
            ps1: block.ps1,
            rprompt: block.rprompt,
            honor_ps1: block.honor_ps1,
            session_id: None,
            is_background: block.is_background,
            prompt_snapshot: block.prompt_snapshot,
            ai_metadata: block.ai_metadata,
            is_local: block.is_local,
            agent_view_visibility: block
                .agent_view_visibility
                .and_then(|json| serde_json::from_str(&json).ok()),
        }
    }
}

#[cfg(test)]
#[path = "serialized_block_tests.rs"]
mod tests;
