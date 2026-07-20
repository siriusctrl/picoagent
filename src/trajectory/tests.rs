use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;
use chrono::DateTime;
use regex::Regex;
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::tempdir;

use crate::{
    artifact::{ArtifactRef, ResultMetadata},
    model::{Message, MessageContent, Role},
};

use super::*;
use super::{
    artifacts::LocalRunArtifactSource,
    local::{read_messages, search_messages},
};

struct StaticSource(Vec<TrajectoryMessage>);

struct StaticReader {
    messages: Vec<TrajectoryMessage>,
    artifact_workspace: Option<PathBuf>,
}

#[async_trait]
impl TrajectoryReader for StaticReader {
    async fn search(&self, request: HistorySearchRequest) -> Result<HistorySearchResult> {
        let mut artifact_search = match &self.artifact_workspace {
            Some(workspace) => Some(
                LocalRunArtifactSource::new(workspace)
                    .begin_search(&request.run_id)
                    .await?,
            ),
            None => None,
        };
        search_messages(
            self.messages.clone(),
            artifact_search.as_mut(),
            &request.pattern,
            request.max_matches,
        )
        .await
    }

    async fn read(&self, request: HistoryReadRequest) -> Result<HistoryReadResult> {
        read_messages(self.messages.clone(), request)
    }
}

fn record(seq: u64, role: Role, content: Vec<MessageContent>) -> TrajectoryMessage {
    TrajectoryMessage {
        message_ref: format!("m{seq}"),
        seq,
        created_at: DateTime::from_timestamp(seq as i64, 0).unwrap(),
        message: Message { role, content },
        pending_input_id: None,
        compaction: None,
    }
}

fn reader(messages: Vec<TrajectoryMessage>) -> StaticReader {
    StaticReader {
        messages,
        artifact_workspace: None,
    }
}

fn reader_with_artifacts(source: StaticSource, workspace: &Path) -> StaticReader {
    StaticReader {
        messages: source.0,
        artifact_workspace: Some(workspace.to_owned()),
    }
}

#[test]
fn message_refs_are_canonical_sequence_addresses() {
    assert_eq!(message_ref(37), "m37");
    assert_eq!(message_ref_seq("m37"), Some(37));
    for invalid in ["", "m", "m0", "m01", "msg_1", "m-1"] {
        assert_eq!(message_ref_seq(invalid), None, "accepted {invalid}");
    }
}

async fn write_artifact(
    workspace: &Path,
    run_id: &str,
    call_id: &str,
    content: &str,
) -> ArtifactRef {
    let sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));
    let stable_name = format!("{call_id}-{}", &sha256[..12]);
    let artifact_directory = workspace.join(format!(".pico/runs/{run_id}/artifacts"));
    tokio::fs::create_dir_all(&artifact_directory)
        .await
        .unwrap();
    let artifact_path = artifact_directory.join(format!("{stable_name}.txt"));
    tokio::fs::write(&artifact_path, content).await.unwrap();
    let artifact = ArtifactRef {
        version: 1,
        artifact_id: format!("sha256:{sha256}"),
        run_id: run_id.to_owned(),
        call_id: call_id.to_owned(),
        path: format!(".pico/runs/{run_id}/artifacts/{stable_name}.txt"),
        media_type: "text/plain; charset=utf-8".to_owned(),
        bytes: content.len() as u64,
        sha256,
    };
    tokio::fs::write(
        artifact_directory.join(format!("{stable_name}.artifact.json")),
        serde_json::to_vec(&artifact).unwrap(),
    )
    .await
    .unwrap();
    artifact
}

fn artifact_metadata(artifact: &ArtifactRef) -> ResultMetadata {
    ResultMetadata {
        artifact: Some(artifact.clone()),
    }
}

#[tokio::test]
async fn search_is_newest_first_and_reports_a_limit() {
    let reader = reader(vec![
        record(
            1,
            Role::User,
            vec![MessageContent::Text {
                text: "alpha old".to_owned(),
            }],
        ),
        record(
            2,
            Role::Assistant,
            vec![MessageContent::Text {
                text: "alpha new".to_owned(),
            }],
        ),
    ]);

    let result = reader
        .search(HistorySearchRequest {
            run_id: "run".to_owned(),
            pattern: Regex::new("alpha").unwrap(),
            max_matches: 1,
        })
        .await
        .unwrap();

    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].message_ref, "m2");
    assert!(result.truncated);
}

