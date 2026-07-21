use std::{
    collections::BTreeSet,
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
        Message, MessageContent, ModelModality, ModelProvider, ModelRequest, ModelResponse,
        ModelUsage, Role, ToolCall, echo::EchoProvider,
    },
    storage::{DelegateContext, RunDirStore, RunRecord, RunState},
    tools::{RawToolOutput, Tool, ToolContext, ToolRegistry},
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

fn background_task_id(content: &str) -> Option<String> {
    content
        .split_once("task_id=\"")?
        .1
        .split_once('"')
        .map(|(task_id, _)| task_id.to_owned())
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
        .register(Arc::new(CountingTool(tool_calls.clone())))
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
        }] if metadata.artifact.is_none() && !content.is_empty()
    ));
}

struct CompletedPromotionRecoveryProvider;

#[async_trait]
impl ModelProvider for CompletedPromotionRecoveryProvider {
    fn name(&self) -> &str {
        "completed-promotion-recovery"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let acknowledgement = request
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .find_map(|content| match content {
                MessageContent::ToolResult {
                    call_id,
                    content,
                    is_error,
                    ..
                } if call_id == "promoted-call" => Some((content, is_error)),
                _ => None,
            })
            .context("resume omitted the promoted tool acknowledgement")?;
        if *acknowledgement.1
            || !acknowledgement
                .0
                .contains("<background_task task_id=\"t1\"")
            || !acknowledgement.0.contains("name=\"side_effect\"")
            || acknowledgement.0.contains("status=")
        {
            bail!("resume synthesized the wrong acknowledgement: {acknowledgement:?}");
        }
        let delivered = request
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|content| {
                matches!(
                    content,
                    MessageContent::BackgroundTask {
                        task_id,
                        status,
                        content,
                        ..
                    } if task_id == "t1"
                        && status.as_deref() == Some("completed")
                        && content.starts_with(".pico/runs/")
                        && content.contains("/artifacts/background-t1-")
                )
            });
        if !delivered {
            bail!("resume omitted the durable completed background result");
        }
        Ok(text_response(
            "recovered completed promotion",
            ModelUsage::default(),
        ))
    }
}

#[tokio::test]
async fn resume_reconstructs_a_missing_promotion_ack_from_its_terminal_task() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let provider = Arc::new(CompletedPromotionRecoveryProvider);
    store
        .create_run(
            &RunRecord::new(
                "resume-completed-promotion",
                "resume a promoted tool",
                provider.name(),
                "scripted",
                workspace.path().to_path_buf(),
                None,
            )
            .with_provider_resume_fingerprint(provider.resume_fingerprint()),
        )
        .await
        .unwrap();
    store
        .update_state("resume-completed-promotion", RunState::Running)
        .await
        .unwrap();
    store
        .append_message(
            "resume-completed-promotion",
            &Message::text(Role::User, "resume a promoted tool"),
        )
        .await
        .unwrap();
    store
        .append_message(
            "resume-completed-promotion",
            &Message::assistant(vec![MessageContent::ToolCall {
                id: "promoted-call".to_owned(),
                name: "side_effect".to_owned(),
                arguments: json!({}),
            }]),
        )
        .await
        .unwrap();
    let tasks = store
        .paths("resume-completed-promotion")
        .directory
        .join("tasks");
    tokio::fs::create_dir_all(&tasks).await.unwrap();
    tokio::fs::write(
        tasks.join("t1.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 9,
            "id": "t1",
            "kind": "tool",
            "name": "side_effect",
            "origin_call_id": "promoted-call",
            "state": "completed",
            "result": {
                "content": "completed output",
                "metadata": {"artifact": null}
            },
            "error": null,
            "child_run_id": null,
            "child_remaining_delegation_depth": null,
            "delegate_context": null,
            "fork_parent_message_seq": null,
            "prompt": null,
            "created_at": chrono::Utc::now()
        }))
        .unwrap(),
    )
    .await
    .unwrap();

    let mut tools = ToolRegistry::default();
    tools
        .register(Arc::new(CountingTool(Arc::new(AtomicUsize::new(0)))))
        .unwrap();
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider,
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

    let result = runner.resume("resume-completed-promotion").await.unwrap();
    assert_eq!(result.final_output, "recovered completed promotion");
    let messages = store
        .load_messages("resume-completed-promotion")
        .await
        .unwrap();
    assert!(
        !messages
            .iter()
            .any(|message| { message.visible_text().contains("side effects are unknown") })
    );
}

