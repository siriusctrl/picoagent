use std::sync::Arc;

use anyhow::{Context, Result, ensure};

use crate::{
    events::{RuntimeEvent, RuntimeEventKind},
    tools::{RawToolOutput, ToolContext},
};

use super::{BackgroundTaskRecord, BackgroundTaskState, TaskManager, record::BackgroundTaskOutput};

impl TaskManager {
    pub(super) async fn interrupt_agent_activity(
        &self,
        task_id: &str,
        output: &str,
        reminder: &str,
    ) -> Result<BackgroundTaskRecord> {
        let mut records = self.records.lock().await;
        let mut record = records
            .get(task_id)
            .cloned()
            .with_context(|| format!("unknown background task `{task_id}`"))?;
        ensure!(record.kind == "agent", "task `{task_id}` is not an agent");
        if record.state == BackgroundTaskState::Idle {
            if !record.paused {
                record.paused = true;
                self.persist(&record).await?;
                records.insert(task_id.to_owned(), record.clone());
                drop(records);
                self.signal_activity();
            }
            return Ok(record);
        }
        ensure!(
            record.state.is_active(),
            "agent task `{task_id}` is already {}",
            record.status()
        );
        let child_run_id = record
            .child_run_id
            .clone()
            .context("agent task is missing child_run_id")?;
        let child = self.store.load_run(&child_run_id).await?;
        ensure!(
            child.state != crate::storage::RunState::Closed,
            "active agent child `{child_run_id}` is already closed"
        );
        if child.state != crate::storage::RunState::Idle {
            self.store
                .update_state(&child_run_id, crate::storage::RunState::Idle)
                .await?;
        }
        let seq = record.next_output_seq();
        self.store
            .enqueue_runtime_input_with_id(
                &child_run_id,
                format!("activity-interrupted-{task_id}-{seq}"),
                reminder.to_owned(),
            )
            .await?;
        record.state = BackgroundTaskState::Idle;
        record.paused = true;
        record.outputs.push(BackgroundTaskOutput {
            seq,
            status: super::BackgroundTaskOutputStatus::Interrupted,
            content: output.to_owned(),
            metadata: crate::artifact::ResultMetadata::empty(),
        });
        self.persist(&record).await?;
        records.insert(task_id.to_owned(), record.clone());
        drop(records);
        self.signal_activity();
        Ok(record)
    }

    pub(super) async fn finish_agent_output(
        self: &Arc<Self>,
        task_id: &str,
        profile: &str,
        child_run_id: &str,
        output: crate::artifact::ToolOutput,
    ) {
        let expected_seq = match self.get(task_id).await {
            Ok(record) => record.next_output_seq(),
            Err(error) => {
                self.finish_failed(task_id, profile, error).await;
                return;
            }
        };
        let state = self
            .update(task_id, move |record| {
                if record.kind == "agent" && record.state.is_active() {
                    record.state = BackgroundTaskState::Idle;
                    let seq = record.next_output_seq();
                    record.outputs.push(BackgroundTaskOutput {
                        seq,
                        status: super::BackgroundTaskOutputStatus::Completed,
                        content: output.model_content(),
                        metadata: output.result_metadata(),
                    });
                }
            })
            .await;
        match state {
            Ok(record)
                if record.state == BackgroundTaskState::Idle
                    && record.outputs.last().is_some_and(|output| {
                        output.seq == expected_seq
                            && output.status == super::BackgroundTaskOutputStatus::Completed
                    }) =>
            {
                let _ = self
                    .events
                    .emit(&RuntimeEvent::new(
                        &self.parent_run_id,
                        RuntimeEventKind::BackgroundTaskCompleted {
                            task_id: task_id.to_owned(),
                            name: profile.to_owned(),
                        },
                    ))
                    .await;
                let _ = self
                    .events
                    .emit(&RuntimeEvent::new(
                        &self.parent_run_id,
                        RuntimeEventKind::SubagentActivityCompleted {
                            child_run_id: child_run_id.to_owned(),
                        },
                    ))
                    .await;
                if let Err(error) = self.activate_agent_if_pending(task_id).await {
                    tracing::error!(task_id, error = %format!("{error:#}"), "activate queued follow-up after completed agent activity");
                }
            }
            Ok(_) => {}
            Err(error) => {
                self.finish_agent_failed_activity(task_id, profile, child_run_id, error)
                    .await
            }
        }
    }