#[tokio::test]
async fn search_supports_inline_regex_flags_and_hides_internal_content() {
    let reader = reader(vec![
        record(
            1,
            Role::User,
            vec![
                MessageContent::RuntimeReminder {
                    text: "SECRET reminder".to_owned(),
                },
                MessageContent::Text {
                    text: "Visible Answer".to_owned(),
                },
            ],
        ),
        record(
            2,
            Role::Assistant,
            vec![MessageContent::ToolCall {
                id: "internal-call".to_owned(),
                name: "history_search".to_owned(),
                arguments: json!({"pattern": "Visible"}),
            }],
        ),
        record(
            3,
            Role::Tool,
            vec![MessageContent::ToolResult {
                call_id: "internal-call".to_owned(),
                content: "Visible recursive output".to_owned(),
                is_error: false,
                metadata: ResultMetadata::empty(),
            }],
        ),
    ]);

    let visible = reader
        .search(HistorySearchRequest {
            run_id: "run".to_owned(),
            pattern: Regex::new("(?i)visible answer").unwrap(),
            max_matches: 10,
        })
        .await
        .unwrap();
    assert_eq!(visible.matches.len(), 1);
    assert_eq!(visible.matches[0].message_ref, "m1");

    let hidden = reader
        .search(HistorySearchRequest {
            run_id: "run".to_owned(),
            pattern: Regex::new("SECRET|recursive").unwrap(),
            max_matches: 10,
        })
        .await
        .unwrap();
    assert!(hidden.matches.is_empty());
}

#[tokio::test]
async fn read_returns_a_contiguous_window_and_keeps_tool_pairs() {
    let reader = reader(vec![
        record(
            1,
            Role::User,
            vec![MessageContent::Text {
                text: "first".to_owned(),
            }],
        ),
        record(
            2,
            Role::Assistant,
            vec![MessageContent::ToolCall {
                id: "call-1".to_owned(),
                name: "bash".to_owned(),
                arguments: json!({"command": "cargo test"}),
            }],
        ),
        record(
            3,
            Role::Tool,
            vec![MessageContent::ToolResult {
                call_id: "call-1".to_owned(),
                content: "ok".to_owned(),
                is_error: false,
                metadata: ResultMetadata::empty(),
            }],
        ),
        record(
            4,
            Role::Assistant,
            vec![MessageContent::Text {
                text: "done".to_owned(),
            }],
        ),
    ]);

    let result = reader
        .read(HistoryReadRequest {
            run_id: "run".to_owned(),
            message_ref: "m2".to_owned(),
            before: 0,
            after: 0,
        })
        .await
        .unwrap();

    assert_eq!(
        result
            .messages
            .iter()
            .map(|message| message.message_ref.as_str())
            .collect::<Vec<_>>(),
        ["m2", "m3"]
    );
}

#[tokio::test]
async fn read_pairs_reused_call_ids_by_occurrence() {
    let reader = reader(vec![
        record(
            1,
            Role::Assistant,
            vec![MessageContent::ToolCall {
                id: "reused".to_owned(),
                name: "bash".to_owned(),
                arguments: json!({"command": "old"}),
            }],
        ),
        record(
            2,
            Role::Tool,
            vec![MessageContent::ToolResult {
                call_id: "reused".to_owned(),
                content: "old result".to_owned(),
                is_error: false,
                metadata: ResultMetadata::empty(),
            }],
        ),
        record(
            3,
            Role::Assistant,
            vec![MessageContent::ToolCall {
                id: "reused".to_owned(),
                name: "read".to_owned(),
                arguments: json!({"path": "new"}),
            }],
        ),
        record(
            4,
            Role::Tool,
            vec![MessageContent::ToolResult {
                call_id: "reused".to_owned(),
                content: "new result".to_owned(),
                is_error: false,
                metadata: ResultMetadata::empty(),
            }],
        ),
    ]);

    let result = reader
        .read(HistoryReadRequest {
            run_id: "run".to_owned(),
            message_ref: "m4".to_owned(),
            before: 0,
            after: 0,
        })
        .await
        .unwrap();

    assert_eq!(
        result
            .messages
            .iter()
            .map(|message| message.message_ref.as_str())
            .collect::<Vec<_>>(),
        ["m3", "m4"]
    );
}

