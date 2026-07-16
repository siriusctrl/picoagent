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
    artifact::{ArtifactPolicy, ArtifactStore},
    events::{NoopEventSink, RuntimeEvent, RuntimeEventKind, SharedEventSink},
    hooks::HookPipeline,
    model::{
        Message, MessageContent, ModelProvider, ModelRequest, ModelResponse, ModelUsage, Role,
        ToolSpec,
    },
    storage::{RunDirStore, RunState},
    tools::{RawToolOutput, ReadTool, Tool, ToolContext, ToolRegistry},
};
use serde_json::{Value, json};
use tempfile::TempDir;

const SUMMARY_TEXT: &str = "## Progress\nThe old marker result was `result-old`.";

struct MarkerTool;

#[async_trait]
impl Tool for MarkerTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "marker".to_owned(),
            description: "Return a labelled marker".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {"label": {"type": "string"}},
                "required": ["label"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let label = arguments
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("missing");
        Ok(RawToolOutput::text(format!("result-{label}")))
    }
}

struct ScriptedCompactionProvider {
    normal_calls: AtomicUsize,
    requests: Mutex<Vec<ModelRequest>>,
    fail_summary: bool,
}

impl ScriptedCompactionProvider {
    fn new(fail_summary: bool) -> Arc<Self> {
        Arc::new(Self {
            normal_calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            fail_summary,
        })
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ModelProvider for ScriptedCompactionProvider {
    fn name(&self) -> &str {
        "scripted-compaction"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let is_summary = request.tools.is_empty();
        self.requests.lock().unwrap().push(request);
        if is_summary {
            if self.fail_summary {
                bail!("intentional summary failure");
            }
            return Ok(ModelResponse::new(
                Message::text(Role::Assistant, SUMMARY_TEXT),
                ModelUsage {
                    input_tokens: Some(42),
                    output_tokens: Some(9),
                    ..ModelUsage::default()
                },
            ));
        }

        let index = self.normal_calls.fetch_add(1, Ordering::SeqCst);
        match index {
            0 => Ok(tool_call_response("call-old", "old")),
            1 => Ok(tool_call_response("call-new", "new")),
            2 => Ok(ModelResponse::new(
                Message::text(Role::Assistant, "finished after compaction"),
                ModelUsage {
                    input_tokens: Some(80),
                    output_tokens: Some(5),
                    ..ModelUsage::default()
                },
            )),
            unexpected => bail!("unexpected normal model call {unexpected}"),
        }
    }
}

fn tool_call_response(id: &str, label: &str) -> ModelResponse {
    tool_call_response_with_usage(id, label, 100)
}

fn tool_call_response_with_usage(id: &str, label: &str, input_tokens: u64) -> ModelResponse {
    ModelResponse::new(
        Message::assistant(vec![MessageContent::ToolCall {
            id: id.to_owned(),
            name: "marker".to_owned(),
            arguments: json!({"label": label}),
        }]),
        ModelUsage {
            input_tokens: Some(input_tokens),
            output_tokens: Some(10),
            ..ModelUsage::default()
        },
    )
}

fn runner(
    workspace: &TempDir,
    provider: Arc<ScriptedCompactionProvider>,
) -> (Arc<AgentRunner>, RunDirStore) {
    let store = RunDirStore::new(workspace.path());
    let mut tools = ToolRegistry::default();
    tools.register(Arc::new(MarkerTool)).unwrap();
    tools.register(Arc::new(ReadTool)).unwrap();
    let options = RunnerOptions {
        max_steps: 4,
        compaction: CompactionOptions {
            trigger_tokens: Some(10),
            keep_recent_tokens: 1,
            summary_max_output_tokens: 77,
            history_search_max_matches: 7,
        },
        ..RunnerOptions::default()
    };
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider,
        model: "test-model".to_owned(),
        workspace: workspace.path().to_owned(),
        skill_catalog: String::new(),
        tools,
        artifacts: ArtifactStore::new(ArtifactPolicy::default()),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options,
    });
    (runner, store)
}

