use std::{collections::BTreeSet, sync::Arc};

use anyhow::Result;
use serde_json::json;
use ulid::Ulid;

use crate::model::ModelModality;
use crate::{
    events::{CompositeEventSink, RuntimeEvent, RuntimeEventKind, SharedEventSink},
    hooks::HookEvent,
    storage::{RunLease, RunRecord, RunState},
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
    pub(super) modalities: BTreeSet<ModelModality>,
    pub(super) max_output_tokens: Option<u32>,
    pub(super) remaining_delegation_depth: usize,
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
        self.prepare_run(&request, &run_id).await?;

        let lease = self.store.acquire_run_lease(&run_id).await?;
        self.run_with_mode(request, run_id, RunMode::New, lease.clone())
            .await
    }

    /// Persist a queued run before its owner advertises it. Delegation uses
    /// this to guarantee that every committed child task already has a
    /// self-contained run directory which can be resumed after a restart.
    pub(crate) async fn prepare_run(&self, request: &RunRequest, run_id: &str) -> Result<()> {
        let plan = self.plan(request);
        let record = RunRecord::new(
            run_id,
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
            plan.remaining_delegation_depth,
        )
        .with_model_modalities(plan.modalities.clone())
        .with_provider_resume_fingerprint(self.provider.resume_fingerprint());
        self.store.create_run(&record).await?;
        Ok(())
    }

    pub(super) async fn run_with_mode(
        self: &Arc<Self>,
        request: RunRequest,
        run_id: String,
        mode: RunMode,
        cancellation_lease: RunLease,
    ) -> Result<RunResult> {
        let events: SharedEventSink = Arc::new(CompositeEventSink::new(vec![
            self.store.event_sink(),
            self.extra_events.clone(),
        ]));
        let outcome = self
            .run_loop(
                request,
                run_id.clone(),
                events.clone(),
                mode,
                cancellation_lease,
            )
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
        let (profile_model, max_output_tokens) = match request.profile {
            RunProfile::Root => (self.model.clone(), self.options.max_output_tokens),
            RunProfile::GeneralTaskDelegating | RunProfile::GeneralTaskLeaf => (
                self.options
                    .general_task
                    .model
                    .clone()
                    .unwrap_or_else(|| self.model.clone()),
                self.options
                    .general_task
                    .max_output_tokens
                    .or(self.options.max_output_tokens),
            ),
        };
        let remaining_delegation_depth = request
            .remaining_delegation_depth
            .unwrap_or(self.options.max_subagent_depth);
        RunPlan {
            model: profile_model,
            modalities: self.options.model_modalities.clone(),
            max_output_tokens,
            remaining_delegation_depth,
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
