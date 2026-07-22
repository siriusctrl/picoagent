use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::{Result, bail};
use async_trait::async_trait;
use fiasco::{
    agent::runner::{AgentRunner, AgentRunnerConfig, RunnerOptions},
    artifact::ArtifactStore,
    events::{NoopEventSink, SharedEventSink},
    hooks::HookPipeline,
    model::{
        Message, MessageContent, ModelProvider, ModelRequest, ModelResponse, ModelUsage, Role,
    },
    storage::{RunDirStore, RunRecord, RunState},
    tools::ToolRegistry,
};
use serde_json::json;
use tempfile::TempDir;

const ACTIVE_MARKER: &str = "<active-background-tasks>";

struct ResumeActiveTaskProvider {
    root_calls: AtomicUsize,
    child_calls: AtomicUsize,
    requests: Mutex<Vec<ModelRequest>>,
}

impl ResumeActiveTaskProvider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            root_calls: AtomicUsize::new(0),
            child_calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
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
            let call = self.root_calls.fetch_add(1, Ordering::SeqCst);
            if call != 0 {
                bail!("unexpected resumed root call {call}");
            }
            let interrupted = request
                .messages
                .iter()
                .flat_map(|message| &message.content)
                .any(|content| {
                    matches!(
                        content,
                        MessageContent::BackgroundTask {
                            task_id,
                            status: Some(status),
                            ..
                        } if task_id == "t1" && status == "interrupted"
                    )
                });
            if !interrupted || request_contains(&request, ACTIVE_MARKER) {
                bail!("resume did not expose one inactive interrupted child");
            }
            Ok(text_response("resume completed after interruption"))
        } else if request.run_id == "resume-child" {
            self.child_calls.fetch_add(1, Ordering::SeqCst);
            bail!("restart must not automatically resume the child")
        } else {
            bail!("unexpected run {}", request.run_id)
        }
    }
}

#[tokio::test]
async fn resume_reports_an_interrupted_child_without_restarting_it() {
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
    assert_eq!(result.final_output, "resume completed after interruption");
    assert_eq!(provider.root_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.child_calls.load(Ordering::SeqCst), 0);
    {
        let requests = provider.requests.lock().unwrap();
        let root = requests
            .iter()
            .filter(|request| request.run_id == "resume-parent")
            .collect::<Vec<_>>();
        assert_eq!(root.len(), 1);
        assert!(!request_contains(root[0], ACTIVE_MARKER));
    }

    let durable = store.load_messages("resume-parent").await.unwrap();
    assert!(
        durable
            .iter()
            .all(|message| !message_contains(message, ACTIVE_MARKER))
    );
    assert_eq!(
        durable
            .iter()
            .flat_map(|message| &message.content)
            .filter(|content| matches!(
                content,
                MessageContent::BackgroundTask {
                    task_id,
                    status: Some(status),
                    ..
                } if task_id == "t1" && status == "interrupted"
            ))
            .count(),
        1
    );
    assert_eq!(
        store.load_run("resume-child").await.unwrap().state,
        RunState::Idle
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
        .append_checkpoint(
            "resume-parent",
            &[
                Message {
                    role: Role::Assistant,
                    content: vec![MessageContent::ToolCall {
                        id: "resume-delegate-call".to_owned(),
                        name: "delegate".to_owned(),
                        arguments: json!({}).into(),
                    }],
                },
                Message {
                    role: Role::Tool,
                    content: vec![MessageContent::ToolResult {
                        call_id: "resume-delegate-call".to_owned(),
                        content: "background task started".to_owned(),
                        is_error: false,
                        metadata: fiasco::artifact::ResultMetadata::empty(),
                    }],
                },
            ],
        )
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
                Some(fiasco::prompts::agent_prompts().general_task.clone()),
                0,
            )
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
            "version": 12,
            "id": "t1",
            "kind": "agent",
            "name": "recovered review",
            "origin_call_id": "resume-delegate-call",
            "state": "running",
            "outputs": [],
            "pending_followups": [],
            "paused": false,
            "child_run_id": "resume-child",
            "child_remaining_delegation_depth": 0,
            "prompt": "finish recovered work",
            "created_at": chrono::Utc::now() - chrono::Duration::seconds(1)
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
