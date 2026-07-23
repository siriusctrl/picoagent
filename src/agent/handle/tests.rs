use std::{sync::Arc, time::Duration};

use crate::{
    agent::runner::{AgentRunner, AgentRunnerConfig, RunnerOptions},
    artifact::{ArtifactStore, ResultMetadata},
    events::NoopEventSink,
    hooks::HookPipeline,
    model::{ModelProvider, echo::EchoProvider},
    storage::{RunDirStore, RunRecord, RunState},
    tools::ToolRegistry,
};
use tempfile::TempDir;

use super::{
    HandleKind, HandleOutput, HandleState, RuntimeHandleManager, RuntimeHandleManagerConfig,
    SendMode,
};

fn test_manager(wait_timeout_seconds: u64) -> (TempDir, Arc<RuntimeHandleManager>) {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(EchoProvider),
        model: "echo".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools: ToolRegistry::default(),
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions::default(),
    });
    let manager = RuntimeHandleManager::new(RuntimeHandleManagerConfig {
        runner,
        artifacts: ArtifactStore::default(),
        store,
        workspace: workspace.path().to_path_buf(),
        parent_run_id: "parent".to_owned(),
        parent_depth: 0,
        remaining_delegation_depth: 1,
        events: Arc::new(NoopEventSink),
        max_parallel_subagents: 1,
        wait_timeout_seconds,
    });
    (workspace, manager)
}

async fn create_open_child(manager: &RuntimeHandleManager, workspace: &TempDir, handle: &str) {
    let mut child = RunRecord::new(
        handle,
        "child",
        "child objective",
        EchoProvider.name(),
        "echo",
        workspace.path().to_path_buf(),
        Some("parent".to_owned()),
    )
    .with_execution_context("general_task_leaf", 1, None, 0)
    .with_provider_resume_fingerprint(EchoProvider.resume_fingerprint());
    child.state = RunState::Open;
    manager.store.create_run(&child).await.unwrap();
}

#[tokio::test]
async fn wait_returns_immediately_for_empty_and_idle_handle_sets() {
    let (_workspace, manager) = test_manager(30);
    let empty = tokio::time::timeout(Duration::from_millis(100), manager.wait(&[]))
        .await
        .expect("empty wait must not use the bounded interval")
        .unwrap();
    assert!(empty.is_empty());

    manager
        .insert_agent("idle-child".to_owned(), "idle".to_owned())
        .await
        .unwrap();
    let idle = tokio::time::timeout(
        Duration::from_millis(100),
        manager.wait(&["idle-child".to_owned()]),
    )
    .await
    .expect("idle selected handle must return immediately")
    .unwrap();
    assert_eq!(idle[0].status, HandleState::Idle);
}

#[tokio::test]
async fn wait_observes_a_change_that_happens_while_its_initial_snapshot_is_blocked() {
    let (_workspace, manager) = test_manager(30);
    manager
        .insert_agent("child".to_owned(), "child".to_owned())
        .await
        .unwrap();
    let mut records = manager.records.lock().await;
    records.get_mut("child").unwrap().state = HandleState::Running;

    let waiting = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.wait(&["child".to_owned()]).await })
    };
    tokio::task::yield_now().await;
    records.get_mut("child").unwrap().state = HandleState::Idle;
    manager.signal_activity();
    drop(records);

    let result = tokio::time::timeout(Duration::from_millis(100), waiting)
        .await
        .expect("subscribed waiter lost a concurrent state change")
        .unwrap()
        .unwrap();
    assert_eq!(result[0].status, HandleState::Idle);
}

