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
    /// Run a new explicitly requested activity on an existing child thread.
    ChildActivity,
    /// Resume only the root transcript after its owning process stopped.
    RootRestart,
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
        self.run_with_mode(request, run_id, RunMode::New, lease.clone(), None)
            .await
    }

    /// Persist a queued run before its owner advertises the new agent handle.
    pub(crate) async fn prepare_run(&self, request: &RunRequest, run_id: &str) -> Result<()> {
        let plan = self.plan(request);
        let mut record = RunRecord::new(
            run_id,
            &request.name,
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
        if request.parent_run_id.is_some() {
            record.state = RunState::Open;
        }
        self.store.create_run(&record).await?;
        Ok(())
    }

    pub(super) async fn run_with_mode(
        self: &Arc<Self>,
        request: RunRequest,
        run_id: String,
        mode: RunMode,
        cancellation_lease: RunLease,
        cleanup_done: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> Result<RunResult> {
        let is_child = request.parent_run_id.is_some();
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
                cleanup_done,
            )
            .await;
        if let Err(error) = &outcome {
            if !is_child {
                let _ = self.store.update_state(&run_id, RunState::Failed).await;
            }
            let kind = if is_child {
                RuntimeEventKind::RunActivityFailed {
                    error: format!("{error:#}"),
                }
            } else {
                RuntimeEventKind::RunFailed {
                    error: format!("{error:#}"),
                }
            };
            let _ = events.emit(&RuntimeEvent::new(&run_id, kind)).await;
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
        let run = self.store.load_run(run_id).await?;
        let is_child = run.parent_run_id.is_some();
        if !is_child {
            self.store.update_state(run_id, RunState::Completed).await?;
        }
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
        let kind = if is_child {
            RuntimeEventKind::RunActivityCompleted {
                final_output: final_output.to_owned(),
            }
        } else {
            RuntimeEventKind::RunCompleted {
                final_output: final_output.to_owned(),
            }
        };
        if let Err(error) = events.emit(&RuntimeEvent::new(run_id, kind)).await {
            tracing::warn!(run_id, error = %format!("{error:#}"), "run completion event failed after completion");
        }
        Ok(())
    }
}
