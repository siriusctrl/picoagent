use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::sync::{Mutex, Semaphore, watch};

use crate::{
    artifact::{ArtifactStore, ToolOutput},
    events::SharedEventSink,
    storage::RunDirStore,
};

use super::runner::AgentRunner;

mod control;
mod coordination;
mod execution;
mod lifecycle;
mod record;
mod recovery;

pub use control::TaskSendMode;
use record::TaskRecordStore;
pub use record::{
    BackgroundTaskOutput, BackgroundTaskOutputStatus, BackgroundTaskRecord, BackgroundTaskState,
    PendingTaskInput,
};

pub struct TaskManager {
    runner: Arc<AgentRunner>,
    artifacts: ArtifactStore,
    store: RunDirStore,
    workspace: PathBuf,
    parent_run_id: String,
    parent_depth: usize,
    remaining_delegation_depth: usize,
    events: SharedEventSink,
    records: Mutex<BTreeMap<String, BackgroundTaskRecord>>,
    /// Includes orphan task files which are intentionally hidden after
    /// recovery, so their readable `t<N>` ids are never reused.
    reserved_task_ids: BTreeSet<String>,
    delivered: Mutex<BTreeMap<String, u64>>,
    task_store: TaskRecordStore,
    handles: StdMutex<BTreeMap<String, TrackedTask>>,
    activity: watch::Sender<u64>,
    subagent_slots: Arc<Semaphore>,
    default_wait_timeout: Duration,
}

pub struct TaskManagerConfig {
    pub runner: Arc<AgentRunner>,
    pub artifacts: ArtifactStore,
    pub store: RunDirStore,
    pub workspace: PathBuf,
    pub parent_run_id: String,
    pub parent_depth: usize,
    pub remaining_delegation_depth: usize,
    pub events: SharedEventSink,
    pub max_parallel_subagents: usize,
    pub wait_timeout_seconds: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct TaskOutputNotice {
    pub task_id: String,
    pub name: String,
    pub output: BackgroundTaskOutput,
}

pub(crate) enum PendingTaskBoundary {
    Ready(Vec<TaskOutputNotice>),
    Active,
    None,
}

struct TrackedTask {
    handle: tokio::task::JoinHandle<()>,
    cleanup_done: Option<tokio::sync::oneshot::Receiver<()>>,
}

impl TrackedTask {
    fn abort(&self) {
        self.handle.abort();
    }

    async fn wait(self) {
        let _ = self.handle.await;
        if let Some(cleanup_done) = self.cleanup_done {
            let _ = cleanup_done.await;
        }
    }
}

impl TaskManager {
    pub fn new(config: TaskManagerConfig) -> Arc<Self> {
        Self::from_config(config, BTreeMap::new(), BTreeMap::new(), BTreeSet::new())
    }

    fn from_config(
        config: TaskManagerConfig,
        records: BTreeMap<String, BackgroundTaskRecord>,
        delivered: BTreeMap<String, u64>,
        reserved_task_ids: BTreeSet<String>,
    ) -> Arc<Self> {
        let (activity, _) = watch::channel(0);
        let task_store = TaskRecordStore::new(
            config
                .store
                .paths(&config.parent_run_id)
                .directory
                .join("tasks"),
        );
        Arc::new(Self {
            runner: config.runner,
            artifacts: config.artifacts,
            store: config.store,
            workspace: config.workspace,
            parent_run_id: config.parent_run_id,
            parent_depth: config.parent_depth,
            remaining_delegation_depth: config.remaining_delegation_depth,
            events: config.events,
            records: Mutex::new(records),
            reserved_task_ids,
            delivered: Mutex::new(delivered),
            task_store,
            handles: StdMutex::new(BTreeMap::new()),
            activity,
            subagent_slots: Arc::new(Semaphore::new(config.max_parallel_subagents.max(1))),
            default_wait_timeout: Duration::from_secs(config.wait_timeout_seconds.max(1)),
        })
    }

    async fn create_tool_task(
        self: &Arc<Self>,
        name: String,
        origin_call_id: String,
    ) -> Result<String> {
        let mut records = self.records.lock().await;
        let task_id = next_task_id(&records, &self.reserved_task_ids);
        let record = BackgroundTaskRecord::queued_tool(task_id.clone(), name, origin_call_id);
        self.persist(&record).await?;
        records.insert(task_id.clone(), record);
        Ok(task_id)
    }

    async fn create_agent_task(
        self: &Arc<Self>,
        name: String,
        child_run_id: String,
        prompt: String,
        origin_call_id: String,
    ) -> Result<String> {
        let mut records = self.records.lock().await;
        let task_id = next_task_id(&records, &self.reserved_task_ids);
        let record = BackgroundTaskRecord::queued_agent_with_origin(
            task_id.clone(),
            name,
            child_run_id,
            prompt,
            self.remaining_delegation_depth.saturating_sub(1),
            origin_call_id,
        );
        self.persist(&record).await?;
        records.insert(task_id.clone(), record);
        Ok(task_id)
    }

    async fn set_running(&self, task_id: &str) -> Result<BackgroundTaskRecord> {
        self.update(task_id, |record| {
            if !record.state.is_terminal() {
                record.state = BackgroundTaskState::Running;
            }
        })
        .await
    }

