use std::path::Path;

use picoagent::{
    artifact::{ArtifactRef, ResultMetadata},
    model::{Message, MessageContent, Role},
    storage::{MESSAGE_FORMAT, RunDirStore, RunRecord},
};

fn result_metadata(call_id: &str) -> ResultMetadata {
    ResultMetadata {
        artifact: Some(ArtifactRef {
            version: 1,
            artifact_id: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_owned(),
            run_id: "run-1".to_owned(),
            call_id: call_id.to_owned(),
            path: ".pico/runs/run-1/artifacts/result.txt".to_owned(),
            media_type: "text/plain; charset=utf-8".to_owned(),
            bytes: 100,
            sha256: "a".repeat(64),
        }),
    }
}
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::io::AsyncWriteExt;

fn record(workspace: &Path) -> RunRecord {
    RunRecord::new(
        "run-1",
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
async fn messages_are_native_chat_json_and_sidecar_preserves_stable_refs() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();

    let first = store
        .append_message(
            "run-1",
            &Message {
                role: Role::User,
                content: vec![
                    MessageContent::RuntimeReminder {
                        text: "<runtime-reminder>context</runtime-reminder>".into(),
                    },
                    MessageContent::Text {
                        text: "first".into(),
                    },
                ],
            },
        )
        .await
        .unwrap();
    let second = store
        .append_message(
            "run-1",
            &Message {
                role: Role::Assistant,
                content: vec![
                    MessageContent::Reasoning {
                        text: "inspect".into(),
                    },
                    MessageContent::Text {
                        text: "second".into(),
                    },
                    MessageContent::ToolCall {
                        id: "call_1".into(),
                        name: "read".into(),
                        arguments: json!({"path": "README.md"}),
                    },
                ],
            },
        )
        .await
        .unwrap();
    let third = store
        .append_message(
            "run-1",
            &Message {
                role: Role::Tool,
                content: vec![MessageContent::ToolResult {
                    call_id: "call_1".into(),
                    content: "file contents".into(),
                    is_error: true,
                    metadata: result_metadata("call_1"),
                }],
            },
        )
        .await
        .unwrap();

    let lines = read_jsonl(&paths.messages).await;
    assert_eq!(
        lines,
        [
            json!({
                "role": "user",
                "content": "<runtime-reminder>context</runtime-reminder>\n\nfirst"
            }),
            json!({
                "role": "assistant",
                "content": "second",
                "reasoning_content": "inspect",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "read",
                        "arguments": r#"{"path":"README.md"}"#
                    }
                }]
            }),
            json!({
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "file contents"
            }),
        ]
    );
    for line in &lines {
        for local_field in ["message_id", "seq", "created_at", "version"] {
            assert!(line.get(local_field).is_none());
        }
        assert!(!line.to_string().contains("runtime_reminder"));
    }

    let metadata = read_jsonl(&paths.message_metadata).await;
    assert_eq!(metadata.len(), 3);
    assert_eq!(first.message_ref, "m1");
    assert_eq!(second.message_ref, "m2");
    assert_eq!(third.message_ref, "m3");
    for (index, (item, expected_ref)) in metadata
        .iter()
        .zip([&first.message_ref, &second.message_ref, &third.message_ref])
        .enumerate()
    {
        assert_eq!(&item["message_id"], expected_ref);
        assert_eq!(item["seq"], index + 1);
        assert!(item["created_at"].is_string());
        assert_eq!(item["message_sha256"].as_str().unwrap().len(), 64);
        assert_eq!(item["reconstruction_sha256"].as_str().unwrap().len(), 64);
        assert!(item["layout"].is_array());
    }
    assert_eq!(
        metadata[2]["layout"][0]["metadata"]["artifact"]["call_id"],
        "call_1"
    );
    assert!(!lines[2].to_string().contains("artifact"));

    let persisted_run: Value =
        serde_json::from_slice(&tokio::fs::read(&paths.metadata).await.unwrap()).unwrap();
    assert_eq!(persisted_run["message_format"], MESSAGE_FORMAT);

    let loaded_once = store.load_trajectory("run-1").await.unwrap();
    let loaded_twice = store.load_trajectory("run-1").await.unwrap();
    assert_eq!(
        loaded_once
            .iter()
            .map(|message| (&message.message_ref, message.seq))
            .collect::<Vec<_>>(),
        loaded_twice
            .iter()
            .map(|message| (&message.message_ref, message.seq))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn round_trips_all_internal_content_through_native_messages_and_sidecar() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();
    let expected = vec![
        Message {
            role: Role::User,
            content: vec![
                MessageContent::RuntimeReminder {
                    text: "运行上下文".into(),
                },
                MessageContent::Text {
                    text: "用户正文".into(),
                },
                MessageContent::BackgroundTaskResult {
                    task_id: "task-1".into(),
                    name: "worker".into(),
                    status: "completed".into(),
                    content: "后台结果".into(),
                    metadata: result_metadata("background-task-1"),
                },
            ],
        },
        Message {
            role: Role::Assistant,
            content: vec![
                MessageContent::ProviderItem {
                    provider: "openai".into(),
                    item: json!({"type": "reasoning", "encrypted_content": "opaque"}),
                },
                MessageContent::Reasoning {
                    text: "先检查".into(),
                },
                MessageContent::Text {
                    text: "结果".into(),
                },
                MessageContent::ToolCall {
                    id: "call_opaque".into(),
                    name: "bash".into(),
                    arguments: json!({"cmd": "pwd"}),
                },
            ],
        },
        Message {
            role: Role::Tool,
            content: vec![MessageContent::ToolResult {
                call_id: "call_opaque".into(),
                content: "command failed".into(),
                is_error: true,
                metadata: ResultMetadata::empty(),
            }],
        },
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

    let native = read_jsonl(&paths.messages).await;
    assert!(native[0]["content"].as_str().unwrap().contains("后台结果"));
    assert_eq!(native[1]["reasoning_content"], "先检查");
    assert!(!native[1].to_string().contains("encrypted_content"));
    assert_eq!(native[2]["content"], "command failed");
}

#[tokio::test]
async fn rejects_legacy_provider_neutral_message_records() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();
    let legacy = serde_json::to_string(&Message::text(Role::User, "legacy")).unwrap();
    tokio::fs::write(&paths.messages, format!("{legacy}\n"))
        .await
        .unwrap();

    let error = store.load_trajectory("run-1").await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("parse completed OpenAI Chat message")
    );
}

