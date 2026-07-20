use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use picoagent::{
    agent::{
        runner::{AgentRunner, AgentRunnerConfig, RunRequest, RunnerOptions},
        task::{BackgroundTaskState, TaskManager, TaskManagerConfig},
    },
    artifact::{ArtifactPolicy, ArtifactStore},
    events::{NoopEventSink, SharedEventSink},
    hooks::{CommandHook, HookEvent, HookPipeline},
    model::{
        Message, MessageContent, ModelProvider, ModelRequest, ModelResponse, ModelUsage, Role,
        ToolCall, echo::EchoProvider,
    },
    storage::{RunDirStore, RunRecord, RunState},
    tools::{ExplicitSpawn, RawToolOutput, Tool, ToolContext, ToolRegistry},
};
use serde_json::{Value, json};
use tempfile::TempDir;

fn text_response(text: impl Into<String>, usage: ModelUsage) -> ModelResponse {
    ModelResponse::new(Message::text(Role::Assistant, text), usage)
}

fn tool_response(calls: Vec<ToolCall>, usage: ModelUsage) -> ModelResponse {
    ModelResponse::new(
        Message::assistant(
            calls
                .into_iter()
                .map(|call| MessageContent::ToolCall {
                    id: call.id,
                    name: call.name,
                    arguments: call.arguments,
                })
                .collect(),
        ),
        usage,
    )
}