#[tokio::test]
async fn resume_rejects_changed_model_modalities_before_calling_the_provider() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = ResumeProvider {
        calls: calls.clone(),
        require_interrupted_result: false,
    };
    store
        .create_run(
            &RunRecord::new(
                "resume-modalities",
                "resume me",
                provider.name(),
                "scripted",
                workspace.path().to_path_buf(),
                None,
            )
            .with_model_modalities(BTreeSet::from([ModelModality::Text, ModelModality::Image]))
            .with_provider_resume_fingerprint(provider.resume_fingerprint()),
        )
        .await
        .unwrap();
    store
        .update_state("resume-modalities", RunState::Running)
        .await
        .unwrap();

    let runner = resume_runner(
        workspace.path(),
        &store,
        ResumeProvider {
            calls: calls.clone(),
            require_interrupted_result: false,
        },
        ToolRegistry::default(),
    );
    let error = runner.resume("resume-modalities").await.unwrap_err();

    assert!(error.to_string().contains("model modalities"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
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
    tools.register(Arc::new(HangingTool)).unwrap();
    let runner = resume_runner(workspace.path(), &store, provider, tools.clone());
    let background_runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(SlowModelProvider),
        model: "scripted".to_owned(),
        workspace: workspace.path().to_owned(),
        skill_catalog: String::new(),
        tools: ToolRegistry::default(),
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions::default(),
    });
    let manager = TaskManager::new(TaskManagerConfig {
        runner: background_runner,
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        workspace: workspace.path().to_owned(),
        parent_run_id: "resume-schema-preflight".to_owned(),
        parent_depth: 0,
        remaining_delegation_depth: 1,
        events: Arc::new(NoopEventSink),
        max_parallel_subagents: 1,
        wait_timeout_seconds: 1,
    });
    let task = manager
        .delegate(
            "hang_child".to_owned(),
            "hang child".to_owned(),
            DelegateContext::Fresh,
            "delegate-schema-call",
        )
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
            .with_execution_context("general_task_leaf", 1, None, 0)
            .with_delegate_context(DelegateContext::Fresh, None)
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
        let completed = request
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|content| match content {
                MessageContent::ToolResult { call_id, .. } => Some(call_id.as_str()),
                _ => None,
            })
            .collect::<std::collections::HashSet<_>>();
        if completed.contains("small-call") {
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
        } else if completed.contains("large-call") {
            Ok(tool_response(
                vec![ToolCall {
                    id: "small-call".to_owned(),
                    name: "small_output".to_owned(),
                    arguments: json!({}),
                }],
                ModelUsage::default(),
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

struct SmallOutputTool;

#[async_trait]
impl Tool for SmallOutputTool {
    fn spec(&self) -> picoagent::model::ToolSpec {
        picoagent::model::ToolSpec {
            name: "small_output".to_owned(),
            description: "Produce a small deterministic result".to_owned(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn execute(&self, _context: ToolContext, _arguments: Value) -> Result<RawToolOutput> {
        Ok(RawToolOutput::text("small"))
    }
}

#[tokio::test]
async fn runner_spills_a_large_result_without_affecting_the_next_small_result() {
    let workspace = TempDir::new().unwrap();
    let mut tools = ToolRegistry::default();
    tools.register(Arc::new(LargeOutputTool)).unwrap();
    tools.register(Arc::new(SmallOutputTool)).unwrap();
    let store = RunDirStore::new(workspace.path());
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(FileProducingProvider),
        model: "scripted".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools,
        artifacts: ArtifactStore::new(ArtifactPolicy {
            inline_limit_bytes: 64,
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
    assert_eq!(messages.len(), 6);
    assert_eq!(
        messages
            .iter()
            .flat_map(|message| &message.content)
            .filter(|content| matches!(content, MessageContent::Reasoning { .. }))
            .count(),
        2
    );
    let tool_results = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|content| match content {
            MessageContent::ToolResult {
                call_id,
                content,
                metadata,
                ..
            } => Some((call_id.as_str(), content.as_str(), metadata)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_results.len(), 2);
    assert_eq!(tool_results[0].0, "large-call");
    assert!(tool_results[0].1.contains("[Tool output]"));
    assert!(tool_results[0].1.contains("artifact: .pico/runs/"));
    assert!(tool_results[0].1.contains("truncated: true"));
    assert!(tool_results[0].2.artifact.is_some());
    assert_eq!(tool_results[1].0, "small-call");
    assert_eq!(tool_results[1].1, "small");
    assert!(tool_results[1].2.artifact.is_none());
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
        if first_user == "delegate work" && !has_result {
            return Ok(tool_response(
                vec![
                    ToolCall {
                        id: "delegate-one".to_owned(),
                        name: "delegate".to_owned(),
                        arguments: json!({"name": "child_one", "prompt": "child one", "context": "fresh"}),
                    },
                    ToolCall {
                        id: "delegate-two".to_owned(),
                        name: "delegate".to_owned(),
                        arguments: json!({"name": "child_two", "prompt": "child two", "context": "fresh"}),
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

    let parent = runner.run(RunRequest::root("delegate work")).await.unwrap();
    assert_eq!(parent.final_output, "done: delegate work");

    let run_root = workspace.path().join(".pico/runs");
    let mut children = Vec::new();
    for entry in std::fs::read_dir(&run_root).unwrap() {
        let id = entry.unwrap().file_name().to_string_lossy().into_owned();
        if id != parent.run_id {
            children.push(store.load_run(&id).await.unwrap());
        }
    }
    assert_eq!(children.len(), 2);
    let task_call_ids = std::fs::read_dir(store.paths(&parent.run_id).directory.join("tasks"))
        .unwrap()
        .map(|entry| {
            let task: Value =
                serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap();
            task["origin_call_id"].as_str().unwrap().to_owned()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        task_call_ids,
        BTreeSet::from(["delegate-one".to_owned(), "delegate-two".to_owned()])
    );
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
            .filter(|content| matches!(content, MessageContent::BackgroundTask { .. }))
            .count(),
        2
    );
    let terminal_messages = messages
        .iter()
        .filter(|message| {
            message.role == Role::User
                && message
                    .content
                    .iter()
                    .any(|content| matches!(content, MessageContent::BackgroundTask { .. }))
        })
        .collect::<Vec<_>>();
    assert!(!terminal_messages.is_empty());
    assert!(terminal_messages.len() <= 2);
    assert!(terminal_messages.iter().all(|message| {
        message.content.iter().all(|content| {
            matches!(
                content,
                MessageContent::BackgroundTask {
                    status: Some(_),
                    ..
                }
            )
        })
    }));
    for (path, artifact) in messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|content| match content {
            MessageContent::BackgroundTask {
                status: Some(_),
                content,
                metadata,
                ..
            } => Some((content, metadata.artifact.as_ref())),
            _ => None,
        })
    {
        let artifact = artifact.expect("terminal task notice omitted artifact metadata");
        assert_eq!(path, &artifact.path);
        assert!(path.contains("/artifacts/background-t"));
        assert!(!path.contains("delegate-one") && !path.contains("delegate-two"));
        let body = tokio::fs::read_to_string(workspace.path().join(path))
            .await
            .unwrap();
        assert!(!body.is_empty());
    }
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
        let has_delegate_result = request.messages.iter().any(|message| {
            message.content.iter().any(|content| {
                matches!(content, MessageContent::ToolResult { call_id, .. } if call_id == "delegate-edge")
            })
        });
        let has_background_result = request.messages.iter().any(|message| {
            message
                .content
                .iter()
                .any(|content| matches!(content, MessageContent::BackgroundTask { .. }))
        });
        if !has_delegate_result {
            return Ok(tool_response(
                vec![ToolCall {
                    id: "delegate-edge".to_owned(),
                    name: "delegate".to_owned(),
                    arguments: json!({"name": "slow_child", "prompt": "slow child", "context": "fresh"}),
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
        tokio::time::sleep(Duration::from_millis(1_100)).await;
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
            .find(|(call_id, _)| *call_id == "slow-call")
        {
            let task_id = background_task_id(content).context("task notice omitted task_id")?;
            return Ok(tool_response(
                vec![ToolCall {
                    id: "wait-call".to_owned(),
                    name: "task_wait".to_owned(),
                    arguments: json!({"task_ids": [task_id]}),
                }],
                ModelUsage::default(),
            ));
        }
        Ok(tool_response(
            vec![ToolCall {
                id: "slow-call".to_owned(),
                name: "slow_output".to_owned(),
                arguments: json!({}),
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
    tools.register(Arc::new(SlowOutputTool)).unwrap();
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
            preview_head_bytes: 32,
            preview_tail_bytes: 32,
        }),
        store: store.clone(),
        hooks,
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions {
            foreground_tool_timeout_seconds: 1,
            ..RunnerOptions::default()
        },
    });

    let result = runner.run(RunRequest::root("wait for tool")).await.unwrap();
    assert_eq!(result.final_output, "joined");
    let messages = store.load_messages(&result.run_id).await.unwrap();
    let acknowledgement_task_id = messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|content| match content {
            MessageContent::ToolResult {
                call_id, content, ..
            } if call_id == "slow-call" => background_task_id(content),
            _ => None,
        })
        .unwrap();
    let (terminal_task_id, artifact_call_id) = messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|content| match content {
            MessageContent::BackgroundTask {
                task_id, metadata, ..
            } => Some((task_id.clone(), metadata.artifact.as_ref()?.call_id.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(terminal_task_id, acknowledgement_task_id);
    assert_eq!(artifact_call_id, "slow-call");
    let reloaded = RunDirStore::new(workspace.path())
        .load_messages(&result.run_id)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&reloaded).unwrap(),
        serde_json::to_vec(&messages).unwrap()
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
    let hooks = tokio::fs::read_to_string(workspace.path().join("hooks.log"))
        .await
        .unwrap();
    assert!(hooks.contains("\"name\":\"slow_output\""));
    assert!(hooks.contains("\"call_id\":\"slow-call\""));
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
        .register(Arc::new(CountingTool(tool_calls.clone())))
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
                .any(|content| matches!(content, MessageContent::BackgroundTask { .. }))
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
                .find(|(call_id, _)| *call_id == "delegate-steered-child")
                .and_then(|(_, content)| background_task_id(content))
                .context("delegate result omitted task_id")?;
            return Ok(tool_response(
                vec![ToolCall {
                    id: "wait-steered-child".to_owned(),
                    name: "task_wait".to_owned(),
                    arguments: json!({"task_ids": [task_id]}),
                }],
                ModelUsage::default(),
            ));
        }
        if let Some((_, content)) = tool_results
            .iter()
            .find(|(call_id, _)| *call_id == "delegate-steered-child")
        {
            self.child_started.notified().await;
            let task_id = background_task_id(content).context("delegate result omitted task_id")?;
            return Ok(tool_response(
                vec![ToolCall {
                    id: "steer-call".to_owned(),
                    name: "task_steer".to_owned(),
                    arguments: json!({
                        "task_id": task_id,
                        "message": "take the steered path"
                    }),
                }],
                ModelUsage::default(),
            ));
        }
        Ok(tool_response(
            vec![ToolCall {
                id: "delegate-steered-child".to_owned(),
                name: "delegate".to_owned(),
                arguments: json!({"name": "steer_target", "prompt": "child steer target", "context": "fresh"}),
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
    tools.register(Arc::new(SteeringGateTool)).unwrap();
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

#[derive(Default)]
struct YieldingSerializedEventSink {
    lock: tokio::sync::Mutex<()>,
    tool_starts: AtomicUsize,
}

#[async_trait]
impl picoagent::events::EventSink for YieldingSerializedEventSink {
    async fn emit(&self, event: &picoagent::events::RuntimeEvent) -> Result<()> {
        let _guard = self.lock.lock().await;
        if matches!(
            event.kind,
            picoagent::events::RuntimeEventKind::ToolStarted { .. }
        ) && self.tool_starts.fetch_add(1, Ordering::SeqCst) == 1
        {
            tokio::task::yield_now().await;
        }
        Ok(())
    }
}

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
            vec![
                ToolCall {
                    id: "hanging-call-1".to_owned(),
                    name: "hanging".to_owned(),
                    arguments: json!({}),
                },
                ToolCall {
                    id: "hanging-call-2".to_owned(),
                    name: "hanging".to_owned(),
                    arguments: json!({}),
                },
            ],
            ModelUsage::default(),
        ))
    }
}

#[tokio::test]
async fn parent_failure_aborts_and_settles_background_tasks() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let mut tools = ToolRegistry::default();
    tools.register(Arc::new(HangingTool)).unwrap();
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
        // The second direct future yields while holding an event-sink lock.
        // Promotion must resume every pending future before any promotion
        // awaits task events from the same sink.
        extra_events: Arc::new(YieldingSerializedEventSink::default()),
        options: RunnerOptions {
            foreground_tool_timeout_seconds: 0,
            ..RunnerOptions::default()
        },
    });

    let error = tokio::time::timeout(
        Duration::from_secs(2),
        runner.run(RunRequest::root("fail after promotion")),
    )
    .await
    .expect("multi-call promotion deadlocked on an in-flight tool event")
    .unwrap_err();
    assert!(format!("{error:#}").contains("scripted parent failure"));
    let run_root = workspace.path().join(".pico/runs");
    let parent_dir = std::fs::read_dir(&run_root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut tasks = Vec::new();
    for entry in std::fs::read_dir(parent_dir.join("tasks")).unwrap() {
        let bytes = tokio::fs::read(entry.unwrap().path()).await.unwrap();
        tasks.push(serde_json::from_slice::<Value>(&bytes).unwrap());
    }
    assert_eq!(tasks.len(), 2);
    for task in tasks {
        assert_eq!(task["state"], "cancelled");
        assert!(task["error"].as_str().unwrap().contains("parent run ended"));
    }
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
