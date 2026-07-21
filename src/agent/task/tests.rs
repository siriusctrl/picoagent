use super::*;
use crate::{
    agent::types::{AgentRunnerConfig, RunnerOptions},
    artifact::{ArtifactPolicy, ToolOutput},
    events::{EventSink, NoopEventSink, RuntimeEvent, RuntimeEventKind},
    hooks::HookPipeline,
    model::{Message, MessageContent, ModelProvider, Role, echo::EchoProvider},
    storage::{RunRecord, RunState},
    tools::ToolRegistry,
};

async fn create_run(store: &RunDirStore, id: &str, parent: Option<String>) {
    store
        .create_run(
            &RunRecord::new(
                id,
                "test",
                "echo",
                "echo",
                store.workspace().to_path_buf(),
                parent,
            )
            .with_provider_resume_fingerprint(EchoProvider.resume_fingerprint()),
        )
        .await
        .unwrap();
}

async fn create_child_run(
    store: &RunDirStore,
    id: &str,
    parent: &str,
    prompt: &str,
    state: RunState,
) {
    store
        .create_run(
            &RunRecord::new(
                id,
                prompt,
                "echo",
                "echo",
                store.workspace().to_path_buf(),
                Some(parent.to_owned()),
            )
            .with_execution_context("general_task_leaf", 1, None, 0)
            .with_provider_resume_fingerprint(EchoProvider.resume_fingerprint()),
        )
        .await
        .unwrap();
    store.update_state(id, state).await.unwrap();
}

fn config(
    workspace: &std::path::Path,
    store: &RunDirStore,
    parent_run_id: &str,
) -> TaskManagerConfig {
    let artifacts = ArtifactStore::new(ArtifactPolicy::default());
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(EchoProvider),
        model: "echo".to_owned(),
        workspace: workspace.to_path_buf(),
        skill_catalog: String::new(),
        tools: ToolRegistry::default(),
        artifacts: artifacts.clone(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions::default(),
    });
    TaskManagerConfig {
        runner,
        artifacts,
        store: store.clone(),
        workspace: workspace.to_path_buf(),
        parent_run_id: parent_run_id.to_owned(),
        parent_depth: 0,
        remaining_delegation_depth: 0,
        events: Arc::new(NoopEventSink),
        max_parallel_subagents: 2,
        wait_timeout_seconds: 30,
    }
}

#[tokio::test]
async fn task_ids_are_short_and_sequential_within_the_parent_run() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let manager = TaskManager::new(config(workspace.path(), &store, "parent"));

    let (first, second) = tokio::join!(
        manager.create_tool_task("first".to_owned(), "first-call".to_owned()),
        manager.create_tool_task("second".to_owned(), "second-call".to_owned())
    );
    let mut ids = vec![first.unwrap(), second.unwrap()];
    ids.sort();

    assert_eq!(ids, ["t1", "t2"]);
}

