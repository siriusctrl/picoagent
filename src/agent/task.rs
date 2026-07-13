use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use chrono::Utc;
use tokio::sync::{Mutex, Notify, Semaphore};
use ulid::Ulid;

use crate::{
    artifact::{ArtifactStore, ToolOutput},
    events::{RuntimeEvent, RuntimeEventKind, SharedEventSink},
    hooks::HookPipeline,
    storage::RunDirStore,
    tools::ToolRegistry,
};

use super::runner::AgentRunner;

mod execution;
mod lifecycle;
mod record;
mod tools;

pub use record::{BackgroundTaskRecord, BackgroundTaskState};
pub use tools::{SpawnTool, WaitTool};

pub struct TaskManager {
    runner: Arc<AgentRunner>,
    tools: ToolRegistry,
    artifacts: ArtifactStore,
    preview_budget: Arc<Mutex<usize>>,
    store: RunDirStore,
    workspace: PathBuf,
    parent_run_id: String,
    parent_depth: usize,
    events: SharedEventSink,
    hooks: HookPipeline,
    records: Mutex<BTreeMap<String, BackgroundTaskRecord>>,
    handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    notify: Notify,
    slots: Arc<Semaphore>,
    default_execution_timeout: Duration,
    default_wait_timeout: Duration,
    max_execution_timeout: Duration,
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
    pub events: SharedEventSink,
    pub hooks: HookPipeline,
    pub max_parallel_tasks: usize,
    pub default_execution_timeout_seconds: u64,
    pub default_wait_timeout_seconds: u64,
    pub max_execution_timeout_seconds: u64,
}

impl TaskManager {
    pub fn new(config: TaskManagerConfig) -> Arc<Self> {
        Arc::new(Self {
            runner: config.runner,
            tools: config.tools,
            artifacts: config.artifacts,
            preview_budget: config.preview_budget,
            store: config.store,
            workspace: config.workspace,
            parent_run_id: config.parent_run_id,
            parent_depth: config.parent_depth,
            events: config.events,
            hooks: config.hooks,
            records: Mutex::new(BTreeMap::new()),
            handles: Mutex::new(Vec::new()),
            notify: Notify::new(),
            slots: Arc::new(Semaphore::new(config.max_parallel_tasks.max(1))),
            default_execution_timeout: Duration::from_secs(
                config.default_execution_timeout_seconds.max(1),
            ),
            default_wait_timeout: Duration::from_secs(config.default_wait_timeout_seconds.max(1)),
            max_execution_timeout: Duration::from_secs(config.max_execution_timeout_seconds.max(1)),
        })
    }

    async fn create_task(
        self: &Arc<Self>,
        kind: &str,
        name: String,
        child_run_id: Option<String>,
    ) -> Result<String> {
        let task_id = Ulid::new().to_string();
        let now = Utc::now();
        let record = BackgroundTaskRecord {
            version: 1,
            id: task_id.clone(),
            kind: kind.to_owned(),
            name,
            state: BackgroundTaskState::Queued,
            delivered: false,
            result: None,
            error: None,
            child_run_id,
            created_at: now,
            updated_at: now,
        };
        let mut records = self.records.lock().await;
        self.persist(&record).await?;
        records.insert(task_id.clone(), record);
        Ok(task_id)
    }

    async fn set_running(&self, task_id: &str) -> Result<BackgroundTaskRecord> {
        self.update(task_id, |record| {
            record.state = BackgroundTaskState::Running
        })
        .await
    }

    async fn complete(&self, task_id: &str, result: String) -> Result<BackgroundTaskRecord> {
        self.update(task_id, |record| {
            record.state = BackgroundTaskState::Completed;
            record.result = Some(result);
        })
        .await
    }

    async fn fail(&self, task_id: &str, error: String) -> Result<BackgroundTaskRecord> {
        self.update(task_id, |record| {
            record.state = BackgroundTaskState::Failed;
            record.error = Some(error);
        })
        .await
    }

    async fn fail_in_memory(&self, task_id: &str, error: String) {
        let mut records = self.records.lock().await;
        if let Some(record) = records.get_mut(task_id) {
            record.state = BackgroundTaskState::Failed;
            record.error = Some(error);
            record.updated_at = Utc::now();
        }
        drop(records);
        self.notify.notify_one();
    }

