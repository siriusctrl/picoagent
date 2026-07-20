use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    artifact::ArtifactStore,
    events::{RuntimeEvent, RuntimeEventKind, SharedEventSink},
    hooks::{HookEvent, HookPipeline},
    memory::MemoryPaths,
    model::{Message, MessageContent, ModelProvider, ModelRequest, Role},
    prompts::agent_prompts,
    storage::{RunDirStore, RunLease, RunState},
    tools::{RunToolAssembly, ToolRegistry},
    trajectory::LocalTrajectoryReader,
};

use super::{
    compaction::{
        CompactionAttempt, build_active_context, estimate_message_tokens,
        estimate_request_input_tokens, maybe_compact,
    },
    context::build_runtime_reminder,
    task::{TaskManager, TaskManagerConfig},
    tool_execution::DirectToolRuntime,
};

pub use super::types::{AgentRunnerConfig, RunRequest, RunResult, RunnerOptions};

mod lifecycle;
mod recovery;

use lifecycle::RunMode;
use recovery::{
    append_background_results, append_interrupted_tool_results, remaining_preview_budget,
    resumable_final_text,
};

pub struct AgentRunner {
    provider: Arc<dyn ModelProvider>,
    model: String,
    workspace: PathBuf,
    skill_catalog: String,
    base_tools: ToolRegistry,
    artifacts: ArtifactStore,
    store: RunDirStore,
    hooks: HookPipeline,
    memory: Option<MemoryPaths>,
    extra_events: SharedEventSink,
    options: RunnerOptions,
    model_slots: Arc<tokio::sync::Semaphore>,
}

impl AgentRunner {
    pub fn new(config: AgentRunnerConfig) -> Arc<Self> {
        let model_slots = Arc::new(tokio::sync::Semaphore::new(
            config.options.max_parallel_model_calls.max(1),
        ));
        Arc::new(Self {
            provider: config.provider,
            model: config.model,
            workspace: config.workspace,
            skill_catalog: config.skill_catalog,
            base_tools: config.tools,
            artifacts: config.artifacts,
            store: config.store,
            hooks: config.hooks,
            memory: config.memory,
            extra_events: config.extra_events,
            options: config.options,
            model_slots,
        })
    }

    pub fn store(&self) -> &RunDirStore {
        &self.store
    }

