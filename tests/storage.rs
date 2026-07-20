use picoagent::{
    events::{EventSink, RuntimeEvent, RuntimeEventKind},
    model::{Message, Role},
    storage::{RunDirStore, RunRecord, RunState},
};
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
    .with_provider_resume_fingerprint("sha256:test-provider-fingerprint")
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
        updated.provider_resume_fingerprint,
        "sha256:test-provider-fingerprint"
    );
    updated
        .verify_provider_resume_fingerprint("sha256:test-provider-fingerprint")
        .unwrap();
    assert!(
        updated
            .verify_provider_resume_fingerprint("sha256:different")
            .is_err()
    );
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
async fn only_one_process_lease_can_advance_a_run() {
    let workspace = tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    store.create_run(&record(workspace.path())).await.unwrap();

    let lease = store.acquire_run_lease("run-1").await.unwrap();
    let error = store.acquire_run_lease("run-1").await.unwrap_err();
    assert!(error.to_string().contains("already being executed"));
    drop(lease);
    store.acquire_run_lease("run-1").await.unwrap();
}
