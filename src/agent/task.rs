use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::sync::{Mutex, Notify, Semaphore};
use ulid::Ulid;

use crate::{
    artifact::{ArtifactStore, ToolOutput},
    events::{RuntimeEvent, RuntimeEventKind, SharedEventSink},
    hooks::HookPipeline,
    storage::RunDirStore,
    tools::ToolRegistry,
};

use super::{runner::AgentRunner, tool_execution::ToolExecutor};

mod control;
mod execution;
mod lifecycle;
mod record;
mod recovery;
mod tools;

use record::TaskRecordStore;
pub use record::{BackgroundTaskRecord, BackgroundTaskState};
pub use recovery::RecoverableSubagent;
pub use tools::{SpawnTool, TaskTool};

pub struct TaskManager {
    runner: Arc<AgentRunner>,
    tools: ToolRegistry,
    artifacts: ArtifactStore,
    preview_budget: Arc<Mutex<usize>>,
    store: RunDirStore,
    workspace: PathBuf,
    parent_run_id: String,
    parent_depth: usize,
    child_can_delegate: bool,
    events: SharedEventSink,
    hooks: HookPipeline,
    records: Mutex<BTreeMap<String, BackgroundTaskRecord>>,
    delivered: Mutex<BTreeSet<String>>,
    task_store: TaskRecordStore,
    handles: StdMutex<BTreeMap<String, tokio::task::JoinHandle<()>>>,
    notify: Notify,
    slots: Arc<Semaphore>,
    default_wait_timeout: Duration,
}

pub struct TaskManagerConfig {
    pub runner: Arc<AgentRunner>,
    pub tools: ToolRegistry,
    pub artifacts: ArtifactStore,
    pub preview_budget: Arc<Mutex<usize>>,
    pub store: RunDirStore,
    pub workspace: PathBuf,
    pub parent_run_id: String,
    pub parent_depth: usize,
    pub child_can_delegate: bool,
    pub events: SharedEventSink,
    pub hooks: HookPipeline,
    pub max_parallel_tasks: usize,
    pub wait_timeout_seconds: u64,
}

impl TaskManager {
    pub fn new(config: TaskManagerConfig) -> Arc<Self> {
        Self::from_config(config, BTreeMap::new(), BTreeSet::new())
    }

    fn from_config(
        config: TaskManagerConfig,
        records: BTreeMap<String, BackgroundTaskRecord>,
        delivered: BTreeSet<String>,
    ) -> Arc<Self> {
        let task_store = TaskRecordStore::new(
            config
                .store
                .paths(&config.parent_run_id)
                .directory
                .join("tasks"),
        );
        Arc::new(Self {
            runner: config.runner,
            tools: config.tools,
            artifacts: config.artifacts,
            preview_budget: config.preview_budget,
            store: config.store,
            workspace: config.workspace,
            parent_run_id: config.parent_run_id,
            parent_depth: config.parent_depth,
            child_can_delegate: config.child_can_delegate,
            events: config.events,
            hooks: config.hooks,
            records: Mutex::new(records),
            delivered: Mutex::new(delivered),
            task_store,
            handles: StdMutex::new(BTreeMap::new()),
            notify: Notify::new(),
            slots: Arc::new(Semaphore::new(config.max_parallel_tasks.max(1))),
            default_wait_timeout: Duration::from_secs(config.wait_timeout_seconds.max(1)),
        })
    }

    async fn create_tool_task(self: &Arc<Self>, name: String) -> Result<String> {
        let task_id = Ulid::new().to_string();
        let record = BackgroundTaskRecord::queued_tool(task_id.clone(), name);
        let mut records = self.records.lock().await;
        self.persist(&record).await?;
        records.insert(task_id.clone(), record);
        Ok(task_id)
    }

    async fn create_agent_task(
        self: &Arc<Self>,
        profile: String,
        child_run_id: String,
        prompt: String,
    ) -> Result<String> {
        let task_id = Ulid::new().to_string();
        let record =
            BackgroundTaskRecord::queued_agent(task_id.clone(), profile, child_run_id, prompt);
        let mut records = self.records.lock().await;
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
        let result = record::BackgroundTaskOutput {
            content: output.model_content(),
            metadata: output.result_metadata(),
        };
        self.update(task_id, |record| {
            if !record.state.is_terminal() {
                record.state = BackgroundTaskState::Completed;
                record.result = Some(result);
            }
        })
        .await
    }

