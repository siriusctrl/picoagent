use anyhow::Result;
use serde_json::{Value, json};

use crate::{
    events::{RuntimeEvent, RuntimeEventKind},
    hooks::HookEvent,
    tools::ToolContext,
};

use super::TaskManager;

impl TaskManager {
    pub(super) async fn finish_failed(&self, task_id: &str, name: &str, error: anyhow::Error) {
        let mut error = format!("{error:#}");
        if let Err(state_error) = self.fail(task_id, error.clone()).await {
            error.push_str(&format!(
                "; failed to persist task failure: {state_error:#}"
            ));
            self.fail_in_memory(task_id, error.clone()).await;
        }
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

    pub(super) async fn start_tool_lifecycle(
        &self,
        context: &ToolContext,
        name: &str,
        arguments: Value,
    ) -> Result<Value> {
        let before = self
            .hooks
            .run(
                HookEvent::ToolBefore,
                json!({
                    "run_id": context.run_id,
                    "call_id": context.call_id,
                    "name": name,
                    "arguments": arguments,
                    "background": true,
                }),
                &self.workspace,
            )
            .await?;
        self.events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::ToolStarted {
                    call_id: context.call_id.clone(),
                    name: name.to_owned(),
                },
            ))
            .await?;
        Ok(before
            .payload
            .get("arguments")
            .cloned()
            .unwrap_or(arguments))
    }

    pub(super) async fn finish_tool_lifecycle(
        &self,
        context: &ToolContext,
        name: &str,
        output: Option<&crate::artifact::ToolOutput>,
        is_error: bool,
    ) -> Result<()> {
        if let Some(artifact) = output.and_then(|output| output.artifact.as_ref()) {
            self.events
                .emit(&RuntimeEvent::new(
                    &self.parent_run_id,
                    RuntimeEventKind::ArtifactCreated {
                        call_id: context.call_id.clone(),
                        path: artifact.path.clone(),
                        bytes: artifact.bytes,
                    },
                ))
                .await?;
        }
        self.events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::ToolCompleted {
                    call_id: context.call_id.clone(),
                    name: name.to_owned(),
                },
            ))
            .await?;
        self.hooks
            .run(
                HookEvent::ToolAfter,
                json!({
                    "run_id": context.run_id,
                    "call_id": context.call_id,
                    "name": name,
                    "background": true,
                    "truncated": output.is_some_and(|output| output.truncated),
                    "artifact": output.and_then(|output| output.artifact.as_ref()),
                    "is_error": is_error,
                }),
                &self.workspace,
            )
            .await?;
        Ok(())
    }
}