fn first_user_text(request: &ModelRequest) -> &str {
    request
        .messages
        .iter()
        .find(|message| message.role == Role::User)
        .and_then(|message| {
            message.content.iter().find_map(|content| match content {
                MessageContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
        })
        .unwrap_or_default()
}

struct ResumeProvider {
    calls: Arc<AtomicUsize>,
    require_interrupted_result: bool,
}

#[async_trait]
impl ModelProvider for ResumeProvider {
    fn name(&self) -> &str {
        "resume-scripted"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.require_interrupted_result
            && !request.messages.iter().any(|message| {
                message.content.iter().any(|content| match content {
                    MessageContent::ToolResult {
                        content, is_error, ..
                    } => *is_error && content.contains("side effects are unknown"),
                    _ => false,
                })
            })
        {
            bail!("resume request omitted the interrupted tool result");
        }
        Ok(text_response("resumed", ModelUsage::default()))
    }
}

struct CountingTool(Arc<AtomicUsize>);

#[async_trait]
impl Tool for CountingTool {
    fn spec(&self) -> picoagent::model::ToolSpec {
        picoagent::model::ToolSpec {
            name: "side_effect".to_owned(),
            description: "Count executions".to_owned(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn execute(&self, _context: ToolContext, _arguments: Value) -> Result<RawToolOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(RawToolOutput::text("executed"))
    }
}

fn resume_runner(
    workspace: &Path,
    store: &RunDirStore,
    provider: ResumeProvider,
    tools: ToolRegistry,
) -> Arc<AgentRunner> {
    AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(provider),
        model: "scripted".to_owned(),
        workspace: workspace.to_path_buf(),
        skill_catalog: String::new(),
        tools,
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions::default(),
    })
}

async fn create_interrupted_run(store: &RunDirStore, workspace: &Path, run_id: &str) {
    store
        .create_run(
            &RunRecord::new(
                run_id,
                "resume me",
                "resume-scripted",
                "scripted",
                workspace.to_path_buf(),
                None,
            )
            .with_provider_resume_fingerprint(
                ResumeProvider {
                    calls: Arc::new(AtomicUsize::new(0)),
                    require_interrupted_result: false,
                }
                .resume_fingerprint(),
            ),
        )
        .await
        .unwrap();
    store.update_state(run_id, RunState::Running).await.unwrap();
    store
        .append_message(run_id, &Message::text(Role::User, "resume me"))
        .await
        .unwrap();
}

#[tokio::test]
async fn resume_marks_incomplete_tool_calls_interrupted_without_reexecution() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_interrupted_run(&store, workspace.path(), "resume-tool").await;
    store
        .append_message(
            "resume-tool",
            &Message::assistant(vec![MessageContent::ToolCall {
                id: "side-effect-call".to_owned(),
                name: "side_effect".to_owned(),
                arguments: json!({}),
            }]),
        )
        .await
        .unwrap();
    let model_calls = Arc::new(AtomicUsize::new(0));
    let tool_calls = Arc::new(AtomicUsize::new(0));
    let mut tools = ToolRegistry::default();
    tools
        .register(
            Arc::new(CountingTool(tool_calls.clone())),
            ExplicitSpawn::Allowed,
        )
        .unwrap();
    let runner = resume_runner(
        workspace.path(),
        &store,
        ResumeProvider {
            calls: model_calls.clone(),
            require_interrupted_result: true,
        },
        tools,
    );

    let result = runner.resume("resume-tool").await.unwrap();
    assert_eq!(result.final_output, "resumed");
    assert_eq!(model_calls.load(Ordering::SeqCst), 1);
    assert_eq!(tool_calls.load(Ordering::SeqCst), 0);
    let messages = store.load_messages("resume-tool").await.unwrap();
    assert_eq!(messages.len(), 4);
    assert!(matches!(
        messages[2].content.as_slice(),
        [MessageContent::ToolResult {
            content,
            is_error: true,
            metadata,
            ..
        }] if metadata.preview_bytes == content.len()
    ));
}

#[tokio::test]
async fn resume_schema_mismatch_does_not_reconcile_background_tasks() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_interrupted_run(&store, workspace.path(), "resume-schema-preflight").await;
    let model_calls = Arc::new(AtomicUsize::new(0));
    let provider = ResumeProvider {
        calls: model_calls.clone(),
        require_interrupted_result: false,
    };
    let mut tools = ToolRegistry::default();
    tools
        .register(Arc::new(HangingTool), ExplicitSpawn::Allowed)
        .unwrap();
    let runner = resume_runner(workspace.path(), &store, provider, tools.clone());
    let manager = TaskManager::new(TaskManagerConfig {
        runner: runner.clone(),
        candidate_tools: tools,
        artifacts: ArtifactStore::default(),
        preview_budget: Arc::new(tokio::sync::Mutex::new(128 * 1024)),
        store: store.clone(),
        workspace: workspace.path().to_owned(),
        parent_run_id: "resume-schema-preflight".to_owned(),
        parent_depth: 0,
        child_can_delegate: false,
        events: Arc::new(NoopEventSink),
        hooks: HookPipeline::new(),
        max_parallel_tasks: 1,
        wait_timeout_seconds: 1,
    });
    let task = manager
        .spawn_tool("hanging".to_owned(), json!({}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let status = manager
                .status(std::slice::from_ref(&task.id))
                .await
                .unwrap();
            if status[0].state == BackgroundTaskState::Running {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let task_path = store
        .paths("resume-schema-preflight")
        .directory
        .join("tasks")
        .join(format!("{}.json", task.id));
    let task_before = tokio::fs::read(&task_path).await.unwrap();
    store
        .verify_tool_schema("resume-schema-preflight", "deliberate-mismatch")
        .await
        .unwrap();

    let error = runner.resume("resume-schema-preflight").await.unwrap_err();
    assert!(error.to_string().contains("tool schemas differ"));
    assert_eq!(model_calls.load(Ordering::SeqCst), 0);
    assert_eq!(tokio::fs::read(&task_path).await.unwrap(), task_before);

    manager.stop(&task.id).await.unwrap();
}

#[tokio::test]
async fn resume_finalizes_an_already_durable_final_assistant_without_model_replay() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_interrupted_run(&store, workspace.path(), "resume-final").await;
    store
        .append_message(
            "resume-final",
            &Message::text(Role::Assistant, "already finished"),
        )
        .await
        .unwrap();
    let model_calls = Arc::new(AtomicUsize::new(0));
    let runner = resume_runner(
        workspace.path(),
        &store,
        ResumeProvider {
            calls: model_calls.clone(),
            require_interrupted_result: false,
        },
        ToolRegistry::default(),
    );

    let result = runner.resume("resume-final").await.unwrap();
    assert_eq!(result.final_output, "already finished");
    assert_eq!(model_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        store.load_run("resume-final").await.unwrap().state,
        RunState::Completed
    );
    let events = tokio::fs::read_to_string(store.paths("resume-final").events)
        .await
        .unwrap();
    assert!(events.contains("\"type\":\"run_resumed\""));
}

#[tokio::test]
async fn public_resume_rejects_a_child_run_in_favor_of_parent_recovery() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    store
        .create_run(
            &RunRecord::new(
                "child",
                "child work",
                "resume-scripted",
                "scripted",
                workspace.path().to_path_buf(),
                Some("parent".to_owned()),
            )
            .with_execution_context("general_task_leaf", 1, None)
            .with_provider_resume_fingerprint(
                ResumeProvider {
                    calls: Arc::new(AtomicUsize::new(0)),
                    require_interrupted_result: false,
                }
                .resume_fingerprint(),
            ),
        )
        .await
        .unwrap();
    let runner = resume_runner(
        workspace.path(),
        &store,
        ResumeProvider {
            calls: Arc::new(AtomicUsize::new(0)),
            require_interrupted_result: false,
        },
        ToolRegistry::default(),
    );

    let error = runner.resume("child").await.unwrap_err();
    assert!(format!("{error:#}").contains("resume its parent `parent` instead"));
}

#[tokio::test]
async fn run_end_hook_failure_does_not_downgrade_a_durable_completion() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let mut hooks = HookPipeline::new();
    hooks.register(CommandHook::new(
        "failing-run-end",
        HookEvent::RunEnd,
        "sh",
        vec!["-c".into(), "exit 7".into()],
    ));
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(EchoProvider),
        model: "echo".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools: ToolRegistry::default(),
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks,
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions::default(),
    });

    let result = runner.run(RunRequest::root("finish once")).await.unwrap();
    assert_eq!(result.final_output, "received: finish once");
    assert_eq!(
        store.load_run(&result.run_id).await.unwrap().state,
        RunState::Completed
    );
    let events = tokio::fs::read_to_string(store.paths(&result.run_id).events)
        .await
        .unwrap();
    assert!(events.contains("\"type\":\"run_completed\""));
    assert!(!events.contains("\"type\":\"run_failed\""));
}