#[tokio::test]
async fn delegate_at_zero_remaining_depth_fails_without_creating_a_task() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let manager = TaskManager::new(config(workspace.path(), &store, "parent"));

    let error = manager
        .delegate("too-deep".to_owned(), "must not start".to_owned())
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("remaining delegation depth is 0")
    );
    assert!(manager.status(&[]).await.unwrap().is_empty());
    assert!(
        !tokio::fs::try_exists(store.paths("parent").directory.join("tasks"))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn promotion_recovery_does_not_reuse_an_older_task_with_the_same_call_id() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let manager = TaskManager::new(config(workspace.path(), &store, "parent"));
    let old_id = manager
        .create_tool_task("read".to_owned(), "provider-reused-call-id".to_owned())
        .await
        .unwrap();
    let boundary = chrono::Utc::now();
    manager
        .records
        .lock()
        .await
        .get_mut(&old_id)
        .unwrap()
        .created_at = boundary - chrono::Duration::seconds(1);

    assert!(
        manager
            .find_undelivered_promotion("provider-reused-call-id", "read", boundary)
            .await
            .is_none()
    );

    let current_id = manager
        .create_tool_task("read".to_owned(), "provider-reused-call-id".to_owned())
        .await
        .unwrap();
    assert_eq!(
        manager
            .find_undelivered_promotion("provider-reused-call-id", "read", boundary)
            .await
            .unwrap()
            .id,
        current_id
    );
    assert!(
        manager
            .find_undelivered_promotion("provider-reused-call-id", "bash", boundary)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn task_activity_broadcast_wakes_every_waiter_for_its_completed_task() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let mut task_config = config(workspace.path(), &store, "parent");
    task_config.wait_timeout_seconds = 5;
    let manager = TaskManager::new(task_config);
    let first = manager
        .create_tool_task("first".to_owned(), "first-call".to_owned())
        .await
        .unwrap();
    let second = manager
        .create_tool_task("second".to_owned(), "second-call".to_owned())
        .await
        .unwrap();
    manager.set_running(&first).await.unwrap();
    manager.set_running(&second).await.unwrap();

    let wait_for = |task_id: String| {
        let manager = manager.clone();
        tokio::spawn(async move { manager.wait(&[task_id]).await })
    };
    let first_waiters = [wait_for(first.clone()), wait_for(first.clone())];
    let second_waiters = [wait_for(second.clone()), wait_for(second.clone())];

    tokio::time::timeout(Duration::from_secs(1), async {
        while manager.activity.receiver_count() < 4 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("waiters did not subscribe to task activity");

    let output = || ToolOutput {
        preview: "done".to_owned(),
        artifact: None,
        truncated: false,
        is_error: false,
        preview_info: None,
        attachment: None,
    };
    manager.complete(&first, output()).await.unwrap();
    for waiter in first_waiters {
        let records = tokio::time::timeout(Duration::from_millis(500), waiter)
            .await
            .expect("a waiter slept after its selected task completed")
            .unwrap()
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, first);
        assert_eq!(records[0].state, BackgroundTaskState::Completed);
    }
    assert!(second_waiters.iter().all(|waiter| !waiter.is_finished()));

    manager.complete(&second, output()).await.unwrap();
    for waiter in second_waiters {
        let records = tokio::time::timeout(Duration::from_millis(500), waiter)
            .await
            .expect("a waiter slept after its selected task completed")
            .unwrap()
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, second);
        assert_eq!(records[0].state, BackgroundTaskState::Completed);
    }
}

#[tokio::test]
async fn restored_parent_continues_its_local_task_sequence() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let task_store = TaskRecordStore::new(store.paths("parent").directory.join("tasks"));
    task_store
        .write(&BackgroundTaskRecord::queued_tool(
            "t1".to_owned(),
            "earlier".to_owned(),
            "earlier-call".to_owned(),
        ))
        .await
        .unwrap();

    let (manager, _) = TaskManager::restore(config(workspace.path(), &store, "parent"))
        .await
        .unwrap();

    assert_eq!(
        manager
            .create_tool_task("later".to_owned(), "later-call".to_owned())
            .await
            .unwrap(),
        "t2"
    );
}

#[tokio::test]
async fn restore_reconciles_tools_and_completed_children() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    create_child_run(&store, "child", "parent", "do work", RunState::Completed).await;
    store.write_final("child", "child result").await.unwrap();

    let task_store = TaskRecordStore::new(store.paths("parent").directory.join("tasks"));
    let mut tool = BackgroundTaskRecord::queued_tool(
        "tool-task".to_owned(),
        "bash".to_owned(),
        "bash-call".to_owned(),
    );
    tool.state = BackgroundTaskState::Running;
    task_store.write(&tool).await.unwrap();
    let mut agent = BackgroundTaskRecord::queued_agent(
        "agent-task".to_owned(),
        "general-task".to_owned(),
        "child".to_owned(),
        "do work".to_owned(),
        0,
    );
    agent.state = BackgroundTaskState::Running;
    task_store.write(&agent).await.unwrap();

    let (manager, recoverable) = TaskManager::restore(config(workspace.path(), &store, "parent"))
        .await
        .unwrap();
    assert!(recoverable.is_empty());
    assert_eq!(
        manager.get("tool-task").await.unwrap().state,
        BackgroundTaskState::Interrupted
    );
    let agent = manager.get("agent-task").await.unwrap();
    assert_eq!(agent.state, BackgroundTaskState::Completed);
    assert_eq!(
        agent.result.as_ref().map(|result| result.content.as_str()),
        Some("child result")
    );

    let restored = task_store.load().await.unwrap();
    assert_eq!(
        restored["tool-task"].state,
        BackgroundTaskState::Interrupted
    );
    assert_eq!(restored["agent-task"].state, BackgroundTaskState::Completed);
}

