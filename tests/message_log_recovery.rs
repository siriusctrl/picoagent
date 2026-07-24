use std::{path::Path, sync::Arc};

use fiasco::{
    artifact::ResultMetadata,
    model::{Message, MessageContent, Role, ToolCall},
    storage::{RunDirStore, RunRecord},
};
use tempfile::tempdir;
use tokio::io::AsyncWriteExt;

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

#[tokio::test]
async fn viewers_ignore_and_the_writer_repairs_a_torn_tail() {
    let workspace = tempdir().unwrap();
    let original = RunDirStore::new(workspace.path());
    let paths = original
        .create_run(&record(workspace.path()))
        .await
        .unwrap();
    original
        .append_message("run-1", &Message::text(Role::User, "complete"))
        .await
        .unwrap();
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&paths.messages)
        .await
        .unwrap();
    file.write_all(b"{\"ref\":\"m2\",\"created_at\":")
        .await
        .unwrap();
    drop(file);

    let reopened = RunDirStore::new(workspace.path());
    assert_eq!(reopened.load_trajectory("run-1").await.unwrap().len(), 1);
    let appended = reopened
        .append_message("run-1", &Message::text(Role::Assistant, "after crash"))
        .await
        .unwrap();
    assert_eq!(appended.seq, 2);
    let recovered = reopened.load_trajectory("run-1").await.unwrap();
    assert_eq!(recovered.len(), 2);
    assert_eq!(recovered[1].message_ref, "m2");
}

#[tokio::test]
async fn a_valid_record_without_a_newline_is_not_complete() {
    let workspace = tempdir().unwrap();
    let original = RunDirStore::new(workspace.path());
    let paths = original
        .create_run(&record(workspace.path()))
        .await
        .unwrap();
    original
        .append_message("run-1", &Message::text(Role::User, "complete"))
        .await
        .unwrap();
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&paths.messages)
        .await
        .unwrap();
    file.write_all(
        br#"{"ref":"m2","created_at":"2026-07-21T00:00:00Z","role":"assistant","content":[{"type":"text","text":"not complete"}]}"#,
    )
    .await
    .unwrap();
    drop(file);

    let reopened = RunDirStore::new(workspace.path());
    assert_eq!(reopened.load_trajectory("run-1").await.unwrap().len(), 1);
    reopened
        .append_message("run-1", &Message::text(Role::Assistant, "replacement"))
        .await
        .unwrap();
    let durable = tokio::fs::read_to_string(paths.messages).await.unwrap();
    assert_eq!(durable.lines().count(), 2);
    assert!(!durable.contains("not complete"));
    assert!(durable.contains("replacement"));
}

#[tokio::test]
async fn viewer_exposes_but_resume_discards_an_incomplete_tool_turn() {
    let workspace = tempdir().unwrap();
    let original = RunDirStore::new(workspace.path());
    let paths = original
        .create_run(&record(workspace.path()))
        .await
        .unwrap();
    original
        .append_message("run-1", &Message::text(Role::User, "committed"))
        .await
        .unwrap();
    original
        .append_messages(
            "run-1",
            &[
                Message::assistant(vec![
                    MessageContent::ToolCall(ToolCall {
                        id: "call-1".into(),
                        name: "bash".into(),
                        arguments: serde_json::json!({"command": "one"}).into(),
                    }),
                    MessageContent::ToolCall(ToolCall {
                        id: "call-2".into(),
                        name: "bash".into(),
                        arguments: serde_json::json!({"command": "two"}).into(),
                    }),
                ]),
                Message::new(
                    Role::Tool,
                    vec![MessageContent::ToolResult {
                        call_id: "call-1".into(),
                        content: "first result".into(),
                        is_error: false,
                        metadata: ResultMetadata::empty(),
                    }],
                ),
            ],
        )
        .await
        .unwrap();

    let reopened = RunDirStore::new(workspace.path());
    let visible = reopened.load_trajectory("run-1").await.unwrap();
    assert_eq!(visible.len(), 3);
    assert_eq!(visible[1].message.tool_calls().len(), 2);

    let replacement = reopened
        .append_message("run-1", &Message::text(Role::Assistant, "replacement"))
        .await
        .unwrap();
    assert_eq!(replacement.message_ref, "m2");
    let final_messages = reopened.load_trajectory("run-1").await.unwrap();
    assert_eq!(final_messages.len(), 2);
    assert_eq!(final_messages[1].message.visible_text(), "replacement");
    let repaired = tokio::fs::read_to_string(paths.messages).await.unwrap();
    assert!(!repaired.contains("call-1"));
    assert!(!repaired.contains("first result"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_viewers_can_observe_one_writer_without_taking_the_run_lease() {
    let workspace = tempdir().unwrap();
    let store = Arc::new(RunDirStore::new(workspace.path()));
    store.create_run(&record(workspace.path())).await.unwrap();
    let _lease = store.acquire_run_lease("run-1").await.unwrap();

    let writer = {
        let store = store.clone();
        tokio::spawn(async move {
            for index in 0..40 {
                store
                    .append_message(
                        "run-1",
                        &Message::text(Role::User, format!("message-{index}")),
                    )
                    .await
                    .unwrap();
                tokio::task::yield_now().await;
            }
        })
    };
    let mut viewers = Vec::new();
    for _ in 0..8 {
        let workspace = workspace.path().to_owned();
        viewers.push(tokio::spawn(async move {
            let viewer = RunDirStore::new(workspace);
            let mut previous = 0;
            for _ in 0..80 {
                let visible = viewer.load_trajectory("run-1").await.unwrap();
                assert!(visible.len() >= previous);
                assert!(
                    visible
                        .iter()
                        .enumerate()
                        .all(|(index, message)| message.seq == index as u64 + 1)
                );
                previous = visible.len();
                tokio::task::yield_now().await;
            }
        }));
    }
    writer.await.unwrap();
    for viewer in viewers {
        viewer.await.unwrap();
    }
    assert_eq!(store.load_trajectory("run-1").await.unwrap().len(), 40);
}