struct ResumeBudgetProvider;

#[async_trait]
impl ModelProvider for ResumeBudgetProvider {
    fn name(&self) -> &str {
        "resume-budget"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let has_small_result = request.messages.iter().any(|message| {
            message.content.iter().any(|content| {
                matches!(content, MessageContent::ToolResult { call_id, .. } if call_id == "small-call")
            })
        });
        if has_small_result {
            return Ok(text_response("resumed with budget", ModelUsage::default()));
        }
        assert!(request.messages.iter().any(|message| {
            message
                .content
                .iter()
                .any(|content| matches!(content, MessageContent::BackgroundTaskResult { .. }))
        }));
        Ok(tool_response(
            vec![ToolCall {
                id: "small-call".to_owned(),
                name: "small_output".to_owned(),
                arguments: json!({}),
            }],
            ModelUsage::default(),
        ))
    }
}

struct SmallOutputTool;

#[async_trait]
impl Tool for SmallOutputTool {
    fn spec(&self) -> picoagent::model::ToolSpec {
        picoagent::model::ToolSpec {
            name: "small_output".to_owned(),
            description: "Return fifteen bytes".to_owned(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn execute(&self, _context: ToolContext, _arguments: Value) -> Result<RawToolOutput> {
        Ok(RawToolOutput::text("s".repeat(15)))
    }
}

#[tokio::test]
async fn resume_keeps_preview_budget_reserved_for_undelivered_tasks() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let run_id = "resume-budget";
    store
        .create_run(
            &RunRecord::new(
                run_id,
                "resume with a completed task",
                ResumeBudgetProvider.name(),
                "scripted",
                workspace.path().to_path_buf(),
                None,
            )
            .with_provider_resume_fingerprint(ResumeBudgetProvider.resume_fingerprint()),
        )
        .await
        .unwrap();
    store.update_state(run_id, RunState::Running).await.unwrap();
    store
        .append_message(
            run_id,
            &Message::text(Role::User, "resume with a completed task"),
        )
        .await
        .unwrap();
    let task_directory = store.paths(run_id).directory.join("tasks");
    tokio::fs::create_dir_all(&task_directory).await.unwrap();
    tokio::fs::write(
        task_directory.join("reserved.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 4,
            "id": "reserved",
            "kind": "tool",
            "name": "earlier-tool",
            "state": "completed",
            "result": {
                "content": "u".repeat(20),
                "metadata": { "artifact": null, "preview_bytes": 20 }
            },
            "error": null,
            "child_run_id": null,
            "child_can_delegate": null,
            "prompt": null,
            "created_at": chrono::Utc::now()
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    let mut tools = ToolRegistry::default();
    tools
        .register(Arc::new(SmallOutputTool), ExplicitSpawn::Allowed)
        .unwrap();
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(ResumeBudgetProvider),
        model: "scripted".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools,
        artifacts: ArtifactStore::new(ArtifactPolicy {
            inline_limit_bytes: 100,
            max_inline_bytes_per_run: 30,
            preview_head_bytes: 8,
            preview_tail_bytes: 8,
        }),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions::default(),
    });

    let result = runner.resume(run_id).await.unwrap();
    assert_eq!(result.final_output, "resumed with budget");
    let messages = store.load_messages(run_id).await.unwrap();
    assert!(messages.iter().any(|message| {
        message.content.iter().any(|content| {
            matches!(
                content,
                MessageContent::ToolResult {
                    call_id,
                    metadata,
                    ..
                } if call_id == "small-call" && metadata.artifact.is_some()
            )
        })
    }));
}

struct FileProducingProvider;

#[async_trait]
impl ModelProvider for FileProducingProvider {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let has_result = request.messages.iter().any(|message| {
            message
                .content
                .iter()
                .any(|content| matches!(content, MessageContent::ToolResult { .. }))
        });
        if has_result {
            Ok(ModelResponse::new(
                Message::assistant(vec![
                    MessageContent::Reasoning {
                        text: "finish reasoning".to_owned(),
                    },
                    MessageContent::Text {
                        text: "finished".to_owned(),
                    },
                ]),
                ModelUsage {
                    input_tokens: Some(12),
                    output_tokens: Some(2),
                    cached_input_tokens: Some(8),
                    reasoning_tokens: Some(3),
                },
            ))
        } else {
            let call = ToolCall {
                id: "large-call".to_owned(),
                name: "large_output".to_owned(),
                arguments: json!({}),
            };
            Ok(ModelResponse::new(
                Message::assistant(vec![
                    MessageContent::Reasoning {
                        text: "tool reasoning".to_owned(),
                    },
                    MessageContent::ToolCall {
                        id: call.id,
                        name: call.name,
                        arguments: call.arguments,
                    },
                ]),
                ModelUsage {
                    input_tokens: Some(10),
                    output_tokens: Some(1),
                    cached_input_tokens: Some(6),
                    reasoning_tokens: Some(2),
                },
            ))
        }
    }
}

struct LargeOutputTool;

#[async_trait]
impl Tool for LargeOutputTool {
    fn spec(&self) -> picoagent::model::ToolSpec {
        picoagent::model::ToolSpec {
            name: "large_output".to_owned(),
            description: "Produce a large deterministic result".to_owned(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn execute(&self, _context: ToolContext, _arguments: Value) -> Result<RawToolOutput> {
        Ok(RawToolOutput::text("x".repeat(512)))
    }
}

#[tokio::test]
async fn runner_persists_complete_messages_and_spills_large_tool_output() {
    let workspace = TempDir::new().unwrap();
    let mut tools = ToolRegistry::default();
    tools
        .register(Arc::new(LargeOutputTool), ExplicitSpawn::Allowed)
        .unwrap();
    let store = RunDirStore::new(workspace.path());
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(FileProducingProvider),
        model: "scripted".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools,
        artifacts: ArtifactStore::new(ArtifactPolicy {
            inline_limit_bytes: 64,
            max_inline_bytes_per_run: 128,
            preview_head_bytes: 16,
            preview_tail_bytes: 16,
        }),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions::default(),
    });

    let result = runner
        .run(RunRequest::root("test artifacts"))
        .await
        .unwrap();
    assert_eq!(result.final_output, "finished");
    assert_eq!(
        store.load_run(&result.run_id).await.unwrap().state,
        RunState::Completed
    );

    let messages = store.load_messages(&result.run_id).await.unwrap();
    assert_eq!(messages.len(), 4);
    assert_eq!(
        messages
            .iter()
            .flat_map(|message| &message.content)
            .filter(|content| matches!(content, MessageContent::Reasoning { .. }))
            .count(),
        2
    );
    let tool_result = messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|content| match content {
            MessageContent::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .unwrap();
    assert!(tool_result.contains("[Tool output]"));
    assert!(tool_result.contains("artifact: .pico/runs/"));
    assert!(tool_result.contains("truncated: true"));
    let events = tokio::fs::read_to_string(store.paths(&result.run_id).events)
        .await
        .unwrap();
    assert!(events.contains("\"cached_input_tokens\":8"));
    assert!(events.contains("\"reasoning_tokens\":3"));
    assert!(
        std::fs::read_dir(store.paths(&result.run_id).artifacts)
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("large-call-"))
    );
}

struct DelegatingProvider;

#[async_trait]
impl ModelProvider for DelegatingProvider {
    fn name(&self) -> &str {
        "delegating"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let first_user = first_user_text(&request);
        let has_result = request
            .messages
            .iter()
            .any(|message| message.role == Role::Tool);
        if first_user == "child two" {
            bail!("scripted child failure");
        }
        if first_user == "spawn work" && !has_result {
            return Ok(tool_response(
                vec![
                    ToolCall {
                        id: "spawn-one".to_owned(),
                        name: "spawn".to_owned(),
                        arguments: json!({
                            "kind": "agent",
                            "prompt": "child one"
                        }),
                    },
                    ToolCall {
                        id: "spawn-two".to_owned(),
                        name: "spawn".to_owned(),
                        arguments: json!({
                            "kind": "agent",
                            "prompt": "child two"
                        }),
                    },
                ],
                ModelUsage::default(),
            ));
        }
        Ok(text_response(
            format!("done: {first_user}"),
            ModelUsage::default(),
        ))
    }
}

#[tokio::test]
async fn subagents_reuse_the_runner_and_report_failed_children() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(DelegatingProvider),
        model: "scripted".to_owned(),
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

