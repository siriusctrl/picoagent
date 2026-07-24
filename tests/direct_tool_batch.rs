use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use fiasco::{
    agent::runner::{AgentRunner, AgentRunnerConfig, RunRequest, RunnerOptions},
    artifact::ArtifactStore,
    events::{EventSink, NoopEventSink, RuntimeEvent, RuntimeEventKind, SharedEventSink},
    hooks::HookPipeline,
    model::{
        Message, MessageContent, ModelModality, ModelProvider, ModelRequest, ModelResponse,
        ModelUsage, Role, ToolArguments, ToolCall,
    },
    storage::RunDirStore,
    tools::{RawToolOutput, ReadTool, Tool, ToolContext, ToolRegistry},
};
use serde_json::{Value, json};
use tempfile::TempDir;

fn text_response(text: &str) -> ModelResponse {
    ModelResponse::new(Message::text(Role::Assistant, text), ModelUsage::default())
}

fn tool_response(calls: Vec<ToolCall>) -> ModelResponse {
    ModelResponse::new(
        Message::assistant(calls.into_iter().map(MessageContent::ToolCall).collect()),
        ModelUsage::default(),
    )
}

fn calls(specs: &[(&str, &str, u64)]) -> Vec<ToolCall> {
    specs
        .iter()
        .map(|(call_id, label, delay_ms)| ToolCall {
            id: (*call_id).to_owned(),
            name: "scheduled".to_owned(),
            arguments: json!({"label": label, "delay_ms": delay_ms}).into(),
        })
        .collect()
}

fn tool_results(request: &ModelRequest) -> Vec<(&str, &str)> {
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|content| match content {
            MessageContent::ToolResult {
                call_id, content, ..
            } => Some((call_id.as_str(), content.as_str())),
            _ => None,
        })
        .collect()
}

fn runtime_handle_id(content: &str) -> Option<String> {
    content
        .split_once("handle=\"")?
        .1
        .split_once('"')
        .map(|(handle, _)| handle.to_owned())
}

struct ScheduledTool {
    barrier: Arc<tokio::sync::Barrier>,
    completions: Arc<Mutex<Vec<String>>>,
    executions: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for ScheduledTool {
    fn spec(&self) -> fiasco::model::ToolSpec {
        fiasco::model::ToolSpec {
            name: "scheduled".to_owned(),
            description: "Return the requested label after a test delay".to_owned(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        let label = arguments["label"].as_str().unwrap().to_owned();
        let delay_ms = arguments["delay_ms"].as_u64().unwrap();
        self.barrier.wait().await;
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        self.completions.lock().unwrap().push(label.clone());
        Ok(RawToolOutput::text(label))
    }
}

struct OrderedBatchProvider {
    calls: AtomicUsize,
}

#[async_trait]
impl ModelProvider for OrderedBatchProvider {
    fn name(&self) -> &str {
        "ordered-batch"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => Ok(tool_response(calls(&[
                ("call-slow", "slow", 80),
                ("call-fast", "fast", 10),
            ]))),
            1 => {
                let results = tool_results(&request);
                ensure!(
                    results == [("call-slow", "slow"), ("call-fast", "fast")],
                    "tool results were not committed in original call order: {results:?}"
                );
                Ok(text_response("ordered"))
            }
            call => anyhow::bail!("unexpected model call {call}"),
        }
    }
}

fn runner(
    workspace: &TempDir,
    store: &RunDirStore,
    provider: Arc<dyn ModelProvider>,
    tool: ScheduledTool,
    foreground_timeout_seconds: u64,
    extra_events: SharedEventSink,
) -> Arc<AgentRunner> {
    let mut tools = ToolRegistry::default();
    tools.register(Arc::new(tool)).unwrap();
    AgentRunner::new(AgentRunnerConfig {
        provider,
        model: "scripted".to_owned(),
        workspace: workspace.path().to_owned(),
        skill_catalog: String::new(),
        mcp_catalog: String::new(),
        tools,
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events,
        options: RunnerOptions {
            foreground_tool_timeout_seconds: foreground_timeout_seconds,
            handle_wait_timeout_seconds: 5,
            ..RunnerOptions::default()
        },
    })
}

#[tokio::test]
async fn direct_batch_finishes_early_but_commits_in_original_call_order() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let completions = Arc::new(Mutex::new(Vec::new()));
    let executions = Arc::new(AtomicUsize::new(0));
    let runner = runner(
        &workspace,
        &store,
        Arc::new(OrderedBatchProvider {
            calls: AtomicUsize::new(0),
        }),
        ScheduledTool {
            barrier: Arc::new(tokio::sync::Barrier::new(2)),
            completions: completions.clone(),
            executions: executions.clone(),
        },
        5,
        Arc::new(NoopEventSink),
    );

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        runner.run(RunRequest::root("run an ordered batch")),
    )
    .await
    .expect("the completed batch waited for its five-second deadline")
    .unwrap();

