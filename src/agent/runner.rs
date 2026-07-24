use std::{path::PathBuf, sync::Arc};

use anyhow::{Result, bail};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    artifact::ArtifactStore,
    events::{RuntimeEvent, RuntimeEventKind, SharedEventSink},
    hooks::{HookEvent, HookPipeline},
    memory::MemoryPaths,
    model::{Message, MessageContent, ModelProvider, ModelRequest, Role},
    prompts::agent_prompts,
    storage::{RunDirStore, RunLease},
    tools::{RunToolAssembly, ToolRegistry},
    trajectory::LocalTrajectoryReader,
};

use super::{
    compaction::{
        CompactionAttempt, build_active_context, estimate_message_tokens,
        estimate_request_input_tokens, maybe_compact,
    },
    context::{append_active_handle_reminder, build_runtime_reminder},
    handle::{
        AgentMailbox, PendingHandleBoundary, RuntimeHandleManager, RuntimeHandleManagerConfig,
    },
    tool_execution::DirectToolRuntime,
};

pub use super::types::{AgentRunnerConfig, RunRequest, RunResult, RunnerOptions};

mod lifecycle;
mod model_request;
mod recovery;

use lifecycle::RunMode;
use recovery::{append_handle_results, append_restart_reminder};

pub struct AgentRunner {
    provider: Arc<dyn ModelProvider>,
    model: String,
    workspace: PathBuf,
    skill_catalog: String,
    mcp_catalog: String,
    base_tools: ToolRegistry,
    artifacts: ArtifactStore,
    store: RunDirStore,
    hooks: HookPipeline,
    memory: Option<MemoryPaths>,
    extra_events: SharedEventSink,
    options: RunnerOptions,
    model_slots: Arc<tokio::sync::Semaphore>,
}

