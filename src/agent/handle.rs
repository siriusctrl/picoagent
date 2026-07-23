use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::sync::{Mutex, Semaphore, watch};

use crate::{
    artifact::{ArtifactStore, ResultMetadata},
    events::SharedEventSink,
    storage::{RunDirStore, RunState},
};

use super::runner::AgentRunner;

mod control;
mod coordination;
mod execution;
mod lifecycle;
#[cfg(test)]
mod tests;

pub use control::SendMode;

pub struct RuntimeHandleManager {
    runner: Arc<AgentRunner>,
    artifacts: ArtifactStore,
    store: RunDirStore,
    workspace: PathBuf,
    parent_run_id: String,
    parent_depth: usize,
    remaining_delegation_depth: usize,
    events: SharedEventSink,
    records: Mutex<BTreeMap<String, HandleRecord>>,
    executions: StdMutex<BTreeMap<String, TrackedExecution>>,
    activity: watch::Sender<u64>,
    subagent_slots: Arc<Semaphore>,
    default_wait_timeout: Duration,
}

pub struct RuntimeHandleManagerConfig {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HandleKind {
    Agent,
    Tool,
}

impl HandleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HandleState {
    Queued,
    Running,
    Idle,
    Completed,
    Failed,
    Cancelled,
    Closed,
}

impl HandleState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Idle => "idle",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Closed => "closed",
        }
    }

    fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HandleSnapshot {
    pub handle: String,
    pub kind: HandleKind,
    pub name: String,
    pub status: HandleState,
}

#[derive(Debug, Clone)]
pub(crate) struct HandleOutputNotice {
    pub handle: String,
    pub kind: HandleKind,
    pub name: String,
    pub output: HandleOutput,
}

#[derive(Debug, Clone)]
pub(crate) struct HandleOutput {
    pub status: HandleState,
    pub content: String,
    pub metadata: ResultMetadata,
}

pub(crate) enum PendingHandleBoundary {
    Ready(Vec<HandleOutputNotice>),
    Active,
    None,
}

struct HandleRecord {
    kind: HandleKind,
    name: String,
    state: HandleState,
    generation: u64,
    outputs: VecDeque<HandleOutput>,
    followups: Vec<PendingAgentInput>,
}

impl HandleRecord {
    fn agent(name: String) -> Self {
        Self {
            kind: HandleKind::Agent,
            name,
            state: HandleState::Idle,
            generation: 0,
            outputs: VecDeque::new(),
            followups: Vec::new(),
        }
    }

    fn tool(name: String) -> Self {
        Self {
            kind: HandleKind::Tool,
            name,
            state: HandleState::Running,
            generation: 0,
            outputs: VecDeque::new(),
            followups: Vec::new(),
        }
    }

    fn snapshot(&self, handle: &str) -> HandleSnapshot {
        HandleSnapshot {
            handle: handle.to_owned(),
            kind: self.kind,
            name: self.name.clone(),
            status: self.state,
        }
    }
}

struct PendingAgentInput {
    id: String,
    message: String,
}

struct TrackedExecution {
    generation: u64,
    handle: tokio::task::JoinHandle<()>,
    cleanup_done: Option<tokio::sync::oneshot::Receiver<()>>,
}

impl TrackedExecution {
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

impl RuntimeHandleManager {
    pub fn new(config: RuntimeHandleManagerConfig) -> Arc<Self> {
        let (activity, _) = watch::channel(0);
        Arc::new(Self {
            runner: config.runner,
            artifacts: config.artifacts,
            store: config.store,
            workspace: config.workspace,
            parent_run_id: config.parent_run_id,
            parent_depth: config.parent_depth,
            remaining_delegation_depth: config.remaining_delegation_depth,
            events: config.events,
            records: Mutex::new(BTreeMap::new()),
            executions: StdMutex::new(BTreeMap::new()),
            activity,
            subagent_slots: Arc::new(Semaphore::new(config.max_parallel_subagents.max(1))),
            default_wait_timeout: Duration::from_secs(config.wait_timeout_seconds.max(1)),
        })
    }

    async fn insert_agent(&self, handle: String, name: String) -> Result<()> {
        let mut records = self.records.lock().await;
        anyhow::ensure!(
            !records.contains_key(&handle),
            "runtime handle `{handle}` already exists"
        );
        records.insert(handle, HandleRecord::agent(name));
        drop(records);
        self.signal_activity();
        Ok(())
    }

    async fn insert_tool(&self, handle: String, name: String) -> Result<()> {
        let mut records = self.records.lock().await;
        anyhow::ensure!(
            !records.contains_key(&handle),
            "runtime handle `{handle}` already exists"
        );
        records.insert(handle, HandleRecord::tool(name));
        drop(records);
        self.signal_activity();
        Ok(())
    }

    async fn load_agent_thread(&self, handle: &str) -> Result<crate::storage::RunRecord> {
        if self
            .records
            .lock()
            .await
            .get(handle)
            .is_some_and(|record| record.kind == HandleKind::Tool)
        {
            anyhow::bail!("runtime handle `{handle}` is a tool job, not an agent");
        }
        self.load_child_run(handle).await
    }

    async fn load_child_run(&self, handle: &str) -> Result<crate::storage::RunRecord> {
        let child = self
            .store
            .load_run(handle)
            .await
            .with_context(|| format!("unknown runtime handle `{handle}`"))?;
        anyhow::ensure!(
            child.parent_run_id.as_deref() == Some(self.parent_run_id.as_str()),
            "agent handle `{handle}` does not belong to this run"
        );
        anyhow::ensure!(
            matches!(
                child.profile.as_str(),
                "general_task_delegating" | "general_task_leaf"
            ),
            "runtime handle `{handle}` is not an agent"
        );
        Ok(child)
    }

    async fn snapshot_for_handle(&self, handle: &str) -> Result<HandleSnapshot> {
        if let Some(snapshot) = self
            .records
            .lock()
            .await
            .get(handle)
            .map(|record| record.snapshot(handle))
        {
            return Ok(snapshot);
        }
        let child = self.load_agent_thread(handle).await?;
        Ok(HandleSnapshot {
            handle: child.id,
            kind: HandleKind::Agent,
            name: child.name,
            status: if child.state == RunState::Closed {
                HandleState::Closed
            } else {
                HandleState::Idle
            },
        })
    }

    fn signal_activity(&self) {
        self.activity
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }

    fn track(
        &self,
        handle: String,
        generation: u64,
        execution: tokio::task::JoinHandle<()>,
        cleanup_done: Option<tokio::sync::oneshot::Receiver<()>>,
    ) {
        let mut executions = self
            .executions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        executions.retain(|_, tracked| !tracked.handle.is_finished());
        executions.insert(
            handle,
            TrackedExecution {
                generation,
                handle: execution,
                cleanup_done,
            },
        );
    }

    fn take_execution(&self, handle: &str, generation: u64) -> Option<TrackedExecution> {
        let mut executions = self
            .executions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (executions.get(handle)?.generation == generation)
            .then(|| executions.remove(handle))
            .flatten()
    }

    fn take_executions(&self) -> BTreeMap<String, TrackedExecution> {
        std::mem::take(
            &mut *self
                .executions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}
