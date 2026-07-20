use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::model::{Message, MessageContent};

mod artifacts;
mod local;

pub use local::LocalTrajectoryReader;

const SNIPPET_CONTEXT_CHARS: usize = 120;
const HISTORY_TOOL_NAMES: [&str; 2] = ["history_read", "history_search"];

pub(crate) fn is_history_tool(name: &str) -> bool {
    HISTORY_TOOL_NAMES.contains(&name)
}

/// Result-message indexes paired, by occurrence, with internal history calls.
/// Provider call ids may be reused later by ordinary tools, so ids alone are
/// not a safe projection key.
pub(crate) fn history_tool_result_message_indices(
    messages: &[TrajectoryMessage],
) -> HashSet<usize> {
    let mut pending = HashMap::<&str, VecDeque<bool>>::new();
    let mut hidden = HashSet::new();
    for (message_index, record) in messages.iter().enumerate() {
        for content in &record.message.content {
            match content {
                MessageContent::ToolCall { id, name, .. } => pending
                    .entry(id)
                    .or_default()
                    .push_back(is_history_tool(name)),
                MessageContent::ToolResult { call_id, .. }
                    if pending
                        .get_mut(call_id.as_str())
                        .and_then(VecDeque::pop_front)
                        == Some(true) =>
                {
                    hidden.insert(message_index);
                }
                _ => {}
            }
        }
    }
    hidden
}

/// A completed message with a stable identity in an append-only trajectory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryMessage {
    pub message_ref: String,
    pub seq: u64,
    pub created_at: DateTime<Utc>,
    pub message: Message,
    /// Local context-management metadata stored only in message_metadata.jsonl.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompactionMessage {
    Request,
    State { state: CompactionState },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CompactionState {
    pub covered_through_message_ref: String,
    pub first_kept_message_ref: String,
}

impl TrajectoryMessage {
    pub fn compaction_state(&self) -> Option<&CompactionState> {
        match &self.compaction {
            Some(CompactionMessage::State { state }) => Some(state),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HistorySearchRequest {
    pub run_id: String,
    pub pattern: Regex,
    pub max_matches: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryMatchSource {
    Message,
    Artifact,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryMatch {
    #[serde(rename = "ref")]
    pub message_ref: String,
    #[serde(rename = "source")]
    pub match_source: HistoryMatchSource,
    pub snippet: String,
}

#[derive(Debug, Clone)]
pub struct HistorySearchResult {
    pub matches: Vec<HistoryMatch>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct HistoryReadRequest {
    pub run_id: String,
    pub message_ref: String,
    pub before: usize,
    pub after: usize,
}

#[derive(Debug, Clone)]
pub struct HistoryReadMessage {
    pub message_ref: String,
    pub message: Message,
}

#[derive(Debug, Clone)]
pub struct HistoryReadResult {
    pub messages: Vec<HistoryReadMessage>,
}

/// Provider-neutral read access used by the two model-facing history tools.
/// Remote stores can implement this trait directly without exposing paths or
/// database identifiers to the model.
#[async_trait]
pub trait TrajectoryReader: Send + Sync {
    async fn search(&self, request: HistorySearchRequest) -> Result<HistorySearchResult>;
    async fn read(&self, request: HistoryReadRequest) -> Result<HistoryReadResult>;
}

pub(super) fn snippet_around_match(text: &str, start: usize, end: usize) -> String {
    let match_start = text[..start].chars().count();
    let match_len = text[start..end].chars().count();
    let chars = text.chars().collect::<Vec<_>>();
    let snippet_start = match_start.saturating_sub(SNIPPET_CONTEXT_CHARS);
    let snippet_end = match_start
        .saturating_add(match_len)
        .saturating_add(SNIPPET_CONTEXT_CHARS)
        .min(chars.len());
    let mut snippet = chars[snippet_start..snippet_end].iter().collect::<String>();
    if snippet_start > 0 {
        snippet.insert(0, '…');
    }
    if snippet_end < chars.len() {
        snippet.push('…');
    }
    snippet
}

#[cfg(test)]
#[path = "trajectory/tests.rs"]
mod tests;
