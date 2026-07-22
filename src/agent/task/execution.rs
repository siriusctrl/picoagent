use std::sync::Arc;

use anyhow::Result;

use crate::{
    agent::tool_execution::ToolExecutionFuture,
    events::{RuntimeEvent, RuntimeEventKind},
};

use super::TaskManager;

mod delegate;

pub(crate) struct PreparedToolPromotion {
    task_id: String,
    name: String,
    call_id: String,
    promotion_ready: tokio::sync::oneshot::Sender<()>,
}

impl TaskManager {
    /// Continue a direct tool future after its foreground window elapsed. The
    /// future itself is preserved, so no work is restarted and no timeout is
    /// treated as a failure.
    pub(crate) async fn prepare_tool_promotion(
        self: &Arc<Self>,
        name: String,
        call_id: String,
        execution: ToolExecutionFuture,
    ) -> Result<PreparedToolPromotion> {
        let task_id = self.create_tool_task(name.clone(), call_id.clone()).await?;
        self.set_running(&task_id).await?;
        // `execution` has already been polled in the foreground and may be
        // suspended while holding a resource also needed by task events (for
        // example, the run event-log lock). Resume it before awaiting those
        // events, otherwise promotion can deadlock against its own future.
        // Delay the terminal task transition until the promotion events have
        // been committed so task lifecycle events remain ordered.
        let (promotion_ready, wait_for_promotion) = tokio::sync::oneshot::channel();
        let manager = self.clone();
        let task_id_for_future = task_id.clone();
        let name_for_future = name.clone();
        let handle = tokio::spawn(async move {
            let outcome = execution.await;
            let _ = wait_for_promotion.await;
            match outcome {
                Ok(output) => {
                    manager
                        .finish_tool_output(&task_id_for_future, &name_for_future, output)
                        .await;
                }
                Err(error) => {
                    manager
                        .finish_failed(&task_id_for_future, &name_for_future, error)
                        .await;
                }
            }
        });
        self.track(task_id.clone(), handle);
        Ok(PreparedToolPromotion {
            task_id,
            name,
            call_id,
            promotion_ready,
        })
    }

    pub(crate) async fn announce_tool_promotion(
        &self,
        promotion: PreparedToolPromotion,
    ) -> Result<(String, String)> {
        let PreparedToolPromotion {
            task_id,
            name,
            call_id,
            promotion_ready,
        } = promotion;
        self.events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::BackgroundTaskStarted {
                    task_id: task_id.clone(),
                    name: name.clone(),
                },
            ))
            .await?;
        self.events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::BackgroundTaskSentToBackground {
                    task_id: task_id.clone(),
                    name: name.clone(),
                    call_id,
                },
            ))
            .await?;
        let _ = promotion_ready.send(());
        Ok((task_id, name))
    }

    async fn finish_tool_output(
        &self,
        task_id: &str,
        name: &str,
        output: crate::artifact::ToolOutput,
    ) {
        let state = if output.is_error {
            self.fail_with_output(
                task_id,
                format!("tool `{name}` returned an error result"),
                output,
            )
            .await
        } else {
            self.complete(task_id, output).await
        };
        match state {
            Ok(record) if record.state == super::BackgroundTaskState::Completed => {
                let _ = self
                    .events
                    .emit(&RuntimeEvent::new(
                        &self.parent_run_id,
                        RuntimeEventKind::BackgroundTaskCompleted {
                            task_id: task_id.to_owned(),
                            name: name.to_owned(),
                        },
                    ))
                    .await;
            }
            Ok(record) if record.state == super::BackgroundTaskState::Failed => {
                let _ = self
                    .events
                    .emit(&RuntimeEvent::new(
                        &self.parent_run_id,
                        RuntimeEventKind::BackgroundTaskFailed {
                            task_id: task_id.to_owned(),
                            name: name.to_owned(),
                            error: record
                                .outputs
                                .last()
                                .map(|output| output.content.clone())
                                .unwrap_or_else(|| "tool failed".to_owned()),
                        },
                    ))
                    .await;
            }
            Ok(_) => {}
            Err(error) => self.finish_failed(task_id, name, error).await,
        }
    }
}
