use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::{
    events::{RuntimeEvent, RuntimeEventKind},
    model::openai_chat::project_chat_message,
    storage::RunState,
};

use super::{
    HandleKind, HandleRecord, HandleSnapshot, HandleState, PendingAgentInput, RuntimeHandleManager,
};

const AGENT_RESTART_REMINDER: &str = "The previous fiasco process stopped after this agent thread's last complete checkpoint. Its prior activity and pending input were discarded, but workspace or external side effects may already have occurred. Inspect current state before repeating operations.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendMode {
    Steer,
    Followup,
}

impl SendMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Steer => "steer",
            Self::Followup => "followup",
        }
    }
}

impl RuntimeHandleManager {
    pub async fn list_handles(&self, include_closed: bool) -> Result<Vec<HandleSnapshot>> {
        let mut snapshots = self
            .store
            .list_child_runs(&self.parent_run_id)
            .await?
            .into_iter()
            .filter_map(|child| {
                let status = if child.state == RunState::Closed {
                    HandleState::Closed
                } else {
                    HandleState::Idle
                };
                let handle = child.id;
                (include_closed || status != HandleState::Closed).then(|| {
                    let snapshot = HandleSnapshot {
                        handle: handle.clone(),
                        kind: HandleKind::Agent,
                        name: child.name,
                        status,
                    };
                    (handle, snapshot)
                })
            })
            .collect::<BTreeMap<_, _>>();
        for (handle, record) in self.records.lock().await.iter() {
            let snapshot = record.snapshot(handle);
            if include_closed || snapshot.status != HandleState::Closed {
                snapshots.insert(handle.clone(), snapshot);
            }
        }
        Ok(snapshots.into_values().collect())
    }

    pub async fn status(&self, handles: &[String]) -> Result<Vec<HandleSnapshot>> {
        if handles.is_empty() {
            return self.list_handles(false).await;
        }
        let mut snapshots = Vec::with_capacity(handles.len());
        for handle in handles {
            snapshots.push(self.snapshot_for_handle(handle).await?);
        }
        Ok(snapshots)
    }