#[tokio::test]
async fn runner_compacts_active_context_but_preserves_raw_trajectory() {
    let workspace = TempDir::new().unwrap();
    let provider = ScriptedCompactionProvider::new(false);
    let (runner, store) = runner(&workspace, provider.clone());

    let result = runner
        .run(RunRequest::root("exercise compaction"))
        .await
        .unwrap();
    assert_eq!(result.final_output, "finished after compaction");
    assert_eq!(
        store.load_run(&result.run_id).await.unwrap().state,
        RunState::Completed
    );

    let all_requests = provider.requests();
    let summary_requests: Vec<_> = all_requests
        .iter()
        .filter(|request| request.tools.is_empty())
        .collect();
    let normal_requests: Vec<_> = all_requests
        .iter()
        .filter(|request| !request.tools.is_empty())
        .collect();
    assert_eq!(summary_requests.len(), 1);
    assert_eq!(normal_requests.len(), 3);

    let summary = summary_requests[0];
    assert!(
        summary
            .system
            .contains("Summarize the supplied historical transcript")
    );
    assert_eq!(summary.max_output_tokens, Some(77));
    assert_eq!(summary.messages.len(), 1);
    let summary_input = text_content(&summary.messages[0]);
    assert!(summary_input.contains("call-old"));
    assert!(summary_input.contains("result-old"));
    assert!(!summary_input.contains("call-new"));

    for request in &normal_requests {
        let names: Vec<_> = request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert!(names.contains(&"history_search"));
        assert!(names.contains(&"history_read"));
        assert!(names.contains(&"marker"));
    }
    let stable_system = &normal_requests[0].system;
    let stable_tools = serde_json::to_value(&normal_requests[0].tools).unwrap();
    for request in &normal_requests[1..] {
        assert_eq!(&request.system, stable_system);
        assert_eq!(serde_json::to_value(&request.tools).unwrap(), stable_tools);
    }
    assert!(!stable_system.contains("history_search"));
    assert!(text_content(&normal_requests[0].messages[0]).contains("<context-management>"));
    assert!(text_content(&normal_requests[0].messages[0]).contains("history_search"));

    let resumed = normal_requests[2];
    assert_eq!(resumed.messages.len(), 4);
    assert!(text_content(&resumed.messages[0]).contains("exercise compaction"));
    let compacted = text_content(&resumed.messages[1]);
    assert!(compacted.contains("<compacted-history"));
    assert!(compacted.contains(SUMMARY_TEXT));
    assert!(has_tool_call(&resumed.messages[2], "call-new", "marker"));
    assert!(has_tool_result(
        &resumed.messages[3],
        "call-new",
        "result-new"
    ));
    assert!(
        !resumed
            .messages
            .iter()
            .any(|message| has_tool_call(message, "call-old", "marker"))
    );

    let trajectory = store.load_trajectory(&result.run_id).await.unwrap();
    assert_eq!(trajectory.len(), 6);
    assert!(has_tool_call(&trajectory[1].message, "call-old", "marker"));
    assert!(has_tool_result(
        &trajectory[2].message,
        "call-old",
        "result-old"
    ));
    assert!(has_tool_call(&trajectory[3].message, "call-new", "marker"));
    assert!(has_tool_result(
        &trajectory[4].message,
        "call-new",
        "result-new"
    ));

    let checkpoints = store.load_compactions(&result.run_id).await.unwrap();
    assert_eq!(checkpoints.len(), 1);
    let checkpoint = &checkpoints[0];
    assert_eq!(checkpoint.summary, SUMMARY_TEXT);
    assert_eq!(checkpoint.compacted_message_count, 2);
    assert_eq!(
        checkpoint.covered_through_message_ref,
        trajectory[2].message_ref
    );
    assert_eq!(checkpoint.first_kept_message_ref, trajectory[3].message_ref);
    assert_eq!(checkpoint.summary_input_tokens, Some(42));
    assert_eq!(checkpoint.summary_output_tokens, Some(9));

    let events = load_events(&store, &result.run_id).await;
    assert_eq!(count_events(&events, EventClass::Started), 1);
    assert_eq!(count_events(&events, EventClass::Completed), 1);
    assert_eq!(count_events(&events, EventClass::Failed), 0);
    let completed = events.iter().find_map(|event| match &event.kind {
        RuntimeEventKind::CompactionCompleted {
            checkpoint_id,
            covered_through_message_ref,
            first_kept_message_ref,
            ..
        } => Some((
            checkpoint_id,
            covered_through_message_ref,
            first_kept_message_ref,
        )),
        _ => None,
    });
    assert_eq!(
        completed,
        Some((
            &checkpoint.checkpoint_id,
            &checkpoint.covered_through_message_ref,
            &checkpoint.first_kept_message_ref,
        ))
    );
}

#[tokio::test]
async fn summary_failure_is_recorded_and_does_not_abort_the_run() {
    let workspace = TempDir::new().unwrap();
    let provider = ScriptedCompactionProvider::new(true);
    let (runner, store) = runner(&workspace, provider.clone());

    let result = runner
        .run(RunRequest::root("survive summary failure"))
        .await
        .unwrap();
    assert_eq!(result.final_output, "finished after compaction");
    assert!(
        store
            .load_compactions(&result.run_id)
            .await
            .unwrap()
            .is_empty()
    );

    let requests = provider.requests();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.tools.is_empty())
            .count(),
        1
    );
    let final_request = requests
        .iter()
        .filter(|request| !request.tools.is_empty())
        .nth(2)
        .unwrap();
    assert!(
        final_request
            .messages
            .iter()
            .any(|message| has_tool_call(message, "call-old", "marker"))
    );

    let events = load_events(&store, &result.run_id).await;
    assert_eq!(count_events(&events, EventClass::Started), 1);
    assert_eq!(count_events(&events, EventClass::Completed), 0);
    assert_eq!(count_events(&events, EventClass::Failed), 1);
}

fn text_content(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|content| match content {
            MessageContent::RuntimeReminder { text } | MessageContent::Text { text } => {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn has_tool_call(message: &Message, id: &str, name: &str) -> bool {
    message.content.iter().any(|content| {
        matches!(
            content,
            MessageContent::ToolCall {
                id: call_id,
                name: tool_name,
                ..
            } if call_id == id && tool_name == name
        )
    })
}

fn has_tool_result(message: &Message, call_id: &str, expected: &str) -> bool {
    message.content.iter().any(|content| {
        matches!(
            content,
            MessageContent::ToolResult {
                call_id: result_call_id,
                content,
                ..
            } if result_call_id == call_id && content.contains(expected)
        )
    })
}

async fn load_events(store: &RunDirStore, run_id: &str) -> Vec<RuntimeEvent> {
    tokio::fs::read_to_string(store.paths(run_id).events)
        .await
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[derive(Clone, Copy)]
enum EventClass {
    Started,
    Completed,
    Failed,
}

fn count_events(events: &[RuntimeEvent], class: EventClass) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                (&event.kind, class),
                (
                    RuntimeEventKind::CompactionStarted { .. },
                    EventClass::Started
                ) | (
                    RuntimeEventKind::CompactionCompleted { .. },
                    EventClass::Completed
                ) | (
                    RuntimeEventKind::CompactionFailed { .. },
                    EventClass::Failed
                )
            )
        })
        .count()
}
