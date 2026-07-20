use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Result, bail, ensure};
use async_trait::async_trait;
use regex::Regex;

use crate::{artifact::ArtifactRef, model::MessageContent, storage::RunDirStore};

use super::{
    HistoryMatch, HistoryMatchSource, HistoryReadMessage, HistoryReadRequest, HistoryReadResult,
    HistorySearchRequest, HistorySearchResult, TrajectoryMessage, TrajectoryReader,
    artifacts::{LocalArtifactSearch, LocalRunArtifactSource},
    history_tool_result_message_indices, is_history_tool, message_ref, snippet_around_match,
};

struct ToolPair {
    call_message_index: usize,
    result_message_index: usize,
}

/// Local retrieval over the compacted prefix in one RunDirStore.
pub struct LocalTrajectoryReader {
    store: RunDirStore,
    artifacts: LocalRunArtifactSource,
}

impl LocalTrajectoryReader {
    pub fn new(store: RunDirStore) -> Self {
        let artifacts = LocalRunArtifactSource::new(store.workspace());
        Self { store, artifacts }
    }

    async fn messages(&self, run_id: &str) -> Result<Vec<TrajectoryMessage>> {
        self.store.load_compacted_history(run_id).await
    }
}

#[async_trait]
impl TrajectoryReader for LocalTrajectoryReader {
    async fn search(&self, request: HistorySearchRequest) -> Result<HistorySearchResult> {
        let messages = self.messages(&request.run_id).await?;
        let mut artifact_search = self.artifacts.begin_search(&request.run_id).await?;
        search_messages(
            messages,
            Some(&mut artifact_search),
            &request.pattern,
            request.max_matches,
        )
        .await
    }

    async fn read(&self, request: HistoryReadRequest) -> Result<HistoryReadResult> {
        read_messages(self.messages(&request.run_id).await?, request)
    }
}

pub(super) async fn search_messages(
    messages: Vec<TrajectoryMessage>,
    mut artifact_search: Option<&mut LocalArtifactSearch>,
    pattern: &Regex,
    max_matches: usize,
) -> Result<HistorySearchResult> {
    if max_matches == 0 {
        bail!("history search max_matches must be greater than zero");
    }
    let messages = project_searchable_messages(messages)?;
    let mut matches = Vec::with_capacity(max_matches.min(messages.len()));
    let mut truncated = false;

    for record in messages.iter().rev() {
        let found = match match_message(record, pattern) {
            Some(found) => Some(found),
            None => match artifact_search.as_deref_mut() {
                Some(artifacts) => {
                    let artifact_refs = artifact_refs(record)?;
                    artifacts
                        .find(&artifact_refs, pattern)
                        .await?
                        .map(|snippet| HistoryMatch {
                            message_ref: record.message_ref.clone(),
                            match_source: HistoryMatchSource::Artifact,
                            snippet,
                        })
                }
                None => None,
            },
        };
        let Some(found) = found else {
            continue;
        };
        if matches.len() == max_matches {
            truncated = true;
            break;
        }
        matches.push(found);
    }

    Ok(HistorySearchResult { matches, truncated })
}

pub(super) fn read_messages(
    messages: Vec<TrajectoryMessage>,
    request: HistoryReadRequest,
) -> Result<HistoryReadResult> {
    let messages = project_readable_messages(messages)?;
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
        messages: messages[start..end]
            .iter()
            .map(|record| HistoryReadMessage {
                message_ref: record.message_ref.clone(),
                message: record.message.clone(),
            })
            .collect(),
    })
}

