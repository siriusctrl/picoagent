use std::collections::BTreeMap;

use anyhow::{Context, Result};

use crate::events::{RuntimeEvent, RuntimeEventKind};

use super::{BackgroundTaskRecord, PendingTaskBoundary, TaskManager, TaskOutputNotice};

impl TaskManager {
    pub async fn wait(&self, task_ids: &[String]) -> Result<Vec<BackgroundTaskRecord>> {
        let deadline = tokio::time::Instant::now() + self.default_wait_timeout;
        let mut activity = self.activity.subscribe();
        loop {
            let records = self.select(task_ids).await?;
            let delivered = self.delivered.lock().await.clone();
            let has_ready_output = records.iter().any(|record| {
                let cursor = delivered.get(&record.id).copied().unwrap_or(0);
                record.outputs.iter().any(|output| output.seq > cursor)
            });
            if records.is_empty()
                || has_ready_output
                || records.iter().any(|record| !record.state.is_active())
            {
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

    pub(super) async fn select(&self, task_ids: &[String]) -> Result<Vec<BackgroundTaskRecord>> {
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

    /// Mark outputs delivered only after the caller has durably appended their
    /// `BackgroundTask` messages. This cursor is an in-memory fast path;
    /// recovery derives truth from the parent transcript.
    pub(crate) async fn mark_delivered(&self, notices: &[TaskOutputNotice]) -> Result<()> {
        let mut delivered = self.delivered.lock().await;
        for notice in notices {
            let cursor = delivered.entry(notice.task_id.clone()).or_insert(0);
            if notice.output.seq > *cursor {
                *cursor = notice.output.seq;
                self.events
                    .emit(&RuntimeEvent::new(
                        &self.parent_run_id,
                        RuntimeEventKind::BackgroundTaskDelivered {
                            task_id: notice.task_id.clone(),
                            output_seq: notice.output.seq,
                        },
                    ))
                    .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn drain_ready_outputs(&self) -> Result<Vec<TaskOutputNotice>> {
        let delivered = self.delivered.lock().await.clone();
        let records = self.select(&[]).await?;
        Ok(ready_outputs(&records, &delivered))
    }

    /// Capture one coherent model-request view of background work. A task that
    /// finishes after this snapshot remains in `active`, so the current request
    /// cannot briefly forget it before the activity result notice is appended
    /// on the next loop iteration.
    pub(crate) async fn snapshot_for_request(
        &self,
    ) -> (Vec<TaskOutputNotice>, Vec<BackgroundTaskRecord>) {
        let delivered = self.delivered.lock().await.clone();
        let records = self.records.lock().await;
        let records = records.values().cloned().collect::<Vec<_>>();
        let ready = ready_outputs(&records, &delivered);
        let active = records
            .into_iter()
            .filter(|record| record.state.is_active())
            .collect();
        (ready, active)
    }

    /// Return anything the model must see before it can finish: first unseen
    /// activity results, otherwise the currently active tasks after one
    /// bounded wait interval. The pause prevents a fast final-answer loop from
    /// filling the trajectory with duplicate running snapshots.
    pub(crate) async fn pending_before_finish(&self) -> Result<PendingTaskBoundary> {
        let deadline = tokio::time::Instant::now() + self.default_wait_timeout;
        let mut activity = self.activity.subscribe();
        loop {
            let records = self.select(&[]).await?;
            let delivered = self.delivered.lock().await.clone();
            let ready = ready_outputs(&records, &delivered);
            if !ready.is_empty() {
                return Ok(PendingTaskBoundary::Ready(ready));
            }
            if records.iter().all(|record| !record.state.is_active()) {
                return Ok(PendingTaskBoundary::None);
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero()
                || tokio::time::timeout(remaining, activity.changed())
                    .await
                    .is_err()
            {
                return Ok(PendingTaskBoundary::Active);
            }
        }
    }

    pub async fn wait_all(&self) -> Result<Vec<BackgroundTaskRecord>> {
        let mut activity = self.activity.subscribe();
        loop {
            let records = self.select(&[]).await?;
            if records.iter().all(|record| !record.state.is_active()) {
                let delivered = self.delivered.lock().await.clone();
                return Ok(records
                    .into_iter()
                    .filter(|record| {
                        let cursor = delivered.get(&record.id).copied().unwrap_or(0);
                        record.outputs.iter().any(|output| output.seq > cursor)
                    })
                    .collect());
            }
            activity.changed().await?;
        }
    }
}

fn ready_outputs(
    records: &[BackgroundTaskRecord],
    delivered: &BTreeMap<String, u64>,
) -> Vec<TaskOutputNotice> {
    records
        .iter()
        .flat_map(|record| {
            let delivered_seq = delivered.get(&record.id).copied().unwrap_or(0);
            record
                .outputs
                .iter()
                .filter(move |output| output.seq > delivered_seq)
                .cloned()
                .map(|output| TaskOutputNotice {
                    task_id: record.id.clone(),
                    name: record.name.clone(),
                    output,
                })
        })
        .collect()
}