#[tokio::test]
async fn restore_prefers_a_durable_completed_child_regardless_of_task_age() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    create_child_run(&store, "child", "parent", "do work", RunState::Completed).await;
    store
        .write_final("child", "late durable result")
        .await
        .unwrap();
    let task_store = TaskRecordStore::new(store.paths("parent").directory.join("tasks"));
    let mut agent = BackgroundTaskRecord::queued_agent(
        "agent-task".to_owned(),
        "general-task".to_owned(),
        "child".to_owned(),
        "do work".to_owned(),
        0,
    );
    agent.state = BackgroundTaskState::Running;
    agent.created_at = chrono::Utc::now() - chrono::Duration::minutes(10);
    task_store.write(&agent).await.unwrap();

    let (manager, recoverable) = TaskManager::restore(config(workspace.path(), &store, "parent"))
        .await
        .unwrap();

    assert!(recoverable.is_empty());
    let recovered = manager.get("agent-task").await.unwrap();
    assert_eq!(recovered.state, BackgroundTaskState::Completed);
    assert_eq!(
        recovered
            .result
            .as_ref()
            .map(|result| result.content.as_str()),
        Some("late durable result")
    );
}

#[tokio::test]
async fn restore_and_repeated_stop_repair_a_cancelled_tasks_live_child() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    create_child_run(&store, "child", "parent", "do work", RunState::Running).await;
    let task_store = TaskRecordStore::new(store.paths("parent").directory.join("tasks"));
    let mut task = BackgroundTaskRecord::queued_agent(
        "agent-task".to_owned(),
        "general-task".to_owned(),
        "child".to_owned(),
        "do work".to_owned(),
        0,
    );
    task.state = BackgroundTaskState::Cancelled;
    task_store.write(&task).await.unwrap();

    let (manager, recoverable) = TaskManager::restore(config(workspace.path(), &store, "parent"))
        .await
        .unwrap();
    assert!(recoverable.is_empty());
    assert_eq!(
        store.load_run("child").await.unwrap().state,
        RunState::Cancelled
    );

    store
        .update_state("child", RunState::Running)
        .await
        .unwrap();
    let stopped = manager.stop("agent-task").await.unwrap();
    assert_eq!(stopped.state, BackgroundTaskState::Cancelled);
    assert_eq!(
        store.load_run("child").await.unwrap().state,
        RunState::Cancelled
    );
}

#[tokio::test]
async fn restore_validates_a_live_child_against_its_stored_capability() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    store
        .create_run(
            &RunRecord::new(
                "child",
                "do work",
                "echo",
                "echo",
                store.workspace().to_path_buf(),
                Some("parent".to_owned()),
            )
            .with_execution_context("general_task_delegating", 1, None, 1)
            .with_provider_resume_fingerprint(EchoProvider.resume_fingerprint()),
        )
        .await
        .unwrap();
    store
        .update_state("child", RunState::Running)
        .await
        .unwrap();
    let task_store = TaskRecordStore::new(store.paths("parent").directory.join("tasks"));
    let mut task = BackgroundTaskRecord::queued_agent(
        "agent-task".to_owned(),
        "general-task".to_owned(),
        "child".to_owned(),
        "do work".to_owned(),
        1,
    );
    task.state = BackgroundTaskState::Running;
    task_store.write(&task).await.unwrap();

    // The current parent configuration says a new child would be a leaf, but
    // this existing child was durably fixed as delegating before it started.
    let (manager, recoverable) = TaskManager::restore(config(workspace.path(), &store, "parent"))
        .await
        .unwrap();
    assert_eq!(recoverable.len(), 1);
    assert_eq!(
        manager
            .get("agent-task")
            .await
            .unwrap()
            .child_remaining_delegation_depth,
        Some(1)
    );
}

