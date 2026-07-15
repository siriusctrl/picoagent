use picoagent::{
    events::{EventSink, RuntimeEvent, RuntimeEventKind},
    model::{Message, Role},
    storage::{CompactionCheckpoint, RunDirStore, RunRecord, RunState},
    trajectory::CompactedHistorySource,
};
use serde_json::Value;
use tempfile::tempdir;

fn record(workspace: &std::path::Path) -> RunRecord {
    RunRecord::new(
        "run-1",
        "do the work",
        "test-provider",
        "test-model",
        workspace.to_owned(),
        None,
    )
}

#[tokio::test]
async fn persists_run_messages_events_and_final_output() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();
    assert!(paths.metadata.exists());
    assert!(paths.artifacts.is_dir());

    let message = Message::text(Role::User, "hello");
    store.append_message("run-1", &message).await.unwrap();
    let event = RuntimeEvent::new("run-1", RuntimeEventKind::ModelStarted { step: 1 });
    store.emit(&event).await.unwrap();
    store.write_final("run-1", "done\n").await.unwrap();
    let updated = store
        .update_state("run-1", RunState::Completed)
        .await
        .unwrap();

    assert_eq!(updated.state, RunState::Completed);
    assert_eq!(
        store.load_run("run-1").await.unwrap().state,
        RunState::Completed
    );
    assert_eq!(store.load_messages("run-1").await.unwrap().len(), 1);
    assert_eq!(
        tokio::fs::read_to_string(paths.final_output).await.unwrap(),
        "done\n"
    );
    let event_lines = tokio::fs::read_to_string(paths.events).await.unwrap();
    assert_eq!(event_lines.lines().count(), 1);
    let stored_event: RuntimeEvent = serde_json::from_str(event_lines.trim()).unwrap();
    assert_eq!(stored_event.run_id, "run-1");
}

#[tokio::test]
async fn serializes_concurrent_event_appends_as_complete_json_lines() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    store.create_run(&record(workspace.path())).await.unwrap();

    let mut tasks = Vec::new();
    for step in 0..32 {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            store
                .emit(&RuntimeEvent::new(
                    "run-1",
                    RuntimeEventKind::ModelStarted { step },
                ))
                .await
                .unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }

    let events = tokio::fs::read_to_string(store.paths("run-1").events)
        .await
        .unwrap();
    assert_eq!(events.lines().count(), 32);
    for line in events.lines() {
        serde_json::from_str::<RuntimeEvent>(line).unwrap();
    }
}

#[tokio::test]
async fn keeps_stream_deltas_transient_while_persisting_lifecycle_events() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    store.create_run(&record(workspace.path())).await.unwrap();

    for kind in [
        RuntimeEventKind::ModelDelta {
            text: "visible".into(),
        },
        RuntimeEventKind::ModelReasoningDelta {
            text: "reasoning".into(),
        },
        RuntimeEventKind::ModelStarted { step: 1 },
    ] {
        store.emit(&RuntimeEvent::new("run-1", kind)).await.unwrap();
    }

    let events = tokio::fs::read_to_string(store.paths("run-1").events)
        .await
        .unwrap();
    let stored: Vec<RuntimeEvent> = events
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(stored.len(), 1);
    assert!(matches!(
        &stored[0].kind,
        RuntimeEventKind::ModelStarted { step: 1 }
    ));
}

#[tokio::test]
async fn rejects_writes_for_unknown_runs() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let error = store
        .append_message("missing", &Message::text(Role::User, "hello"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("run does not exist"));
}

#[tokio::test]
async fn message_envelope_is_flat_and_preserves_stable_refs() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();

    let first = store
        .append_message("run-1", &Message::text(Role::User, "first"))
        .await
        .unwrap();
    let second = store
        .append_message("run-1", &Message::text(Role::Assistant, "second"))
        .await
        .unwrap();

    let lines: Vec<Value> = tokio::fs::read_to_string(&paths.messages)
        .await
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["version"], 1);
    assert_eq!(lines[0]["message_id"], first.message_ref);
    assert_eq!(lines[0]["seq"], 1);
    assert_eq!(lines[0]["role"], "user");
    assert!(lines[0].get("content").is_some());
    assert!(lines[0].get("message").is_none());
    assert_eq!(lines[1]["message_id"], second.message_ref);
    assert_eq!(lines[1]["seq"], 2);

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
    assert_eq!(loaded_once[0].message_ref, first.message_ref);
    assert_eq!(loaded_once[1].message_ref, second.message_ref);
}

