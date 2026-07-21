use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use async_trait::async_trait;
use picoagent::{
    agent::runner::{AgentRunner, AgentRunnerConfig, RunRequest, RunnerOptions},
    artifact::ArtifactStore,
    events::{EventSink, RuntimeEvent, RuntimeEventKind, SharedEventSink},
    hooks::HookPipeline,
    model::{Message, ModelProvider, ModelRequest, ModelResponse, ModelUsage, Role},
    storage::RunDirStore,
    tools::ToolRegistry,
};
use tempfile::TempDir;

struct BlockingFirstModelProvider {
    calls: AtomicUsize,
    entered: tokio::sync::mpsc::UnboundedSender<String>,
    release_first: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl ModelProvider for BlockingFirstModelProvider {
    fn name(&self) -> &str {
        "blocking-first-model"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.send(request.run_id).unwrap();
        if call == 0 {
            self.release_first.notified().await;
        }
        Ok(ModelResponse::new(
            Message::text(Role::Assistant, "done"),
            ModelUsage::default(),
        ))
    }
}

struct ModelStartRecorder {
    run_started: tokio::sync::mpsc::UnboundedSender<(String, String)>,
    model_started: std::sync::Mutex<Vec<String>>,
}

impl ModelStartRecorder {
    fn saw_model_start(&self, run_id: &str) -> bool {
        self.model_started
            .lock()
            .unwrap()
            .iter()
            .any(|started| started == run_id)
    }
}

#[async_trait]
impl EventSink for ModelStartRecorder {
    async fn emit(&self, event: &RuntimeEvent) -> Result<()> {
        match &event.kind {
            RuntimeEventKind::RunStarted { prompt } => {
                self.run_started
                    .send((prompt.clone(), event.run_id.clone()))
                    .unwrap();
            }
            RuntimeEventKind::ModelStarted { .. } => {
                self.model_started
                    .lock()
                    .unwrap()
                    .push(event.run_id.clone());
            }
            _ => {}
        }
        Ok(())
    }
}

#[tokio::test]
async fn model_started_is_emitted_only_after_the_shared_slot_is_acquired() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let (run_started_tx, mut run_started_rx) = tokio::sync::mpsc::unbounded_channel();
    let release_first = Arc::new(tokio::sync::Notify::new());
    let recorder = Arc::new(ModelStartRecorder {
        run_started: run_started_tx,
        model_started: std::sync::Mutex::new(Vec::new()),
    });
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(BlockingFirstModelProvider {
            calls: AtomicUsize::new(0),
            entered: entered_tx,
            release_first: release_first.clone(),
        }),
        model: "scripted".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools: ToolRegistry::default(),
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: recorder.clone(),
        options: RunnerOptions {
            max_parallel_model_calls: 1,
            ..RunnerOptions::default()
        },
    });

    let first = tokio::spawn({
        let runner = runner.clone();
        async move { runner.run(RunRequest::root("first")).await }
    });
    let first_run_id = tokio::time::timeout(Duration::from_secs(2), entered_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(recorder.saw_model_start(&first_run_id));

    let second = tokio::spawn({
        let runner = runner.clone();
        async move { runner.run(RunRequest::root("second")).await }
    });
    let second_run_id = loop {
        let (prompt, run_id) = tokio::time::timeout(Duration::from_secs(2), run_started_rx.recv())
            .await
            .unwrap()
            .unwrap();
        if prompt == "second" {
            break run_id;
        }
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !store
                .load_run(&second_run_id)
                .await
                .unwrap()
                .tool_schema_sha256
                .is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let premature_start = tokio::time::timeout(Duration::from_millis(100), async {
        loop {
            if recorder.saw_model_start(&second_run_id) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(premature_start.is_err());
    let durable_events = tokio::fs::read_to_string(store.paths(&second_run_id).events)
        .await
        .unwrap();
    assert!(!durable_events.contains("\"type\":\"model_started\""));

    release_first.notify_one();
    let entered_second = tokio::time::timeout(Duration::from_secs(2), entered_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entered_second, second_run_id);
    assert!(recorder.saw_model_start(&second_run_id));
    let durable_events = tokio::fs::read_to_string(store.paths(&second_run_id).events)
        .await
        .unwrap();
    assert!(durable_events.contains("\"type\":\"model_started\""));

    assert_eq!(first.await.unwrap().unwrap().final_output, "done");
    assert_eq!(second.await.unwrap().unwrap().final_output, "done");
}