#[tokio::test]
async fn restore_returns_live_subagents_and_derives_delivery_from_messages() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    store
        .create_run(
            &RunRecord::new(
                "child",
                "resume me",
                "echo",
                "echo",
                store.workspace().to_path_buf(),
                Some("parent".to_owned()),
            )
            .with_execution_context(
                "general_task_leaf",
                1,
                Some("resume the child".to_owned()),
                0,
            )
            .with_provider_resume_fingerprint(EchoProvider.resume_fingerprint()),
        )
        .await
        .unwrap();
    store
        .update_state("child", RunState::Running)
        .await
        .unwrap();
    store
        .append_message("child", &Message::text(Role::User, "resume me"))
        .await
        .unwrap();
    let task_store = TaskRecordStore::new(store.paths("parent").directory.join("tasks"));
    let mut agent = BackgroundTaskRecord::queued_agent(
        "agent-task".to_owned(),
        "general-task".to_owned(),
        "child".to_owned(),
        "resume me".to_owned(),
        0,
    );
    agent.state = BackgroundTaskState::Running;
    task_store.write(&agent).await.unwrap();

    let delivered = BackgroundTaskRecord {
        state: BackgroundTaskState::Completed,
        result: Some(record::BackgroundTaskOutput {
            content: "already delivered".to_owned(),
            metadata: crate::artifact::ResultMetadata::empty(),
        }),
        ..BackgroundTaskRecord::queued_tool(
            "done-task".to_owned(),
            "read".to_owned(),
            "read-call".to_owned(),
        )
    };
    task_store.write(&delivered).await.unwrap();
    store
        .append_message(
            "parent",
            &Message {
                role: Role::User,
                content: vec![MessageContent::BackgroundTask {
                    task_id: "done-task".to_owned(),
                    name: "read".to_owned(),
                    status: Some("completed".to_owned()),
                    content: "already delivered".to_owned(),
                    metadata: crate::artifact::ResultMetadata::empty(),
                }],
            },
        )
        .await
        .unwrap();

    let (manager, recoverable) = TaskManager::restore(config(workspace.path(), &store, "parent"))
        .await
        .unwrap();
    assert_eq!(recoverable.len(), 1);
    assert_eq!(recoverable[0].task_id, "agent-task");
    assert_eq!(recoverable[0].child_run_id, "child");
    assert_eq!(recoverable[0].prompt, "resume me");
    assert_eq!(
        manager
            .drain_completed()
            .await
            .unwrap()
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>(),
        Vec::<&str>::new()
    );

    manager
        .resume_agent_task(recoverable[0].clone())
        .await
        .unwrap();
    let completed = tokio::time::timeout(std::time::Duration::from_secs(5), manager.wait_all())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].id, "agent-task");
    assert_eq!(
        completed[0].state,
        BackgroundTaskState::Completed,
        "{:#?}",
        completed[0]
    );
    assert!(
        completed[0]
            .result
            .as_ref()
            .is_some_and(|result| result.content.contains("received:"))
    );
}

#[tokio::test]
async fn resume_agent_task_creates_a_missing_child_run() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let task_store = TaskRecordStore::new(store.paths("parent").directory.join("tasks"));
    let task = BackgroundTaskRecord::queued_agent(
        "agent-task".to_owned(),
        "general-task".to_owned(),
        "missing-child".to_owned(),
        "start after recovery".to_owned(),
        1,
    );
    task_store.write(&task).await.unwrap();

    let (manager, recoverable) = TaskManager::restore(config(workspace.path(), &store, "parent"))
        .await
        .unwrap();
    assert_eq!(recoverable.len(), 1);
    manager
        .resume_agent_task(recoverable[0].clone())
        .await
        .unwrap();
    let completed = tokio::time::timeout(std::time::Duration::from_secs(5), manager.wait_all())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].state, BackgroundTaskState::Completed);
    assert_eq!(
        store.load_run("missing-child").await.unwrap().state,
        RunState::Completed
    );
    assert_eq!(
        store.load_run("missing-child").await.unwrap().profile,
        "general_task_delegating"
    );
}

#[tokio::test]
async fn recovered_child_retries_a_busy_lease_and_reconciles_completion() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    create_child_run(&store, "child", "parent", "resume me", RunState::Running).await;
    let task_store = TaskRecordStore::new(store.paths("parent").directory.join("tasks"));
    let mut task = BackgroundTaskRecord::queued_agent(
        "agent-task".to_owned(),
        "general-task".to_owned(),
        "child".to_owned(),
        "resume me".to_owned(),
        0,
    );
    task.state = BackgroundTaskState::Running;
    task_store.write(&task).await.unwrap();
    let child_lease = store.acquire_run_lease("child").await.unwrap();
    let (manager, recoverable) = TaskManager::restore(config(workspace.path(), &store, "parent"))
        .await
        .unwrap();

    manager
        .resume_agent_task(recoverable[0].clone())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    store
        .write_final("child", "completed by old owner")
        .await
        .unwrap();
    store
        .update_state("child", RunState::Completed)
        .await
        .unwrap();
    drop(child_lease);
    let completed = tokio::time::timeout(Duration::from_secs(3), manager.wait_all())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].state, BackgroundTaskState::Completed);
    assert_eq!(
        completed[0]
            .result
            .as_ref()
            .map(|result| result.content.as_str()),
        Some("completed by old owner")
    );
}