    let parent = runner.run(RunRequest::root("spawn work")).await.unwrap();
    assert_eq!(parent.final_output, "done: spawn work");

    let run_root = workspace.path().join(".pico/runs");
    let mut children = Vec::new();
    for entry in std::fs::read_dir(&run_root).unwrap() {
        let id = entry.unwrap().file_name().to_string_lossy().into_owned();
        if id != parent.run_id {
            children.push(store.load_run(&id).await.unwrap());
        }
    }
    assert_eq!(children.len(), 2);
    assert!(
        children
            .iter()
            .all(|child| child.parent_run_id.as_deref() == Some(&parent.run_id))
    );
    assert_eq!(
        children
            .iter()
            .filter(|child| child.state == RunState::Completed)
            .count(),
        1
    );
    assert_eq!(
        children
            .iter()
            .filter(|child| child.state == RunState::Failed)
            .count(),
        1
    );
    let events = tokio::fs::read_to_string(store.paths(&parent.run_id).events)
        .await
        .unwrap();
    assert!(events.contains("\"type\":\"subagent_failed\""));
    assert!(events.contains("scripted child failure"));
    let messages = store.load_messages(&parent.run_id).await.unwrap();
    assert_eq!(
        messages
            .iter()
            .flat_map(|message| &message.content)
            .filter(|content| matches!(content, MessageContent::BackgroundTaskResult { .. }))
            .count(),
        2
    );
    assert!(Path::new(&store.paths(&parent.run_id).final_output).is_file());
}

struct LastStepBackgroundProvider;

#[async_trait]
impl ModelProvider for LastStepBackgroundProvider {
    fn name(&self) -> &str {
        "last-step-background"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let first_user = first_user_text(&request);
        if first_user == "slow child" {
            tokio::time::sleep(Duration::from_millis(50)).await;
            return Ok(text_response("child result", ModelUsage::default()));
        }
        let has_spawn_result = request.messages.iter().any(|message| {
            message.content.iter().any(|content| {
                matches!(content, MessageContent::ToolResult { call_id, .. } if call_id == "spawn-edge")
            })
        });
        let has_background_result = request.messages.iter().any(|message| {
            message
                .content
                .iter()
                .any(|content| matches!(content, MessageContent::BackgroundTaskResult { .. }))
        });
        if !has_spawn_result {
            return Ok(tool_response(
                vec![ToolCall {
                    id: "spawn-edge".to_owned(),
                    name: "spawn".to_owned(),
                    arguments: json!({
                        "kind": "agent",
                        "prompt": "slow child"
                    }),
                }],
                ModelUsage::default(),
            ));
        }
        Ok(text_response(
            if has_background_result {
                "parent consumed child result"
            } else {
                "premature final"
            },
            ModelUsage::default(),
        ))
    }
}

#[tokio::test]
async fn background_completion_gets_a_reconciliation_call_without_a_step_cap() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(LastStepBackgroundProvider),
        model: "scripted".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools: ToolRegistry::default(),
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions {
            max_parallel_model_calls: 2,
            ..RunnerOptions::default()
        },
    });

    let result = runner.run(RunRequest::root("edge parent")).await.unwrap();
    assert_eq!(result.final_output, "parent consumed child result");
    let messages = store.load_messages(&result.run_id).await.unwrap();
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.role == Role::Assistant)
            .count(),
        3
    );
}