    assert_eq!(result.final_output, "ordered");
    assert_eq!(executions.load(Ordering::SeqCst), 2);
    assert_eq!(*completions.lock().unwrap(), ["fast", "slow"]);
    let events = tokio::fs::read_to_string(store.paths(&result.run_id).events)
        .await
        .unwrap();
    let completed_call_ids = events
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["type"] == "tool_completed")
        .map(|event| event["call_id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(completed_call_ids, ["call-fast", "call-slow"]);
}

struct ArgumentTool(Arc<AtomicUsize>);

#[async_trait]
impl Tool for ArgumentTool {
    fn spec(&self) -> fiasco::model::ToolSpec {
        fiasco::model::ToolSpec {
            name: "argument_tool".to_owned(),
            description: "Return the supplied label".to_owned(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(RawToolOutput::text(
            arguments["label"].as_str().unwrap().to_owned(),
        ))
    }
}

struct MalformedBatchProvider {
    calls: AtomicUsize,
}

#[async_trait]
impl ModelProvider for MalformedBatchProvider {
    fn name(&self) -> &str {
        "malformed-batch"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => Ok(tool_response(vec![
                ToolCall {
                    id: "valid-one".to_owned(),
                    name: "argument_tool".to_owned(),
                    arguments: json!({"label": "one"}).into(),
                },
                ToolCall {
                    id: "malformed".to_owned(),
                    name: "argument_tool".to_owned(),
                    arguments: ToolArguments::from_raw("{\"label\":"),
                },
                ToolCall {
                    id: "valid-three".to_owned(),
                    name: "argument_tool".to_owned(),
                    arguments: json!({"label": "three"}).into(),
                },
            ])),
            1 => {
                let results = request
                    .messages
                    .iter()
                    .flat_map(|message| &message.content)
                    .filter_map(|content| match content {
                        MessageContent::ToolResult {
                            call_id,
                            content,
                            is_error,
                            ..
                        } => Some((call_id.as_str(), content.as_str(), *is_error)),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                ensure!(
                    results.len() == 3
                        && results[0] == ("valid-one", "one", false)
                        && results[1].0 == "malformed"
                        && results[1].1.contains("tool arguments are not valid JSON")
                        && results[1].2
                        && results[2] == ("valid-three", "three", false),
                    "malformed call did not remain a local ordered tool error: {results:?}"
                );
                Ok(text_response("recovered malformed call"))
            }
            call => anyhow::bail!("unexpected model call {call}"),
        }
    }
}

#[tokio::test]
async fn malformed_arguments_fail_only_their_own_call_and_preserve_raw_text() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let executions = Arc::new(AtomicUsize::new(0));
    let mut tools = ToolRegistry::default();
    tools
        .register(Arc::new(ArgumentTool(executions.clone())))
        .unwrap();
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(MalformedBatchProvider {
            calls: AtomicUsize::new(0),
        }),
        model: "scripted".to_owned(),
        workspace: workspace.path().to_owned(),
        skill_catalog: String::new(),
        mcp_catalog: String::new(),
        tools,
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions::default(),
    });

    let result = runner
        .run(RunRequest::root("execute every call"))
        .await
        .unwrap();
    assert_eq!(result.final_output, "recovered malformed call");
    assert_eq!(executions.load(Ordering::SeqCst), 2);
    let messages = store.load_messages(&result.run_id).await.unwrap();
    let raw = messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|content| match content {
            MessageContent::ToolCall(call) if call.id == "malformed" => {
                Some(call.arguments.as_raw())
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(raw, "{\"label\":");
}

struct ImageTool;

#[async_trait]
impl Tool for ImageTool {
    fn spec(&self) -> fiasco::model::ToolSpec {
        fiasco::model::ToolSpec {
            name: "image".to_owned(),
            description: "Return a test image".to_owned(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let label = arguments["label"].as_str().unwrap();
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(label.as_bytes());
        Ok(RawToolOutput::image(bytes, "image/png"))
    }
}

struct ImageBatchProvider {
    calls: AtomicUsize,
}

#[async_trait]
impl ModelProvider for ImageBatchProvider {
    fn name(&self) -> &str {
        "image-batch"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => Ok(tool_response(vec![
                ToolCall {
                    id: "image-one".into(),
                    name: "image".into(),
                    arguments: json!({"label": "one"}).into(),
                },
                ToolCall {
                    id: "image-two".into(),
                    name: "image".into(),
                    arguments: json!({"label": "two"}).into(),
                },
            ])),
            1 => {
                let tool_message_indexes = request
                    .messages
                    .iter()
                    .enumerate()
                    .filter_map(|(index, message)| (message.role == Role::Tool).then_some(index))
                    .collect::<Vec<_>>();
                ensure!(tool_message_indexes.len() == 2);
                let attachment_index = request
                    .messages
                    .iter()
                    .position(|message| {
                        message
                            .content
                            .iter()
                            .any(|content| matches!(content, MessageContent::Image { .. }))
                    })
                    .context("image batch omitted its attachment message")?;
                ensure!(
                    tool_message_indexes
                        .iter()
                        .all(|index| *index < attachment_index),
                    "image attachment interrupted tool-result pairing"
                );
                let attachment_message = &request.messages[attachment_index];
                let attachments = attachment_message
                    .content
                    .iter()
                    .filter_map(|content| match content {
                        MessageContent::Image { attachment } => Some(attachment),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                ensure!(attachments.len() == 2);
                ensure!(
                    attachments
                        .iter()
                        .all(|image| image.media_type == "image/png")
                );
                ensure!(attachment_message.content.iter().any(|content| {
                    matches!(content, MessageContent::RuntimeReminder { text } if text.contains("image-one") && text.contains("image-two"))
                }));
                Ok(text_response("images attached"))
            }
            call => anyhow::bail!("unexpected model call {call}"),
        }
    }
}

#[tokio::test]
async fn concurrent_image_results_are_attached_after_all_tool_results() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let mut tools = ToolRegistry::default();
    tools.register(Arc::new(ImageTool)).unwrap();
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(ImageBatchProvider {
            calls: AtomicUsize::new(0),
        }),
        model: "vision-model".to_owned(),
        workspace: workspace.path().to_owned(),
        skill_catalog: String::new(),
        mcp_catalog: String::new(),
        tools,
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions {
            model_modalities: BTreeSet::from([ModelModality::Text, ModelModality::Image]),
            ..RunnerOptions::default()
        },
    });

    let result = runner
        .run(RunRequest::root("read two images"))
        .await
        .unwrap();
    assert_eq!(result.final_output, "images attached");
    let messages = store.load_messages(&result.run_id).await.unwrap();
    assert_eq!(
        messages
            .iter()
            .flat_map(|message| &message.content)
            .filter(|content| matches!(content, MessageContent::Image { .. }))
            .count(),
        2
    );
}

struct TextOnlyImageReadProvider {
    calls: AtomicUsize,
}

#[async_trait]
impl ModelProvider for TextOnlyImageReadProvider {
    fn name(&self) -> &str {
        "text-only-image-read"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                ensure!(request.messages[0].content.iter().any(|content| {
                    matches!(
                        content,
                        MessageContent::RuntimeReminder { text }
                            if text.contains("current model supported modalities: [text]")
                    )
                }));
                Ok(tool_response(vec![ToolCall {
                    id: "read-image".into(),
                    name: "read".into(),
                    arguments: json!({"path": "image.png"}).into(),
                }]))
            }
            1 => {
                ensure!(!request.messages.iter().any(|message| {
                    message
                        .content
                        .iter()
                        .any(|content| matches!(content, MessageContent::Image { .. }))
                }));
                ensure!(request.messages.iter().any(|message| {
                    message.content.iter().any(|content| {
                        matches!(
                            content,
                            MessageContent::ToolResult {
                                is_error: true,
                                content,
                                ..
                            } if content.contains("configured model cannot inspect images")
                        )
                    })
                }));
                Ok(text_response("unsupported image reported"))
            }
            call => anyhow::bail!("unexpected model call {call}"),
        }
    }
}

#[tokio::test]
async fn text_only_image_read_returns_a_tool_error_without_an_attachment() {
    let workspace = TempDir::new().unwrap();
    tokio::fs::write(workspace.path().join("image.png"), b"not loaded")
        .await
        .unwrap();
    let store = RunDirStore::new(workspace.path());
    let mut tools = ToolRegistry::default();
    tools.register(Arc::new(ReadTool::new(false))).unwrap();
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(TextOnlyImageReadProvider {
            calls: AtomicUsize::new(0),
        }),
        model: "text-model".to_owned(),
        workspace: workspace.path().to_owned(),
        skill_catalog: String::new(),
        mcp_catalog: String::new(),
        tools,
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions::default(),
    });

    let result = runner
        .run(RunRequest::root("try to read an image"))
        .await
        .unwrap();
    assert_eq!(result.final_output, "unsupported image reported");
    let mut artifacts = tokio::fs::read_dir(store.paths(&result.run_id).artifacts)
        .await
        .unwrap();
    assert!(artifacts.next_entry().await.unwrap().is_none());
}

