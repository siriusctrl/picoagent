use super::*;
use crate::{
    agent::types::{AgentRunnerConfig, RunnerOptions},
    artifact::ArtifactPolicy,
    events::NoopEventSink,
    model::{Message, MessageContent, ModelProvider, Role, echo::EchoProvider},
    storage::{RunRecord, RunState},
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
            .with_execution_context("general_task_leaf", 1, None)
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
        tools: ToolRegistry::default(),
        artifacts,
        preview_budget: Arc::new(Mutex::new(128 * 1024)),
        store: store.clone(),
        workspace: workspace.to_path_buf(),
        parent_run_id: parent_run_id.to_owned(),
        parent_depth: 0,
        child_can_delegate: false,
        events: Arc::new(NoopEventSink),
        hooks: HookPipeline::new(),
        max_parallel_tasks: 2,
        default_execution_timeout_seconds: 300,
        default_wait_timeout_seconds: 30,
        max_execution_timeout_seconds: 1_800,
    }
}

#[tokio::test]
async fn restore_reconciles_tools_and_completed_children() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    create_child_run(&store, "child", "parent", "do work", RunState::Completed).await;
    store.write_final("child", "child result").await.unwrap();

    let task_store = TaskRecordStore::new(store.paths("parent").directory.join("tasks"));
    let mut tool =
        BackgroundTaskRecord::queued_tool("tool-task".to_owned(), "bash".to_owned(), 300);
    tool.state = BackgroundTaskState::Running;
    task_store.write(&tool).await.unwrap();
    let mut agent = BackgroundTaskRecord::queued_agent(
        "agent-task".to_owned(),
        "general-task".to_owned(),
        "child".to_owned(),
        "do work".to_owned(),
        300,
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
async fn restore_prefers_a_durable_completed_child_over_an_expired_task_deadline() {
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
        1,
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
            .with_execution_context("general_task_leaf", 1, Some("resume the child".to_owned()))
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
        300,
    );
    agent.state = BackgroundTaskState::Running;
    task_store.write(&agent).await.unwrap();

    let delivered = BackgroundTaskRecord {
        state: BackgroundTaskState::Completed,
        result: Some(record::BackgroundTaskOutput {
            content: "already delivered".to_owned(),
            metadata: crate::artifact::ResultMetadata::empty(),
        }),
        ..BackgroundTaskRecord::queued_tool("done-task".to_owned(), "read".to_owned(), 300)
    };
    task_store.write(&delivered).await.unwrap();
    store
        .append_message(
            "parent",
            &Message {
                role: Role::User,
                content: vec![MessageContent::BackgroundTaskResult {
                    task_id: "done-task".to_owned(),
                    name: "read".to_owned(),
                    status: "completed".to_owned(),
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
async fn restore_reserves_preview_bytes_for_an_undelivered_completed_task() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let task_store = TaskRecordStore::new(store.paths("parent").directory.join("tasks"));
    let mut task =
        BackgroundTaskRecord::queued_tool("done-task".to_owned(), "read".to_owned(), 300);
    task.state = BackgroundTaskState::Completed;
    task.result = Some(record::BackgroundTaskOutput {
        content: "bounded result".to_owned(),
        metadata: crate::artifact::ResultMetadata {
            artifact: None,
            preview_bytes: 11,
        },
    });
    task_store.write(&task).await.unwrap();

    let config = config(workspace.path(), &store, "parent");
    let preview_budget = config.preview_budget.clone();
    TaskManager::restore(config).await.unwrap();

    assert_eq!(*preview_budget.lock().await, 128 * 1024 - 11);
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
        300,
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
        300,
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
        300,
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
async fn expired_in_flight_tool_recovers_as_interrupted() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let task_store = TaskRecordStore::new(store.paths("parent").directory.join("tasks"));
    let mut task = BackgroundTaskRecord::queued_tool("tool-task".to_owned(), "bash".to_owned(), 1);
    task.state = BackgroundTaskState::Running;
    task.created_at = chrono::Utc::now() - chrono::Duration::minutes(10);
    task_store.write(&task).await.unwrap();

    let (manager, recoverable) = TaskManager::restore(config(workspace.path(), &store, "parent"))
        .await
        .unwrap();
    assert!(recoverable.is_empty());
    let recovered = manager.get("tool-task").await.unwrap();
    assert_eq!(recovered.state, BackgroundTaskState::Interrupted);
    assert!(
        recovered
            .model_content()
            .contains("side effects are unknown")
    );
}

struct FastTool;

#[async_trait::async_trait]
impl crate::tools::Tool for FastTool {
    fn spec(&self) -> crate::model::ToolSpec {
        crate::model::ToolSpec {
            name: "fast".to_owned(),
            description: "Return immediately".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    async fn execute(
        &self,
        _context: crate::tools::ToolContext,
        _arguments: serde_json::Value,
    ) -> anyhow::Result<crate::tools::RawToolOutput> {
        Ok(crate::tools::RawToolOutput::text("done"))
    }
}

#[tokio::test]
async fn background_tool_deadline_includes_before_hooks() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let mut task_config = config(workspace.path(), &store, "parent");
    task_config.tools.register(Arc::new(FastTool)).unwrap();
    task_config.hooks.register(crate::hooks::CommandHook::new(
        "slow-before",
        crate::hooks::HookEvent::ToolBefore,
        "sh",
        vec!["-c".into(), "sleep 5".into()],
    ));
    let manager = TaskManager::new(task_config);

    let task = manager
        .spawn_tool("fast".to_owned(), serde_json::json!({}), Some(1))
        .await
        .unwrap();
    let completed = tokio::time::timeout(Duration::from_secs(3), manager.wait_all())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].id, task.id);
    assert_eq!(completed[0].state, BackgroundTaskState::TimedOut);
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
        anyhow::bail!("{}", "error-detail-".repeat(80))
    }
}

#[tokio::test]
async fn background_tool_errors_use_the_artifact_and_preview_contract() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let mut task_config = config(workspace.path(), &store, "parent");
    task_config.tools.register(Arc::new(FailingTool)).unwrap();
    task_config.artifacts = ArtifactStore::new(ArtifactPolicy {
        inline_limit_bytes: 64,
        max_inline_bytes_per_run: 128,
        preview_head_bytes: 16,
        preview_tail_bytes: 16,
    });
    task_config.preview_budget = Arc::new(Mutex::new(128));
    let manager = TaskManager::new(task_config);

    manager
        .spawn_tool("failing".to_owned(), serde_json::json!({}), None)
        .await
        .unwrap();
    let completed = manager.wait_all().await.unwrap();

    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].state, BackgroundTaskState::Failed);
    assert!(completed[0].result_metadata().artifact.is_some());
    assert!(completed[0].model_content().contains("[Tool output]"));
    assert!(completed[0].model_content().contains("truncated: true"));
}

#[tokio::test]
async fn cancellation_guard_aborts_handles_without_committing_outside_the_run_lease() {
    use std::sync::atomic::{AtomicBool, Ordering};

    struct DropFlag(Arc<AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let manager = TaskManager::new(config(workspace.path(), &store, "parent"));
    let task_id = manager
        .create_tool_task("never".to_owned(), 300)
        .await
        .unwrap();
    manager.set_running(&task_id).await.unwrap();
    let dropped = Arc::new(AtomicBool::new(false));
    let started = Arc::new(tokio::sync::Notify::new());
    let handle = tokio::spawn({
        let dropped = dropped.clone();
        let started = started.clone();
        async move {
            let _flag = DropFlag(dropped);
            started.notify_one();
            std::future::pending::<()>().await;
        }
    });
    manager.track(handle);
    started.notified().await;

    let guard = manager.cancellation_guard();
    drop(guard);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if dropped.load(Ordering::SeqCst) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(
        manager.get(&task_id).await.unwrap().state,
        BackgroundTaskState::Running
    );
}
