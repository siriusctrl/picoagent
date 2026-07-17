use crate::{
    events::{RuntimeEvent, RuntimeEventKind},
    tools::{RawToolOutput, ToolContext},
};

use super::{BackgroundTaskRecord, BackgroundTaskState, TaskManager, record::BackgroundTaskOutput};

impl TaskManager {
    pub(super) async fn fail_with_output(
        &self,
        task_id: &str,
        error: String,
        output: crate::artifact::ToolOutput,
    ) -> anyhow::Result<BackgroundTaskRecord> {
        let result = BackgroundTaskOutput {
            content: output.model_content(),
            metadata: output.result_metadata(),
        };
        self.update(task_id, |record| {
            if !record.state.is_terminal() {
                record.state = BackgroundTaskState::Failed;
                record.error = Some(error);
                record.result = Some(result);
            }
        })
        .await
    }

    pub(super) async fn finish_failed(&self, task_id: &str, name: &str, error: anyhow::Error) {
        let mut error = format!("{error:#}");
        let context = ToolContext {
            run_id: self.parent_run_id.clone(),
            call_id: format!("background-{task_id}"),
            workspace: self.workspace.clone(),
        };
        let raw = RawToolOutput {
            content: format!("background task `{name}` failed: {error}").into_bytes(),
            source_path: None,
            media_type: "text/plain; charset=utf-8".to_owned(),
            is_error: true,
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