#[tokio::test]
async fn recovery_rejects_a_child_owned_by_another_parent() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    create_run(&store, "other-parent", None).await;
    create_child_run(
        &store,
        "child",
        "other-parent",
        "do work",
        RunState::Running,
    )
    .await;
    let task_store = TaskRecordStore::new(store.paths("parent").directory.join("tasks"));
    let mut task = BackgroundTaskRecord::queued_agent(
        "agent-task".to_owned(),
        "general-task".to_owned(),
        "child".to_owned(),
        "do work".to_owned(),
        0,
    );
    task.state = BackgroundTaskState::Running;
    task_store.write(&task).await.unwrap();

    let error = match TaskManager::restore(config(workspace.path(), &store, "parent")).await {
        Ok(_) => panic!("unrelated child run was accepted"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("does not belong to parent"));
}

#[tokio::test]
async fn inspect_pages_native_child_messages_and_steer_appends_after_the_current_batch() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    create_child_run(&store, "child", "parent", "do work", RunState::Running).await;
    for index in 1..=8 {
        let role = if index % 2 == 0 {
            Role::Assistant
        } else {
            Role::User
        };
        store
            .append_message("child", &Message::text(role, format!("message-{index}")))
            .await
            .unwrap();
    }
    let task_store = TaskRecordStore::new(store.paths("parent").directory.join("tasks"));
    let mut task = BackgroundTaskRecord::queued_agent(
        "agent-task".to_owned(),
        "general-task".to_owned(),
        "child".to_owned(),
        "do work".to_owned(),
        0,
    );
    task.state = BackgroundTaskState::Running;
    task_store.write(&task).await.unwrap();
    let (manager, recoverable) = TaskManager::restore(config(workspace.path(), &store, "parent"))
        .await
        .unwrap();
    assert_eq!(recoverable.len(), 1);

    let latest = manager.inspect("agent-task", None, 6).await.unwrap();
    assert_eq!(latest["messages"][0]["seq"], 3);
    assert_eq!(latest["messages"][5]["seq"], 8);
    assert_eq!(latest["messages"][0]["message"]["role"], "user");
    assert_eq!(latest["messages"][5]["message"]["content"], "message-8");
    assert_eq!(latest["has_earlier"], true);
    assert_eq!(latest["next_before_seq"], 3);
    assert!(latest.get("child_run_id").is_none());

    let earlier = manager.inspect("agent-task", Some(3), 6).await.unwrap();
    assert_eq!(earlier["messages"].as_array().unwrap().len(), 2);
    assert_eq!(earlier["messages"][0]["seq"], 1);
    assert_eq!(earlier["messages"][1]["seq"], 2);
    assert_eq!(earlier["has_earlier"], false);
    assert!(earlier["next_before_seq"].is_null());

    let steering = manager
        .steer("agent-task", "change direction".to_owned())
        .await
        .unwrap();
    assert_eq!(
        steering,
        serde_json::json!({
            "task_id": "agent-task",
            "name": "general-task",
            "status": "running"
        })
    );
    let mut trajectory = store.load_trajectory("child").await.unwrap();
    let appended = store
        .append_pending_inputs("child", &mut trajectory)
        .await
        .unwrap();
    assert_eq!(appended.len(), 1);
    assert_eq!(appended[0].seq, 9);
    assert_eq!(appended[0].message_ref, "m9");
    assert!(appended[0].pending_input_id.is_some());
    assert_eq!(appended[0].message.role, Role::User);
    assert_eq!(appended[0].message.visible_text(), "change direction");

    let reopened = RunDirStore::new(workspace.path());
    let mut recovered = reopened.load_trajectory("child").await.unwrap();
    let duplicate = reopened
        .append_pending_inputs("child", &mut recovered)
        .await
        .unwrap();
    assert!(duplicate.is_empty());
}

#[tokio::test]
async fn in_flight_tool_recovers_as_interrupted_regardless_of_task_age() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let task_store = TaskRecordStore::new(store.paths("parent").directory.join("tasks"));
    let mut task = BackgroundTaskRecord::queued_tool(
        "tool-task".to_owned(),
        "bash".to_owned(),
        "bash-call".to_owned(),
    );
    task.state = BackgroundTaskState::Running;
    task.created_at = chrono::Utc::now() - chrono::Duration::minutes(10);
    task_store.write(&task).await.unwrap();

    let (manager, recoverable) = TaskManager::restore(config(workspace.path(), &store, "parent"))
        .await
        .unwrap();
    assert!(recoverable.is_empty());
    let recovered = manager.get("tool-task").await.unwrap();
    assert_eq!(recovered.state, BackgroundTaskState::Interrupted);
    assert_eq!(recovered.origin_call_id.as_deref(), Some("bash-call"));
    assert!(
        recovered
            .model_content()
            .contains("side effects are unknown")
    );
}

