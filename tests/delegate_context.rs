use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::{Result, bail};
use async_trait::async_trait;
use picoagent::{
    agent::{
        CompactionOptions,
        runner::{AgentRunner, AgentRunnerConfig, RunRequest, RunnerOptions},
    },
    artifact::ArtifactStore,
    events::{NoopEventSink, SharedEventSink},
    hooks::HookPipeline,
    model::{
        Message, MessageContent, ModelProvider, ModelRequest, ModelResponse, ModelUsage, Role,
        ToolCall, ToolSpec,
    },
    storage::{DelegateContext, RunDirStore},
    tools::{RawToolOutput, ReadTool, Tool, ToolContext, ToolRegistry},
    trajectory::CompactionMessage,
};
use serde::Serialize;
use serde_json::{Value, json};
use tempfile::TempDir;

#[derive(Default)]
struct ForkCaptureProvider {
    root_run_id: Mutex<Option<String>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl ForkCaptureProvider {
    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ModelProvider for ForkCaptureProvider {
    fn name(&self) -> &str {
        "fork-capture"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let root_run_id = self
            .root_run_id
            .lock()
            .unwrap()
            .get_or_insert_with(|| request.run_id.clone())
            .clone();
        self.requests.lock().unwrap().push(request.clone());
        if request.run_id != root_run_id {
            return Ok(text_response(
                format!("completed {}", last_user_text(&request)),
                ModelUsage {
                    input_tokens: Some(101),
                    output_tokens: Some(7),
                    cached_input_tokens: Some(73),
                    reasoning_tokens: None,
                },
            ));
        }

        if !has_tool_result(&request, "fork-a") {
            return Ok(tool_response(vec![
                delegate_call("fork-a", "fork_a", "fork task a", "fork"),
                delegate_call("fork-b", "fork_b", "fork task b", "fork"),
                delegate_call("fresh-c", "fresh_c", "fresh task c", "fresh"),
            ]));
        }
        Ok(text_response("parent complete", ModelUsage::default()))
    }
}

#[tokio::test]
async fn fork_siblings_share_the_exact_pre_assistant_request_and_fresh_is_isolated() {
    let workspace = TempDir::new().unwrap();
    let provider = Arc::new(ForkCaptureProvider::default());
    let store = RunDirStore::new(workspace.path());
    let runner = runner(
        workspace.path(),
        provider.clone(),
        store.clone(),
        RunnerOptions {
            max_subagent_depth: 1,
            max_parallel_subagents: 3,
            max_parallel_model_calls: 4,
            general_task: picoagent::agent::GeneralTaskProfile {
                model: Some("different-general-model".to_owned()),
                max_output_tokens: Some(4_096),
            },
            ..RunnerOptions::default()
        },
        ToolRegistry::default(),
    );

    let parent = runner
        .run(RunRequest::root("parent inherited context"))
        .await
        .unwrap();
    assert_eq!(parent.final_output, "parent complete");

    let requests = provider.requests();
    let parent_request = requests
        .iter()
        .find(|request| request.run_id == parent.run_id && !has_tool_result(request, "fork-a"))
        .unwrap();
    let fork_a = child_request(&requests, "fork task a");
    let fork_b = child_request(&requests, "fork task b");
    let fresh = child_request(&requests, "fresh task c");

    assert_eq!(fork_a.model, parent_request.model);
    assert_eq!(
        serialized(&fork_a.system),
        serialized(&parent_request.system)
    );
    assert_eq!(serialized(&fork_a.tools), serialized(&parent_request.tools));
    assert_eq!(serialized(&fork_b.tools), serialized(&parent_request.tools));
    assert_eq!(fork_a.messages.len(), parent_request.messages.len() + 1);
    assert_eq!(fork_b.messages.len(), parent_request.messages.len() + 1);
    assert_eq!(
        serialized(&fork_a.messages[..parent_request.messages.len()]),
        serialized(&parent_request.messages)
    );
    assert_eq!(
        serialized(&fork_b.messages[..parent_request.messages.len()]),
        serialized(&parent_request.messages)
    );
    assert_eq!(
        serialized(&fork_a.messages[..parent_request.messages.len()]),
        serialized(&fork_b.messages[..parent_request.messages.len()])
    );
    for child in [fork_a, fork_b] {
        assert!(!child.messages.iter().any(|message| {
            message.content.iter().any(|content| {
                matches!(content, MessageContent::ToolCall { id, .. } if id == "fork-a" || id == "fork-b")
            })
        }));
    }
    assert_eq!(fresh.messages.len(), 1);
    assert_eq!(fresh.model, "different-general-model");
    assert_eq!(last_user_text(fresh), "fresh task c");
    assert!(!serialized(&fresh.messages).contains("parent inherited context"));

    let children = child_runs(&store, &parent.run_id).await;
    assert_eq!(children.len(), 3);
    let fork_runs = children
        .iter()
        .filter(|run| run.delegate_context == Some(DelegateContext::Fork))
        .collect::<Vec<_>>();
    assert_eq!(fork_runs.len(), 2);
    assert!(
        fork_runs
            .iter()
            .all(|run| run.fork_parent_message_seq == Some(1))
    );
    let first_messages = tokio::fs::read_to_string(&store.paths(&fork_runs[0].id).messages)
        .await
        .unwrap();
    let second_messages = tokio::fs::read_to_string(&store.paths(&fork_runs[1].id).messages)
        .await
        .unwrap();
    assert_eq!(
        first_messages.lines().next().unwrap(),
        second_messages.lines().next().unwrap()
    );
    let first_metadata = tokio::fs::read_to_string(&store.paths(&fork_runs[0].id).message_metadata)
        .await
        .unwrap();
    let second_metadata =
        tokio::fs::read_to_string(&store.paths(&fork_runs[1].id).message_metadata)
            .await
            .unwrap();
    assert_eq!(
        first_metadata.lines().next().unwrap(),
        second_metadata.lines().next().unwrap()
    );
    for child in &fork_runs {
        let events = tokio::fs::read_to_string(&store.paths(&child.id).events)
            .await
            .unwrap();
        assert!(events.contains("\"cached_input_tokens\":73"));
    }
}

struct InvalidContextProvider {
    requests: Mutex<Vec<ModelRequest>>,
}

#[async_trait]
impl ModelProvider for InvalidContextProvider {
    fn name(&self) -> &str {
        "invalid-context"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let has_result = has_tool_result(&request, "invalid-delegate");
        self.requests.lock().unwrap().push(request);
        if has_result {
            Ok(text_response("invalid rejected", ModelUsage::default()))
        } else {
            Ok(ModelResponse::new(
                Message::assistant(vec![MessageContent::ToolCall {
                    id: "invalid-delegate".to_owned(),
                    name: "delegate".to_owned(),
                    arguments: json!({"name": "missing_context", "prompt": "must not run"}),
                }]),
                ModelUsage::default(),
            ))
        }
    }
}

#[tokio::test]
async fn delegate_rejects_a_missing_context_without_creating_a_child() {
    let workspace = TempDir::new().unwrap();
    let provider = Arc::new(InvalidContextProvider {
        requests: Mutex::new(Vec::new()),
    });
    let store = RunDirStore::new(workspace.path());
    let runner = runner(
        workspace.path(),
        provider.clone(),
        store.clone(),
        RunnerOptions::default(),
        ToolRegistry::default(),
    );
    let parent = runner
        .run(RunRequest::root("invalid context"))
        .await
        .unwrap();
    assert_eq!(parent.final_output, "invalid rejected");
    let requests = provider.requests.lock().unwrap().clone();
    let result = requests[1]
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|content| match content {
            MessageContent::ToolResult {
                call_id,
                content,
                is_error,
                ..
            } if call_id == "invalid-delegate" => Some((content, is_error)),
            _ => None,
        })
        .unwrap();
    assert!(*result.1);
    assert!(result.0.contains("missing field `context`"));
    assert!(child_runs(&store, &parent.run_id).await.is_empty());
}

struct MarkerTool;

#[async_trait]
impl Tool for MarkerTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "marker".to_owned(),
            description: "Return a marker".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {"label": {"type": "string"}},
                "required": ["label"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        Ok(RawToolOutput::text(format!(
            "marker-{}",
            arguments["label"].as_str().unwrap()
        )))
    }
}

