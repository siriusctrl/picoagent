use std::path::Path;

use picoagent::{
    model::{Message, Role},
    storage::{RunDirStore, RunRecord},
};
use serde_json::Value;
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
async fn ignores_and_repairs_a_torn_native_message_tail() {
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
    file.write_all(b"{\"role\":\"assistant\",\"content\":\xff")
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
    assert_eq!(recovered[1].message_ref, appended.message_ref);
}

#[tokio::test]
async fn ignores_an_orphan_native_message_and_replaces_it_on_append() {
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
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&paths.messages)
        .await
        .unwrap();
    file.write_all(b"{\"role\":\"assistant\",\"content\":\"orphan\"}\n")
        .await
        .unwrap();
    drop(file);

    let reopened = RunDirStore::new(workspace.path());
    assert_eq!(reopened.load_trajectory("run-1").await.unwrap().len(), 1);
    let appended = reopened
        .append_message("run-1", &Message::text(Role::Assistant, "replacement"))
        .await
        .unwrap();
    assert_eq!(appended.seq, 2);

    let native = tokio::fs::read_to_string(paths.messages).await.unwrap();
    assert_eq!(native.lines().count(), 2);
    assert!(!native.contains("orphan"));
    assert!(native.contains("replacement"));
    assert_eq!(read_jsonl(&paths.message_metadata).await.len(), 2);
}

#[tokio::test]
async fn a_cached_store_repairs_an_interrupted_native_append() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();
    store
        .append_message("run-1", &Message::text(Role::User, "committed"))
        .await
        .unwrap();

    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&paths.messages)
        .await
        .unwrap();
    file.write_all(b"{\"role\":\"assistant\",\"content\":\"orphan\"}\n")
        .await
        .unwrap();
    drop(file);

    let appended = store
        .append_message("run-1", &Message::text(Role::Assistant, "replacement"))
        .await
        .unwrap();
    assert_eq!(appended.seq, 2);
    let recovered = store.load_trajectory("run-1").await.unwrap();
    assert_eq!(recovered.len(), 2);
    assert_eq!(recovered[1].message_ref, appended.message_ref);
    let native = tokio::fs::read_to_string(paths.messages).await.unwrap();
    assert!(!native.contains("orphan"));
}

#[tokio::test]
async fn repairs_a_torn_metadata_tail_before_the_next_append() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();
    store
        .append_message("run-1", &Message::text(Role::User, "first"))
        .await
        .unwrap();
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&paths.message_metadata)
        .await
        .unwrap();
    file.write_all(b"{\"message_id\":").await.unwrap();
    drop(file);

    assert_eq!(store.load_trajectory("run-1").await.unwrap().len(), 1);
    let appended = store
        .append_message("run-1", &Message::text(Role::Assistant, "second"))
        .await
        .unwrap();
    assert_eq!(appended.seq, 2);
    assert_eq!(store.load_trajectory("run-1").await.unwrap().len(), 2);
    assert_eq!(read_jsonl(&paths.message_metadata).await.len(), 2);
}

#[tokio::test]
async fn completes_a_valid_metadata_tail_without_a_newline_before_append() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();
    store
        .append_message("run-1", &Message::text(Role::User, "first"))
        .await
        .unwrap();
    let metadata_len = tokio::fs::metadata(&paths.message_metadata)
        .await
        .unwrap()
        .len();
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&paths.message_metadata)
        .await
        .unwrap();
    file.set_len(metadata_len - 1).await.unwrap();
    drop(file);

    let appended = store
        .append_message("run-1", &Message::text(Role::Assistant, "second"))
        .await
        .unwrap();
    assert_eq!(appended.seq, 2);
    assert_eq!(store.load_trajectory("run-1").await.unwrap().len(), 2);
    let metadata = tokio::fs::read(&paths.message_metadata).await.unwrap();
    assert!(metadata.ends_with(b"\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn independent_stores_serialize_concurrent_message_appends() {
    let workspace = tempdir().unwrap();
    let creator = RunDirStore::new(workspace.path());
    creator.create_run(&record(workspace.path())).await.unwrap();
    let first = RunDirStore::new(workspace.path());
    let second = RunDirStore::new(workspace.path());

    let write_first = async {
        for index in 0..20 {
            first
                .append_message(
                    "run-1",
                    &Message::text(Role::User, format!("first-{index}")),
                )
                .await
                .unwrap();
        }
    };
    let write_second = async {
        for index in 0..20 {
            second
                .append_message(
                    "run-1",
                    &Message::text(Role::Assistant, format!("second-{index}")),
                )
                .await
                .unwrap();
        }
    };
    tokio::join!(write_first, write_second);

    let messages = creator.load_trajectory("run-1").await.unwrap();
    assert_eq!(messages.len(), 40);
    assert_eq!(
        messages
            .iter()
            .map(|message| message.seq)
            .collect::<Vec<_>>(),
        (1..=40).collect::<Vec<_>>()
    );
    assert!(
        messages
            .iter()
            .all(|message| message.message_ref == format!("m{}", message.seq))
    );
}
