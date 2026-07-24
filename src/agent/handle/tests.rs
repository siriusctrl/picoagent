use std::{sync::Arc, time::Duration};

use crate::{
    agent::runner::{AgentRunner, AgentRunnerConfig, RunnerOptions},
    artifact::{ArtifactStore, ResultMetadata},
    events::{EventSink, NoopEventSink, RuntimeEvent, SharedEventSink},
    hooks::HookPipeline,
    model::{Message, ModelProvider, Role, echo::EchoProvider},
    storage::{RunDirStore, RunRecord, RunState},
    tools::ToolRegistry,
};
use async_trait::async_trait;
use tempfile::TempDir;

use super::{
    HandleKind, HandleOutput, HandleState, RuntimeHandleManager, RuntimeHandleManagerConfig,
    SendMode,
};

fn test_manager(wait_timeout_seconds: u64) -> (TempDir, Arc<RuntimeHandleManager>) {
    test_manager_with_events(wait_timeout_seconds, Arc::new(NoopEventSink))
}

fn test_manager_with_events(
    wait_timeout_seconds: u64,
    events: SharedEventSink,
) -> (TempDir, Arc<RuntimeHandleManager>) {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(EchoProvider),
        model: "echo".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        mcp_catalog: String::new(),
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
        remaining_delegation_depth: 1,
        events,
        max_parallel_subagents: 1,
        wait_timeout_seconds,
    });
    (workspace, manager)
}

struct FailingEventSink;

#[async_trait]
impl EventSink for FailingEventSink {
    async fn emit(&self, _event: &RuntimeEvent) -> anyhow::Result<()> {
        anyhow::bail!("event sink unavailable")
    }
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
    .with_execution_context("general_task", 0)
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
async fn snapshots_unify_discovery_and_named_status_lookup() {
    let (workspace, manager) = test_manager(30);
    create_open_child(&manager, &workspace, "open-child").await;
    create_open_child(&manager, &workspace, "closed-child").await;
    manager
        .store
        .update_state("closed-child", RunState::Closed)
        .await
        .unwrap();

    let visible = manager.snapshots(&[], false).await.unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].handle, "open-child");
    assert_eq!(visible[0].status, HandleState::Idle);

    let all = manager.snapshots(&[], true).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(
        all.iter()
            .find(|snapshot| snapshot.handle == "closed-child")
            .unwrap()
            .status,
        HandleState::Closed
    );

    let named = manager
        .snapshots(&["closed-child".to_owned()], false)
        .await
        .unwrap();
    assert_eq!(named.len(), 1);
    assert_eq!(named[0].status, HandleState::Closed);
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
async fn send_launches_even_when_the_accepted_message_event_fails() {
    let (workspace, manager) = test_manager_with_events(30, Arc::new(FailingEventSink));
    create_open_child(&manager, &workspace, "child").await;
    manager
        .insert_agent("child".to_owned(), "child".to_owned())
        .await
        .unwrap();

    let sent = manager
        .send("child", "accepted input".to_owned(), SendMode::Followup)
        .await
        .unwrap();
    assert_eq!(sent["accepted_as"], "started");

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let records = manager.records.lock().await;
            if records["child"].state == HandleState::Idle && !records["child"].outputs.is_empty() {
                break;
            }
            drop(records);
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("event failure prevented the accepted message from launching");
    let messages = manager.store.load_messages("child").await.unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message.visible_text() == "accepted input")
    );
}

#[tokio::test]
async fn steer_after_the_final_boundary_queues_the_next_activity() {
    let (workspace, manager) = test_manager(30);
    create_open_child(&manager, &workspace, "child").await;
    manager
        .insert_agent("child".to_owned(), "child".to_owned())
        .await
        .unwrap();
    {
        let mut records = manager.records.lock().await;
        let record = records.get_mut("child").unwrap();
        record.state = HandleState::Running;
        record.generation = 1;
        record.mailbox.open().await;
        record.mailbox.seal().await;
    }

    let sent = manager
        .send("child", "next activity".to_owned(), SendMode::Steer)
        .await
        .unwrap();
    assert_eq!(sent["accepted_as"], "queued_followup");
    let records = manager.records.lock().await;
    assert!(records["child"].mailbox.is_empty().await);
    assert_eq!(records["child"].followups.len(), 1);
    assert_eq!(
        records["child"].followups[0].visible_text(),
        "next activity"
    );
    drop(records);

    manager
        .finish_agent_output(
            "child",
            1,
            HandleOutput {
                status: HandleState::Completed,
                content: "first activity".to_owned(),
                metadata: ResultMetadata::empty(),
            },
        )
        .await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let records = manager.records.lock().await;
            if records["child"].state == HandleState::Idle && records["child"].outputs.len() == 2 {
                break;
            }
            drop(records);
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("sealed steer did not start the next activity");
    let messages = manager.store.load_messages("child").await.unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message.visible_text() == "next activity")
    );
}

