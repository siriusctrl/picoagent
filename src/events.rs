use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub run_id: String,
    pub timestamp: DateTime<Utc>,
    #[serde(flatten)]
    pub kind: RuntimeEventKind,
}

impl RuntimeEvent {
    pub fn new(run_id: impl Into<String>, kind: RuntimeEventKind) -> Self {
        Self {
            run_id: run_id.into(),
            timestamp: Utc::now(),
            kind,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEventKind {
    RunStarted {
        prompt: String,
    },
    RunResumed {
        completed_messages: usize,
    },
    ModelStarted {
        step: usize,
    },
    ModelDelta {
        text: String,
    },
    ModelReasoningDelta {
        text: String,
    },
    ModelCompleted {
        step: usize,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cached_input_tokens: Option<u64>,
        reasoning_tokens: Option<u64>,
    },
    ModelFailed {
        step: usize,
        error: String,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cached_input_tokens: Option<u64>,
        reasoning_tokens: Option<u64>,
    },
    CompactionStarted {
        state_message_ref: String,
        tokens_before: u64,
        attempt: usize,
    },
    CompactionCompleted {
        state_message_ref: String,
        covered_through_message_ref: String,
        first_kept_message_ref: String,
        attempt: usize,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cached_input_tokens: Option<u64>,
        reasoning_tokens: Option<u64>,
    },
    CompactionFailed {
        state_message_ref: String,
        /// `None` means compaction was rejected before a provider request.
        attempt: Option<usize>,
        error: String,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cached_input_tokens: Option<u64>,
        reasoning_tokens: Option<u64>,
    },
    ToolStarted {
        call_id: String,
        name: String,
    },
    ToolCompleted {
        call_id: String,
        name: String,
    },
    BackgroundTaskStarted {
        task_id: String,
        name: String,
    },
    BackgroundTaskCompleted {
        task_id: String,
        name: String,
    },
    BackgroundTaskFailed {
        task_id: String,
        name: String,
        error: String,
    },
    BackgroundTaskSentToBackground {
        task_id: String,
        name: String,
        call_id: String,
    },
    BackgroundTaskCancelled {
        task_id: String,
        name: String,
    },
    BackgroundTaskDelivered {
        task_id: String,
        output_seq: u64,
    },
    ArtifactCreated {
        call_id: String,
        path: String,
        bytes: u64,
    },
    SubagentActivityStarted {
        child_run_id: String,
        task: String,
    },
    SubagentMessageQueued {
        task_id: String,
        child_run_id: String,
        input_id: String,
        mode: String,
    },
    SubagentActivityCompleted {
        child_run_id: String,
    },
    SubagentActivityFailed {
        child_run_id: String,
        error: String,
    },
    SubagentActivityStopped {
        child_run_id: String,
    },
    SubagentClosed {
        child_run_id: String,
    },
    RunActivityCompleted {
        final_output: String,
    },
    RunActivityFailed {
        error: String,
    },
    RunCompleted {
        final_output: String,
    },
    RunFailed {
        error: String,
    },
}

#[async_trait]
pub trait EventSink: Send + Sync {
    async fn emit(&self, event: &RuntimeEvent) -> Result<()>;
}

#[derive(Default)]
pub struct NoopEventSink;

#[async_trait]
impl EventSink for NoopEventSink {
    async fn emit(&self, _event: &RuntimeEvent) -> Result<()> {
        Ok(())
    }
}

pub type SharedEventSink = Arc<dyn EventSink>;

pub struct NdjsonEventSink;

#[async_trait]
impl EventSink for NdjsonEventSink {
    async fn emit(&self, event: &RuntimeEvent) -> Result<()> {
        println!("{}", serde_json::to_string(event)?);
        Ok(())
    }
}

pub struct CompositeEventSink {
    sinks: Vec<SharedEventSink>,
}

impl CompositeEventSink {
    pub fn new(sinks: Vec<SharedEventSink>) -> Self {
        Self { sinks }
    }
}

#[async_trait]
impl EventSink for CompositeEventSink {
    async fn emit(&self, event: &RuntimeEvent) -> Result<()> {
        for sink in &self.sinks {
            sink.emit(event).await?;
        }
        Ok(())
    }
}
