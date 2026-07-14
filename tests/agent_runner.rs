use std::{path::Path, sync::Arc, time::Duration};

use anyhow::{Result, bail};
use async_trait::async_trait;
use picoagent::{
    agent::runner::{AgentRunner, AgentRunnerConfig, RunRequest, RunnerOptions},
    artifact::{ArtifactPolicy, ArtifactStore},
    events::{NoopEventSink, SharedEventSink},
    hooks::{CommandHook, HookEvent, HookPipeline},
    memory::MemoryPaths,
    model::{
        MessageContent, ModelProvider, ModelRequest, ModelResponse, ModelUsage, Role, ToolCall,
    },
    storage::{RunDirStore, RunState},
    tools::{RawToolOutput, Tool, ToolContext, ToolRegistry, builtin::WriteTool},
};
use serde_json::{Value, json};
use tempfile::TempDir;

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
            Ok(ModelResponse {
                text: "finished".to_owned(),
                tool_calls: Vec::new(),
                assistant_content: vec![
                    MessageContent::Reasoning {
                        text: "finish reasoning".to_owned(),
                    },
                    MessageContent::Text {
                        text: "finished".to_owned(),
                    },
                ],
                usage: ModelUsage {
                    input_tokens: Some(12),
                    output_tokens: Some(2),
                    cached_input_tokens: Some(8),
                    reasoning_tokens: Some(3),
                },
            })
        } else {
            let call = ToolCall {
                id: "large-call".to_owned(),
                name: "large_output".to_owned(),
                arguments: json!({}),
            };
            Ok(ModelResponse {
                text: String::new(),
                tool_calls: vec![call.clone()],
                assistant_content: vec![
                    MessageContent::Reasoning {
                        text: "tool reasoning".to_owned(),
                    },
                    MessageContent::ToolCall {
                        id: call.id,
                        name: call.name,
                        arguments: call.arguments,
                    },
                ],
                usage: ModelUsage {
                    input_tokens: Some(10),
                    output_tokens: Some(1),
                    cached_input_tokens: Some(6),
                    reasoning_tokens: Some(2),
                },
            })
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
    tools.register(Arc::new(LargeOutputTool)).unwrap();
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
    assert!(tool_result.contains("[Full output artifact]"));
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
            return Ok(ModelResponse {
                text: String::new(),
                tool_calls: vec![
                    ToolCall {
                        id: "spawn-one".to_owned(),
                        name: "spawn".to_owned(),
                        arguments: json!({
                            "kind": "agent",
                            "profile": "general-task",
                            "prompt": "child one"
                        }),
                    },
                    ToolCall {
                        id: "spawn-two".to_owned(),
                        name: "spawn".to_owned(),
                        arguments: json!({
                            "kind": "agent",
                            "profile": "general-task",
                            "prompt": "child two"
                        }),
                    },
                ],
                assistant_content: Vec::new(),
                usage: ModelUsage::default(),
            });
        }
        Ok(ModelResponse {
            text: format!("done: {first_user}"),
            tool_calls: Vec::new(),
            assistant_content: Vec::new(),
            usage: ModelUsage::default(),
        })
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
            return Ok(ModelResponse {
                text: "joined".to_owned(),
                tool_calls: Vec::new(),
                assistant_content: Vec::new(),
                usage: ModelUsage::default(),
            });
        }
        if let Some((_, content)) = tool_results
            .iter()
            .find(|(call_id, _)| *call_id == "spawn-call")
        {
            let task_id = serde_json::from_str::<Value>(content)?["task_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            return Ok(ModelResponse {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "wait-call".to_owned(),
                    name: "wait".to_owned(),
                    arguments: json!({"task_ids": [task_id], "timeout_seconds": 1}),
                }],
                assistant_content: Vec::new(),
                usage: ModelUsage::default(),
            });
        }
        Ok(ModelResponse {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "spawn-call".to_owned(),
                name: "spawn".to_owned(),
                arguments: json!({
                    "kind": "tool",
                    "tool": "slow_output",
                    "arguments": {}
                }),
            }],
            assistant_content: Vec::new(),
            usage: ModelUsage::default(),
        })
    }
}

#[tokio::test]
async fn wait_joins_a_background_tool_without_duplicate_result_injection() {
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
        !messages
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
    assert_eq!(task["delivered"], true);
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
        Ok(ModelResponse {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "spawn-hanging".to_owned(),
                name: "spawn".to_owned(),
                arguments: json!({"kind": "tool", "tool": "hanging", "arguments": {}}),
            }],
            assistant_content: Vec::new(),
            usage: ModelUsage::default(),
        })
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
    assert_eq!(task["state"], "failed");
    assert!(task["error"].as_str().unwrap().contains("parent run ended"));
}