#[tokio::test]
async fn failed_activity_discards_mailbox_but_still_runs_explicit_followups() {
    let (workspace, manager) = test_manager(30);
    create_open_child(&manager, &workspace, "child").await;
    manager
        .insert_agent("child".to_owned(), "child".to_owned())
        .await
        .unwrap();
    {
        let mut records = manager.records.lock().await;
        let record = records.get_mut("child").unwrap();
        record.state = HandleState::Running;
        record.generation = 1;
        record
            .mailbox
            .queue(Message::text(Role::User, "uncommitted steer"))
            .await;
        record
            .followups
            .push(Message::text(Role::User, "explicit followup"));
    }

    manager
        .finish_agent_output(
            "child",
            1,
            HandleOutput {
                status: HandleState::Failed,
                content: "activity failed before mailbox drain".to_owned(),
                metadata: ResultMetadata::empty(),
            },
        )
        .await;

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let records = manager.records.lock().await;
            if records["child"].state == HandleState::Idle && records["child"].outputs.len() == 2 {
                break;
            }
            drop(records);
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("explicit followup did not start after failed activity");
    let messages = manager.store.load_messages("child").await.unwrap();
    assert!(
        !messages
            .iter()
            .any(|message| message.visible_text() == "uncommitted steer")
    );
    assert!(
        messages
            .iter()
            .any(|message| message.visible_text() == "explicit followup")
    );
}

#[tokio::test]
async fn idle_send_preserves_mailbox_then_followup_fifo_order() {
    let (workspace, manager) = test_manager(30);
    create_open_child(&manager, &workspace, "child").await;
    manager
        .insert_agent("child".to_owned(), "child".to_owned())
        .await
        .unwrap();
    {
        let mut records = manager.records.lock().await;
        let record = records.get_mut("child").unwrap();
        record
            .mailbox
            .queue(Message::text(Role::User, "retained mailbox input"))
            .await;
        record
            .followups
            .push(Message::text(Role::User, "followup one"));
        record
            .followups
            .push(Message::text(Role::User, "followup two"));
    }

    manager
        .send("child", "wakeup".to_owned(), SendMode::Steer)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let records = manager.records.lock().await;
            if records["child"].state == HandleState::Idle && !records["child"].outputs.is_empty() {
                break;
            }
            drop(records);
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("idle wakeup activity did not finish");

    let expected = [
        "retained mailbox input",
        "followup one",
        "followup two",
        "wakeup",
    ];
    let messages = manager.store.load_messages("child").await.unwrap();
    let observed = messages
        .iter()
        .map(Message::visible_text)
        .filter(|text| expected.contains(&text.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(observed, expected);
}

#[tokio::test]
async fn close_cancels_an_active_agent_before_durably_closing_it() {
    let (workspace, manager) = test_manager(30);
    create_open_child(&manager, &workspace, "child").await;
    manager
        .insert_agent("child".to_owned(), "child".to_owned())
        .await
        .unwrap();
    {
        let mut records = manager.records.lock().await;
        let record = records.get_mut("child").unwrap();
        record.state = HandleState::Running;
        record.generation = 1;
        record
            .mailbox
            .queue(Message::text(Role::User, "queued input"))
            .await;
        record.followups.push(Message::text(Role::User, "later"));
    }
    let activity = tokio::spawn(std::future::pending::<()>());
    let (cleanup_done, wait_for_cleanup) = tokio::sync::oneshot::channel();
    manager.track("child".to_owned(), 1, activity, Some(wait_for_cleanup));

    let closing = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.close("child").await })
    };
    tokio::time::timeout(Duration::from_millis(100), async {
        loop {
            if manager.records.lock().await["child"].state == HandleState::Closed {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("close did not block new input before waiting for child cleanup");
    assert!(!closing.is_finished());
    let error = manager
        .send("child", "too late".to_owned(), SendMode::Followup)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("closed"));

    cleanup_done.send(()).unwrap();
    let snapshot = closing.await.unwrap().unwrap();
    assert_eq!(snapshot.status, HandleState::Closed);
    assert_eq!(
        manager.store.load_run("child").await.unwrap().state,
        RunState::Closed
    );
    let records = manager.records.lock().await;
    assert!(records["child"].followups.is_empty());
    assert!(records["child"].mailbox.is_empty().await);
    assert!(records["child"].outputs.is_empty());
    assert!(!manager.executions.lock().unwrap().contains_key("child"));
}

#[tokio::test]
async fn concurrent_stop_finishes_cleanup_before_close() {
    let (workspace, manager) = test_manager(30);
    create_open_child(&manager, &workspace, "child").await;
    manager
        .insert_agent("child".to_owned(), "child".to_owned())
        .await
        .unwrap();
    {
        let mut records = manager.records.lock().await;
        let record = records.get_mut("child").unwrap();
        record.state = HandleState::Running;
        record.generation = 1;
    }
    let activity = tokio::spawn(std::future::pending::<()>());
    let (cleanup_done, wait_for_cleanup) = tokio::sync::oneshot::channel();
    manager.track("child".to_owned(), 1, activity, Some(wait_for_cleanup));

    let stopping = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.stop("child").await })
    };
    tokio::time::timeout(Duration::from_millis(100), async {
        loop {
            if !manager.executions.lock().unwrap().contains_key("child") {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("stop did not take ownership of the active execution");

    let closing = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.close("child").await })
    };
    tokio::task::yield_now().await;
    assert!(!closing.is_finished(), "close raced ahead of stop cleanup");

    cleanup_done.send(()).unwrap();
    let stopped = stopping.await.unwrap().unwrap();
    assert_eq!(stopped.status, HandleState::Idle);
    let closed = closing.await.unwrap().unwrap();
    assert_eq!(closed.status, HandleState::Closed);
    assert_eq!(
        manager.store.load_run("child").await.unwrap().state,
        RunState::Closed
    );
    assert!(!manager.executions.lock().unwrap().contains_key("child"));
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
    .with_execution_context("general_task", 0);
    child.state = RunState::Open;
    manager.store.create_run(&child).await.unwrap();

    assert_eq!(
        manager.store.load_run("opaque-name").await.unwrap().name,
        raw_name
    );
}