#[tokio::test]
async fn history_projection_hides_only_the_matching_reused_call_occurrence() {
    let reader = reader(vec![
        record(
            1,
            Role::Assistant,
            vec![MessageContent::ToolCall {
                id: "reused".to_owned(),
                name: "history_search".to_owned(),
                arguments: json!({"pattern": "old"}),
            }],
        ),
        record(
            2,
            Role::Tool,
            vec![MessageContent::ToolResult {
                call_id: "reused".to_owned(),
                content: "derived internal result".to_owned(),
                is_error: false,
                metadata: ResultMetadata::empty(),
            }],
        ),
        record(
            3,
            Role::Assistant,
            vec![MessageContent::ToolCall {
                id: "reused".to_owned(),
                name: "bash".to_owned(),
                arguments: json!({"command": "real work"}),
            }],
        ),
        record(
            4,
            Role::Tool,
            vec![MessageContent::ToolResult {
                call_id: "reused".to_owned(),
                content: "ordinary durable result".to_owned(),
                is_error: false,
                metadata: ResultMetadata::empty(),
            }],
        ),
    ]);

    let found = reader
        .search(HistorySearchRequest {
            run_id: "run".to_owned(),
            pattern: Regex::new("ordinary durable result").unwrap(),
            max_matches: 10,
        })
        .await
        .unwrap();
    assert_eq!(found.matches.len(), 1);
    assert_eq!(found.matches[0].message_ref, "m4");

    let hidden = reader
        .search(HistorySearchRequest {
            run_id: "run".to_owned(),
            pattern: Regex::new("derived internal result").unwrap(),
            max_matches: 10,
        })
        .await
        .unwrap();
    assert!(hidden.matches.is_empty());
}

#[tokio::test]
async fn read_preserves_reasoning_in_exact_assistant_messages() {
    let reader = reader(vec![record(
        1,
        Role::Assistant,
        vec![
            MessageContent::Reasoning {
                text: "inspect the omitted evidence".to_owned(),
            },
            MessageContent::Text {
                text: "the answer".to_owned(),
            },
        ],
    )]);

    let result = reader
        .read(HistoryReadRequest {
            run_id: "run".to_owned(),
            message_ref: "m1".to_owned(),
            before: 0,
            after: 0,
        })
        .await
        .unwrap();

    assert!(matches!(
        &result.messages[0].message.content[0],
        MessageContent::Reasoning { text } if text == "inspect the omitted evidence"
    ));
}

#[tokio::test]
async fn search_matches_the_complete_tool_result_artifact() {
    let workspace = tempdir().unwrap();
    let content = format!(
        "head\n{}\nneedle only in omitted middle\n{}\ntail",
        "before ".repeat(8_000),
        " after".repeat(8_000)
    );
    let artifact = write_artifact(workspace.path(), "run", "call-1", &content).await;

    let source = StaticSource(vec![
        record(
            1,
            Role::Assistant,
            vec![MessageContent::ToolCall {
                id: "call-1".to_owned(),
                name: "bash".to_owned(),
                arguments: json!({"command": "large-output"}),
            }],
        ),
        record(
            2,
            Role::Tool,
            vec![MessageContent::ToolResult {
                call_id: "call-1".to_owned(),
                content: "bounded preview without artifact identity text".to_owned(),
                is_error: false,
                metadata: artifact_metadata(&artifact),
            }],
        ),
    ]);
    let reader = reader_with_artifacts(source, workspace.path());
    let result = reader
        .search(HistorySearchRequest {
            run_id: "run".to_owned(),
            pattern: Regex::new("needle").unwrap(),
            max_matches: 10,
        })
        .await
        .unwrap();

    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].message_ref, "m2");
    assert_eq!(result.matches[0].match_source, HistoryMatchSource::Artifact);
    assert!(result.matches[0].snippet.contains("needle"));
}

