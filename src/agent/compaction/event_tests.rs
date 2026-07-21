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
    model::{Message, MessageContent, ModelProvider, ModelRequest, ModelResponse, Role},
    storage::RunDirStore,
    trajectory::TrajectoryMessage,
};

use super::{CompactionAttempt, maybe_compact};

struct FailingSummaryProvider {
    calls: AtomicUsize,
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
        message: Message {
            role,
            content: vec![MessageContent::Text { text }],
        },
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
        RuntimeEventKind::CompactionStarted { .. }
    ));
    assert!(matches!(
        events[1],
        RuntimeEventKind::CompactionFailed { .. }
    ));
}
