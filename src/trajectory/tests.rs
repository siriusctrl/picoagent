use std::{path::Path, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use chrono::DateTime;
use regex::Regex;
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::tempdir;

use crate::{
    artifact::ArtifactRef,
    model::{Message, MessageContent, Role},
};

use super::*;

struct StaticSource(Vec<TrajectoryMessage>);

#[async_trait]
impl CompactedHistorySource for StaticSource {
    async fn load_compacted_history(&self, _run_id: &str) -> Result<CompactedHistory> {
        Ok(CompactedHistory {
            messages: self.0.clone(),
        })
    }
}

fn record(seq: u64, role: Role, content: Vec<MessageContent>) -> TrajectoryMessage {
    TrajectoryMessage {
        message_ref: format!("msg-{seq}"),
        seq,
        created_at: DateTime::from_timestamp(seq as i64, 0).unwrap(),
        message: Message { role, content },
    }
}

fn reader(messages: Vec<TrajectoryMessage>) -> LocalTrajectoryReader {
    LocalTrajectoryReader::new(Arc::new(StaticSource(messages)))
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

fn current_artifact_envelope(artifact: &ArtifactRef) -> String {
    format!(
        "[Tool output]\ntruncated: true\nreason: output_exceeds_inline_limit\npreview: head_tail; omitted_region: middle\nbytes: total={}; shown=8 (head=4, tail=4); omitted={}\nartifact: {}\nmedia_type: {}\nsha256: {}\ninstruction: inspect the immutable artifact",
        artifact.bytes,
        artifact.bytes.saturating_sub(8),
        artifact.path,
        artifact.media_type,
        artifact.sha256,
    )
}

fn legacy_artifact_envelope(artifact: &ArtifactRef) -> String {
    format!(
        "bounded preview\n\n[Full output artifact]\ntruncated: true\npath: {}\nmedia_type: {}\nbytes: {}\nsha256: {}\nUse read with offset/limit or bash with rg to inspect it.",
        artifact.path, artifact.media_type, artifact.bytes, artifact.sha256,
    )
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
    assert_eq!(result.matches[0].message_ref, "msg-2");
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
    assert_eq!(visible.matches[0].message_ref, "msg-1");

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
            message_ref: "msg-2".to_owned(),
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
        ["msg-2", "msg-3"]
    );
    assert!(result.tool_pairs_expanded);
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
            }],
        ),
    ]);

    let result = reader
        .read(HistoryReadRequest {
            run_id: "run".to_owned(),
            message_ref: "msg-4".to_owned(),
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
        ["msg-3", "msg-4"]
    );
    assert!(result.tool_pairs_expanded);
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
                content: current_artifact_envelope(&artifact),
                is_error: false,
            }],
        ),
    ]);
    let reader =
        LocalTrajectoryReader::with_local_artifacts(Arc::new(source), workspace.path().to_owned());
    let result = reader
        .search(HistorySearchRequest {
            run_id: "run".to_owned(),
            pattern: Regex::new("needle").unwrap(),
            max_matches: 10,
        })
        .await
        .unwrap();

    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].message_ref, "msg-2");
    assert_eq!(result.matches[0].tool_name.as_deref(), Some("bash"));
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
            content: current_artifact_envelope(&artifact),
            is_error: false,
        }],
    )]);
    let reader =
        LocalTrajectoryReader::with_local_artifacts(Arc::new(source), workspace.path().to_owned());
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
async fn search_stops_before_an_older_corrupt_artifact_after_limit_is_known() {
    let workspace = tempdir().unwrap();
    let artifact_directory = workspace.path().join(".pico/runs/run/artifacts");
    tokio::fs::create_dir_all(&artifact_directory)
        .await
        .unwrap();
    tokio::fs::write(
        artifact_directory.join("old-call-deadbeef0000.artifact.json"),
        b"this sidecar must stay unread",
    )
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
    let reader =
        LocalTrajectoryReader::with_local_artifacts(Arc::new(source), workspace.path().to_owned());
    let result = reader
        .search(HistorySearchRequest {
            run_id: "run".to_owned(),
            pattern: Regex::new("needle").unwrap(),
            max_matches: 1,
        })
        .await
        .unwrap();

    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].message_ref, "msg-4");
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
                content: current_artifact_envelope(&old_artifact),
                is_error: false,
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
                content: current_artifact_envelope(&new_artifact),
                is_error: false,
            }],
        ),
    ]);
    let reader =
        LocalTrajectoryReader::with_local_artifacts(Arc::new(source), workspace.path().to_owned());

    let result = reader
        .search(HistorySearchRequest {
            run_id: "run".to_owned(),
            pattern: Regex::new("historical needle").unwrap(),
            max_matches: 10,
        })
        .await
        .unwrap();

    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].message_ref, "msg-2");
    assert_eq!(result.matches[0].tool_name.as_deref(), Some("bash"));
    assert_eq!(result.matches[0].kind, HistoryMatchKind::ToolResult);
    assert_eq!(result.matches[0].match_source, HistoryMatchSource::Artifact);
}

#[tokio::test]
async fn background_result_searches_its_full_legacy_envelope_artifact() {
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
            content: legacy_artifact_envelope(&artifact),
        }],
    )]);
    let reader =
        LocalTrajectoryReader::with_local_artifacts(Arc::new(source), workspace.path().to_owned());

    let result = reader
        .search(HistorySearchRequest {
            run_id: "run".to_owned(),
            pattern: Regex::new("delegated needle").unwrap(),
            max_matches: 10,
        })
        .await
        .unwrap();

    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].message_ref, "msg-1");
    assert_eq!(
        result.matches[0].kind,
        HistoryMatchKind::BackgroundTaskResult
    );
    assert_eq!(result.matches[0].tool_name.as_deref(), Some("bash"));
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
                content: current_artifact_envelope(&artifact),
                is_error: false,
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
            }],
        ),
    ]);
    let reader =
        LocalTrajectoryReader::with_local_artifacts(Arc::new(source), workspace.path().to_owned());

    let result = reader
        .search(HistorySearchRequest {
            run_id: "run".to_owned(),
            pattern: Regex::new("exact linkage needle").unwrap(),
            max_matches: 10,
        })
        .await
        .unwrap();

    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].message_ref, "msg-2");
    assert_eq!(result.matches[0].tool_name.as_deref(), Some("bash"));
}

#[tokio::test]
async fn identity_free_lookup_does_not_guess_between_sidecars() {
    let workspace = tempdir().unwrap();
    write_artifact(
        workspace.path(),
        "run",
        "ambiguous-call",
        "first artifact contains ambiguous needle",
    )
    .await;
    write_artifact(
        workspace.path(),
        "run",
        "ambiguous-call",
        "second artifact is unrelated",
    )
    .await;
    let source = LocalRunArtifactSource::new(workspace.path());
    let mut search = source.begin_search("run").await.unwrap();

    let result = search
        .find(
            &[ArtifactLookup {
                call_id: "ambiguous-call".to_owned(),
                sha256: None,
            }],
            &Regex::new("ambiguous needle").unwrap(),
        )
        .await
        .unwrap();

    assert!(result.is_none());
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
            content: current_artifact_envelope(&artifact),
            is_error: false,
        }],
    )]);
    let reader =
        LocalTrajectoryReader::with_local_artifacts(Arc::new(source), workspace.path().to_owned());

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