    async fn run_loop(
        self: &Arc<Self>,
        request: RunRequest,
        run_id: String,
        events: SharedEventSink,
        mode: RunMode,
        cancellation_lease: RunLease,
    ) -> Result<RunResult> {
        let plan = self.plan(&request);
        let model = plan.model.clone();
        let max_output_tokens = plan.max_output_tokens;
        self.store.update_state(&run_id, RunState::Running).await?;
        let (mut trajectory, needs_initial_message) = match mode {
            RunMode::New => {
                events
                    .emit(&RuntimeEvent::new(
                        &run_id,
                        RuntimeEventKind::RunStarted {
                            prompt: request.prompt.clone(),
                        },
                    ))
                    .await?;
                self.hooks
                    .run(
                        HookEvent::RunStart,
                        json!({ "run_id": run_id, "prompt": request.prompt, "depth": request.depth }),
                        &self.workspace,
                    )
                    .await?;
                (Vec::new(), true)
            }
            RunMode::Resume => {
                let trajectory = self.store.load_trajectory(&run_id).await?;
                let needs_initial_message = trajectory.is_empty();
                events
                    .emit(&RuntimeEvent::new(
                        &run_id,
                        RuntimeEventKind::RunResumed {
                            completed_messages: trajectory.len(),
                        },
                    ))
                    .await?;
                (trajectory, needs_initial_message)
            }
        };

        let system = agent_prompts().system.clone();
        if needs_initial_message {
            let runtime_reminder = build_runtime_reminder(
                &self.workspace,
                &self.skill_catalog,
                self.memory.as_ref(),
                request.additional_instructions.as_deref(),
            )?;
            let user_message = Message {
                role: Role::User,
                content: vec![
                    MessageContent::RuntimeReminder {
                        text: runtime_reminder,
                    },
                    MessageContent::Text {
                        text: request.prompt.clone(),
                    },
                ],
            };
            trajectory.push(self.store.append_message(&run_id, &user_message).await?);
        }
        let tool_assembly = RunToolAssembly::new(
            self.base_tools.clone(),
            Arc::new(LocalTrajectoryReader::new(self.store.clone())),
            self.options.compaction.history_search_max_matches,
        )?;
        let automatic_compaction_enabled = self
            .options
            .compaction
            .compact_at_tokens
            .is_some_and(|tokens| tokens > 0)
            && (tool_assembly.contains("read") || tool_assembly.contains("bash"));

        let tool_preview_budget = Arc::new(tokio::sync::Mutex::new(remaining_preview_budget(
            self.artifacts.policy().max_inline_bytes_per_run,
            &trajectory,
        )));
        let task_config = TaskManagerConfig {
            runner: self.clone(),
            candidate_tools: tool_assembly.task_candidates(),
            artifacts: self.artifacts.clone(),
            preview_budget: tool_preview_budget.clone(),
            store: self.store.clone(),
            workspace: self.workspace.clone(),
            parent_run_id: run_id.clone(),
            parent_depth: request.depth,
            child_can_delegate: request.depth + 1 < self.options.max_subagent_depth,
            events: events.clone(),
            hooks: self.hooks.clone(),
            max_parallel_tasks: self.options.max_parallel_tasks,
            wait_timeout_seconds: self.options.task_wait_timeout_seconds,
        };
        let manager = if mode == RunMode::Resume {
            TaskManager::load_for_resume(task_config).await?
        } else {
            TaskManager::new(task_config)
        };
        let task_manager = manager;
        let registry = tool_assembly.finish(task_manager.clone(), plan.may_delegate)?;
        // Freeze the model-facing schema set once per run. Tool execution keeps
        // using the registry, but every normal model request receives the exact
        // same sorted schema prefix.
        let tool_specs = registry.specs();
        let tool_schema_sha256 = format!("{:x}", Sha256::digest(serde_json::to_vec(&tool_specs)?));
        self.store
            .verify_tool_schema(&run_id, &tool_schema_sha256)
            .await?;
        let recoverable_subagents = if mode == RunMode::Resume {
            task_manager.reconcile_after_restart().await?
        } else {
            Vec::new()
        };
        let mut task_guard = task_manager.cancellation_guard(cancellation_lease);
        let direct_tools = DirectToolRuntime {
            registry: &registry,
            hooks: &self.hooks,
            artifacts: &self.artifacts,
            preview_budget: &tool_preview_budget,
            events: &events,
            workspace: &self.workspace,
            run_id: &run_id,
            manager: task_manager.clone(),
            foreground_timeout_seconds: self.options.foreground_tool_timeout_seconds,
        };
        for task in recoverable_subagents {
            task_manager.resume_agent_task(task).await?;
        }
        let outcome: Result<RunResult> = async {
            let completed_steps = trajectory
                .iter()
                .filter(|record| {
                    record.compaction.is_none() && record.message.role == Role::Assistant
                })
                .count();
            if mode == RunMode::Resume {
                let interrupted_preview_bytes =
                    append_interrupted_tool_results(&self.store, &run_id, &mut trajectory).await?;
                let mut remaining = tool_preview_budget.lock().await;
                *remaining = remaining.saturating_sub(interrupted_preview_bytes);
                drop(remaining);
                let resumed_inputs = self
                    .store
                    .append_pending_inputs(&run_id, &mut trajectory)
                    .await?;
                if resumed_inputs.is_empty()
                    && let Some(final_text) = resumable_final_text(&trajectory)
                {
                    let ready = task_manager.pending_before_finish().await?;
                    if ready.is_empty() {
                        let input_lock = self.store.pending_input_lock();
                        let _input_guard = input_lock.lock().await;
                        let steered = self
                            .store
                            .append_pending_inputs_locked(&run_id, &mut trajectory)
                            .await?;
                        if steered.is_empty() {
                            self.finish_success(&run_id, &final_text, events.clone())
                                .await?;
                            return Ok(RunResult {
                                run_id: run_id.clone(),
                                final_output: final_text,
                            });
                        }
                    } else {
                        append_background_results(&self.store, &run_id, &mut trajectory, &ready)
                            .await?;
                        task_manager.mark_delivered(&ready).await?;
                    }
                }
            }

            let mut context_tokens = estimate_request_input_tokens(
                &system,
                &build_active_context(&trajectory)?,
                &tool_specs,
            );

            let mut step = completed_steps.saturating_add(1);
            loop {
                let ready = task_manager.drain_completed().await?;
                let added =
                    append_background_results(&self.store, &run_id, &mut trajectory, &ready)
                        .await?;
                task_manager.mark_delivered(&ready).await?;
                context_tokens = context_tokens.saturating_add(added);
                let steered = self
                    .store
                    .append_pending_inputs(&run_id, &mut trajectory)
                    .await?;
                context_tokens = steered.iter().fold(context_tokens, |total, record| {
                    total.saturating_add(estimate_message_tokens(&record.message))
                });

                if automatic_compaction_enabled
                    && let Some(completed) = maybe_compact(CompactionAttempt {
                        provider: &self.provider,
                        model: &model,
                        run_id: &run_id,
                        system: &system,
                        tools: &tool_specs,
                        trajectory: &trajectory,
                        tokens_before: context_tokens,
                        options: &self.options.compaction,
                        store: &self.store,
                        events: &events,
                        model_slots: &self.model_slots,
                        stream_idle_timeout_seconds: self.options.model_stream_idle_timeout_seconds,
                        request_deadline_seconds: self.options.model_request_deadline_seconds,
                    })
                    .await?
                {
                    context_tokens = completed.estimated_context_tokens;
                    trajectory.extend(completed.records);
                }
                let active_messages = build_active_context(&trajectory)?;
                context_tokens = context_tokens.max(estimate_request_input_tokens(
                    &system,
                    &active_messages,
                    &tool_specs,
                ));
                if let Some(context_window) = self.options.compaction.context_window_tokens {
                    let reserved_output = max_output_tokens.ok_or_else(|| {
                        anyhow::anyhow!(
                            "context_window_tokens requires a configured max_output_tokens for this agent profile"
                        )
                    })? as u64;
                    let estimated_total = context_tokens.saturating_add(reserved_output);
                    if estimated_total >= context_window {
                        bail!(
                            "estimated model context is {estimated_total} tokens ({context_tokens} input + {reserved_output} configured output), at or above context_window_tokens={context_window}; compaction did not reduce it enough"
                        )
                    }
                }

                events
                    .emit(&RuntimeEvent::new(
                        &run_id,
                        RuntimeEventKind::ModelStarted { step },
                    ))
                    .await?;
                let model_permit = self
                    .model_slots
                    .acquire()
                    .await
                    .context("model concurrency limiter closed")?;
                let response = tokio::time::timeout(
                    Duration::from_secs(self.options.model_request_deadline_seconds),
                    self.provider.complete(
                        ModelRequest {
                            run_id: run_id.clone(),
                            model: model.clone(),
                            system: system.clone(),
                            messages: active_messages,
                            tools: tool_specs.clone(),
                            max_output_tokens,
                            stream_idle_timeout: Duration::from_secs(
                                self.options.model_stream_idle_timeout_seconds,
                            ),
                        },
                        events.clone(),
                    ),
                )
                .await;
                drop(model_permit);
                let response = response
                    .with_context(|| {
                        format!(
                            "{} model request deadline exceeded {} seconds",
                            self.provider.name(),
                            self.options.model_request_deadline_seconds
                        )
                    })?
                    .with_context(|| format!("{} model call failed", self.provider.name()))?;
                response.validate_completed()?;
                events
                    .emit(&RuntimeEvent::new(
                        &run_id,
                        RuntimeEventKind::ModelCompleted {
                            step,
                            input_tokens: response.usage.input_tokens,
                            output_tokens: response.usage.output_tokens,
                            cached_input_tokens: response.usage.cached_input_tokens,
                            reasoning_tokens: response.usage.reasoning_tokens,
                        },
                    ))
                    .await?;
                let final_text = response.text();
                let tool_calls = response.tool_calls();
                let assistant_message = response.assistant;
                context_tokens = response
                    .usage
                    .input_tokens
                    .unwrap_or(context_tokens)
                    .saturating_add(estimate_message_tokens(&assistant_message));
                let assistant_record = self
                    .store
                    .append_message(&run_id, &assistant_message)
                    .await?;
                trajectory.push(assistant_record);

                if tool_calls.is_empty() {
                    let steered = self
                        .store
                        .append_pending_inputs(&run_id, &mut trajectory)
                        .await?;
                    if !steered.is_empty() {
                        context_tokens = steered.iter().fold(context_tokens, |total, record| {
                            total.saturating_add(estimate_message_tokens(&record.message))
                        });
                        step = step.saturating_add(1);
                        continue;
                    }
                    let ready = task_manager.pending_before_finish().await?;
                    if !ready.is_empty() {
                        let added = append_background_results(
                            &self.store,
                            &run_id,
                            &mut trajectory,
                            &ready,
                        )
                        .await?;
                        task_manager.mark_delivered(&ready).await?;
                        context_tokens = context_tokens.saturating_add(added);
                        step = step.saturating_add(1);
                        continue;
                    }
                    let input_lock = self.store.pending_input_lock();
                    let _input_guard = input_lock.lock().await;
                    let steered = self
                        .store
                        .append_pending_inputs_locked(&run_id, &mut trajectory)
                        .await?;
                    if !steered.is_empty() {
                        context_tokens = steered.iter().fold(context_tokens, |total, record| {
                            total.saturating_add(estimate_message_tokens(&record.message))
                        });
                        step = step.saturating_add(1);
                        continue;
                    }
                    self.finish_success(&run_id, &final_text, events.clone())
                        .await?;
                    return Ok(RunResult {
                        run_id: run_id.clone(),
                        final_output: final_text,
                    });
                }

                for call in tool_calls {
                    let tool_message = direct_tools.execute(call).await?;
                    let record = self.store.append_message(&run_id, &tool_message).await?;
                    context_tokens =
                        context_tokens.saturating_add(estimate_message_tokens(&tool_message));
                    trajectory.push(record);
                }
                step = step.saturating_add(1);
            }
        }
        .await;
        if outcome.is_err() {
            task_manager
                .abort_and_settle("parent run ended before background task completion")
                .await;
        }
        task_guard.disarm();
        outcome
    }
}
