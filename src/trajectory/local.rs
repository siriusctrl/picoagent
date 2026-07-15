use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::Arc,
};

use anyhow::{Result, bail};
use async_trait::async_trait;
use regex::Regex;

use crate::model::MessageContent;

use super::{
    ArtifactLookup, CompactedHistorySource, HistoryMatch, HistoryMatchKind, HistoryMatchSource,
    HistoryReadRequest, HistoryReadResult, HistorySearchRequest, HistorySearchResult,
    LocalRunArtifactSource, TrajectoryArtifactSource, TrajectoryMessage, TrajectoryReader,
    is_history_tool, snippet_around_match,
};

type ToolResultNames = HashMap<(u64, usize), String>;

struct ToolPair {
    call_message_index: usize,
    result_message_index: usize,
    result_key: (u64, usize),
    tool_name: String,
}

struct MessageArtifactTarget {
    lookup: ArtifactLookup,
    kind: HistoryMatchKind,
    tool_name: Option<String>,
}

/// Local retrieval over completed messages supplied by the active compaction
/// checkpoint. The source remains responsible for persistence and updates.
pub struct LocalTrajectoryReader {
    source: Arc<dyn CompactedHistorySource>,
    artifacts: Option<Arc<dyn TrajectoryArtifactSource>>,
}

impl LocalTrajectoryReader {
    pub fn new(source: Arc<dyn CompactedHistorySource>) -> Self {
        Self {
            source,
            artifacts: None,
        }
    }

    pub fn with_artifacts(
        source: Arc<dyn CompactedHistorySource>,
        artifacts: Arc<dyn TrajectoryArtifactSource>,
    ) -> Self {
        Self {
            source,
            artifacts: Some(artifacts),
        }
    }

    pub fn with_local_artifacts(
        source: Arc<dyn CompactedHistorySource>,
        workspace: impl Into<PathBuf>,
    ) -> Self {
        Self::with_artifacts(source, Arc::new(LocalRunArtifactSource::new(workspace)))
    }

    async fn messages(&self, run_id: &str) -> Result<Vec<TrajectoryMessage>> {
        let history = self.source.load_compacted_history(run_id).await?;
        project_retrievable_messages(history.messages)
    }
}

#[async_trait]
impl TrajectoryReader for LocalTrajectoryReader {
    async fn search(&self, request: HistorySearchRequest) -> Result<HistorySearchResult> {
        if request.max_matches == 0 {
            bail!("history search max_matches must be greater than zero");
        }
        let messages = self.messages(&request.run_id).await?;
        let tool_names = tool_result_names(&messages);
        let mut artifact_search = match &self.artifacts {
            Some(artifacts) => Some(artifacts.begin_search(&request.run_id).await?),
            None => None,
        };
        let mut matches = Vec::with_capacity(request.max_matches.min(messages.len()));
        let mut truncated = false;

        for record in messages.iter().rev() {
            let found = match match_message(record, &request.pattern, &tool_names) {
                Some(found) => Some(found),
                None => match artifact_search.as_mut() {
                    Some(artifacts) => {
                        let targets = artifact_targets(record, &tool_names);
                        let lookups = targets
                            .iter()
                            .map(|target| target.lookup.clone())
                            .collect::<Vec<_>>();
                        artifacts
                            .find(&lookups, &request.pattern)
                            .await?
                            .map(|found| artifact_history_match(record, found, &targets))
                    }
                    None => None,
                },
            };
            let Some(found) = found else {
                continue;
            };
            if matches.len() == request.max_matches {
                truncated = true;
                break;
            }
            matches.push(found);
        }

        Ok(HistorySearchResult { matches, truncated })
    }

    async fn read(&self, request: HistoryReadRequest) -> Result<HistoryReadResult> {
        let messages = self.messages(&request.run_id).await?;
        let Some(anchor) = messages
            .iter()
            .position(|message| message.message_ref == request.message_ref)
        else {
            bail!("message ref is not available in compacted history");
        };

        let requested_start = anchor.saturating_sub(request.before);
        let requested_end = anchor
            .saturating_add(request.after)
            .saturating_add(1)
            .min(messages.len());
        let (start, end) = expand_for_tool_pairs(&messages, requested_start, requested_end);

        Ok(HistoryReadResult {
            anchor_ref: request.message_ref,
            messages: messages[start..end].to_vec(),
            tool_pairs_expanded: start != requested_start || end != requested_end,
        })
    }
}