#[tokio::test]
async fn artifact_snippet_trims_partial_multibyte_edges_without_replacement_characters() {
    let workspace = tempdir().unwrap();
    let content = format!("{}needle{}", "界".repeat(600), "文".repeat(600));
    let artifact = write_artifact(workspace.path(), "run", "call-1", &content).await;

    let source = StaticSource(vec![record(
        1,
        Role::Tool,
        vec![MessageContent::ToolResult {
            call_id: "call-1".to_owned(),
            content: "bounded preview".to_owned(),
            is_error: false,
            metadata: artifact_metadata(&artifact),
        }],
    )]);
    let reader = reader_with_artifacts(source, workspace.path());
    let result = reader
        .search(HistorySearchRequest {
            run_id: "run".to_owned(),
            pattern: Regex::new("needle").unwrap(),
            max_matches: 10,
        })
        .await
        .unwrap();

    assert_eq!(result.matches.len(), 1);
    assert!(result.matches[0].snippet.contains("needle"));
    assert!(!result.matches[0].snippet.contains('\u{fffd}'));
}

#[tokio::test]
async fn search_stops_before_an_older_missing_artifact_after_limit_is_known() {
    let workspace = tempdir().unwrap();
    let old_artifact = write_artifact(workspace.path(), "run", "old-call", "unread artifact").await;
    tokio::fs::remove_file(workspace.path().join(&old_artifact.path))
        .await
        .unwrap();

    let source = StaticSource(vec![
        record(
            1,
            Role::Assistant,
            vec![MessageContent::ToolCall {
                id: "old-call".to_owned(),
                name: "bash".to_owned(),
                arguments: json!({"command": "huge-output"}),
            }],
        ),
        record(
            2,
            Role::Tool,
            vec![MessageContent::ToolResult {
                call_id: "old-call".to_owned(),
                content: "preview without the query".to_owned(),
                is_error: false,
                metadata: artifact_metadata(&old_artifact),
            }],
        ),
        record(
            3,
            Role::User,
            vec![MessageContent::Text {
                text: "needle second newest".to_owned(),
            }],
        ),
        record(
            4,
            Role::Assistant,
            vec![MessageContent::Text {
                text: "needle newest".to_owned(),
            }],
        ),
    ]);
    let reader = reader_with_artifacts(source, workspace.path());
    let result = reader
        .search(HistorySearchRequest {
            run_id: "run".to_owned(),
            pattern: Regex::new("needle").unwrap(),
            max_matches: 1,
        })
        .await
        .unwrap();

    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].message_ref, "m4");
    assert!(result.truncated);
}

#[tokio::test]
async fn reused_call_ids_resolve_each_result_to_its_exact_artifact() {
    let workspace = tempdir().unwrap();
    let old_artifact = write_artifact(
        workspace.path(),
        "run",
        "reused-call",
        "old artifact has the historical needle",
    )
    .await;
    let new_artifact = write_artifact(
        workspace.path(),
        "run",
        "reused-call",
        "new artifact has unrelated content",
    )
    .await;
    let source = StaticSource(vec![
        record(
            1,
            Role::Assistant,
            vec![MessageContent::ToolCall {
                id: "reused-call".to_owned(),
                name: "bash".to_owned(),
                arguments: json!({"command": "old-output"}),
            }],
        ),
        record(
            2,
            Role::Tool,
            vec![MessageContent::ToolResult {
                call_id: "reused-call".to_owned(),
                content: "old bounded preview".to_owned(),
                is_error: false,
                metadata: artifact_metadata(&old_artifact),
            }],
        ),
        record(
            3,
            Role::Assistant,
            vec![MessageContent::ToolCall {
                id: "reused-call".to_owned(),
                name: "read".to_owned(),
                arguments: json!({"path": "new"}),
            }],
        ),
        record(
            4,
            Role::Tool,
            vec![MessageContent::ToolResult {
                call_id: "reused-call".to_owned(),
                content: "new bounded preview".to_owned(),
                is_error: false,
                metadata: artifact_metadata(&new_artifact),
            }],
        ),
    ]);
    let reader = reader_with_artifacts(source, workspace.path());

    let result = reader
        .search(HistorySearchRequest {
            run_id: "run".to_owned(),
            pattern: Regex::new("historical needle").unwrap(),
            max_matches: 10,
        })
        .await
        .unwrap();

    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].message_ref, "m2");
    assert_eq!(result.matches[0].match_source, HistoryMatchSource::Artifact);
}

