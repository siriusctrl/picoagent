use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::{Result, bail};
use async_trait::async_trait;
use picoagent::{
    agent::runner::{AgentRunner, AgentRunnerConfig, RunnerOptions},
    artifact::ArtifactStore,
    events::{NoopEventSink, SharedEventSink},
    hooks::HookPipeline,
    model::{
        Message, MessageContent, ModelProvider, ModelRequest, ModelResponse, ModelUsage, Role,
    },
    storage::{DelegateContext, RunDirStore, RunRecord, RunState},
    tools::ToolRegistry,
};
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Notify;

const ACTIVE_MARKER: &str = "<active-background-tasks>";

struct ResumeActiveTaskProvider {
    root_calls: AtomicUsize,
    requests: Mutex<Vec<ModelRequest>>,
    release_child: Notify,
}

impl ResumeActiveTaskProvider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            root_calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            release_child: Notify::new(),
        })
    }
}

#[async_trait]
impl ModelProvider for ResumeActiveTaskProvider {
    fn name(&self) -> &str {
        "resume-active-task"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        self.requests.lock().unwrap().push(request.clone());
        if request.run_id == "resume-parent" {
            match self.root_calls.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    self.release_child.notify_one();
                    Ok(text_response("wait for recovered work"))
                }
                1 => Ok(text_response("resume completed")),
                unexpected => bail!("unexpected resumed root call {unexpected}"),
            }
        } else if request.run_id == "resume-child" {
            self.release_child.notified().await;
            Ok(text_response("recovered child completed"))
        } else {
            bail!("unexpected run {}", request.run_id)
        }
    }
}

#[tokio::test]
async fn resume_rebuilds_active_task_reminder_without_persisting_it() {
    let workspace = TempDir::new().unwrap();
    let provider = ResumeActiveTaskProvider::new();
    let store = RunDirStore::new(workspace.path());
    create_resumable_parent_and_child(&store, workspace.path(), provider.as_ref()).await;
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: provider.clone(),
        model: "test-model".to_owned(),
        workspace: workspace.path().to_owned(),
        skill_catalog: String::new(),
        tools: ToolRegistry::default(),
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions {
            max_parallel_model_calls: 2,
            task_wait_timeout_seconds: 2,
            ..RunnerOptions::default()
        },
    });

    let result = runner.resume("resume-parent").await.unwrap();
    assert_eq!(result.final_output, "resume completed");
    {
        let requests = provider.requests.lock().unwrap();
        let root = requests
            .iter()
            .filter(|request| request.run_id == "resume-parent")
            .collect::<Vec<_>>();
        assert_eq!(root.len(), 2);
        assert!(request_contains(root[0], ACTIVE_MARKER));
        assert!(request_contains(
            root[0],
            "<task task_id=\"t1\" name=\"recovered review\" state=\"running\" />"
        ));
        assert!(!request_contains(root[0], "resume-child"));
        assert!(!request_contains(root[1], ACTIVE_MARKER));
    }

    let durable = store.load_messages("resume-parent").await.unwrap();
    assert!(
        durable
            .iter()
            .all(|message| !message_contains(message, ACTIVE_MARKER))
    );
}

async fn create_resumable_parent_and_child(
    store: &RunDirStore,
    workspace: &std::path::Path,
    provider: &dyn ModelProvider,
) {
    let fingerprint = provider.resume_fingerprint();
    store
        .create_run(
            &RunRecord::new(
                "resume-parent",
                "resume parent",
                provider.name(),
                "test-model",
                workspace.to_owned(),
                None,
            )
            .with_execution_context("root", 0, None, 1)
            .with_provider_resume_fingerprint(fingerprint.clone()),
        )
        .await
        .unwrap();
    store
        .update_state("resume-parent", RunState::Running)
        .await
        .unwrap();
    store
        .append_message("resume-parent", &Message::text(Role::User, "resume parent"))
        .await
        .unwrap();

    store
        .create_run(
            &RunRecord::new(
                "resume-child",
                "finish recovered work",
                provider.name(),
                "test-model",
                workspace.to_owned(),
                Some("resume-parent".to_owned()),
            )
            .with_execution_context(
                "general_task_leaf",
                1,
                Some(picoagent::prompts::agent_prompts().general_task.clone()),
                0,
            )
            .with_delegate_context(DelegateContext::Fresh, None)
            .with_provider_resume_fingerprint(fingerprint),
        )
        .await
        .unwrap();
    store
        .update_state("resume-child", RunState::Running)
        .await
        .unwrap();
    store
        .append_message(
            "resume-child",
            &Message::text(Role::User, "finish recovered work"),
        )
        .await
        .unwrap();

    let tasks = store.paths("resume-parent").directory.join("tasks");
    tokio::fs::create_dir_all(&tasks).await.unwrap();
    tokio::fs::write(
        tasks.join("t1.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 9,
            "id": "t1",
            "kind": "agent",
            "name": "recovered review",
            "origin_call_id": "resume-delegate-call",
            "state": "running",
            "result": null,
            "error": null,
            "child_run_id": "resume-child",
            "child_remaining_delegation_depth": 0,
            "delegate_context": "fresh",
            "fork_parent_message_seq": null,
            "prompt": "finish recovered work",
            "created_at": chrono::Utc::now()
        }))
        .unwrap(),
    )
    .await
    .unwrap();
}

fn text_response(text: &str) -> ModelResponse {
    ModelResponse::new(Message::text(Role::Assistant, text), ModelUsage::default())
}

fn request_contains(request: &ModelRequest, expected: &str) -> bool {
    request
        .messages
        .iter()
        .any(|message| message_contains(message, expected))
}

fn message_contains(message: &Message, expected: &str) -> bool {
    message.content.iter().any(|content| match content {
        MessageContent::RuntimeReminder { text } | MessageContent::Text { text } => {
            text.contains(expected)
        }
        _ => false,
    })
}
