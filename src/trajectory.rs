use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::model::{Message, Role};

mod artifacts;
mod local;

pub use artifacts::LocalRunArtifactSource;
pub use local::LocalTrajectoryReader;

const SNIPPET_CONTEXT_CHARS: usize = 120;
const HISTORY_TOOL_NAMES: [&str; 2] = ["history_read", "history_search"];

pub(crate) fn is_history_tool(name: &str) -> bool {
    HISTORY_TOOL_NAMES.contains(&name)
}

/// A completed message with a stable identity in an append-only trajectory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryMessage {
    pub message_ref: String,
    pub seq: u64,
    pub created_at: DateTime<Utc>,
    pub message: Message,
}

/// The compacted prefix visible to history retrieval for one run.
///
/// The source, rather than the reader, owns the compaction boundary. It must
/// not return messages that are still present in the active model context.
#[derive(Debug, Clone, Default)]
pub struct CompactedHistory {
    pub messages: Vec<TrajectoryMessage>,
}

#[async_trait]
pub trait CompactedHistorySource: Send + Sync {
    async fn load_compacted_history(&self, run_id: &str) -> Result<CompactedHistory>;
}

/// Immutable artifact identity linked from one completed result message.
///
/// `sha256` is optional only for older or external trajectories whose message
/// does not carry a picoagent artifact envelope. Implementations must not guess
/// between multiple artifacts that share such a lookup's call id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactLookup {
    pub call_id: String,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ArtifactSearchMatch {
    pub lookup: ArtifactLookup,
    pub snippet: String,
}

/// Optional full-text access to immutable result artifacts. The trajectory
/// projection resolves picoagent's known envelope to this identity before the
/// storage implementation opens any sidecar or content.
#[async_trait]
pub trait TrajectoryArtifactSource: Send + Sync {
    /// Opens one query-local search session. Implementations should index
    /// cheap metadata here and defer artifact content access until `find`.
    async fn begin_search(&self, run_id: &str) -> Result<Box<dyn TrajectoryArtifactSearch>>;
}

/// A query-local artifact index. The trajectory reader calls `find` in message
/// order so it can stop as soon as the requested matches plus one are known.
#[async_trait]
pub trait TrajectoryArtifactSearch: Send {
    async fn find(
        &mut self,
        lookups: &[ArtifactLookup],
        pattern: &Regex,
    ) -> Result<Option<ArtifactSearchMatch>>;
}

#[derive(Debug, Clone)]
pub struct HistorySearchRequest {
    pub run_id: String,
    pub pattern: Regex,
    pub max_matches: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryMatchKind {
    Text,
    ToolCall,
    ToolResult,
    BackgroundTaskResult,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryMatchSource {
    Message,
    Artifact,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryMatch {
    pub message_ref: String,
    pub seq: u64,
    pub created_at: DateTime<Utc>,
    pub role: Role,
    pub kind: HistoryMatchKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
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
pub struct HistoryReadResult {
    pub anchor_ref: String,
    pub messages: Vec<TrajectoryMessage>,
    pub tool_pairs_expanded: bool,
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