struct SlowOutputTool;

#[async_trait]
impl Tool for SlowOutputTool {
    fn spec(&self) -> picoagent::model::ToolSpec {
        picoagent::model::ToolSpec {
            name: "slow_output".to_owned(),
            description: "Return a delayed test result".to_owned(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn execute(&self, _context: ToolContext, _arguments: Value) -> Result<RawToolOutput> {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(RawToolOutput::text("x".repeat(512)))
    }
}

struct WaitingProvider;

#[async_trait]
impl ModelProvider for WaitingProvider {
    fn name(&self) -> &str {
        "waiting"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let tool_results = request
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|content| match content {
                MessageContent::ToolResult {
                    call_id, content, ..
                } => Some((call_id.as_str(), content.as_str())),
                _ => None,
            })
            .collect::<Vec<_>>();
        if tool_results
            .iter()
            .any(|(call_id, _)| *call_id == "wait-call")
        {
            return Ok(text_response("joined", ModelUsage::default()));
        }
        if let Some((_, content)) = tool_results
            .iter()
            .find(|(call_id, _)| *call_id == "spawn-call")
        {
            let task_id = serde_json::from_str::<Value>(content)?["task_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            return Ok(tool_response(
                vec![ToolCall {
                    id: "wait-call".to_owned(),
                    name: "task".to_owned(),
                    arguments: json!({"action": "wait", "task_ids": [task_id]}),
                }],
                ModelUsage::default(),
            ));
        }
        Ok(tool_response(
            vec![ToolCall {
                id: "spawn-call".to_owned(),
                name: "spawn".to_owned(),
                arguments: json!({
                    "kind": "tool",
                    "tool": "slow_output",
                    "arguments": {}
                }),
            }],
            ModelUsage::default(),
        ))
    }
}

#[tokio::test]
async fn task_wait_joins_a_background_tool_without_duplicate_result_injection() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let mut tools = ToolRegistry::default();
    tools
        .register(Arc::new(SlowOutputTool), ExplicitSpawn::Allowed)
        .unwrap();
    let mut hooks = HookPipeline::new();
    hooks.register(CommandHook::new(
        "record-before",
        HookEvent::ToolBefore,
        "sh",
        vec!["-c".into(), "tee -a hooks.log".into()],
    ));
    hooks.register(CommandHook::new(
        "record-after",
        HookEvent::ToolAfter,
        "sh",
        vec!["-c".into(), "tee -a hooks.log".into()],
    ));
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(WaitingProvider),
        model: "scripted".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools,
        artifacts: ArtifactStore::new(ArtifactPolicy {
            inline_limit_bytes: 256,
            max_inline_bytes_per_run: 300,
            preview_head_bytes: 32,
            preview_tail_bytes: 32,
        }),
        store: store.clone(),
        hooks,
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions::default(),
    });

    let result = runner.run(RunRequest::root("wait for tool")).await.unwrap();
    assert_eq!(result.final_output, "joined");
    let messages = store.load_messages(&result.run_id).await.unwrap();
    assert!(
        messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|content| matches!(content, MessageContent::BackgroundTaskResult { .. }))
    );
    let task_dir = store.paths(&result.run_id).directory.join("tasks");
    let task_path = std::fs::read_dir(task_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let task: Value = serde_json::from_slice(&tokio::fs::read(task_path).await.unwrap()).unwrap();
    assert_eq!(task["state"], "completed");
    assert!(task.get("delivered").is_none());
    let events = tokio::fs::read_to_string(store.paths(&result.run_id).events)
        .await
        .unwrap();
    assert!(events.contains("\"type\":\"tool_started\""));
    assert!(events.contains("\"name\":\"slow_output\""));
    assert!(events.contains("\"type\":\"artifact_created\""));
    assert!(events.contains("background-"));
    let hooks = tokio::fs::read_to_string(workspace.path().join("hooks.log"))
        .await
        .unwrap();
    assert!(hooks.contains("\"name\":\"slow_output\""));
    assert!(hooks.contains("\"background\":true"));
}