    async fn fail(&self, task_id: &str, error: String) -> Result<BackgroundTaskRecord> {
        self.update(task_id, |record| {
            if !record.state.is_terminal() {
                record.state = BackgroundTaskState::Failed;
                record.error = Some(error);
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
            record.error = Some(error);
        }
        drop(records);
        self.notify.notify_one();
    }

    async fn interrupt(&self, task_id: &str, error: String) -> Result<BackgroundTaskRecord> {
        self.update(task_id, |record| {
            if !record.state.is_terminal() {
                record.state = BackgroundTaskState::Interrupted;
                record.error = Some(error);
            }
        })
        .await
    }

    async fn cancel(&self, task_id: &str, reason: String) -> Result<BackgroundTaskRecord> {
        self.update(task_id, |record| {
            if !record.state.is_terminal() {
                record.state = BackgroundTaskState::Cancelled;
                record.error = Some(reason);
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
        // `notify_one` retains a permit when completion races with a waiter
        // between its state check and `.notified().await`.
        self.notify.notify_one();
        Ok(record)
    }

    async fn persist(&self, record: &BackgroundTaskRecord) -> Result<()> {
        self.task_store.write(record).await
    }

    fn tool_executor(&self) -> ToolExecutor<'_> {
        ToolExecutor::new(
            &self.tools,
            &self.hooks,
            &self.artifacts,
            &self.preview_budget,
            &self.events,
            &self.workspace,
            &self.parent_run_id,
        )
    }

    async fn persist_output(
        &self,
        context: &crate::tools::ToolContext,
        output: crate::tools::RawToolOutput,
    ) -> Result<ToolOutput> {
        self.tool_executor().persist_output(context, output).await
    }

    async fn get(&self, task_id: &str) -> Result<BackgroundTaskRecord> {
        self.records
            .lock()
            .await
            .get(task_id)
            .cloned()
            .with_context(|| format!("unknown background task `{task_id}`"))
    }

    pub async fn wait(&self, task_ids: &[String]) -> Result<Vec<BackgroundTaskRecord>> {
        let deadline = tokio::time::Instant::now() + self.default_wait_timeout;
        loop {
            let records = self.select(task_ids).await?;
            if records.iter().all(|record| record.state.is_terminal()) {
                return Ok(records);
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero()
                || tokio::time::timeout(remaining, self.notify.notified())
                    .await
                    .is_err()
            {
                return Ok(records);
            }
        }
    }

    pub async fn status(&self, task_ids: &[String]) -> Result<Vec<BackgroundTaskRecord>> {
        self.select(task_ids).await
    }

    async fn select(&self, task_ids: &[String]) -> Result<Vec<BackgroundTaskRecord>> {
        let records = self.records.lock().await;
        if task_ids.is_empty() {
            return Ok(records.values().cloned().collect());
        }
        task_ids
            .iter()
            .map(|task_id| {
                records
                    .get(task_id)
                    .cloned()
                    .with_context(|| format!("unknown background task `{task_id}`"))
            })
            .collect()
    }

    /// Mark terminal records delivered only after the caller has durably
    /// appended their `BackgroundTaskResult` messages. This marker is an
    /// in-memory fast path; recovery derives truth from the parent transcript.
    pub async fn mark_delivered(&self, records: &[BackgroundTaskRecord]) -> Result<()> {
        let mut delivered = self.delivered.lock().await;
        for record in records.iter().filter(|record| record.state.is_terminal()) {
            if delivered.insert(record.id.clone()) {
                self.events
                    .emit(&RuntimeEvent::new(
                        &self.parent_run_id,
                        RuntimeEventKind::BackgroundTaskDelivered {
                            task_id: record.id.clone(),
                        },
                    ))
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn drain_completed(&self) -> Result<Vec<BackgroundTaskRecord>> {
        let delivered = self.delivered.lock().await.clone();
        let records = self.select(&[]).await?;
        Ok(records
            .into_iter()
            .filter(|record| record.state.is_terminal() && !delivered.contains(&record.id))
            .collect())
    }

    /// Return anything the model must see before it can finish: first unseen
    /// terminal results, otherwise the currently active tasks after one
    /// bounded wait interval. The pause prevents a fast final-answer loop from
    /// filling the trajectory with duplicate running snapshots.
    pub async fn pending_before_finish(&self) -> Result<Vec<BackgroundTaskRecord>> {
        let deadline = tokio::time::Instant::now() + self.default_wait_timeout;
        loop {
            let records = self.select(&[]).await?;
            let delivered = self.delivered.lock().await.clone();
            let ready = records
                .iter()
                .filter(|record| record.state.is_terminal() && !delivered.contains(&record.id))
                .cloned()
                .collect::<Vec<_>>();
            if !ready.is_empty() {
                return Ok(ready);
            }
            let active = records
                .into_iter()
                .filter(|record| !record.state.is_terminal())
                .collect::<Vec<_>>();
            if active.is_empty() {
                return Ok(Vec::new());
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero()
                || tokio::time::timeout(remaining, self.notify.notified())
                    .await
                    .is_err()
            {
                return Ok(active);
            }
        }
    }

    fn track(&self, task_id: String, handle: tokio::task::JoinHandle<()>) {
        let mut handles = self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        handles.retain(|_, handle| !handle.is_finished());
        handles.insert(task_id, handle);
    }

    fn take_handles(&self) -> BTreeMap<String, tokio::task::JoinHandle<()>> {
        std::mem::take(
            &mut *self
                .handles
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    fn take_handle(&self, task_id: &str) -> Option<tokio::task::JoinHandle<()>> {
        self.handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(task_id)
    }

    pub async fn wait_all(&self) -> Result<Vec<BackgroundTaskRecord>> {
        loop {
            let records = self.select(&[]).await?;
            if records.iter().all(|record| record.state.is_terminal()) {
                let delivered = self.delivered.lock().await.clone();
                return Ok(records
                    .into_iter()
                    .filter(|record| !delivered.contains(&record.id))
                    .collect());
            }
            self.notify.notified().await;
        }
    }
}

#[cfg(test)]
mod tests;