    pub async fn wait(&self, handles: &[String]) -> Result<Vec<HandleSnapshot>> {
        let mut activity = self.activity.subscribe();
        let initial = self.status(handles).await?;
        if initial.is_empty()
            || initial.iter().any(|snapshot| !snapshot.status.is_active())
            || self.has_ready_output(handles).await
        {
            return Ok(initial);
        }
        let initial_states = initial
            .iter()
            .map(|snapshot| (snapshot.handle.clone(), snapshot.status))
            .collect::<BTreeMap<_, _>>();
        let deadline = tokio::time::Instant::now() + self.default_wait_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero()
                || tokio::time::timeout(remaining, activity.changed())
                    .await
                    .is_err()
            {
                return self.status(handles).await;
            }
            let current = self.status(handles).await?;
            if current.is_empty()
                || current.iter().any(|snapshot| !snapshot.status.is_active())
                || self.has_ready_output(handles).await
                || current.iter().any(|snapshot| {
                    initial_states.get(&snapshot.handle).copied() != Some(snapshot.status)
                })
            {
                return Ok(current);
            }
        }
    }

    pub async fn inspect(
        &self,
        handle: &str,
        before_seq: Option<u64>,
        limit: usize,
    ) -> Result<Value> {
        let child = self.load_agent_thread(handle).await?;
        let trajectory = self.store.load_trajectory(handle).await?;
        let before_seq = before_seq.unwrap_or(u64::MAX);
        let eligible = trajectory
            .iter()
            .filter(|message| message.seq < before_seq)
            .collect::<Vec<_>>();
        let start = eligible.len().saturating_sub(limit);
        let messages = eligible[start..]
            .iter()
            .map(|record| {
                json!({
                    "seq": record.seq,
                    "message": project_chat_message(&record.message),
                })
            })
            .collect::<Vec<_>>();
        let has_earlier = start > 0;
        let next_before_seq = has_earlier
            .then(|| eligible[start].seq)
            .map_or(Value::Null, Value::from);
        Ok(json!({
            "handle": handle,
            "name": child.name,
            "status": self.snapshot_for_handle(handle).await?.status,
            "messages": messages,
            "has_earlier": has_earlier,
            "next_before_seq": next_before_seq,
        }))
    }

    pub async fn send(
        self: &std::sync::Arc<Self>,
        handle: &str,
        message: String,
        mode: SendMode,
    ) -> Result<Value> {
        ensure!(
            !message.trim().is_empty(),
            "agent message must not be empty"
        );
        self.load_agent_for_send(handle).await?;
        let input_id = format!("input_{}", ulid::Ulid::new());
        let mut launch_generation = None;
        let mut records = self.records.lock().await;
        let record = records
            .get_mut(handle)
            .with_context(|| format!("unknown runtime handle `{handle}`"))?;
        ensure!(
            record.kind == HandleKind::Agent,
            "runtime handle `{handle}` is a tool job, not an agent"
        );
        ensure!(
            record.state != HandleState::Closed,
            "agent handle `{handle}` is closed"
        );
        let accepted_as = match (record.state, mode) {
            (HandleState::Queued | HandleState::Running, SendMode::Steer) => {
                self.store
                    .enqueue_user_input_with_id(handle, input_id.clone(), message)
                    .await?;
                "steered"
            }
            (HandleState::Queued | HandleState::Running, SendMode::Followup) => {
                record.followups.push(PendingAgentInput {
                    id: input_id.clone(),
                    message,
                });
                "queued_followup"
            }
            (HandleState::Idle, _) => {
                self.store
                    .enqueue_user_input_with_id(handle, input_id.clone(), message)
                    .await?;
                record.state = HandleState::Queued;
                record.generation = record.generation.saturating_add(1);
                launch_generation = Some(record.generation);
                "started"
            }
            _ => anyhow::bail!(
                "agent handle `{handle}` is already {}",
                record.state.as_str()
            ),
        };
        let name = record.name.clone();
        let status = record.state;
        self.events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::AgentMessageQueued {
                    handle: handle.to_owned(),
                    input_id: input_id.clone(),
                    mode: mode.as_str().to_owned(),
                },
            ))
            .await?;
        if let Some(generation) = launch_generation {
            self.launch_agent_activity(handle.to_owned(), generation);
        }
        drop(records);
        Ok(json!({
            "handle": handle,
            "name": name,
            "status": status,
            "message_id": input_id,
            "requested_mode": mode.as_str(),
            "accepted_as": accepted_as,
        }))
    }

    pub async fn stop(&self, handle: &str) -> Result<HandleSnapshot> {
        let kind = self
            .records
            .lock()
            .await
            .get(handle)
            .map(|record| record.kind);
        if let Some(kind) = kind {
            match kind {
                HandleKind::Agent => return self.stop_agent(handle).await,
                HandleKind::Tool => return self.stop_tool(handle).await,
            }
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

    pub async fn close(&self, handle: &str) -> Result<HandleSnapshot> {
        let _close_guard = self.close_lock.lock().await;
        let child = self.load_child_run(handle).await?;
        if child.state == RunState::Closed {
            return Ok(HandleSnapshot {
                handle: child.id,
                kind: HandleKind::Agent,
                name: child.name,
                status: HandleState::Closed,
            });
        }
        let (snapshot, active_generation) = {
            let mut records = self.records.lock().await;
            if let Some(record) = records.get(handle) {
                ensure!(
                    record.kind == HandleKind::Agent,
                    "runtime handle `{handle}` is a tool job, not an agent"
                );
            }
            let record = records
                .entry(handle.to_owned())
                .or_insert_with(|| HandleRecord::agent(child.name.clone()));
            let active_generation = record.state.is_active().then_some(record.generation);
            record.state = HandleState::Closed;
            record.followups.clear();
            (record.snapshot(handle), active_generation)
        };
        self.signal_activity();
        if let Some(generation) = active_generation
            && let Some(tracked) = self.take_execution(handle, generation)
        {
            tracked.abort();
            tracked.wait().await;
        }
        self.store.clear_pending_inputs(handle).await?;
        self.store.update_state(handle, RunState::Closed).await?;
        self.events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::AgentClosed {
                    handle: handle.to_owned(),
                },
            ))
            .await?;
        Ok(snapshot)
    }

    async fn load_agent_for_send(&self, handle: &str) -> Result<()> {
        let mut records = self.records.lock().await;
        if let Some(record) = records.get(handle) {
            ensure!(
                record.kind == HandleKind::Agent,
                "runtime handle `{handle}` is a tool job, not an agent"
            );
            return Ok(());
        }
        let child = self.load_child_run(handle).await?;
        ensure!(
            child.state != RunState::Closed,
            "agent handle `{handle}` is closed"
        );
        self.store.clear_pending_inputs(handle).await?;
        self.store
            .enqueue_runtime_input_with_id(
                handle,
                format!("restart-{}", ulid::Ulid::new()),
                AGENT_RESTART_REMINDER.to_owned(),
            )
            .await?;
        records
            .entry(handle.to_owned())
            .or_insert_with(|| HandleRecord::agent(child.name));
        drop(records);
        self.signal_activity();
        Ok(())
    }
}