#[tokio::test]
async fn resume_load_does_not_reconcile_tasks_before_capability_validation() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let task_store = TaskRecordStore::new(store.paths("parent").directory.join("tasks"));
    let mut task = BackgroundTaskRecord::queued_tool(
        "tool-task".to_owned(),
        "bash".to_owned(),
        "bash-call".to_owned(),
    );
    task.state = BackgroundTaskState::Running;
    task_store.write(&task).await.unwrap();

    let manager = TaskManager::load_for_resume(config(workspace.path(), &store, "parent"))
        .await
        .unwrap();
    assert_eq!(
        manager.get("tool-task").await.unwrap().state,
        BackgroundTaskState::Running
    );

    manager.reconcile_after_restart().await.unwrap();
    assert_eq!(
        manager.get("tool-task").await.unwrap().state,
        BackgroundTaskState::Interrupted
    );
}

struct FailingTool;

#[async_trait::async_trait]
impl crate::tools::Tool for FailingTool {
    fn spec(&self) -> crate::model::ToolSpec {
        crate::model::ToolSpec {
            name: "failing".to_owned(),
            description: "Return a large error".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    async fn execute(
        &self,
        _context: crate::tools::ToolContext,
        _arguments: serde_json::Value,
    ) -> anyhow::Result<crate::tools::RawToolOutput> {
        tokio::time::sleep(Duration::from_millis(20)).await;
        anyhow::bail!("{}", "error-detail-".repeat(80))
    }
}

#[tokio::test]
async fn background_tool_errors_use_the_artifact_and_preview_contract() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let mut tools = ToolRegistry::default();
    tools.register(Arc::new(FailingTool)).unwrap();
    let artifacts = ArtifactStore::new(ArtifactPolicy {
        inline_limit_bytes: 64,
        preview_head_bytes: 16,
        preview_tail_bytes: 16,
    });
    let hooks = HookPipeline::new();
    let events: crate::events::SharedEventSink = Arc::new(NoopEventSink);
    let mut task_config = config(workspace.path(), &store, "parent");
    task_config.artifacts = artifacts.clone();
    let manager = TaskManager::new(task_config);
    let runtime = crate::agent::tool_execution::DirectToolRuntime {
        registry: &tools,
        hooks: &hooks,
        artifacts: &artifacts,
        events: &events,
        workspace: workspace.path(),
        run_id: "parent",
        manager: manager.clone(),
        foreground_timeout_seconds: 0,
    };
    runtime
        .execute(crate::model::ToolCall {
            id: "failing-call".to_owned(),
            name: "failing".to_owned(),
            arguments: serde_json::json!({}),
        })
        .await
        .unwrap();
    let completed = manager.wait_all().await.unwrap();

    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].state, BackgroundTaskState::Failed);
    let artifact = completed[0].result_metadata().artifact.unwrap();
    assert_eq!(artifact.call_id, "failing-call");
    assert!(completed[0].model_content().contains("[Tool output]"));
    assert!(completed[0].model_content().contains("truncated: true"));
}