struct LongLoopProvider(Arc<AtomicUsize>);

#[async_trait]
impl ModelProvider for LongLoopProvider {
    fn name(&self) -> &str {
        "long-loop"
    }

    async fn complete(
        &self,
        _request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let call = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        if call <= 40 {
            return Ok(tool_response(
                vec![ToolCall {
                    id: format!("long-call-{call}"),
                    name: "side_effect".to_owned(),
                    arguments: json!({}),
                }],
                ModelUsage::default(),
            ));
        }
        Ok(text_response(
            "finished after 40 tools",
            ModelUsage::default(),
        ))
    }
}

#[tokio::test]
async fn agent_loop_has_no_model_step_cap() {
    let workspace = TempDir::new().unwrap();
    let model_calls = Arc::new(AtomicUsize::new(0));
    let tool_calls = Arc::new(AtomicUsize::new(0));
    let mut tools = ToolRegistry::default();
    tools
        .register(
            Arc::new(CountingTool(tool_calls.clone())),
            ExplicitSpawn::Allowed,
        )
        .unwrap();
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(LongLoopProvider(model_calls.clone())),
        model: "scripted".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools,
        artifacts: ArtifactStore::default(),
        store: RunDirStore::new(workspace.path()),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions {
            max_subagent_depth: 0,
            ..RunnerOptions::default()
        },
    });

    let result = runner.run(RunRequest::root("keep going")).await.unwrap();
    assert_eq!(result.final_output, "finished after 40 tools");
    assert_eq!(model_calls.load(Ordering::SeqCst), 41);
    assert_eq!(tool_calls.load(Ordering::SeqCst), 40);
}

