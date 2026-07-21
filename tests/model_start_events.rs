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

struct ImmediateNotifyingProvider {
    entered: tokio::sync::mpsc::UnboundedSender<String>,
}

#[async_trait]
impl ModelProvider for ImmediateNotifyingProvider {
    fn name(&self) -> &str {
        "immediate-notifying"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        self.entered.send(request.run_id).unwrap();
        Ok(ModelResponse::new(
            Message::text(Role::Assistant, "done"),
            ModelUsage::default(),
        ))
    }
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
    model_lifecycle: std::sync::Mutex<Vec<(String, &'static str)>>,
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
                self.model_lifecycle
                    .lock()
                    .unwrap()
                    .push((event.run_id.clone(), "started"));
            }
            RuntimeEventKind::ModelCompleted { .. } => {
                self.model_lifecycle
                    .lock()
                    .unwrap()
                    .push((event.run_id.clone(), "completed"));
            }
            RuntimeEventKind::ModelFailed { .. } => {
                self.model_lifecycle
                    .lock()
                    .unwrap()
                    .push((event.run_id.clone(), "failed"));
            }
            _ => {}
        }
        Ok(())
    }
}

struct BlockingFirstCompletionSink {
    block_next: std::sync::atomic::AtomicBool,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl EventSink for BlockingFirstCompletionSink {
    async fn emit(&self, event: &RuntimeEvent) -> Result<()> {
        if matches!(&event.kind, RuntimeEventKind::ModelCompleted { .. })
            && self.block_next.swap(false, Ordering::SeqCst)
        {
            self.entered.notify_one();
            self.release.notified().await;
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
        model_lifecycle: std::sync::Mutex::new(Vec::new()),
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
    let lifecycle = recorder.model_lifecycle.lock().unwrap();
    let first_completed = lifecycle
        .iter()
        .position(|event| event == &(first_run_id.clone(), "completed"))
        .unwrap();
    let second_started = lifecycle
        .iter()
        .position(|event| event.0 == second_run_id && event.1 == "started")
        .unwrap();
    assert!(first_completed < second_started, "{lifecycle:?}");
}

#[tokio::test]
async fn model_holds_the_shared_slot_until_completed_is_emitted() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let completion_entered = Arc::new(tokio::sync::Notify::new());
    let completion_release = Arc::new(tokio::sync::Notify::new());
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(ImmediateNotifyingProvider {
            entered: entered_tx,
        }),
        model: "scripted".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools: ToolRegistry::default(),
        artifacts: ArtifactStore::default(),
        store,
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(BlockingFirstCompletionSink {
            block_next: std::sync::atomic::AtomicBool::new(true),
            entered: completion_entered.clone(),
            release: completion_release.clone(),
        }),
        options: RunnerOptions {
            max_parallel_model_calls: 1,
            ..RunnerOptions::default()
        },
    });

    let first = tokio::spawn({
        let runner = runner.clone();
        async move { runner.run(RunRequest::root("first")).await }
    });
    entered_rx.recv().await.unwrap();
    completion_entered.notified().await;

    let second = tokio::spawn({
        let runner = runner.clone();
        async move { runner.run(RunRequest::root("second")).await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), entered_rx.recv())
            .await
            .is_err()
    );

    completion_release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), entered_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.await.unwrap().unwrap().final_output, "done");
    assert_eq!(second.await.unwrap().unwrap().final_output, "done");
}

#[derive(Clone, Copy)]
enum FailureMode {
    Transport,
    Validation,
}

struct FailingModelProvider(FailureMode);

#[async_trait]
impl ModelProvider for FailingModelProvider {
    fn name(&self) -> &str {
        "failing-model"
    }

    async fn complete(
        &self,
        _request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        match self.0 {
            FailureMode::Transport => anyhow::bail!("intentional transport failure"),
            FailureMode::Validation => Ok(ModelResponse {
                assistant: Message::text(Role::User, "not an assistant response"),
                usage: ModelUsage::default(),
            }),
        }
    }
}

#[tokio::test]
async fn model_transport_and_validation_failures_close_the_started_request() {
    for (mode, expected) in [
        (FailureMode::Transport, "intentional transport failure"),
        (FailureMode::Validation, "assistant"),
    ] {
        let workspace = TempDir::new().unwrap();
        let store = RunDirStore::new(workspace.path());
        let runner = AgentRunner::new(AgentRunnerConfig {
            provider: Arc::new(FailingModelProvider(mode)),
            model: "scripted".to_owned(),
            workspace: workspace.path().to_path_buf(),
            skill_catalog: String::new(),
            tools: ToolRegistry::default(),
            artifacts: ArtifactStore::default(),
            store: store.clone(),
            hooks: HookPipeline::new(),
            memory: None,
            extra_events: Arc::new(picoagent::events::NoopEventSink),
            options: RunnerOptions::default(),
        });

        let error = runner.run(RunRequest::root("fail once")).await.unwrap_err();
        assert!(format!("{error:#}").contains(expected));
        let mut runs = tokio::fs::read_dir(workspace.path().join(".pico/runs"))
            .await
            .unwrap();
        let run_id = runs
            .next_entry()
            .await
            .unwrap()
            .unwrap()
            .file_name()
            .to_string_lossy()
            .into_owned();
        let events = tokio::fs::read_to_string(store.paths(&run_id).events)
            .await
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<RuntimeEvent>(line).unwrap())
            .collect::<Vec<_>>();
        let started = events
            .iter()
            .position(|event| matches!(&event.kind, RuntimeEventKind::ModelStarted { step: 1 }))
            .unwrap();
        let failed = events
            .iter()
            .position(|event| {
                matches!(
                    &event.kind,
                    RuntimeEventKind::ModelFailed { step: 1, error }
                        if error.contains(expected)
                )
            })
            .unwrap();
        let run_failed = events
            .iter()
            .position(|event| matches!(&event.kind, RuntimeEventKind::RunFailed { .. }))
            .unwrap();
        assert!(started < failed && failed < run_failed, "{events:?}");
        assert!(
            !events
                .iter()
                .any(|event| matches!(&event.kind, RuntimeEventKind::ModelCompleted { .. }))
        );
    }
}
