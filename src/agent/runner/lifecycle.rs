use std::sync::Arc;

use anyhow::Result;
use serde_json::json;
use ulid::Ulid;

use crate::{
    events::{CompositeEventSink, RuntimeEvent, RuntimeEventKind, SharedEventSink},
    hooks::HookEvent,
    storage::{RunRecord, RunState},
};

use super::{AgentRunner, RunRequest, RunResult};
use crate::agent::types::RunProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RunMode {
    New,
    Resume,
}

pub(super) struct RunPlan {
    pub(super) model: String,
    pub(super) max_steps: usize,
    pub(super) max_output_tokens: Option<u32>,
    pub(super) may_delegate: bool,
}

impl AgentRunner {
    pub async fn run(self: &Arc<Self>, request: RunRequest) -> Result<RunResult> {
        self.run_with_id(request, Ulid::new().to_string()).await
    }

    pub(crate) async fn run_with_id(
        self: &Arc<Self>,
        request: RunRequest,
        run_id: String,
    ) -> Result<RunResult> {
        let plan = self.plan(&request);
        let record = RunRecord::new(
            &run_id,
            &request.prompt,
            self.provider.name(),
            &plan.model,
            self.workspace.clone(),
            request.parent_run_id.clone(),
        )
        .with_execution_context(
            request.profile.as_str(),
            request.depth,
            request.additional_instructions.clone(),
        )
        .with_provider_resume_fingerprint(self.provider.resume_fingerprint());
        self.store.create_run(&record).await?;

        let _lease = self.store.acquire_run_lease(&run_id).await?;
        self.run_with_mode(request, run_id, RunMode::New).await
    }

    pub(super) async fn run_with_mode(
        self: &Arc<Self>,
        request: RunRequest,
        run_id: String,
        mode: RunMode,
    ) -> Result<RunResult> {
        let events: SharedEventSink = Arc::new(CompositeEventSink::new(vec![
            self.store.event_sink(),
            self.extra_events.clone(),
        ]));
        let outcome = self
            .run_loop(request, run_id.clone(), events.clone(), mode)
            .await;
        if let Err(error) = &outcome {
            let _ = self.store.update_state(&run_id, RunState::Failed).await;
            let _ = events
                .emit(&RuntimeEvent::new(
                    &run_id,
                    RuntimeEventKind::RunFailed {
                        error: format!("{error:#}"),
                    },
                ))
                .await;
        }
        outcome
    }

    pub(super) fn plan(&self, request: &RunRequest) -> RunPlan {
        let (model, max_steps, max_output_tokens) = match request.profile {
            RunProfile::Root => (
                self.model.clone(),
                self.options.max_steps,
                self.options.max_output_tokens,
            ),
            RunProfile::GeneralTaskDelegating | RunProfile::GeneralTaskLeaf => (
                self.options
                    .general_task
                    .model
                    .clone()
                    .unwrap_or_else(|| self.model.clone()),
                self.options.general_task.max_steps,
                self.options.general_task.max_output_tokens,
            ),
        };
        let may_delegate = match request.profile {
            RunProfile::Root => request.depth < self.options.max_subagent_depth,
            RunProfile::GeneralTaskDelegating => true,
            RunProfile::GeneralTaskLeaf => false,
        };
        RunPlan {
            model,
            max_steps,
            max_output_tokens,
            may_delegate,
        }
    }

    pub(super) async fn finish_success(
        &self,
        run_id: &str,
        final_output: &str,
        events: SharedEventSink,
    ) -> Result<()> {
        self.store.write_final(run_id, final_output).await?;
        self.store.update_state(run_id, RunState::Completed).await?;
        if let Err(error) = self
            .hooks
            .run(
                HookEvent::RunEnd,
                json!({ "run_id": run_id, "final_output": final_output }),
                &self.workspace,
            )
            .await
        {
            tracing::warn!(run_id, error = %format!("{error:#}"), "run_end hook failed after completion");
        }
        if let Err(error) = events
            .emit(&RuntimeEvent::new(
                run_id,
                RuntimeEventKind::RunCompleted {
                    final_output: final_output.to_owned(),
                },
            ))
            .await
        {
            tracing::warn!(run_id, error = %format!("{error:#}"), "run_completed event failed after completion");
        }
        Ok(())
    }
}