fn project_readable_messages(
    mut messages: Vec<TrajectoryMessage>,
) -> Result<Vec<TrajectoryMessage>> {
    messages.sort_by_key(|message| message.seq);
    let mut sequences = HashSet::new();
    for record in &messages {
        ensure!(
            record.message_ref == message_ref(record.seq),
            "trajectory message ref `{}` does not match sequence {}",
            record.message_ref,
            record.seq
        );
        if !sequences.insert(record.seq) {
            bail!(
                "trajectory contains duplicate message sequence {}",
                record.seq
            );
        }
    }

    let hidden_result_messages = history_tool_result_message_indices(&messages);

    Ok(messages
        .into_iter()
        .enumerate()
        .filter_map(|(message_index, mut record)| {
            record.message.content.retain(|content| match content {
                MessageContent::RuntimeReminder { .. } | MessageContent::ProviderItem { .. } => {
                    false
                }
                MessageContent::Reasoning { .. } => true,
                MessageContent::ToolCall { name, .. } => !is_history_tool(name),
                MessageContent::ToolResult { .. } => {
                    !hidden_result_messages.contains(&message_index)
                }
                MessageContent::BackgroundTaskResult { name, .. } => !is_history_tool(name),
                MessageContent::Text { .. } => true,
            });
            (!record.message.content.is_empty()).then_some(record)
        })
        .collect())
}

fn project_searchable_messages(messages: Vec<TrajectoryMessage>) -> Result<Vec<TrajectoryMessage>> {
    Ok(project_readable_messages(messages)?
        .into_iter()
        .filter_map(|mut record| {
            record
                .message
                .content
                .retain(|content| !matches!(content, MessageContent::Reasoning { .. }));
            (!record.message.content.is_empty()).then_some(record)
        })
        .collect())
}

fn tool_pairs(messages: &[TrajectoryMessage]) -> Vec<ToolPair> {
    let mut pending = HashMap::<String, VecDeque<usize>>::new();
    let mut pairs = Vec::new();
    for (message_index, record) in messages.iter().enumerate() {
        for content in &record.message.content {
            match content {
                MessageContent::ToolCall { id, .. } => {
                    pending
                        .entry(id.clone())
                        .or_default()
                        .push_back(message_index);
                }
                MessageContent::ToolResult { call_id, .. } => {
                    if let Some(call_message_index) =
                        pending.get_mut(call_id).and_then(VecDeque::pop_front)
                    {
                        pairs.push(ToolPair {
                            call_message_index,
                            result_message_index: message_index,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    pairs
}

fn artifact_refs(record: &TrajectoryMessage) -> Result<Vec<&ArtifactRef>> {
    let mut artifacts = Vec::new();
    for content in &record.message.content {
        let (expected_call_id, artifact) = match content {
            MessageContent::ToolResult {
                call_id, metadata, ..
            } => (call_id.clone(), metadata.artifact.as_ref()),
            MessageContent::BackgroundTaskResult {
                task_id, metadata, ..
            } => (format!("background-{task_id}"), metadata.artifact.as_ref()),
            _ => continue,
        };
        if let Some(artifact) = artifact {
            ensure!(
                artifact.call_id == expected_call_id,
                "result metadata artifact call id `{}` does not match `{expected_call_id}`",
                artifact.call_id
            );
            artifacts.push(artifact);
        }
    }
    Ok(artifacts)
}

fn match_message(record: &TrajectoryMessage, pattern: &Regex) -> Option<HistoryMatch> {
    record.message.content.iter().find_map(|content| {
        let searchable = match content {
            MessageContent::Text { text } => text.clone(),
            MessageContent::ToolCall {
                name, arguments, ..
            } => format!("{name} {}", serde_json::to_string(arguments).ok()?),
            MessageContent::ToolResult { content, .. } => content.clone(),
            MessageContent::BackgroundTaskResult {
                task_id,
                name,
                status,
                content,
                ..
            } => format!("{task_id} {name} {status} {content}"),
            MessageContent::RuntimeReminder { .. }
            | MessageContent::Reasoning { .. }
            | MessageContent::ProviderItem { .. } => return None,
        };
        let found = pattern.find(&searchable)?;
        Some(HistoryMatch {
            message_ref: record.message_ref.clone(),
            match_source: HistoryMatchSource::Message,
            snippet: snippet_around_match(&searchable, found.start(), found.end()),
        })
    })
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