struct SteeringGateTool;

#[async_trait]
impl Tool for SteeringGateTool {
    fn spec(&self) -> picoagent::model::ToolSpec {
        picoagent::model::ToolSpec {
            name: "steering_gate".to_owned(),
            description: "Hold a child tool batch briefly".to_owned(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn execute(&self, _context: ToolContext, _arguments: Value) -> Result<RawToolOutput> {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(RawToolOutput::text("gate complete"))
    }
}

struct SteeringProvider {
    child_started: Arc<tokio::sync::Notify>,
    verified: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl ModelProvider for SteeringProvider {
    fn name(&self) -> &str {
        "steering"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        if first_user_text(&request) == "child steer target" {
            let steer_index = request.messages.iter().position(|message| {
                message.role == Role::User && message.visible_text() == "take the steered path"
            });
            if let Some(steer_index) = steer_index {
                let tool_index = request
                    .messages
                    .iter()
                    .position(|message| {
                        message.content.iter().any(|content| {
                            matches!(
                                content,
                                MessageContent::ToolResult { call_id, .. }
                                    if call_id == "child-gate-call"
                            )
                        })
                    })
                    .context("steered child request omitted the completed tool result")?;
                if steer_index <= tool_index {
                    bail!("steering was inserted before the child's tool batch completed");
                }
                self.verified
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                return Ok(text_response(
                    "steered child complete",
                    ModelUsage::default(),
                ));
            }
            self.child_started.notify_one();
            return Ok(tool_response(
                vec![ToolCall {
                    id: "child-gate-call".to_owned(),
                    name: "steering_gate".to_owned(),
                    arguments: json!({}),
                }],
                ModelUsage::default(),
            ));
        }

        let tool_results = request
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|content| match content {
                MessageContent::ToolResult {
                    call_id, content, ..
                } => Some((call_id.as_str(), content.as_str())),
                _ => None,
            })
            .collect::<Vec<_>>();
        if request.messages.iter().any(|message| {
            message
                .content
                .iter()
                .any(|content| matches!(content, MessageContent::BackgroundTaskResult { .. }))
        }) {
            return Ok(text_response(
                "parent received steered child",
                ModelUsage::default(),
            ));
        }
        if tool_results
            .iter()
            .any(|(call_id, _)| *call_id == "steer-call")
        {
            let task_id = tool_results
                .iter()
                .find(|(call_id, _)| *call_id == "spawn-steered-child")
                .and_then(|(_, content)| serde_json::from_str::<Value>(content).ok())
                .and_then(|value| value["task_id"].as_str().map(str::to_owned))
                .context("spawn result omitted task_id")?;
            return Ok(tool_response(
                vec![ToolCall {
                    id: "wait-steered-child".to_owned(),
                    name: "task".to_owned(),
                    arguments: json!({"action": "wait", "task_ids": [task_id]}),
                }],
                ModelUsage::default(),
            ));
        }
        if let Some((_, content)) = tool_results
            .iter()
            .find(|(call_id, _)| *call_id == "spawn-steered-child")
        {
            self.child_started.notified().await;
            let task_id = serde_json::from_str::<Value>(content)?["task_id"]
                .as_str()
                .context("spawn result omitted task_id")?
                .to_owned();
            return Ok(tool_response(
                vec![ToolCall {
                    id: "steer-call".to_owned(),
                    name: "task".to_owned(),
                    arguments: json!({
                        "action": "steer",
                        "task_id": task_id,
                        "message": "take the steered path"
                    }),
                }],
                ModelUsage::default(),
            ));
        }
        Ok(tool_response(
            vec![ToolCall {
                id: "spawn-steered-child".to_owned(),
                name: "spawn".to_owned(),
                arguments: json!({
                    "kind": "agent",
                    "prompt": "child steer target"
                }),
            }],
            ModelUsage::default(),
        ))
    }
}

#[tokio::test]
async fn steer_is_delivered_after_the_childs_current_tool_batch_before_its_next_model_call() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let child_started = Arc::new(tokio::sync::Notify::new());
    let verified = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut tools = ToolRegistry::default();
    tools
        .register(Arc::new(SteeringGateTool), ExplicitSpawn::Allowed)
        .unwrap();
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(SteeringProvider {
            child_started,
            verified: verified.clone(),
        }),
        model: "scripted".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools,
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions {
            max_parallel_model_calls: 2,
            ..RunnerOptions::default()
        },
    });

