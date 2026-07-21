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
    events::{RuntimeEvent, RuntimeEventKind, SharedEventSink},
    storage::RunDirStore,
};

use super::runner::AgentRunner;

mod control;
mod execution;
mod lifecycle;
mod record;
mod recovery;

use record::TaskRecordStore;
pub use record::{BackgroundTaskRecord, BackgroundTaskState};
pub use recovery::RecoverableSubagent;

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
    delivered: Mutex<BTreeSet<String>>,
    task_store: TaskRecordStore,
    handles: StdMutex<BTreeMap<String, tokio::task::JoinHandle<()>>>,
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

impl TaskManager {
    pub fn new(config: TaskManagerConfig) -> Arc<Self> {
        Self::from_config(config, BTreeMap::new(), BTreeSet::new())
    }

    fn from_config(
        config: TaskManagerConfig,
        records: BTreeMap<String, BackgroundTaskRecord>,
        delivered: BTreeSet<String>,
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
        let task_id = next_task_id(&records);
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
    ) -> Result<String> {
        let mut records = self.records.lock().await;
        let task_id = next_task_id(&records);
        let record = BackgroundTaskRecord::queued_agent(
            task_id.clone(),
            name,
            child_run_id,
            prompt,
            self.remaining_delegation_depth.saturating_sub(1),
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
        self.signal_activity();
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

    pub(crate) async fn prepare_delivery(
        &self,
        records: &[BackgroundTaskRecord],
    ) -> Result<Vec<BackgroundTaskRecord>> {
        let mut prepared = Vec::with_capacity(records.len());
        for record in records {
            if !record.state.is_terminal() || record.result_metadata().artifact.is_some() {
                prepared.push(record.clone());
                continue;
            }
            let content = record.model_content();
            let context = crate::tools::ToolContext {
                run_id: self.parent_run_id.clone(),
                call_id: format!("background-{}", record.id),
                workspace: self.workspace.clone(),
            };
            let mut raw = crate::tools::RawToolOutput::text(content.clone());
            raw.is_error = record.state != BackgroundTaskState::Completed;
            let output = self.artifacts.persist_artifact(&context, raw).await?;
            let artifact = output
                .artifact
                .clone()
                .context("forced background result persistence produced no artifact")?;
            self.events
                .emit(&RuntimeEvent::new(
                    &self.parent_run_id,
                    RuntimeEventKind::ArtifactCreated {
                        call_id: context.call_id,
                        path: artifact.path.clone(),
                        bytes: artifact.bytes,
                    },
                ))
                .await?;
            let updated = self
                .update(&record.id, |stored| {
                    stored.result = Some(record::BackgroundTaskOutput {
                        content,
                        metadata: output.result_metadata(),
                    });
                })
                .await?;
            prepared.push(updated);
        }
        Ok(prepared)
    }

    async fn get(&self, task_id: &str) -> Result<BackgroundTaskRecord> {
        self.records
            .lock()
            .await
            .get(task_id)
            .cloned()
            .with_context(|| format!("unknown background task `{task_id}`"))
    }

    pub(crate) async fn find_undelivered_promotion(
        &self,
        call_id: &str,
        tool_name: &str,
        not_before: chrono::DateTime<chrono::Utc>,
    ) -> Option<BackgroundTaskRecord> {
        let delivered = self.delivered.lock().await.clone();
        self.records
            .lock()
            .await
            .values()
            .filter(|record| {
                record.origin_call_id.as_deref() == Some(call_id)
                    && record.name == tool_name
                    && record.created_at >= not_before
                    && !delivered.contains(&record.id)
            })
            .max_by(|left, right| left.created_at.cmp(&right.created_at))
            .cloned()
    }

    pub async fn wait(&self, task_ids: &[String]) -> Result<Vec<BackgroundTaskRecord>> {
        let deadline = tokio::time::Instant::now() + self.default_wait_timeout;
        let mut activity = self.activity.subscribe();
        loop {
            let records = self.select(task_ids).await?;
            if records.iter().all(|record| record.state.is_terminal()) {
                return Ok(records);
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero()
                || tokio::time::timeout(remaining, activity.changed())
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
    /// appended their terminal `BackgroundTask` messages. This marker is an
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
        let mut activity = self.activity.subscribe();
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
                || tokio::time::timeout(remaining, activity.changed())
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
        let mut activity = self.activity.subscribe();
        loop {
            let records = self.select(&[]).await?;
            if records.iter().all(|record| record.state.is_terminal()) {
                let delivered = self.delivered.lock().await.clone();
                return Ok(records
                    .into_iter()
                    .filter(|record| !delivered.contains(&record.id))
                    .collect());
            }
            activity.changed().await?;
        }
    }
}

fn next_task_id(records: &BTreeMap<String, BackgroundTaskRecord>) -> String {
    let mut number = records.len().saturating_add(1);
    loop {
        let candidate = format!("t{number}");
        if !records.contains_key(&candidate) {
            return candidate;
        }
        number = number.saturating_add(1);
    }
}

#[cfg(test)]
mod tests;