#[tokio::test]
async fn rejects_a_native_message_that_no_longer_matches_its_sha() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();
    store
        .append_message("run-1", &Message::text(Role::User, "original"))
        .await
        .unwrap();
    let tampered = tokio::fs::read_to_string(&paths.messages)
        .await
        .unwrap()
        .replace("original", "tampered");
    tokio::fs::write(&paths.messages, tampered).await.unwrap();

    let error = store.load_trajectory("run-1").await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("does not match its metadata sha256")
    );
    let before = tokio::fs::metadata(&paths.messages).await.unwrap().len();
    let append_error = store
        .append_message("run-1", &Message::text(Role::Assistant, "must not append"))
        .await
        .unwrap_err();
    assert!(
        append_error
            .to_string()
            .contains("does not match its metadata sha256")
    );
    assert_eq!(
        tokio::fs::metadata(&paths.messages).await.unwrap().len(),
        before
    );
}

#[tokio::test]
async fn rejects_reconstruction_metadata_that_no_longer_matches_its_sha() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();
    store
        .append_message(
            "run-1",
            &Message {
                role: Role::Tool,
                content: vec![MessageContent::ToolResult {
                    call_id: "call_1".into(),
                    content: "failed".into(),
                    is_error: true,
                    metadata: ResultMetadata::empty(),
                }],
            },
        )
        .await
        .unwrap();
    let tampered = tokio::fs::read_to_string(&paths.message_metadata)
        .await
        .unwrap()
        .replace("\"is_error\":true", "\"is_error\":false");
    tokio::fs::write(&paths.message_metadata, tampered)
        .await
        .unwrap();

    let error = store.load_trajectory("run-1").await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("reconstruction metadata does not match its sha256")
    );
}

#[tokio::test]
async fn rejects_message_metadata_that_is_ahead_of_native_messages() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();
    store
        .append_message("run-1", &Message::text(Role::User, "committed"))
        .await
        .unwrap();
    let first = tokio::fs::read(&paths.message_metadata).await.unwrap();
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&paths.message_metadata)
        .await
        .unwrap();
    file.write_all(&first).await.unwrap();
    drop(file);

    let error = store.load_trajectory("run-1").await.unwrap_err();
    assert!(error.to_string().contains("message metadata is ahead"));
}

#[tokio::test]
async fn rejects_corruption_in_a_completed_native_jsonl_record() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();
    tokio::fs::write(&paths.messages, b"{not-json}\n")
        .await
        .unwrap();

    let error = store.load_trajectory("run-1").await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("parse completed OpenAI Chat message")
    );
}

#[tokio::test]
async fn rejects_missing_initialized_message_log_files() {
    for missing in ["messages", "metadata", "both"] {
        let workspace = tempdir().unwrap();
        let store = RunDirStore::new(workspace.path());
        let paths = store.create_run(&record(workspace.path())).await.unwrap();
        store
            .append_message("run-1", &Message::text(Role::User, "committed"))
            .await
            .unwrap();
        if matches!(missing, "messages" | "both") {
            tokio::fs::remove_file(&paths.messages).await.unwrap();
        }
        if matches!(missing, "metadata" | "both") {
            tokio::fs::remove_file(&paths.message_metadata)
                .await
                .unwrap();
        }

        let error = store.load_trajectory("run-1").await.unwrap_err();
        assert!(
            error.to_string().contains("initialized message log"),
            "unexpected error: {error:#}"
        );
    }
}