fn project_retrievable_messages(
    mut messages: Vec<TrajectoryMessage>,
) -> Result<Vec<TrajectoryMessage>> {
    messages.sort_by_key(|message| message.seq);
    let mut refs = HashSet::new();
    let mut sequences = HashSet::new();
    for record in &messages {
        if record.message_ref.is_empty() {
            bail!("trajectory contains an empty message ref");
        }
        if !refs.insert(record.message_ref.as_str()) {
            bail!(
                "trajectory contains duplicate message ref `{}`",
                record.message_ref
            );
        }
        if !sequences.insert(record.seq) {
            bail!(
                "trajectory contains duplicate message sequence {}",
                record.seq
            );
        }
    }

    let hidden_call_ids = messages
        .iter()
        .flat_map(|record| &record.message.content)
        .filter_map(|content| match content {
            MessageContent::ToolCall { id, name, .. } if is_history_tool(name) => Some(id.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();

    Ok(messages
        .into_iter()
        .filter_map(|mut record| {
            record.message.content.retain(|content| match content {
                MessageContent::RuntimeReminder { .. }
                | MessageContent::Reasoning { .. }
                | MessageContent::ProviderItem { .. } => false,
                MessageContent::ToolCall { name, .. } => !is_history_tool(name),
                MessageContent::ToolResult { call_id, .. } => !hidden_call_ids.contains(call_id),
                MessageContent::BackgroundTaskResult { name, .. } => !is_history_tool(name),
                MessageContent::Text { .. } => true,
            });
            (!record.message.content.is_empty()).then_some(record)
        })
        .collect())
}

fn tool_result_names(messages: &[TrajectoryMessage]) -> ToolResultNames {
    tool_pairs(messages)
        .into_iter()
        .map(|pair| (pair.result_key, pair.tool_name))
        .collect()
}

fn tool_pairs(messages: &[TrajectoryMessage]) -> Vec<ToolPair> {
    let mut pending = HashMap::<String, VecDeque<(usize, String)>>::new();
    let mut pairs = Vec::new();
    for (message_index, record) in messages.iter().enumerate() {
        for (index, content) in record.message.content.iter().enumerate() {
            match content {
                MessageContent::ToolCall { id, name, .. } => {
                    pending
                        .entry(id.clone())
                        .or_default()
                        .push_back((message_index, name.clone()));
                }
                MessageContent::ToolResult { call_id, .. } => {
                    if let Some((call_message_index, tool_name)) =
                        pending.get_mut(call_id).and_then(VecDeque::pop_front)
                    {
                        pairs.push(ToolPair {
                            call_message_index,
                            result_message_index: message_index,
                            result_key: (record.seq, index),
                            tool_name,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    pairs
}

fn artifact_targets(
    record: &TrajectoryMessage,
    tool_names: &ToolResultNames,
) -> Vec<MessageArtifactTarget> {
    record
        .message
        .content
        .iter()
        .enumerate()
        .filter_map(|(index, content)| match content {
            MessageContent::ToolResult {
                call_id, content, ..
            } => {
                let sha256 = artifact_sha256_from_model_content(content)?;
                Some(MessageArtifactTarget {
                    lookup: ArtifactLookup {
                        call_id: call_id.clone(),
                        sha256: Some(sha256),
                    },
                    kind: HistoryMatchKind::ToolResult,
                    tool_name: tool_names.get(&(record.seq, index)).cloned(),
                })
            }
            MessageContent::BackgroundTaskResult {
                task_id,
                name,
                content,
                ..
            } => {
                let sha256 = artifact_sha256_from_model_content(content)?;
                Some(MessageArtifactTarget {
                    lookup: ArtifactLookup {
                        call_id: format!("background-{task_id}"),
                        sha256: Some(sha256),
                    },
                    kind: HistoryMatchKind::BackgroundTaskResult,
                    tool_name: Some(name.clone()),
                })
            }
            _ => None,
        })
        .collect()
}

fn match_message(
    record: &TrajectoryMessage,
    pattern: &Regex,
    tool_names: &ToolResultNames,
) -> Option<HistoryMatch> {
    record
        .message
        .content
        .iter()
        .enumerate()
        .find_map(|(index, content)| {
            let (searchable, kind, tool_name) = match content {
                MessageContent::Text { text } => (text.clone(), HistoryMatchKind::Text, None),
                MessageContent::ToolCall {
                    name, arguments, ..
                } => (
                    format!("{name} {}", serde_json::to_string(arguments).ok()?),
                    HistoryMatchKind::ToolCall,
                    Some(name.clone()),
                ),
                MessageContent::ToolResult { content, .. } => (
                    content.clone(),
                    HistoryMatchKind::ToolResult,
                    tool_names.get(&(record.seq, index)).cloned(),
                ),
                MessageContent::BackgroundTaskResult {
                    task_id,
                    name,
                    status,
                    content,
                } => (
                    format!("{task_id} {name} {status} {content}"),
                    HistoryMatchKind::BackgroundTaskResult,
                    Some(name.clone()),
                ),
                MessageContent::RuntimeReminder { .. }
                | MessageContent::Reasoning { .. }
                | MessageContent::ProviderItem { .. } => return None,
            };
            let found = pattern.find(&searchable)?;
            Some(HistoryMatch {
                message_ref: record.message_ref.clone(),
                seq: record.seq,
                created_at: record.created_at,
                role: record.message.role.clone(),
                kind,
                tool_name,
                match_source: HistoryMatchSource::Message,
                snippet: snippet_around_match(&searchable, found.start(), found.end()),
            })
        })
}

fn artifact_history_match(
    record: &TrajectoryMessage,
    found: super::ArtifactSearchMatch,
    targets: &[MessageArtifactTarget],
) -> HistoryMatch {
    let target = targets
        .iter()
        .find(|target| target.lookup == found.lookup)
        .expect("artifact search returned a lookup outside its request");
    HistoryMatch {
        message_ref: record.message_ref.clone(),
        seq: record.seq,
        created_at: record.created_at,
        role: record.message.role.clone(),
        kind: target.kind.clone(),
        tool_name: target.tool_name.clone(),
        match_source: HistoryMatchSource::Artifact,
        snippet: found.snippet,
    }
}

pub(super) fn artifact_sha256_from_model_content(content: &str) -> Option<String> {
    let header = if let Some(header) = content.strip_prefix("[Tool output]\n") {
        let header = header
            .split_once("\n\n[Preview]\n")
            .map_or(header, |part| part.0);
        required_fields_present(header, &["truncated: ", "artifact: ", "media_type: "])
            .then_some(header)?
    } else if let Some((_, header)) = content.rsplit_once("\n\n[Full output artifact]\n") {
        required_fields_present(
            header,
            &["truncated: ", "path: ", "media_type: ", "bytes: "],
        )
        .then_some(header)?
    } else {
        let header = content.strip_prefix("[Full output artifact]\n")?;
        required_fields_present(
            header,
            &["truncated: ", "path: ", "media_type: ", "bytes: "],
        )
        .then_some(header)?
    };
    let sha256 = header
        .lines()
        .find_map(|line| line.strip_prefix("sha256: "))?;
    (sha256.len() == 64 && sha256.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| sha256.to_ascii_lowercase())
}

fn required_fields_present(header: &str, fields: &[&str]) -> bool {
    fields
        .iter()
        .all(|field| header.lines().any(|line| line.starts_with(field)))
}

fn expand_for_tool_pairs(
    messages: &[TrajectoryMessage],
    mut start: usize,
    mut end: usize,
) -> (usize, usize) {
    let pairs = tool_pairs(messages);
    loop {
        let mut expanded_start = start;
        let mut expanded_end = end;
        for pair in &pairs {
            let call_selected = (start..end).contains(&pair.call_message_index);
            let result_selected = (start..end).contains(&pair.result_message_index);
            if call_selected || result_selected {
                expanded_start = expanded_start
                    .min(pair.call_message_index)
                    .min(pair.result_message_index);
                expanded_end = expanded_end
                    .max(pair.call_message_index + 1)
                    .max(pair.result_message_index + 1);
            }
        }

        if expanded_start == start && expanded_end == end {
            return (start, end);
        }
        start = expanded_start;
        end = expanded_end;
    }
}
