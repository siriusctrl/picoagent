use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::{Result, bail};
use async_trait::async_trait;
use chrono::Utc;
use tempfile::TempDir;

use crate::{
    agent::CompactionOptions,
    events::{EventSink, RuntimeEvent, RuntimeEventKind, SharedEventSink},
    model::{
        Message, MessageContent, ModelProvider, ModelRequest, ModelResponse, ModelUsage, Role,
    },
    storage::{RunDirStore, RunRecord},
    trajectory::TrajectoryMessage,
};

use super::{CompactionAttempt, maybe_compact};

struct FailingSummaryProvider {
    calls: AtomicUsize,
}

struct InvalidSummaryProvider;

struct SuccessfulSummaryProvider;

#[async_trait]
impl ModelProvider for InvalidSummaryProvider {
    fn name(&self) -> &str {
        "invalid-summary"
    }

    async fn complete(
        &self,
        _request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        Ok(ModelResponse {
            assistant: Message::text(Role::User, "not an assistant message"),
            usage: ModelUsage {
                input_tokens: Some(41),
                output_tokens: Some(7),
                cached_input_tokens: Some(23),
                reasoning_tokens: Some(5),
            },
        })
    }
}

#[async_trait]
impl ModelProvider for SuccessfulSummaryProvider {
    fn name(&self) -> &str {
        "successful-summary"
    }

    async fn complete(
        &self,
        _request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        Ok(ModelResponse::new(
            Message::text(Role::Assistant, "# Compacted state\nDone"),
            ModelUsage::default(),
        ))
    }
}

#[async_trait]
impl ModelProvider for FailingSummaryProvider {
    fn name(&self) -> &str {
        "failing-summary"
    }

    async fn complete(
        &self,
        _request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        bail!("intentional summary failure")
    }
}

#[derive(Default)]
struct RecordingEventSink {
    events: Mutex<Vec<RuntimeEventKind>>,
}