    pub(super) async fn finish_agent_failed_activity(
        self: &Arc<Self>,
        task_id: &str,
        name: &str,
        child_run_id: &str,
        error: anyhow::Error,
    ) {
        let message = format!("{error:#}");
        let expected_seq = match self.get(task_id).await {
            Ok(record) => record.next_output_seq(),
            Err(load_error) => {
                tracing::error!(task_id, error = %format!("{load_error:#}"), "load failed agent activity");
                return;
            }
        };
        let context = ToolContext {
            run_id: self.parent_run_id.clone(),
            call_id: format!("background-{task_id}"),
            workspace: self.workspace.clone(),
        };
        let raw = RawToolOutput {
            content: message.as_bytes().to_vec(),
            source_path: None,
            media_type: "text/plain; charset=utf-8".to_owned(),
            is_error: true,
            attach_to_model: false,
        };
        let (content, metadata) = match self.persist_output(&context, raw).await {
            Ok(output) => (output.model_content(), output.result_metadata()),
            Err(persist_error) => {
                tracing::error!(
                    task_id,
                    error = %format!("{persist_error:#}"),
                    "preserve failed agent activity"
                );
                (message.clone(), crate::artifact::ResultMetadata::empty())
            }
        };
        let state = self
            .update(task_id, move |record| {
                if record.kind == "agent" && record.state.is_active() {
                    record.state = BackgroundTaskState::Idle;
                    let seq = record.next_output_seq();
                    record.outputs.push(BackgroundTaskOutput {
                        seq,
                        status: super::BackgroundTaskOutputStatus::Failed,
                        content,
                        metadata,
                    });
                }
            })
            .await;
        let appended = match state {
            Ok(record) => {
                record.state == BackgroundTaskState::Idle
                    && record.outputs.last().is_some_and(|output| {
                        output.seq == expected_seq
                            && output.status == super::BackgroundTaskOutputStatus::Failed
                    })
            }
            Err(state_error) => {
                tracing::error!(task_id, error = %format!("{state_error:#}"), "persist failed agent activity state");
                false
            }
        };
        if appended {
            if let Err(state_error) = self
                .store
                .update_state(child_run_id, crate::storage::RunState::Idle)
                .await
            {
                tracing::error!(task_id, error = %format!("{state_error:#}"), "reset failed child activity to idle");
                return;
            }
            let _ = self
                .events
                .emit(&RuntimeEvent::new(
                    &self.parent_run_id,
                    RuntimeEventKind::BackgroundTaskFailed {
                        task_id: task_id.to_owned(),
                        name: name.to_owned(),
                        error: message.clone(),
                    },
                ))
                .await;
            let _ = self
                .events
                .emit(&RuntimeEvent::new(
                    &self.parent_run_id,
                    RuntimeEventKind::SubagentActivityFailed {
                        child_run_id: child_run_id.to_owned(),
                        error: message,
                    },
                ))
                .await;
            if let Err(error) = self.activate_agent_if_pending(task_id).await {
                tracing::error!(task_id, error = %format!("{error:#}"), "activate queued follow-up after failed agent activity");
            }
        }
    }

    pub(super) async fn fail_with_output(
        &self,
        task_id: &str,
        _error: String,
        output: crate::artifact::ToolOutput,
    ) -> anyhow::Result<BackgroundTaskRecord> {
        self.update(task_id, |record| {
            if !record.state.is_terminal() {
                record.state = BackgroundTaskState::Failed;
                let seq = record.next_output_seq();
                record.outputs.push(BackgroundTaskOutput {
                    seq,
                    status: super::BackgroundTaskOutputStatus::Failed,
                    content: output.model_content(),
                    metadata: output.result_metadata(),
                });
            }
        })
        .await
    }

    pub(super) async fn finish_failed(&self, task_id: &str, name: &str, error: anyhow::Error) {
        let mut error = format!("{error:#}");
        let artifact_call_id = self
            .get(task_id)
            .await
            .ok()
            .and_then(|record| (record.kind == "tool").then_some(record.origin_call_id))
            .unwrap_or_else(|| format!("background-{task_id}"));
        let context = ToolContext {
            run_id: self.parent_run_id.clone(),
            call_id: artifact_call_id,
            workspace: self.workspace.clone(),
        };
        let raw = RawToolOutput {
            content: error.as_bytes().to_vec(),
            source_path: None,
            media_type: "text/plain; charset=utf-8".to_owned(),
            is_error: true,
            attach_to_model: false,
        };
        let state_result = match self.persist_output(&context, raw).await {
            Ok(output) => self.fail_with_output(task_id, error.clone(), output).await,
            Err(persist_error) => {
                error.push_str(&format!(
                    "; failed to preserve bounded task error: {persist_error:#}"
                ));
                self.fail(task_id, error.clone()).await
            }
        };
        if let Err(state_error) = state_result {
            error.push_str(&format!(
                "; failed to persist task failure: {state_error:#}"
            ));
            self.fail_in_memory(task_id, error.clone()).await;
        }
        if self
            .get(task_id)
            .await
            .is_ok_and(|record| record.state == BackgroundTaskState::Failed)
        {
            let _ = self
                .events
                .emit(&RuntimeEvent::new(
                    &self.parent_run_id,
                    RuntimeEventKind::BackgroundTaskFailed {
                        task_id: task_id.to_owned(),
                        name: name.to_owned(),
                        error,
                    },
                ))
                .await;
        }
    }
}