#[derive(Default)]
struct CompletionEventSink {
    background_completed: std::sync::atomic::AtomicBool,
    subagent_completed: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl EventSink for CompletionEventSink {
    async fn emit(&self, event: &RuntimeEvent) -> anyhow::Result<()> {
        use std::sync::atomic::Ordering;

        match &event.kind {
            RuntimeEventKind::BackgroundTaskCompleted { .. } => {
                self.background_completed.store(true, Ordering::SeqCst);
            }
            RuntimeEventKind::SubagentCompleted { .. } => {
                self.subagent_completed.store(true, Ordering::SeqCst);
            }
            _ => {}
        }
        Ok(())
    }
}

#[tokio::test]
async fn cancelled_agent_does_not_emit_completion_events_when_its_output_arrives() {
    use std::sync::atomic::Ordering;

    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let events = Arc::new(CompletionEventSink::default());
    let mut task_config = config(workspace.path(), &store, "parent");
    task_config.events = events.clone();
    let manager = TaskManager::new(task_config);
    let task_id = manager
        .create_agent_task(
            "general-task".to_owned(),
            "child".to_owned(),
            "do work".to_owned(),
        )
        .await
        .unwrap();
    manager
        .cancel(&task_id, "stopped by parent agent".to_owned())
        .await
        .unwrap();

    manager
        .finish_agent_output(
            &task_id,
            "general-task",
            "child",
            ToolOutput {
                preview: "late result".to_owned(),
                artifact: None,
                truncated: false,
                is_error: false,
                preview_info: None,
                attachment: None,
            },
        )
        .await;

    assert_eq!(
        manager.get(&task_id).await.unwrap().state,
        BackgroundTaskState::Cancelled
    );
    assert!(!events.background_completed.load(Ordering::SeqCst));
    assert!(!events.subagent_completed.load(Ordering::SeqCst));
}

struct FailingEventSink;

#[async_trait::async_trait]
impl EventSink for FailingEventSink {
    async fn emit(&self, _event: &RuntimeEvent) -> anyhow::Result<()> {
        anyhow::bail!("event sink unavailable")
    }
}

#[tokio::test]
async fn committed_steering_succeeds_when_its_observation_event_fails() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    create_child_run(&store, "child", "parent", "do work", RunState::Running).await;
    let task_store = TaskRecordStore::new(store.paths("parent").directory.join("tasks"));
    let mut task = BackgroundTaskRecord::queued_agent(
        "agent-task".to_owned(),
        "general-task".to_owned(),
        "child".to_owned(),
        "do work".to_owned(),
        0,
    );
    task.state = BackgroundTaskState::Running;
    task_store.write(&task).await.unwrap();
    let mut task_config = config(workspace.path(), &store, "parent");
    task_config.events = Arc::new(FailingEventSink);
    let (manager, _) = TaskManager::restore(task_config).await.unwrap();

    manager
        .steer("agent-task", "change direction".to_owned())
        .await
        .unwrap();
    let mut trajectory = store.load_trajectory("child").await.unwrap();
    let appended = store
        .append_pending_inputs("child", &mut trajectory)
        .await
        .unwrap();
    assert_eq!(appended.len(), 1);
    assert_eq!(appended[0].message.visible_text(), "change direction");
}

#[tokio::test]
async fn cancellation_cleanup_holds_the_parent_lease_until_tasks_are_settled() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let manager = TaskManager::new(config(workspace.path(), &store, "parent"));
    let task_id = manager
        .create_tool_task("never".to_owned(), "never-call".to_owned())
        .await
        .unwrap();
    manager.set_running(&task_id).await.unwrap();
    let started = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let handle = tokio::task::spawn_blocking({
        let started = started.clone();
        let release = release.clone();
        move || {
            started.store(true, Ordering::SeqCst);
            while !release.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
        }
    });
    manager.track(task_id.clone(), handle);
    while !started.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }

    let guard = manager.cancellation_guard(store.acquire_run_lease("parent").await.unwrap());
    drop(guard);
    let busy = store.acquire_run_lease("parent").await.unwrap_err();
    assert!(
        busy.downcast_ref::<crate::storage::RunLeaseBusy>()
            .is_some()
    );

    release.store(true, Ordering::SeqCst);
    let _lease = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if manager.get(&task_id).await.unwrap().state != BackgroundTaskState::Cancelled {
                tokio::task::yield_now().await;
                continue;
            }
            match store.acquire_run_lease("parent").await {
                Ok(lease) => break lease,
                Err(error)
                    if error
                        .downcast_ref::<crate::storage::RunLeaseBusy>()
                        .is_some() =>
                {
                    tokio::task::yield_now().await;
                }
                Err(error) => panic!("acquire failed after cleanup: {error:#}"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        manager.get(&task_id).await.unwrap().state,
        BackgroundTaskState::Cancelled
    );
}

struct SlowContinuationTool(Arc<std::sync::atomic::AtomicUsize>);

