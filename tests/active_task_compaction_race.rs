use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
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
    },
    storage::RunDirStore,
    tools::{ReadTool, ToolRegistry},
};
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Notify;

const ACTIVE_MARKER: &str = "<active-background-tasks>";
const COMPACTION_INSTRUCTION: &str = "Compact the conversation state before this message";

struct CompletingDuringCompactionProvider {
    workspace: PathBuf,
    root_run_id: Mutex<Option<String>>,
    root_calls: AtomicUsize,
    requests: Mutex<Vec<ModelRequest>>,
    release_child: Notify,
}

impl CompletingDuringCompactionProvider {
    fn new(workspace: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            workspace,
            root_run_id: Mutex::new(None),
            root_calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            release_child: Notify::new(),
        })
    }

    async fn wait_for_terminal_task(&self, run_id: &str) -> Result<()> {
        let path = self
            .workspace
            .join(".pico/runs")
            .join(run_id)
            .join("tasks/t1.json");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if tokio::fs::read_to_string(&path)
                    .await
                    .is_ok_and(|record| record.contains("\"state\": \"completed\""))
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .with_context(|| format!("task record did not become terminal: {}", path.display()))?;
        Ok(())
    }
}

#[async_trait]
impl ModelProvider for CompletingDuringCompactionProvider {
    fn name(&self) -> &str {
        "complete-during-compaction"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let is_compaction = request
            .messages
            .last()
            .is_some_and(|message| message.visible_text().contains(COMPACTION_INSTRUCTION));
        self.requests.lock().unwrap().push(request.clone());
        if is_compaction {
            self.release_child.notify_one();
            self.wait_for_terminal_task(&request.run_id).await?;
            return Ok(text_response(
                "# Compacted state\n\nThe delegated review was pending.",
            ));
        }

        let root_run_id = self
            .root_run_id
            .lock()
            .unwrap()
            .get_or_insert_with(|| request.run_id.clone())
            .clone();
        if request.run_id != root_run_id {
            self.release_child.notified().await;
            return Ok(text_response("child completed during compaction"));
        }
        match self.root_calls.fetch_add(1, Ordering::SeqCst) {
            0 => Ok(tool_response(
                "delegate-review",
                "delegate",
                json!({
                    "name": "compaction review",
                    "prompt": "Inspect only and return a report.",
                    "context": "fresh"
                }),
            )),
            1 => Ok(tool_response(
                "status-review",
                "task_status",
                json!({"task_ids": ["t1"]}),
            )),
            2 => Ok(text_response("parent observed terminal result")),
            unexpected => bail!("unexpected root model call {unexpected}"),
        }
    }
}

#[tokio::test]
async fn task_finishing_during_compaction_is_delivered_without_a_stale_active_reminder() {
    let workspace = TempDir::new().unwrap();
    let provider = CompletingDuringCompactionProvider::new(workspace.path().to_owned());
    let store = RunDirStore::new(workspace.path());
    let mut tools = ToolRegistry::default();
    tools.register(Arc::new(ReadTool::default())).unwrap();
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: provider.clone(),
        model: "test-model".to_owned(),
        workspace: workspace.path().to_owned(),
        skill_catalog: String::new(),
        tools,
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions {
            max_parallel_model_calls: 2,
            max_output_tokens: Some(1_024),
            compaction: CompactionOptions {
                compact_at_tokens: Some(100),
                context_window_tokens: Some(100_000),
                keep_recent_tokens: 1,
                summary_max_output_tokens: 512,
                history_search_max_matches: 10,
            },
            ..RunnerOptions::default()
        },
    });

    let result = runner
        .run(RunRequest::root("delegate a review across compaction"))
        .await
        .unwrap();
    assert_eq!(result.final_output, "parent observed terminal result");

    let requests = provider.requests.lock().unwrap();
    let post_compaction = requests
        .iter()
        .find(|request| {
            request.run_id == result.run_id
                && request_contains(request, "# Compacted state")
                && !request
                    .messages
                    .last()
                    .is_some_and(|message| message.visible_text().contains(COMPACTION_INSTRUCTION))
        })
        .unwrap();
    assert!(!request_contains(post_compaction, ACTIVE_MARKER));
    let terminal = post_compaction
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|content| match content {
            MessageContent::BackgroundTask {
                task_id,
                name,
                status,
                content,
                ..
            } if task_id == "t1" => Some((name, status, content)),
            _ => None,
        })
        .expect("terminal task notice must be included after compaction");
    assert_eq!(terminal.0, "compaction review");
    assert_eq!(terminal.1.as_deref(), Some("completed"));
    assert_eq!(terminal.2, "child completed during compaction");
}

fn tool_response(id: &str, name: &str, arguments: serde_json::Value) -> ModelResponse {
    ModelResponse::new(
        Message::assistant(vec![MessageContent::ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments: arguments.into(),
        }]),
        ModelUsage {
            input_tokens: Some(1_000),
            output_tokens: Some(10),
            ..ModelUsage::default()
        },
    )
}

fn text_response(text: &str) -> ModelResponse {
    ModelResponse::new(
        Message::text(Role::Assistant, text),
        ModelUsage {
            input_tokens: Some(10),
            output_tokens: Some(10),
            ..ModelUsage::default()
        },
    )
}

fn request_contains(request: &ModelRequest, expected: &str) -> bool {
    request.messages.iter().any(|message| {
        message.content.iter().any(|content| match content {
            MessageContent::RuntimeReminder { text } | MessageContent::Text { text } => {
                text.contains(expected)
            }
            _ => false,
        })
    })
}
