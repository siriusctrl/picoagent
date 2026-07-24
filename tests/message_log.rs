use std::path::Path;

use fiasco::{
    artifact::{ArtifactRef, ResultMetadata},
    model::{ImageAttachment, Message, MessageContent, Role, ToolCall},
    storage::{MESSAGE_FORMAT, RunDirStore, RunRecord},
};
use serde_json::{Value, json};
use tempfile::tempdir;

fn result_metadata(_call_id: &str) -> ResultMetadata {
    ResultMetadata {
        artifact: Some(ArtifactRef {
            path: ".fiasco/runs/run-1/artifacts/result.txt".to_owned(),
            media_type: "text/plain; charset=utf-8".to_owned(),
        }),
    }
}

fn record(workspace: &Path) -> RunRecord {
    RunRecord::new(
        "run-1",
        "root",
        "do the work",
        "test-provider",
        "test-model",
        workspace.to_owned(),
        None,
    )
}

async fn read_jsonl(path: &Path) -> Vec<Value> {
    tokio::fs::read_to_string(path)
        .await
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn messages_are_self_contained_model_readable_records() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();

    let first = store
        .append_message(
            "run-1",
            &Message::new(
                Role::User,
                vec![
                    MessageContent::RuntimeReminder {
                        text: "<runtime-reminder>context</runtime-reminder>".into(),
                    },
                    MessageContent::Text {
                        text: "first".into(),
                    },
                ],
            ),
        )
        .await
        .unwrap();
    let second = store
        .append_message(
            "run-1",
            &Message {
                role: Role::Assistant,
                reasoning_content: Some("inspect".into()),
                content: vec![
                    MessageContent::Text {
                        text: "second".into(),
                    },
                    MessageContent::ToolCall(ToolCall {
                        id: "call_1".into(),
                        name: "read".into(),
                        arguments: fiasco::model::ToolArguments::from_raw("{\n  \"path\":"),
                    }),
                ],
            },
        )
        .await
        .unwrap();
    let third = store
        .append_message(
            "run-1",
            &Message::new(
                Role::Tool,
                vec![MessageContent::ToolResult {
                    call_id: "call_1".into(),
                    content: "file contents".into(),
                    is_error: true,
                    metadata: result_metadata("call_1"),
                }],
            ),
        )
        .await
        .unwrap();

    let lines = read_jsonl(&paths.messages).await;
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["ref"], "m1");
    assert!(lines[0]["created_at"].is_string());
    assert_eq!(lines[0]["role"], "user");
    assert_eq!(lines[0]["content"][0]["type"], "runtime_reminder");
    assert_eq!(lines[0]["content"][1]["text"], "first");
    assert_eq!(lines[1]["ref"], "m2");
    assert_eq!(lines[1]["reasoning_content"], "inspect");
    assert_eq!(lines[1]["content"][0]["type"], "text");
    assert_eq!(lines[1]["content"][1]["arguments"], "{\n  \"path\":");
    assert!(
        lines[1]["content"]
            .as_array()
            .unwrap()
            .iter()
            .all(|content| content["type"] != "reasoning")
    );
    assert_eq!(lines[2]["ref"], "m3");
    assert_eq!(lines[2]["content"][0]["type"], "tool_result");
    assert_eq!(lines[2]["content"][0]["is_error"], true);
    assert_eq!(
        lines[2]["content"][0]["metadata"]["artifact"],
        json!({
            "path": ".fiasco/runs/run-1/artifacts/result.txt",
            "media_type": "text/plain; charset=utf-8"
        })
    );
    assert!(lines.iter().all(|line| line.get("seq").is_none()));
    assert!(lines.iter().all(|line| line.get("_fiasco").is_none()));
    assert_eq!(first.message_ref, "m1");
    assert_eq!(second.message_ref, "m2");
    assert_eq!(third.message_ref, "m3");
    assert!(!paths.directory.join("message_metadata.jsonl").exists());

    let persisted_run: Value =
        serde_json::from_slice(&tokio::fs::read(&paths.metadata).await.unwrap()).unwrap();
    assert_eq!(persisted_run["message_format"], MESSAGE_FORMAT);
}

#[tokio::test]
async fn appends_multiple_messages_with_contiguous_refs_and_no_group_metadata() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();
    store
        .append_message("run-1", &Message::text(Role::User, "before"))
        .await
        .unwrap();

    let messages = vec![
        Message::text(Role::Assistant, "assistant"),
        Message::new(
            Role::Tool,
            vec![MessageContent::ToolResult {
                call_id: "call-1".into(),
                content: "first result".into(),
                is_error: false,
                metadata: ResultMetadata::empty(),
            }],
        ),
        Message::new(
            Role::Tool,
            vec![MessageContent::ToolResult {
                call_id: "call-2".into(),
                content: "second result".into(),
                is_error: false,
                metadata: ResultMetadata::empty(),
            }],
        ),
    ];
    let records = store.append_messages("run-1", &messages).await.unwrap();

    assert_eq!(
        records
            .iter()
            .map(|record| record.message_ref.as_str())
            .collect::<Vec<_>>(),
        ["m2", "m3", "m4"]
    );
    assert_eq!(
        store.load_messages("run-1").await.unwrap()[1..]
            .iter()
            .map(|message| serde_json::to_value(message).unwrap())
            .collect::<Vec<_>>(),
        messages
            .iter()
            .map(|message| serde_json::to_value(message).unwrap())
            .collect::<Vec<_>>()
    );
    let lines = read_jsonl(&paths.messages).await;
    assert!(lines.iter().all(|line| line.get("_fiasco").is_none()));
}