    let result = runner.run(RunRequest::root("steer child")).await.unwrap();
    assert_eq!(result.final_output, "parent received steered child");
    assert!(verified.load(std::sync::atomic::Ordering::SeqCst));
    let events = tokio::fs::read_to_string(store.paths(&result.run_id).events)
        .await
        .unwrap();
    assert!(events.contains("\"type\":\"subagent_steered\""));
}

struct HangingTool;

#[async_trait]
impl Tool for HangingTool {
    fn spec(&self) -> picoagent::model::ToolSpec {
        picoagent::model::ToolSpec {
            name: "hanging".to_owned(),
            description: "Wait until cancelled".to_owned(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn execute(&self, _context: ToolContext, _arguments: Value) -> Result<RawToolOutput> {
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok(RawToolOutput::text("too late"))
    }
}

struct FailingParentProvider;

#[async_trait]
impl ModelProvider for FailingParentProvider {
    fn name(&self) -> &str {
        "failing-parent"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        if request
            .messages
            .iter()
            .any(|message| message.role == Role::Tool)
        {
            bail!("scripted parent failure");
        }
        Ok(tool_response(
            vec![ToolCall {
                id: "spawn-hanging".to_owned(),
                name: "spawn".to_owned(),
                arguments: json!({"kind": "tool", "tool": "hanging", "arguments": {}}),
            }],
            ModelUsage::default(),
        ))
    }
}

#[tokio::test]
async fn parent_failure_aborts_and_settles_background_tasks() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let mut tools = ToolRegistry::default();
    tools
        .register(Arc::new(HangingTool), ExplicitSpawn::Allowed)
        .unwrap();
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(FailingParentProvider),
        model: "scripted".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools,
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions::default(),
    });

    let error = runner
        .run(RunRequest::root("fail after spawn"))
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("scripted parent failure"));
    let run_root = workspace.path().join(".pico/runs");
    let parent_dir = std::fs::read_dir(&run_root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let task_path = std::fs::read_dir(parent_dir.join("tasks"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let task: Value = serde_json::from_slice(&tokio::fs::read(task_path).await.unwrap()).unwrap();
    assert_eq!(task["state"], "cancelled");
    assert!(task["error"].as_str().unwrap().contains("parent run ended"));
}

struct SlowModelProvider;

#[async_trait]
impl ModelProvider for SlowModelProvider {
    fn name(&self) -> &str {
        "slow-model"
    }

    async fn complete(
        &self,
        _request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok(text_response("too late", ModelUsage::default()))
    }
}

#[tokio::test]
async fn model_requests_have_a_runtime_deadline() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(SlowModelProvider),
        model: "scripted".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools: ToolRegistry::default(),
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions {
            model_request_deadline_seconds: 1,
            ..RunnerOptions::default()
        },
    });

    let error = runner
        .run(RunRequest::root("wait forever"))
        .await
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("model request deadline exceeded 1 seconds"),
        "{error:#}"
    );
    let run_id = std::fs::read_dir(workspace.path().join(".pico/runs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        store.load_run(&run_id).await.unwrap().state,
        RunState::Failed
    );
}