#[async_trait::async_trait]
impl crate::tools::Tool for SlowContinuationTool {
    fn spec(&self) -> crate::model::ToolSpec {
        crate::model::ToolSpec {
            name: "slow_continuation".to_owned(),
            description: "Finish after the foreground window".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    async fn execute(
        &self,
        _context: crate::tools::ToolContext,
        _arguments: serde_json::Value,
    ) -> anyhow::Result<crate::tools::RawToolOutput> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        Ok(crate::tools::RawToolOutput::text("continued result"))
    }
}

#[tokio::test]
async fn foreground_timeout_promotes_the_same_tool_future_instead_of_stopping_or_restarting_it() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    store
        .update_state("parent", RunState::Running)
        .await
        .unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut tools = ToolRegistry::default();
    tools
        .register(Arc::new(SlowContinuationTool(calls.clone())))
        .unwrap();
    let artifacts = ArtifactStore::new(ArtifactPolicy::default());
    let hooks = HookPipeline::new();
    let events: crate::events::SharedEventSink = Arc::new(NoopEventSink);
    let mut task_config = config(workspace.path(), &store, "parent");
    task_config.artifacts = artifacts.clone();
    task_config.events = events.clone();
    let manager = TaskManager::new(task_config);
    let runtime = crate::agent::tool_execution::DirectToolRuntime {
        registry: &tools,
        hooks: &hooks,
        artifacts: &artifacts,
        events: &events,
        workspace: workspace.path(),
        run_id: "parent",
        manager: manager.clone(),
        foreground_timeout_seconds: 1,
    };

    let message = runtime
        .execute(crate::model::ToolCall {
            id: "slow-call".to_owned(),
            name: "slow_continuation".to_owned(),
            arguments: serde_json::json!({}),
        })
        .await
        .unwrap();
    let acknowledgement = match &message.content[0] {
        MessageContent::ToolResult { content, .. } => content,
        other => panic!("unexpected foreground acknowledgement: {other:?}"),
    };
    assert!(acknowledgement.contains("<background_task task_id=\"t1\""));
    assert!(acknowledgement.contains("name=\"slow_continuation\""));
    assert!(!acknowledgement.contains("status="));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

    let completed = tokio::time::timeout(Duration::from_secs(2), manager.wait_all())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].state, BackgroundTaskState::Completed);
    assert_eq!(completed[0].model_content(), "continued result");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

struct BlockingDropTool {
    started: Arc<tokio::sync::Notify>,
    dropped: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl crate::tools::Tool for BlockingDropTool {
    fn spec(&self) -> crate::model::ToolSpec {
        crate::model::ToolSpec {
            name: "blocking_drop".to_owned(),
            description: "Block until cancelled".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    async fn execute(
        &self,
        _context: crate::tools::ToolContext,
        _arguments: serde_json::Value,
    ) -> anyhow::Result<crate::tools::RawToolOutput> {
        struct DropFlag(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let _drop = DropFlag(self.dropped.clone());
        self.started.notify_one();
        std::future::pending::<()>().await;
        unreachable!()
    }
}

#[tokio::test]
async fn stop_aborts_only_the_selected_background_task_and_commits_cancelled_state() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let started = Arc::new(tokio::sync::Notify::new());
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut tools = ToolRegistry::default();
    tools
        .register(Arc::new(BlockingDropTool {
            started: started.clone(),
            dropped: dropped.clone(),
        }))
        .unwrap();
    let artifacts = ArtifactStore::new(ArtifactPolicy::default());
    let hooks = HookPipeline::new();
    let events: crate::events::SharedEventSink = Arc::new(NoopEventSink);
    let task_config = config(workspace.path(), &store, "parent");
    let manager = TaskManager::new(task_config);
    let runtime = crate::agent::tool_execution::DirectToolRuntime {
        registry: &tools,
        hooks: &hooks,
        artifacts: &artifacts,
        events: &events,
        workspace: workspace.path(),
        run_id: "parent",
        manager: manager.clone(),
        foreground_timeout_seconds: 0,
    };
    let message = runtime
        .execute(crate::model::ToolCall {
            id: "blocking-call".to_owned(),
            name: "blocking_drop".to_owned(),
            arguments: serde_json::json!({}),
        })
        .await
        .unwrap();
    let task_id = match &message.content[0] {
        MessageContent::ToolResult { content, .. } => {
            assert!(content.contains("<background_task task_id=\"t1\""));
            "t1".to_owned()
        }
        other => panic!("unexpected foreground acknowledgement: {other:?}"),
    };
    started.notified().await;

    let stopped = manager.stop(&task_id).await.unwrap();
    assert_eq!(stopped.state, BackgroundTaskState::Cancelled);
    assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        manager.get(&task_id).await.unwrap().state,
        BackgroundTaskState::Cancelled
    );
    assert_eq!(
        TaskRecordStore::new(store.paths("parent").directory.join("tasks"))
            .load()
            .await
            .unwrap()[&task_id]
            .state,
        BackgroundTaskState::Cancelled
    );
}
