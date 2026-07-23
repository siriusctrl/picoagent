use anyhow::Result;

use super::{HandleOutputNotice, PendingHandleBoundary, RuntimeHandleManager};

impl RuntimeHandleManager {
    pub(super) async fn has_ready_output(&self, handles: &[String]) -> bool {
        let records = self.records.lock().await;
        if handles.is_empty() {
            return records.values().any(|record| !record.outputs.is_empty());
        }
        handles.iter().any(|handle| {
            records
                .get(handle)
                .is_some_and(|record| !record.outputs.is_empty())
        })
    }

    pub(crate) async fn drain_ready_outputs(&self) -> Vec<HandleOutputNotice> {
        let mut records = self.records.lock().await;
        let mut notices = Vec::new();
        for (handle, record) in records.iter_mut() {
            while let Some(output) = record.outputs.pop_front() {
                notices.push(HandleOutputNotice {
                    handle: handle.clone(),
                    kind: record.kind,
                    name: record.name.clone(),
                    output,
                });
            }
        }
        notices
    }

    pub(crate) async fn snapshot_for_request(
        &self,
    ) -> (Vec<HandleOutputNotice>, Vec<super::HandleSnapshot>) {
        let mut records = self.records.lock().await;
        let mut ready = Vec::new();
        let mut active = Vec::new();
        for (handle, record) in records.iter_mut() {
            while let Some(output) = record.outputs.pop_front() {
                ready.push(HandleOutputNotice {
                    handle: handle.clone(),
                    kind: record.kind,
                    name: record.name.clone(),
                    output,
                });
            }
            if record.state.is_active() {
                active.push(record.snapshot(handle));
            }
        }
        (ready, active)
    }

    pub(crate) async fn pending_before_finish(&self) -> Result<PendingHandleBoundary> {
        let deadline = tokio::time::Instant::now() + self.default_wait_timeout;
        let mut activity = self.activity.subscribe();
        loop {
            let ready = self.drain_ready_outputs().await;
            if !ready.is_empty() {
                return Ok(PendingHandleBoundary::Ready(ready));
            }
            if self
                .records
                .lock()
                .await
                .values()
                .all(|record| !record.state.is_active())
            {
                return Ok(PendingHandleBoundary::None);
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero()
                || tokio::time::timeout(remaining, activity.changed())
                    .await
                    .is_err()
            {
                return Ok(PendingHandleBoundary::Active);
            }
        }
    }
}