struct RunInvocation {
    mode: RunMode,
    mailbox: Option<AgentMailbox>,
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
            mcp_catalog: config.mcp_catalog,
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
        invocation: RunInvocation,
        cancellation_lease: RunLease,
        cleanup_done: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> Result<RunResult> {
        let RunInvocation { mode, mailbox } = invocation;
        let plan = self.plan(&request);
        let model = plan.model.clone();
        let max_output_tokens = plan.max_output_tokens;
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
                        json!({ "run_id": run_id, "prompt": request.prompt }),
                        &self.workspace,
                    )
                    .await?;
                (Vec::new(), true)
            }
            RunMode::ChildActivity | RunMode::RootRestart => {
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
                &plan.modalities,
                &self.skill_catalog,
                &self.mcp_catalog,
                self.memory.as_ref(),
                request.profile,
                plan.remaining_delegation_depth,
            )?;
            let user_message = Message::new(
                Role::User,
                vec![
                    MessageContent::RuntimeReminder {
                        text: runtime_reminder,
                    },
                    MessageContent::Text {
                        text: request.prompt.clone(),
                    },
                ],
            );
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

        let handle_config = RuntimeHandleManagerConfig {
            runner: self.clone(),
            artifacts: self.artifacts.clone(),
            store: self.store.clone(),
            workspace: self.workspace.clone(),
            parent_run_id: run_id.clone(),
            remaining_delegation_depth: plan.remaining_delegation_depth,
            events: events.clone(),
            max_parallel_subagents: self.options.max_parallel_subagents,
            wait_timeout_seconds: self.options.handle_wait_timeout_seconds,
        };
        let handles = RuntimeHandleManager::new(handle_config);
        let registry = tool_assembly.finish(handles.clone())?;
        // Freeze the model-facing schema set once per run. Tool execution keeps
        // using the registry, but every normal model request receives the exact
        // same sorted schema prefix.
        let tool_specs = registry.specs();
        let tool_schema_sha256 = format!("{:x}", Sha256::digest(serde_json::to_vec(&tool_specs)?));
        self.store
            .verify_tool_schema(&run_id, &tool_schema_sha256)
            .await?;
        if mode == RunMode::RootRestart {
            append_restart_reminder(&self.store, &run_id, &mut trajectory).await?;
        }
        let mut handle_guard = handles.cancellation_guard(cancellation_lease, cleanup_done);
        let direct_tools = DirectToolRuntime {
            registry: &registry,
            hooks: &self.hooks,
            artifacts: &self.artifacts,
            events: &events,
            workspace: &self.workspace,
            run_id: &run_id,
            handles: handles.clone(),
            foreground_timeout_seconds: self.options.foreground_tool_timeout_seconds,
        };
        let outcome: Result<RunResult> = async {
            let completed_steps = trajectory
                .iter()
                .filter(|record| {
                    record.compaction.is_none() && record.message.role == Role::Assistant
                })
                .count();
            if mode == RunMode::ChildActivity
                && let Some(mailbox) = &mailbox
            {
                mailbox
                    .append_messages(&self.store, &run_id, &mut trajectory)
                    .await?;
            }

            let mut context_tokens = estimate_request_input_tokens(
                &system,
                &build_active_context(&trajectory)?,
                &tool_specs,
            );

            let mut step = completed_steps.saturating_add(1);
            loop {
                let ready = handles.drain_ready_outputs().await;
                let added =
                    append_handle_results(&self.store, &run_id, &mut trajectory, &ready).await?;
                context_tokens = context_tokens.saturating_add(added);
                let steered = match &mailbox {
                    Some(mailbox) => {
                        mailbox
                            .append_messages(&self.store, &run_id, &mut trajectory)
                            .await?
                    }
                    None => Vec::new(),
                };
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
                // Compaction is a model call and can take long enough for
                // background work to finish. Snapshot again afterwards.
                let (ready, active_handles) = handles.snapshot_for_request().await;
                let added = append_handle_results(
                    &self.store,
                    &run_id,
                    &mut trajectory,
                    &ready,
                )
                .await?;
                context_tokens = context_tokens.saturating_add(added);
                let mut active_messages = build_active_context(&trajectory)?;
                append_active_handle_reminder(&mut active_messages, &active_handles);
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

                let response = self
                    .complete_model_step(
                        &run_id,
                        step,
                        ModelRequest {
                            run_id: run_id.clone(),
                            model: model.clone(),
                            system: system.clone(),
                            messages: active_messages,
                            tools: tool_specs.clone(),
                            max_output_tokens,
                            stream_idle_timeout: std::time::Duration::from_secs(
                                self.options.model_stream_idle_timeout_seconds,
                            ),
                        },
                        events.clone(),
                    )
                    .await?;
                let final_text = response.text();
                let tool_calls = response.tool_calls();
                let assistant_message = response.assistant;
                context_tokens = response
                    .usage
                    .input_tokens
                    .unwrap_or(context_tokens)
                    .saturating_add(estimate_message_tokens(&assistant_message));

                if tool_calls.is_empty() {
                    let assistant_record = self
                        .store
                        .append_message(&run_id, &assistant_message)
                        .await?;
                    trajectory.push(assistant_record);
                    let steered = match &mailbox {
                        Some(mailbox) => {
                            mailbox
                                .finish_boundary(&self.store, &run_id, &mut trajectory)
                                .await?
                        }
                        None => Vec::new(),
                    };
                    if !steered.is_empty() {
                        context_tokens = steered.iter().fold(context_tokens, |total, record| {
                            total.saturating_add(estimate_message_tokens(&record.message))
                        });
                        step = step.saturating_add(1);
                        continue;
                    }
                    match handles.pending_before_finish().await? {
                        PendingHandleBoundary::Ready(ready) => {
                            let added = append_handle_results(
                                &self.store,
                                &run_id,
                                &mut trajectory,
                                &ready,
                            )
                            .await?;
                            context_tokens = context_tokens.saturating_add(added);
                            step = step.saturating_add(1);
                            continue;
                        }
                        PendingHandleBoundary::Active => {
                            step = step.saturating_add(1);
                            continue;
                        }
                        PendingHandleBoundary::None => {}
                    }
                    self.finish_success(&run_id, &final_text, events.clone())
                        .await?;
                    return Ok(RunResult {
                        run_id: run_id.clone(),
                        final_output: final_text,
                    });
                }

                let tool_messages = direct_tools.execute_batch(tool_calls).await?;
                let mut messages = Vec::with_capacity(tool_messages.len().saturating_add(1));
                messages.push(assistant_message);
                for tool_message in tool_messages {
                    context_tokens =
                        context_tokens.saturating_add(estimate_message_tokens(&tool_message));
                    messages.push(tool_message);
                }
                trajectory.extend(self.store.append_messages(&run_id, &messages).await?);
                step = step.saturating_add(1);
            }
        }
        .await;
        if outcome.is_err() {
            handles.abort_and_settle().await;
        }
        handle_guard.disarm();
        outcome
    }
}