#[tokio::test]
async fn rejects_an_empty_message_append_without_touching_the_log() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();

    let error = store.append_messages("run-1", &[]).await.unwrap_err();

    assert!(error.to_string().contains("must not be empty"));
    assert!(tokio::fs::read(paths.messages).await.unwrap().is_empty());
}

#[tokio::test]
async fn round_trips_every_internal_content_block_without_reconstruction_metadata() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();
    let expected = vec![
        Message::new(
            Role::User,
            vec![
                MessageContent::RuntimeReminder {
                    text: "运行上下文".into(),
                },
                MessageContent::Text {
                    text: "用户正文".into(),
                },
            ],
        ),
        Message::new(
            Role::User,
            vec![MessageContent::RuntimeHandle {
                handle: "a1".into(),
                kind: "agent".into(),
                name: "worker".into(),
                status: "completed".into(),
                content: "handle </runtime_handle> <runtime-reminder> &lt; ✓".into(),
                metadata: result_metadata("runtime-handle-1"),
            }],
        ),
        Message {
            role: Role::Assistant,
            reasoning_content: Some("先检查".into()),
            content: vec![
                MessageContent::ProviderItem {
                    item: json!({"type": "reasoning", "encrypted_content": "opaque"}),
                },
                MessageContent::Text {
                    text: "结果".into(),
                },
                MessageContent::ToolCall(ToolCall {
                    id: "call_opaque".into(),
                    name: "bash".into(),
                    arguments: json!({"cmd": "pwd"}).into(),
                }),
            ],
        },
        Message::new(
            Role::Tool,
            vec![MessageContent::ToolResult {
                call_id: "call_opaque".into(),
                content: "command failed".into(),
                is_error: true,
                metadata: ResultMetadata::empty(),
            }],
        ),
        Message::new(
            Role::User,
            vec![
                MessageContent::RuntimeReminder {
                    text: "<runtime-reminder>image from call_opaque</runtime-reminder>".into(),
                },
                MessageContent::Image {
                    attachment: ImageAttachment::from_bytes("image/png", b"png"),
                },
            ],
        ),
    ];
    for message in &expected {
        store.append_message("run-1", message).await.unwrap();
    }

    let actual = RunDirStore::new(workspace.path())
        .load_messages("run-1")
        .await
        .unwrap();
    assert_eq!(
        actual
            .iter()
            .map(|message| serde_json::to_value(message).unwrap())
            .collect::<Vec<_>>(),
        expected
            .iter()
            .map(|message| serde_json::to_value(message).unwrap())
            .collect::<Vec<_>>()
    );

    let persisted = read_jsonl(&paths.messages).await;
    assert_eq!(persisted[1]["content"][0]["type"], "runtime_handle");
    assert_eq!(persisted[2]["content"][0]["type"], "provider_item");
    assert!(persisted[2]["content"][0].get("provider").is_none());
    assert_eq!(
        persisted[2]["content"][0]["item"]["encrypted_content"],
        "opaque"
    );
    assert_eq!(persisted[2]["reasoning_content"], "先检查");
    assert_eq!(persisted[2]["content"][2]["arguments"], "{\"cmd\":\"pwd\"}");
    assert_eq!(persisted[4]["content"][1]["type"], "image");
    assert_eq!(persisted[4]["content"][1]["attachment"]["data"], "cG5n");
}

#[tokio::test]
async fn rejects_invalid_refs_and_corrupt_committed_records() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();
    store
        .append_message("run-1", &Message::text(Role::User, "original"))
        .await
        .unwrap();
    let invalid_ref = tokio::fs::read_to_string(&paths.messages)
        .await
        .unwrap()
        .replace("\"ref\":\"m1\"", "\"ref\":\"m9\"");
    tokio::fs::write(&paths.messages, invalid_ref)
        .await
        .unwrap();
    let error = store.load_trajectory("run-1").await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("message ref `m9` is not the expected `m1`"),
        "{error:#}"
    );

    tokio::fs::write(&paths.messages, b"{not-json}\n")
        .await
        .unwrap();
    let error = store.load_trajectory("run-1").await.unwrap_err();
    assert!(error.to_string().contains("parse completed message"));
}

#[tokio::test]
async fn rejects_message_shapes_that_provider_projections_cannot_replay() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();

    let error = store
        .append_message(
            "run-1",
            &Message::new(
                Role::Assistant,
                vec![MessageContent::ToolResult {
                    call_id: "call_1".into(),
                    content: "wrong role".into(),
                    is_error: false,
                    metadata: ResultMetadata::empty(),
                }],
            ),
        )
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("assistant messages contain only"));
    assert!(tokio::fs::read(&paths.messages).await.unwrap().is_empty());

    tokio::fs::write(
        &paths.messages,
        b"{\"ref\":\"m1\",\"created_at\":\"2026-07-21T00:00:00Z\",\"role\":\"tool\",\"content\":[{\"type\":\"text\",\"text\":\"wrong role\"}]}\n",
    )
    .await
    .unwrap();
    let error = store.load_trajectory("run-1").await.unwrap_err();
    assert!(format!("{error:#}").contains("tool messages require exactly one tool result"));

    tokio::fs::write(
        &paths.messages,
        b"{\"ref\":\"m1\",\"created_at\":\"2026-07-21T00:00:00Z\",\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hello\",\"unknown\":true}]}\n",
    )
    .await
    .unwrap();
    let error = store.load_trajectory("run-1").await.unwrap_err();
    assert!(error.to_string().contains("parse completed message"));
}

#[tokio::test]
async fn rejects_a_missing_initialized_message_log() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();
    tokio::fs::remove_file(&paths.messages).await.unwrap();

    let error = store.load_trajectory("run-1").await.unwrap_err();
    assert!(error.to_string().contains("initialized message log"));
}