    async fn complete(&self, task_id: &str, output: ToolOutput) -> Result<BackgroundTaskRecord> {
        self.update(task_id, |record| {
            if !record.state.is_terminal() {
                record.state = BackgroundTaskState::Completed;
                let seq = record.next_output_seq();
                record.outputs.push(BackgroundTaskOutput {
                    seq,
                    status: BackgroundTaskOutputStatus::Completed,
                    content: output.model_content(),
                    metadata: output.result_metadata(),
                });
            }
        })
        .await
    }

    async fn fail(&self, task_id: &str, error: String) -> Result<BackgroundTaskRecord> {
        self.update(task_id, |record| {
            if !record.state.is_terminal() {
                record.state = BackgroundTaskState::Failed;
                let seq = record.next_output_seq();
                record.outputs.push(BackgroundTaskOutput {
                    seq,
                    status: BackgroundTaskOutputStatus::Failed,
                    content: format!("background task failed: {error}"),
                    metadata: crate::artifact::ResultMetadata::empty(),
                });
            }
        })
        .await
    }

    async fn fail_in_memory(&self, task_id: &str, error: String) {
        let mut records = self.records.lock().await;
        if let Some(record) = records.get_mut(task_id)
            && !record.state.is_terminal()
        {
            record.state = BackgroundTaskState::Failed;
            let seq = record.next_output_seq();
            record.outputs.push(BackgroundTaskOutput {
                seq,
                status: BackgroundTaskOutputStatus::Failed,
                content: format!("background task failed: {error}"),
                metadata: crate::artifact::ResultMetadata::empty(),
            });
        }
        drop(records);
        self.signal_activity();
    }

    async fn interrupt(&self, task_id: &str, error: String) -> Result<BackgroundTaskRecord> {
        self.update(task_id, |record| {
            if !record.state.is_terminal() {
                record.state = BackgroundTaskState::Interrupted;
                let seq = record.next_output_seq();
                record.outputs.push(BackgroundTaskOutput {
                    seq,
                    status: BackgroundTaskOutputStatus::Interrupted,
                    content: format!(
                        "background task was interrupted; its side effects are unknown: {error}"
                    ),
                    metadata: crate::artifact::ResultMetadata::empty(),
                });
            }
        })
        .await
    }

    async fn cancel(&self, task_id: &str, reason: String) -> Result<BackgroundTaskRecord> {
        self.update(task_id, |record| {
            if !record.state.is_terminal() {
                record.state = BackgroundTaskState::Cancelled;
                let seq = record.next_output_seq();
                record.outputs.push(BackgroundTaskOutput {
                    seq,
                    status: BackgroundTaskOutputStatus::Cancelled,
                    content: format!("background task was cancelled: {reason}"),
                    metadata: crate::artifact::ResultMetadata::empty(),
                });
            }
        })
        .await
    }

    async fn update(
        &self,
        task_id: &str,
        update: impl FnOnce(&mut BackgroundTaskRecord),
    ) -> Result<BackgroundTaskRecord> {
        let mut records = self.records.lock().await;
        let mut record = records
            .get(task_id)
            .cloned()
            .with_context(|| format!("unknown background task `{task_id}`"))?;
        update(&mut record);
        self.persist(&record).await?;
        records.insert(task_id.to_owned(), record.clone());
        drop(records);
        self.signal_activity();
        Ok(record)
    }

    fn signal_activity(&self) {
        self.activity
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }

    async fn persist(&self, record: &BackgroundTaskRecord) -> Result<()> {
        self.task_store.write(record).await
    }

    async fn persist_output(
        &self,
        context: &crate::tools::ToolContext,
        output: crate::tools::RawToolOutput,
    ) -> Result<ToolOutput> {
        self.artifacts.persist_output(context, output).await
    }

    async fn get(&self, task_id: &str) -> Result<BackgroundTaskRecord> {
        self.records
            .lock()
            .await
            .get(task_id)
            .cloned()
            .with_context(|| format!("unknown background task `{task_id}`"))
    }

    fn track(&self, task_id: String, handle: tokio::task::JoinHandle<()>) {
        self.track_with_cleanup(task_id, handle, None);
    }

    fn track_agent(
        &self,
        task_id: String,
        handle: tokio::task::JoinHandle<()>,
        cleanup_done: tokio::sync::oneshot::Receiver<()>,
    ) {
        self.track_with_cleanup(task_id, handle, Some(cleanup_done));
    }

    fn track_with_cleanup(
        &self,
        task_id: String,
        handle: tokio::task::JoinHandle<()>,
        cleanup_done: Option<tokio::sync::oneshot::Receiver<()>>,
    ) {
        let mut handles = self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        handles.retain(|_, tracked| !tracked.handle.is_finished());
        handles.insert(
            task_id,
            TrackedTask {
                handle,
                cleanup_done,
            },
        );
    }

    fn take_handles(&self) -> BTreeMap<String, TrackedTask> {
        std::mem::take(
            &mut *self
                .handles
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    fn take_handle(&self, task_id: &str) -> Option<TrackedTask> {
        self.handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(task_id)
    }
}

fn next_task_id(
    records: &BTreeMap<String, BackgroundTaskRecord>,
    reserved_task_ids: &BTreeSet<String>,
) -> String {
    let mut number = records.len().saturating_add(1);
    loop {
        let candidate = format!("t{number}");
        if !records.contains_key(&candidate) && !reserved_task_ids.contains(&candidate) {
            return candidate;
        }
        number = number.saturating_add(1);
    }
}

#[cfg(test)]
mod tests;