#[tokio::test]
async fn loads_legacy_raw_messages_and_continues_the_sequence() {
    let workspace = tempdir().unwrap();
    let original_store = RunDirStore::new(workspace.path());
    let paths = original_store
        .create_run(&record(workspace.path()))
        .await
        .unwrap();
    let legacy = serde_json::to_string(&Message::text(Role::User, "legacy")).unwrap();
    tokio::fs::write(&paths.messages, format!("{legacy}\n"))
        .await
        .unwrap();

    // Re-opening the store models an existing run written by an older version.
    let reopened = RunDirStore::new(workspace.path());
    let loaded = reopened.load_trajectory("run-1").await.unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].message_ref, "legacy_00000001");
    assert_eq!(loaded[0].seq, 1);

    let appended = reopened
        .append_message("run-1", &Message::text(Role::Assistant, "new"))
        .await
        .unwrap();
    assert_eq!(appended.seq, 2);
    assert!(appended.message_ref.starts_with("msg_"));

    let all = reopened.load_trajectory("run-1").await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].message_ref, "legacy_00000001");
    assert_eq!(all[1].message_ref, appended.message_ref);
}

#[tokio::test]
async fn ignores_and_repairs_a_torn_message_tail() {
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
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&paths.messages)
        .await
        .unwrap();
    file.write_all(b"{\"role\":\"assistant\",\"content\":[\xff")
        .await
        .unwrap();
    drop(file);

    let reopened = RunDirStore::new(workspace.path());
    let recovered = reopened.load_trajectory("run-1").await.unwrap();
    assert_eq!(recovered.len(), 1);
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
async fn rejects_corruption_in_a_completed_jsonl_record() {
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
            .contains("parse stored trajectory message")
    );
}

#[tokio::test]
async fn compaction_checkpoints_are_append_only_and_latest_wins() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();
    let initial = store
        .append_message("run-1", &Message::text(Role::User, "initial"))
        .await
        .unwrap();
    let older = store
        .append_message("run-1", &Message::text(Role::Assistant, "older"))
        .await
        .unwrap();
    let recent = store
        .append_message("run-1", &Message::text(Role::Assistant, "recent"))
        .await
        .unwrap();

    let first = checkpoint(
        "cmp_1",
        None,
        &older.message_ref,
        &recent.message_ref,
        "first summary",
    );
    let second = checkpoint(
        "cmp_2",
        Some("cmp_1"),
        &recent.message_ref,
        &recent.message_ref,
        "second summary",
    );
    store.append_compaction("run-1", &first).await.unwrap();
    store.append_compaction("run-1", &second).await.unwrap();

    let persisted = tokio::fs::read_to_string(paths.compactions).await.unwrap();
    assert_eq!(persisted.lines().count(), 2);
    let checkpoints = store.load_compactions("run-1").await.unwrap();
    assert_eq!(checkpoints.len(), 2);
    assert_eq!(checkpoints[0].checkpoint_id, "cmp_1");
    assert_eq!(checkpoints[1].checkpoint_id, "cmp_2");
    let latest = store
        .load_latest_compaction("run-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.checkpoint_id, "cmp_2");
    assert_eq!(latest.previous_checkpoint_id.as_deref(), Some("cmp_1"));

    // Checkpoints are a derived index; raw messages stay untouched.
    let messages = store.load_trajectory("run-1").await.unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].message_ref, initial.message_ref);

    let history = store.load_compacted_history("run-1").await.unwrap();
    assert_eq!(history.messages.len(), 1);
    assert_eq!(history.messages[0].message_ref, older.message_ref);
}

#[tokio::test]
async fn checkpoint_append_recovers_a_torn_tail() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.create_run(&record(workspace.path())).await.unwrap();
    let first = checkpoint("cmp_1", None, "msg_1", "msg_2", "first");
    store.append_compaction("run-1", &first).await.unwrap();
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&paths.compactions)
        .await
        .unwrap();
    file.write_all(b"{\"checkpoint_id\":").await.unwrap();
    drop(file);

    assert_eq!(store.load_compactions("run-1").await.unwrap().len(), 1);
    let second = checkpoint("cmp_2", Some("cmp_1"), "msg_2", "msg_3", "second");
    store.append_compaction("run-1", &second).await.unwrap();
    let recovered = store.load_compactions("run-1").await.unwrap();
    assert_eq!(recovered.len(), 2);
    assert_eq!(recovered[1].checkpoint_id, "cmp_2");
}

fn checkpoint(
    checkpoint_id: &str,
    previous_checkpoint_id: Option<&str>,
    covered_through_message_ref: &str,
    first_kept_message_ref: &str,
    summary: &str,
) -> CompactionCheckpoint {
    CompactionCheckpoint {
        version: 1,
        checkpoint_id: checkpoint_id.to_owned(),
        created_at: chrono::Utc::now(),
        strategy: "local_summary_v1".to_owned(),
        previous_checkpoint_id: previous_checkpoint_id.map(str::to_owned),
        covered_through_message_ref: covered_through_message_ref.to_owned(),
        first_kept_message_ref: first_kept_message_ref.to_owned(),
        summary: summary.to_owned(),
        provider: "test-provider".to_owned(),
        model: "test-model".to_owned(),
        tokens_before: 100,
        summary_input_tokens: Some(20),
        summary_output_tokens: Some(5),
        compacted_message_count: 1,
    }
}