#[tokio::test]
async fn close_wins_atomically_over_a_concurrent_idle_send() {
    let (workspace, manager) = test_manager(30);
    create_open_child(&manager, &workspace, "child").await;
    manager
        .insert_agent("child".to_owned(), "child".to_owned())
        .await
        .unwrap();

    let input_lock = manager.store.pending_input_lock();
    let input_guard = input_lock.lock().await;
    let closing = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.close("child").await })
    };
    tokio::time::timeout(Duration::from_millis(100), async {
        loop {
            if manager.records.try_lock().is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("close did not enter its serialized section");

    let sending = {
        let manager = manager.clone();
        tokio::spawn(async move {
            manager
                .send("child", "late input".to_owned(), SendMode::Followup)
                .await
        })
    };
    tokio::task::yield_now().await;
    assert!(!sending.is_finished());
    drop(input_guard);

    assert_eq!(closing.await.unwrap().unwrap().status, HandleState::Closed);
    assert!(
        sending
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("closed")
    );
    assert_eq!(
        manager.records.lock().await["child"].state,
        HandleState::Closed
    );
    assert!(!manager.executions.lock().unwrap().contains_key("child"));
    assert_eq!(
        manager.store.load_run("child").await.unwrap().state,
        RunState::Closed
    );
}

#[tokio::test]
async fn stale_stop_cannot_cancel_or_append_output_to_a_new_agent_generation() {
    let (_workspace, manager) = test_manager(30);
    manager
        .insert_agent("child".to_owned(), "child".to_owned())
        .await
        .unwrap();
    {
        let mut records = manager.records.lock().await;
        let record = records.get_mut("child").unwrap();
        record.state = HandleState::Running;
        record.generation = 2;
        record.outputs.push_back(HandleOutput {
            status: HandleState::Completed,
            content: "first generation completed".to_owned(),
            metadata: ResultMetadata::empty(),
        });
    }
    let next_activity = tokio::spawn(std::future::pending::<()>());
    manager.track("child".to_owned(), 2, next_activity, None);

    assert!(manager.take_execution("child", 1).is_none());
    let (snapshot, stopped) = manager.record_agent_stop("child", 1).await.unwrap();
    assert!(!stopped);
    assert_eq!(snapshot.status, HandleState::Running);
    let records = manager.records.lock().await;
    let record = &records["child"];
    assert_eq!(record.outputs.len(), 1);
    assert_eq!(record.outputs[0].status, HandleState::Completed);
    drop(records);

    let tracked = manager.take_execution("child", 2).unwrap();
    tracked.abort();
    tracked.wait().await;
}

#[tokio::test]
async fn tool_stop_does_not_append_cancelled_after_natural_completion() {
    let (_workspace, manager) = test_manager(30);
    manager
        .insert_tool("j_done".to_owned(), "tool".to_owned())
        .await
        .unwrap();
    {
        let mut records = manager.records.lock().await;
        let record = records.get_mut("j_done").unwrap();
        record.state = HandleState::Completed;
        record.outputs.push_back(HandleOutput {
            status: HandleState::Completed,
            content: "done".to_owned(),
            metadata: ResultMetadata::empty(),
        });
    }

    let (snapshot, stopped) = manager.record_tool_stop("j_done").await.unwrap();
    assert!(!stopped);
    assert_eq!(snapshot.kind, HandleKind::Tool);
    assert_eq!(snapshot.status, HandleState::Completed);
    let records = manager.records.lock().await;
    assert_eq!(records["j_done"].outputs.len(), 1);
    assert_eq!(records["j_done"].outputs[0].status, HandleState::Completed);
}

#[tokio::test]
async fn run_name_is_preserved_as_opaque_display_metadata() {
    let (workspace, manager) = test_manager(30);
    let raw_name = " ../验证\u{0} ";
    let mut child = RunRecord::new(
        "opaque-name",
        raw_name,
        "objective",
        EchoProvider.name(),
        "echo",
        workspace.path().to_path_buf(),
        Some("parent".to_owned()),
    )
    .with_execution_context("general_task_leaf", 1, None, 0);
    child.state = RunState::Open;
    manager.store.create_run(&child).await.unwrap();

    assert_eq!(
        manager.store.load_run("opaque-name").await.unwrap().name,
        raw_name
    );
}