#[tokio::test]
async fn background_result_searches_its_linked_full_artifact() {
    let workspace = tempdir().unwrap();
    let artifact = write_artifact(
        workspace.path(),
        "run",
        "background-task-1",
        "background artifact contains delegated needle",
    )
    .await;
    let source = StaticSource(vec![record(
        1,
        Role::User,
        vec![MessageContent::BackgroundTaskResult {
            task_id: "task-1".to_owned(),
            name: "bash".to_owned(),
            status: "completed".to_owned(),
            content: "bounded preview".to_owned(),
            metadata: artifact_metadata(&artifact),
        }],
    )]);
    let reader = reader_with_artifacts(source, workspace.path());

    let result = reader
        .search(HistorySearchRequest {
            run_id: "run".to_owned(),
            pattern: Regex::new("delegated needle").unwrap(),
            max_matches: 10,
        })
        .await
        .unwrap();

    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].message_ref, "m1");
    assert_eq!(result.matches[0].match_source, HistoryMatchSource::Artifact);
}

#[tokio::test]
async fn plain_result_does_not_claim_a_reused_call_ids_only_artifact() {
    let workspace = tempdir().unwrap();
    let artifact = write_artifact(
        workspace.path(),
        "run",
        "reused-call",
        "older artifact contains exact linkage needle",
    )
    .await;
    let source = StaticSource(vec![
        record(
            1,
            Role::Assistant,
            vec![MessageContent::ToolCall {
                id: "reused-call".to_owned(),
                name: "bash".to_owned(),
                arguments: json!({"command": "large"}),
            }],
        ),
        record(
            2,
            Role::Tool,
            vec![MessageContent::ToolResult {
                call_id: "reused-call".to_owned(),
                content: "bounded preview".to_owned(),
                is_error: false,
                metadata: artifact_metadata(&artifact),
            }],
        ),
        record(
            3,
            Role::Assistant,
            vec![MessageContent::ToolCall {
                id: "reused-call".to_owned(),
                name: "read".to_owned(),
                arguments: json!({"path": "small"}),
            }],
        ),
        record(
            4,
            Role::Tool,
            vec![MessageContent::ToolResult {
                call_id: "reused-call".to_owned(),
                content: "small inline result without an artifact envelope".to_owned(),
                is_error: false,
                metadata: ResultMetadata::empty(),
            }],
        ),
    ]);
    let reader = reader_with_artifacts(source, workspace.path());

    let result = reader
        .search(HistorySearchRequest {
            run_id: "run".to_owned(),
            pattern: Regex::new("exact linkage needle").unwrap(),
            max_matches: 10,
        })
        .await
        .unwrap();

    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].message_ref, "m2");
}

#[tokio::test]
async fn same_length_artifact_tampering_fails_integrity_check() {
    let workspace = tempdir().unwrap();
    let artifact = write_artifact(workspace.path(), "run", "call-1", "trusted-content").await;
    assert_eq!("trusted-content".len(), "changed-content".len());
    tokio::fs::write(workspace.path().join(&artifact.path), "changed-content")
        .await
        .unwrap();
    let source = StaticSource(vec![record(
        1,
        Role::Tool,
        vec![MessageContent::ToolResult {
            call_id: "call-1".to_owned(),
            content: "bounded preview".to_owned(),
            is_error: false,
            metadata: artifact_metadata(&artifact),
        }],
    )]);
    let reader = reader_with_artifacts(source, workspace.path());

    let error = reader
        .search(HistorySearchRequest {
            run_id: "run".to_owned(),
            pattern: Regex::new("changed-content").unwrap(),
            max_matches: 10,
        })
        .await
        .unwrap_err();

    assert!(error.to_string().contains("artifact content hash changed"));
}