struct PartialPromotionProvider {
    calls: AtomicUsize,
    promoted_handle: Mutex<Option<String>>,
}

#[async_trait]
impl ModelProvider for PartialPromotionProvider {
    fn name(&self) -> &str {
        "partial-promotion"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => Ok(tool_response(calls(&[
                ("call-first", "first", 20),
                ("call-background", "background", 2_200),
                ("call-last", "last", 40),
            ]))),
            1 => {
                let results = tool_results(&request);
                ensure!(
                    results.iter().map(|(id, _)| *id).collect::<Vec<_>>()
                        == ["call-first", "call-background", "call-last"],
                    "partial batch results were not committed in call order: {results:?}"
                );
                ensure!(results[0].1 == "first" && results[2].1 == "last");
                ensure!(!results[1].1.contains("status="));
                ensure!(results[1].1.contains("The asynchronous work is active."));
                let handle = runtime_handle_id(results[1].1)
                    .expect("promotion acknowledgement omitted handle");
                *self.promoted_handle.lock().unwrap() = Some(handle);
                Ok(text_response("waiting for automatic delivery"))
            }
            2 => {
                let expected = self
                    .promoted_handle
                    .lock()
                    .unwrap()
                    .clone()
                    .expect("promotion was not observed");
                let background = request
                    .messages
                    .iter()
                    .flat_map(|message| &message.content)
                    .filter_map(|content| match content {
                        MessageContent::RuntimeHandle {
                            handle, content, ..
                        } => Some((handle.as_str(), content.as_str())),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                ensure!(
                    background.len() == 1
                        && background[0].0 == expected
                        && background[0].1 == "background",
                    "small background result did not stay inline: {background:?}"
                );
                ensure!(
                    tool_results(&request)
                        .iter()
                        .filter(|(call_id, _)| *call_id == "call-background")
                        .count()
                        == 1,
                    "promoted call received a duplicate tool result"
                );
                Ok(text_response("delivered"))
            }
            call => anyhow::bail!("unexpected model call {call}"),
        }
    }
}

#[tokio::test]
async fn direct_batch_promotes_only_the_unfinished_future_and_delivers_it_by_handle() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let completions = Arc::new(Mutex::new(Vec::new()));
    let executions = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(PartialPromotionProvider {
        calls: AtomicUsize::new(0),
        promoted_handle: Mutex::new(None),
    });
    let runner = runner(
        &workspace,
        &store,
        provider,
        ScheduledTool {
            barrier: Arc::new(tokio::sync::Barrier::new(3)),
            completions: completions.clone(),
            executions: executions.clone(),
        },
        1,
        Arc::new(NoopEventSink),
    );