#[derive(Default)]
struct CompactedForkProvider {
    root_run_id: Mutex<Option<String>>,
    root_calls: AtomicUsize,
    requests: Mutex<Vec<ModelRequest>>,
    delegate_input: Mutex<Option<ModelRequest>>,
}

#[async_trait]
impl ModelProvider for CompactedForkProvider {
    fn name(&self) -> &str {
        "compacted-fork"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let is_compaction = request.messages.last().is_some_and(|message| {
            message
                .visible_text()
                .contains("Compact the conversation state before this message")
        });
        self.requests.lock().unwrap().push(request.clone());
        if is_compaction {
            return Ok(text_response(
                "# Compacted state\n\nThe old marker was inspected.",
                ModelUsage {
                    input_tokens: Some(80),
                    output_tokens: Some(12),
                    ..ModelUsage::default()
                },
            ));
        }
        let root_run_id = self
            .root_run_id
            .lock()
            .unwrap()
            .get_or_insert_with(|| request.run_id.clone())
            .clone();
        if request.run_id != root_run_id {
            return Ok(text_response(
                "fork after compaction done",
                ModelUsage::default(),
            ));
        }
        match self.root_calls.fetch_add(1, Ordering::SeqCst) {
            0 => Ok(tool_response(vec![ToolCall {
                id: "old-marker".to_owned(),
                name: "marker".to_owned(),
                arguments: json!({"label": "old"}),
            }])),
            1 => Ok(tool_response(vec![ToolCall {
                id: "new-marker".to_owned(),
                name: "marker".to_owned(),
                arguments: json!({"label": "new"}),
            }])),
            2 => {
                *self.delegate_input.lock().unwrap() = Some(request);
                Ok(tool_response(vec![delegate_call(
                    "fork-compacted",
                    "fork_compacted",
                    "fork compacted task",
                    "fork",
                )]))
            }
            3 | 4 => Ok(text_response(
                "compacted parent done",
                ModelUsage::default(),
            )),
            unexpected => bail!("unexpected compacted fork root call {unexpected}"),
        }
    }
}