struct BlockingCompletionSink {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl EventSink for BlockingCompletionSink {
    async fn emit(&self, event: &RuntimeEvent) -> Result<()> {
        if matches!(&event.kind, RuntimeEventKind::CompactionCompleted { .. }) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(())
    }
}

#[async_trait]
impl EventSink for RecordingEventSink {
    async fn emit(&self, event: &RuntimeEvent) -> Result<()> {
        self.events.lock().unwrap().push(event.kind.clone());
        Ok(())
    }
}

fn record(seq: u64, role: Role, text: String) -> TrajectoryMessage {
    TrajectoryMessage {
        message_ref: format!("m{seq}"),
        seq,
        created_at: Utc::now(),
        message: Message::new(role, vec![MessageContent::Text { text }]),
        pending_input_id: None,
        compaction: None,
    }
}

#[tokio::test]
async fn compaction_started_waits_for_the_model_slot() {
    let trajectory = vec![
        record(1, Role::User, "initial".into()),
        record(2, Role::Assistant, "old work".repeat(100)),
        record(3, Role::User, "recent".repeat(100)),
    ];
    let options = CompactionOptions {
        compact_at_tokens: Some(10),
        context_window_tokens: None,
        keep_recent_tokens: 1,
        summary_max_output_tokens: 64,
        history_search_max_matches: 7,
    };
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let provider = Arc::new(FailingSummaryProvider {
        calls: AtomicUsize::new(0),
    });
    let provider_dyn: Arc<dyn ModelProvider> = provider.clone();
    let recorder = Arc::new(RecordingEventSink::default());
    let events: SharedEventSink = recorder.clone();
    let model_slots = tokio::sync::Semaphore::new(1);
    let held_permit = model_slots.acquire().await.unwrap();
    let tools = Vec::new();

    let attempt = maybe_compact(CompactionAttempt {
        provider: &provider_dyn,
        model: "test-model",
        run_id: "run",
        system: "system",
        tools: &tools,
        trajectory: &trajectory,
        tokens_before: 1_000,
        options: &options,
        store: &store,
        events: &events,
        model_slots: &model_slots,
        stream_idle_timeout_seconds: 30,
        request_deadline_seconds: 30,
    });
    tokio::pin!(attempt);

    tokio::select! {
        biased;
        _ = &mut attempt => panic!("compaction completed while its model slot was held"),
        _ = tokio::task::yield_now() => {}
    }
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert!(recorder.events.lock().unwrap().is_empty());

    drop(held_permit);
    assert!(attempt.await.unwrap().is_none());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let events = recorder.events.lock().unwrap();
    assert!(matches!(
        events[0],
        RuntimeEventKind::CompactionStarted { attempt: 1, .. }
    ));
    assert!(matches!(
        events[1],
        RuntimeEventKind::CompactionFailed {
            attempt: Some(1),
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: None,
            reasoning_tokens: None,
            ..
        }
    ));
}

#[tokio::test]
async fn compaction_validation_failure_closes_the_attempt_with_reported_usage() {
    let trajectory = vec![
        record(1, Role::User, "initial".into()),
        record(2, Role::Assistant, "old work".repeat(100)),
        record(3, Role::User, "recent".repeat(100)),
    ];
    let options = CompactionOptions {
        compact_at_tokens: Some(10),
        context_window_tokens: None,
        keep_recent_tokens: 1,
        summary_max_output_tokens: 64,
        history_search_max_matches: 7,
    };
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let provider: Arc<dyn ModelProvider> = Arc::new(InvalidSummaryProvider);
    let recorder = Arc::new(RecordingEventSink::default());
    let events: SharedEventSink = recorder.clone();
    let model_slots = tokio::sync::Semaphore::new(1);
    let tools = Vec::new();

    let result = maybe_compact(CompactionAttempt {
        provider: &provider,
        model: "test-model",
        run_id: "run",
        system: "system",
        tools: &tools,
        trajectory: &trajectory,
        tokens_before: 1_000,
        options: &options,
        store: &store,
        events: &events,
        model_slots: &model_slots,
        stream_idle_timeout_seconds: 30,
        request_deadline_seconds: 30,
    })
    .await
    .unwrap();

    assert!(result.is_none());
    assert_eq!(model_slots.available_permits(), 1);
    let events = recorder.events.lock().unwrap();
    assert!(matches!(
        events.as_slice(),
        [
            RuntimeEventKind::CompactionStarted { attempt: 1, .. },
            RuntimeEventKind::CompactionFailed {
                attempt: Some(1),
                input_tokens: Some(41),
                output_tokens: Some(7),
                cached_input_tokens: Some(23),
                reasoning_tokens: Some(5),
                ..
            }
        ]
    ));
}

#[tokio::test]
async fn compaction_holds_the_model_slot_until_completed_is_emitted() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let provider: Arc<dyn ModelProvider> = Arc::new(SuccessfulSummaryProvider);
    store
        .create_run(&RunRecord::new(
            "run",
            "root",
            "initial",
            provider.name(),
            "test-model",
            workspace.path().to_owned(),
            None,
        ))
        .await
        .unwrap();
    for message in [
        Message::text(Role::User, "initial"),
        Message::text(Role::Assistant, "old work".repeat(100)),
        Message::text(Role::User, "recent".repeat(100)),
    ] {
        store.append_message("run", &message).await.unwrap();
    }
    let trajectory = store.load_trajectory("run").await.unwrap();
    let options = CompactionOptions {
        compact_at_tokens: Some(10),
        context_window_tokens: None,
        keep_recent_tokens: 1,
        summary_max_output_tokens: 64,
        history_search_max_matches: 7,
    };
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let events: SharedEventSink = Arc::new(BlockingCompletionSink {
        entered: entered.clone(),
        release: release.clone(),
    });
    let model_slots = tokio::sync::Semaphore::new(1);
    let tools = Vec::new();
    let attempt = maybe_compact(CompactionAttempt {
        provider: &provider,
        model: "test-model",
        run_id: "run",
        system: "system",
        tools: &tools,
        trajectory: &trajectory,
        tokens_before: 1_000,
        options: &options,
        store: &store,
        events: &events,
        model_slots: &model_slots,
        stream_idle_timeout_seconds: 30,
        request_deadline_seconds: 30,
    });
    tokio::pin!(attempt);

    tokio::select! {
        _ = entered.notified() => {}
        _ = &mut attempt => panic!("compaction returned before completed event was released"),
    }
    assert!(model_slots.try_acquire().is_err());
    release.notify_one();
    assert!(attempt.await.unwrap().is_some());
    assert_eq!(model_slots.available_permits(), 1);
}
