use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, ensure};
use async_trait::async_trait;
use picoagent::{
    agent::runner::{AgentRunner, AgentRunnerConfig, RunRequest, RunnerOptions},
    artifact::ArtifactStore,
    events::{EventSink, NoopEventSink, RuntimeEvent, RuntimeEventKind, SharedEventSink},
    hooks::HookPipeline,
    model::{
        Message, MessageContent, ModelProvider, ModelRequest, ModelResponse, ModelUsage, Role,
        ToolCall,
    },
    storage::RunDirStore,
    tools::{RawToolOutput, Tool, ToolContext, ToolRegistry},
};
use serde_json::{Value, json};
use tempfile::TempDir;

fn text_response(text: &str) -> ModelResponse {
    ModelResponse::new(Message::text(Role::Assistant, text), ModelUsage::default())
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

fn calls(specs: &[(&str, &str, u64)]) -> Vec<ToolCall> {
    specs
        .iter()
        .map(|(call_id, label, delay_ms)| ToolCall {
            id: (*call_id).to_owned(),
            name: "scheduled".to_owned(),
            arguments: json!({"label": label, "delay_ms": delay_ms}),
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

fn background_task_id(content: &str) -> Option<String> {
    content
        .split_once("task_id=\"")?
        .1
        .split_once('"')
        .map(|(task_id, _)| task_id.to_owned())
}

struct ScheduledTool {
    barrier: Arc<tokio::sync::Barrier>,
    completions: Arc<Mutex<Vec<String>>>,
    executions: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for ScheduledTool {
    fn spec(&self) -> picoagent::model::ToolSpec {
        picoagent::model::ToolSpec {
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
        tools,
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events,
        options: RunnerOptions {
            foreground_tool_timeout_seconds: foreground_timeout_seconds,
            task_wait_timeout_seconds: 5,
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

struct PartialPromotionProvider {
    calls: AtomicUsize,
    promoted_task_id: Mutex<Option<String>>,
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
                ensure!(
                    results[1]
                        .1
                        .contains("The task is now running in the background.")
                );
                let task_id = background_task_id(results[1].1)
                    .expect("promotion acknowledgement omitted task_id");
                *self.promoted_task_id.lock().unwrap() = Some(task_id);
                Ok(text_response("waiting for automatic delivery"))
            }
            2 => {
                let expected = self
                    .promoted_task_id
                    .lock()
                    .unwrap()
                    .clone()
                    .expect("promotion was not observed");
                let background = request
                    .messages
                    .iter()
                    .flat_map(|message| &message.content)
                    .filter_map(|content| match content {
                        MessageContent::BackgroundTask {
                            task_id, content, ..
                        } => Some((task_id.as_str(), content.as_str())),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                ensure!(
                    background.len() == 1
                        && background[0].0 == expected
                        && background[0].1.starts_with(".pico/runs/")
                        && background[0].1.contains("/artifacts/background-t1-"),
                    "background result did not reference its complete artifact: {background:?}"
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
async fn direct_batch_promotes_only_the_unfinished_future_and_delivers_it_by_task_id() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let completions = Arc::new(Mutex::new(Vec::new()));
    let executions = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(PartialPromotionProvider {
        calls: AtomicUsize::new(0),
        promoted_task_id: Mutex::new(None),
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
            .filter(|content| matches!(content, MessageContent::BackgroundTask { .. }))
            .count(),
        1
    );
    let mut task_files = tokio::fs::read_dir(store.paths(&result.run_id).directory.join("tasks"))
        .await
        .unwrap();
    let task_path = task_files.next_entry().await.unwrap().unwrap().path();
    assert!(task_files.next_entry().await.unwrap().is_none());
    let task_record: Value =
        serde_json::from_slice(&tokio::fs::read(task_path).await.unwrap()).unwrap();
    assert_eq!(task_record["origin_call_id"], "call-background");
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

    let run_dir = std::fs::read_dir(workspace.path().join(".pico/runs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let task_path = std::fs::read_dir(run_dir.join("tasks"))
        .expect("the other pending future was dropped instead of promoted")
        .next()
        .unwrap()
        .unwrap()
        .path();
    let task: Value = serde_json::from_slice(&tokio::fs::read(task_path).await.unwrap()).unwrap();
    assert_eq!(task["name"], "scheduled");
    assert_eq!(task["state"], "cancelled");
}