struct MemoryTimeoutProvider;

#[async_trait]
impl ModelProvider for MemoryTimeoutProvider {
    fn name(&self) -> &str {
        "memory-timeout"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let first_user = first_user_text(&request);
        if first_user.starts_with("Update durable") {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        if request
            .messages
            .iter()
            .any(|message| message.role == Role::Tool)
        {
            return Ok(ModelResponse {
                text: "continued after timeout".to_owned(),
                tool_calls: Vec::new(),
                assistant_content: Vec::new(),
                usage: ModelUsage::default(),
            });
        }
        Ok(ModelResponse {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "memory-call".to_owned(),
                name: "memory_update".to_owned(),
                arguments: json!({"scope": "project", "instruction": "remember this"}),
            }],
            assistant_content: Vec::new(),
            usage: ModelUsage::default(),
        })
    }
}

#[tokio::test]
async fn timed_out_memory_update_marks_its_child_run_failed() {
    let workspace = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(MemoryTimeoutProvider),
        model: "scripted".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools: ToolRegistry::default(),
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: Some(MemoryPaths::new(home.path(), workspace.path())),
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions {
            direct_tool_timeout_seconds: 1,
            ..RunnerOptions::default()
        },
    });

    let parent = runner
        .run(RunRequest::root("memory timeout"))
        .await
        .unwrap();
    assert_eq!(parent.final_output, "continued after timeout");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut children = Vec::new();
    for entry in std::fs::read_dir(workspace.path().join(".pico/runs")).unwrap() {
        let id = entry.unwrap().file_name().to_string_lossy().into_owned();
        if id != parent.run_id {
            children.push(store.load_run(&id).await.unwrap());
        }
    }
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].state, RunState::Failed);
}

struct MemorySuccessProvider;

#[async_trait]
impl ModelProvider for MemorySuccessProvider {
    fn name(&self) -> &str {
        "memory-success"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let first_user = first_user_text(&request);
        let has_tool_result = request
            .messages
            .iter()
            .any(|message| message.role == Role::Tool);
        if first_user.starts_with("Update durable") {
            if has_tool_result {
                return Ok(ModelResponse {
                    text: "updated profile.md".to_owned(),
                    tool_calls: Vec::new(),
                    assistant_content: Vec::new(),
                    usage: ModelUsage::default(),
                });
            }
            let root = first_user
                .split_once("stored at ")
                .and_then(|(_, rest)| rest.split_once(".\n\nInstruction"))
                .map(|(path, _)| path)
                .unwrap_or_default();
            return Ok(ModelResponse {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "write-memory".to_owned(),
                    name: "write".to_owned(),
                    arguments: json!({
                        "path": Path::new(root).join("profile.md"),
                        "content": "# Preferences\n\n- Prefers concise output.\n"
                    }),
                }],
                assistant_content: Vec::new(),
                usage: ModelUsage::default(),
            });
        }
        if has_tool_result {
            return Ok(ModelResponse {
                text: "remembered".to_owned(),
                tool_calls: Vec::new(),
                assistant_content: Vec::new(),
                usage: ModelUsage::default(),
            });
        }
        Ok(ModelResponse {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "memory-call".to_owned(),
                name: "memory_update".to_owned(),
                arguments: json!({
                    "scope": "user",
                    "instruction": "Record that the user prefers concise output"
                }),
            }],
            assistant_content: Vec::new(),
            usage: ModelUsage::default(),
        })
    }
}

#[tokio::test]
async fn memory_update_uses_a_restricted_child_run_to_write_markdown() {
    let workspace = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let mut tools = ToolRegistry::default();
    tools.register(Arc::new(WriteTool::default())).unwrap();
    let memory = MemoryPaths::new(home.path(), workspace.path());
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(MemorySuccessProvider),
        model: "scripted".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools,
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: Some(memory.clone()),
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions::default(),
    });

    let parent = runner
        .run(RunRequest::root("remember preference"))
        .await
        .unwrap();
    assert_eq!(parent.final_output, "remembered");
    assert_eq!(
        tokio::fs::read_to_string(memory.user.join("profile.md"))
            .await
            .unwrap(),
        "# Preferences\n\n- Prefers concise output.\n"
    );
    let children = std::fs::read_dir(workspace.path().join(".pico/runs"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|id| id != &parent.run_id)
        .collect::<Vec<_>>();
    assert_eq!(children.len(), 1);
    assert_eq!(
        store.load_run(&children[0]).await.unwrap().state,
        RunState::Completed
    );
}