    async fn time_out(&self, task_id: &str) -> Result<BackgroundTaskRecord> {
        self.update(task_id, |record| {
            record.state = BackgroundTaskState::TimedOut
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
        record.updated_at = Utc::now();
        self.persist(&record).await?;
        records.insert(task_id.to_owned(), record.clone());
        drop(records);
        // `notify_one` retains a permit when completion races with a waiter
        // between its state check and `.notified().await`.
        self.notify.notify_one();
        Ok(record)
    }

    async fn persist(&self, record: &BackgroundTaskRecord) -> Result<()> {
        let directory = self
            .store
            .paths(&self.parent_run_id)
            .directory
            .join("tasks");
        tokio::fs::create_dir_all(&directory).await?;
        let path = directory.join(format!("{}.json", record.id));
        let temporary = directory.join(format!("{}.json.tmp", record.id));
        tokio::fs::write(&temporary, serde_json::to_vec_pretty(record)?).await?;
        tokio::fs::rename(&temporary, &path).await?;
        Ok(())
    }

    fn execution_timeout(&self, requested_seconds: Option<u64>) -> Duration {
        requested_seconds
            .map(|seconds| Duration::from_secs(seconds.max(1)))
            .unwrap_or(self.default_execution_timeout)
            .min(self.max_execution_timeout)
    }

    async fn persist_output(
        &self,
        context: &crate::tools::ToolContext,
        output: crate::tools::RawToolOutput,
    ) -> Result<ToolOutput> {
        let mut budget = self.preview_budget.lock().await;
        let output = self
            .artifacts
            .persist_output_with_budget(context, output, *budget)
            .await?;
        *budget = budget.saturating_sub(output.preview.len());
        Ok(output)
    }

    async fn get(&self, task_id: &str) -> Result<BackgroundTaskRecord> {
        self.records
            .lock()
            .await
            .get(task_id)
            .cloned()
            .with_context(|| format!("unknown background task `{task_id}`"))
    }

    pub async fn wait(
        &self,
        task_ids: &[String],
        timeout_seconds: Option<u64>,
    ) -> Result<Vec<BackgroundTaskRecord>> {
        let timeout = Duration::from_secs(
            timeout_seconds
                .unwrap_or(self.default_wait_timeout.as_secs())
                .max(1),
        );
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let records = self.select(task_ids).await?;
            if records.iter().all(|record| record.state.is_terminal()) {
                return self.deliver(records).await;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero()
                || tokio::time::timeout(remaining, self.notify.notified())
                    .await
                    .is_err()
            {
                return self.deliver(records).await;
            }
        }
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

    async fn deliver(
        &self,
        records: Vec<BackgroundTaskRecord>,
    ) -> Result<Vec<BackgroundTaskRecord>> {
        if records.is_empty() {
            return Ok(Vec::new());
        }
        let ids = records
            .iter()
            .filter(|record| record.state.is_terminal() && !record.delivered)
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        for task_id in ids {
            let record = self
                .update(&task_id, |record| record.delivered = true)
                .await?;
            self.events
                .emit(&RuntimeEvent::new(
                    &self.parent_run_id,
                    RuntimeEventKind::BackgroundTaskDelivered { task_id: record.id },
                ))
                .await?;
        }
        self.select(
            &records
                .iter()
                .map(|record| record.id.clone())
                .collect::<Vec<_>>(),
        )
        .await
    }

    pub async fn drain_completed(&self) -> Result<Vec<BackgroundTaskRecord>> {
        let records = self.select(&[]).await?;
        let ready = records
            .into_iter()
            .filter(|record| record.state.is_terminal() && !record.delivered)
            .collect::<Vec<_>>();
        self.deliver(ready).await
    }

    pub async fn settle_before_finish(&self) -> Result<Vec<BackgroundTaskRecord>> {
        let records = self.select(&[]).await?;
        let ready = records
            .iter()
            .filter(|record| record.state.is_terminal() && !record.delivered)
            .cloned()
            .collect::<Vec<_>>();
        if !ready.is_empty() {
            return self.deliver(ready).await;
        }
        if records.iter().any(|record| !record.state.is_terminal()) {
            return self.wait_all().await;
        }
        self.join_all().await;
        Ok(Vec::new())
    }

    async fn track(&self, handle: tokio::task::JoinHandle<()>) {
        self.handles.lock().await.push(handle);
    }

    async fn join_all(&self) {
        let handles = self.handles.lock().await.drain(..).collect::<Vec<_>>();
        for handle in handles {
            let _ = handle.await;
        }
    }

    pub async fn abort_and_settle(&self, reason: &str) {
        let handles = self.handles.lock().await.drain(..).collect::<Vec<_>>();
        for handle in &handles {
            handle.abort();
        }
        for handle in handles {
            let _ = handle.await;
        }
        let pending = self
            .select(&[])
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|record| !record.state.is_terminal())
            .collect::<Vec<_>>();
        for record in pending {
            if let Some(child_run_id) = &record.child_run_id
                && let Ok(run) = self.store.load_run(child_run_id).await
                && matches!(
                    run.state,
                    crate::storage::RunState::Queued | crate::storage::RunState::Running
                )
            {
                let _ = self
                    .store
                    .update_state(child_run_id, crate::storage::RunState::Failed)
                    .await;
            }
            if self.fail(&record.id, reason.to_owned()).await.is_ok() {
                let _ = self
                    .events
                    .emit(&RuntimeEvent::new(
                        &self.parent_run_id,
                        RuntimeEventKind::BackgroundTaskFailed {
                            task_id: record.id,
                            name: record.name,
                            error: reason.to_owned(),
                        },
                    ))
                    .await;
            }
        }
    }

    pub async fn wait_all(&self) -> Result<Vec<BackgroundTaskRecord>> {
        loop {
            let records = self.select(&[]).await?;
            if records.iter().all(|record| record.state.is_terminal()) {
                return self
                    .deliver(
                        records
                            .into_iter()
                            .filter(|record| !record.delivered)
                            .collect(),
                    )
                    .await;
            }
            self.notify.notified().await;
        }
    }
}