    let result = runner
        .run(RunRequest::root("run a partially slow batch"))
        .await
        .unwrap();

    assert_eq!(result.final_output, "delivered");
    assert_eq!(executions.load(Ordering::SeqCst), 3);
    assert_eq!(
        *completions.lock().unwrap(),
        ["first", "last", "background"]
    );
    let messages = store.load_messages(&result.run_id).await.unwrap();
    assert_eq!(
        messages
            .iter()
            .flat_map(|message| &message.content)
            .filter(|content| matches!(content, MessageContent::RuntimeHandle { .. }))
            .count(),
        1
    );
    assert!(
        !tokio::fs::try_exists(store.paths(&result.run_id).directory.join("tasks"))
            .await
            .unwrap()
    );
}

struct BoundaryFailureProvider;

#[async_trait]
impl ModelProvider for BoundaryFailureProvider {
    fn name(&self) -> &str {
        "batch-boundary-failure"
    }

    async fn complete(
        &self,
        _request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        Ok(tool_response(calls(&[
            ("call-fail", "never-executed", 0),
            ("call-pending", "pending", 10_000),
        ])))
    }
}

struct FailSelectedToolStart;

#[async_trait]
impl EventSink for FailSelectedToolStart {
    async fn emit(&self, event: &RuntimeEvent) -> Result<()> {
        if matches!(
            &event.kind,
            RuntimeEventKind::ToolStarted { call_id, .. } if call_id == "call-fail"
        ) {
            anyhow::bail!("injected tool boundary failure")
        }
        Ok(())
    }
}

#[tokio::test]
async fn batch_boundary_error_promotes_other_pending_futures_before_returning() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let executions = Arc::new(AtomicUsize::new(0));
    let runner = runner(
        &workspace,
        &store,
        Arc::new(BoundaryFailureProvider),
        ScheduledTool {
            barrier: Arc::new(tokio::sync::Barrier::new(1)),
            completions: Arc::new(Mutex::new(Vec::new())),
            executions: executions.clone(),
        },
        1,
        Arc::new(FailSelectedToolStart),
    );

    let error = runner
        .run(RunRequest::root("exercise a batch boundary failure"))
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("injected tool boundary failure"));
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    let run_dir = std::fs::read_dir(workspace.path().join(".fiasco/runs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(!run_dir.join("tasks").exists());
}