#[tokio::test]
async fn fork_preserves_the_active_compacted_projection_and_exact_history() {
    let workspace = TempDir::new().unwrap();
    let provider = Arc::new(CompactedForkProvider::default());
    let store = RunDirStore::new(workspace.path());
    let mut tools = ToolRegistry::default();
    tools.register(Arc::new(MarkerTool)).unwrap();
    tools.register(Arc::new(ReadTool::default())).unwrap();
    let runner = runner(
        workspace.path(),
        provider.clone(),
        store.clone(),
        RunnerOptions {
            max_subagent_depth: 2,
            max_parallel_model_calls: 2,
            max_output_tokens: Some(64),
            compaction: CompactionOptions {
                compact_at_tokens: Some(10),
                context_window_tokens: Some(100_000),
                keep_recent_tokens: 1,
                summary_max_output_tokens: 64,
                history_search_max_matches: 10,
            },
            ..RunnerOptions::default()
        },
        tools,
    );

    let parent = runner
        .run(RunRequest::root("compact before fork"))
        .await
        .unwrap();
    assert_eq!(parent.final_output, "compacted parent done");
    let delegate_input = provider.delegate_input.lock().unwrap().clone().unwrap();
    let requests = provider.requests.lock().unwrap().clone();
    let child_request = child_request(&requests, "fork compacted task");
    assert_eq!(child_request.system, delegate_input.system);
    assert_eq!(
        serialized(&child_request.tools),
        serialized(&delegate_input.tools)
    );
    assert_eq!(
        serialized(&child_request.messages[..delegate_input.messages.len()]),
        serialized(&delegate_input.messages)
    );
    assert_eq!(
        child_request.messages.len(),
        delegate_input.messages.len() + 1
    );

    let child = child_runs(&store, &parent.run_id).await.remove(0);
    let trajectory = store.load_trajectory(&child.id).await.unwrap();
    assert!(
        trajectory
            .iter()
            .any(|record| { matches!(record.compaction, Some(CompactionMessage::Request)) })
    );
    assert!(
        trajectory
            .iter()
            .any(|record| { matches!(record.compaction, Some(CompactionMessage::State { .. })) })
    );
    let history = store.load_compacted_history(&child.id).await.unwrap();
    assert!(!history.is_empty());
    assert!(history.iter().any(|record| {
        serialized(&record.message).contains("old-marker")
            || serialized(&record.message).contains("marker-old")
    }));
}

fn runner(
    workspace: &std::path::Path,
    provider: Arc<dyn ModelProvider>,
    store: RunDirStore,
    options: RunnerOptions,
    tools: ToolRegistry,
) -> Arc<AgentRunner> {
    AgentRunner::new(AgentRunnerConfig {
        provider,
        model: "test-model".to_owned(),
        workspace: workspace.to_owned(),
        skill_catalog: String::new(),
        tools,
        artifacts: ArtifactStore::default(),
        store,
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options,
    })
}

fn delegate_call(id: &str, name: &str, prompt: &str, context: &str) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: "delegate".to_owned(),
        arguments: json!({"name": name, "prompt": prompt, "context": context}),
    }
}

fn tool_response(calls: Vec<ToolCall>) -> ModelResponse {
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
        ModelUsage::default(),
    )
}

fn text_response(text: impl Into<String>, usage: ModelUsage) -> ModelResponse {
    ModelResponse::new(Message::text(Role::Assistant, text), usage)
}

fn has_tool_result(request: &ModelRequest, call_id: &str) -> bool {
    request.messages.iter().any(|message| {
        message.content.iter().any(|content| {
            matches!(content, MessageContent::ToolResult { call_id: id, .. } if id == call_id)
        })
    })
}

fn last_user_text(request: &ModelRequest) -> &str {
    request
        .messages
        .iter()
        .rev()
        .filter(|message| message.role == Role::User)
        .flat_map(|message| message.content.iter().rev())
        .find_map(|content| match content {
            MessageContent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or_default()
}

fn child_request<'a>(requests: &'a [ModelRequest], prompt: &str) -> &'a ModelRequest {
    requests
        .iter()
        .find(|request| last_user_text(request) == prompt)
        .unwrap_or_else(|| panic!("missing child request for {prompt}"))
}

async fn child_runs(
    store: &RunDirStore,
    parent_run_id: &str,
) -> Vec<picoagent::storage::RunRecord> {
    let mut entries = tokio::fs::read_dir(store.workspace().join(".pico/runs"))
        .await
        .unwrap();
    let mut children = Vec::new();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        let id = entry.file_name().to_string_lossy().into_owned();
        if id == parent_run_id {
            continue;
        }
        let record = store.load_run(&id).await.unwrap();
        if record.parent_run_id.as_deref() == Some(parent_run_id) {
            children.push(record);
        }
    }
    children.sort_by(|left, right| left.id.cmp(&right.id));
    children
}

fn serialized<T: Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value).unwrap()
}
