use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result, bail};
use serde_json::json;
use ulid::Ulid;

use crate::{
    artifact::ArtifactStore,
    events::{CompositeEventSink, RuntimeEvent, RuntimeEventKind, SharedEventSink},
    hooks::{HookEvent, HookPipeline},
    memory::{MemoryPaths, MemoryUpdateTool},
    model::{Message, MessageContent, ModelProvider, ModelRequest, Role},
    storage::{RunDirStore, RunRecord, RunState},
    tools::{ToolRegistry, register_history_tools},
    trajectory::{LocalTrajectoryReader, TrajectoryMessage},
};

use super::{
    compaction::{CompactionAttempt, build_active_context, estimate_message_tokens, maybe_compact},
    context::{build_runtime_reminder, build_system_prompt},
    task::{BackgroundTaskRecord, SpawnTool, TaskManager, TaskManagerConfig, WaitTool},
    tool_execution::DirectToolRuntime,
};

pub use super::types::{AgentRunnerConfig, RunRequest, RunResult, RunnerOptions};

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
}

impl AgentRunner {
    pub fn new(config: AgentRunnerConfig) -> Arc<Self> {
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
        })
    }

    pub fn store(&self) -> &RunDirStore {
        &self.store
    }

    pub async fn run(self: &Arc<Self>, request: RunRequest) -> Result<RunResult> {
        self.run_with_id(request, Ulid::new().to_string()).await
    }

    pub(crate) async fn run_with_id(
        self: &Arc<Self>,
        request: RunRequest,
        run_id: String,
    ) -> Result<RunResult> {
        let (model, _, _) = self.profile(&request);
        let record = RunRecord::new(
            &run_id,
            &request.prompt,
            self.provider.name(),
            model,
            self.workspace.clone(),
            request.parent_run_id.clone(),
        );
        self.store.create_run(&record).await?;

        let events: SharedEventSink = Arc::new(CompositeEventSink::new(vec![
            self.store.event_sink(),
            self.extra_events.clone(),
        ]));
        let outcome = self
            .run_created(request, run_id.clone(), events.clone())
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

    fn profile<'a>(&'a self, request: &RunRequest) -> (&'a str, usize, Option<u32>) {
        if request.use_general_task_profile {
            (
                self.options
                    .general_task
                    .model
                    .as_deref()
                    .unwrap_or(&self.model),
                self.options.general_task.max_steps,
                self.options.general_task.max_output_tokens,
            )
        } else {
            (
                &self.model,
                self.options.max_steps,
                self.options.max_output_tokens,
            )
        }
    }

    async fn run_created(
        self: &Arc<Self>,
        request: RunRequest,
        run_id: String,
        events: SharedEventSink,
    ) -> Result<RunResult> {
        let (model, max_steps, max_output_tokens) = self.profile(&request);
        let model = model.to_owned();
        self.store.update_state(&run_id, RunState::Running).await?;
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

        let artifact_inspection_enabled = ["read", "bash"].iter().any(|name| {
            self.base_tools.get(name).is_some() && allowed(&request.tool_allowlist, name)
        });
        let compaction_enabled = self
            .options
            .compaction
            .trigger_tokens
            .is_some_and(|tokens| tokens > 0)
            && allowed(&request.tool_allowlist, "history_search")
            && allowed(&request.tool_allowlist, "history_read")
            && artifact_inspection_enabled;
        let system = build_system_prompt(compaction_enabled);
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
        let user_record = self.store.append_message(&run_id, &user_message).await?;
        let mut trajectory = vec![user_record];
        let mut registry = self.base_tools.clone();
        if compaction_enabled {
            register_history_tools(
                &mut registry,
                Arc::new(LocalTrajectoryReader::with_local_artifacts(
                    Arc::new(self.store.clone()),
                    self.workspace.clone(),
                )),
                self.options.compaction.history_search_max_matches,
            )?;
        }
        if let Some(allowlist) = &request.tool_allowlist {
            registry.retain(allowlist);
        }

        let may_delegate = request.depth < self.options.max_subagent_depth;
        if may_delegate
            && allowed(&request.tool_allowlist, "memory_update")
            && let Some(memory) = &self.memory
        {
            registry.register(Arc::new(MemoryUpdateTool::new(
                self.clone(),
                memory.clone(),
                run_id.clone(),
                request.depth,
                events.clone(),
            )))?;
        }

        let tool_preview_budget = Arc::new(tokio::sync::Mutex::new(
            self.artifacts.policy().max_inline_bytes_per_run,
        ));
        let task_manager = if may_delegate
            && (allowed(&request.tool_allowlist, "spawn")
                || allowed(&request.tool_allowlist, "wait"))
        {
            let manager = TaskManager::new(TaskManagerConfig {
                runner: self.clone(),
                tools: registry.clone(),
                artifacts: self.artifacts.clone(),
                preview_budget: tool_preview_budget.clone(),
                store: self.store.clone(),
                workspace: self.workspace.clone(),
                parent_run_id: run_id.clone(),
                parent_depth: request.depth,
                events: events.clone(),
                hooks: self.hooks.clone(),
                max_parallel_tasks: self.options.max_parallel_tasks,
                default_execution_timeout_seconds: self.options.task_execution_timeout_seconds,
                default_wait_timeout_seconds: self.options.task_wait_timeout_seconds,
                max_execution_timeout_seconds: self.options.task_max_timeout_seconds,
            });
            if allowed(&request.tool_allowlist, "spawn") {
                registry.register(Arc::new(SpawnTool::new(manager.clone())))?;
            }
            if allowed(&request.tool_allowlist, "wait") {
                registry.register(Arc::new(WaitTool::new(manager.clone())))?;
            }
            Some(manager)
        } else {
            None
        };
        let direct_tools = DirectToolRuntime {
            registry: &registry,
            hooks: &self.hooks,
            artifacts: &self.artifacts,
            preview_budget: &tool_preview_budget,
            events: &events,
            workspace: &self.workspace,
            run_id: &run_id,
            timeout_seconds: self.options.direct_tool_timeout_seconds,
        };
        let outcome: Result<RunResult> = async {
            let mut latest_checkpoint = self.store.load_latest_compaction(&run_id).await?;
            let mut context_tokens: Option<u64> = None;
            for step in 1..=max_steps {
                if let Some(manager) = &task_manager {
                    let ready = manager.drain_completed().await?;
                    let added =
                        append_background_results(&self.store, &run_id, &mut trajectory, ready)
                            .await?;
                    if let Some(tokens) = &mut context_tokens {
                        *tokens = tokens.saturating_add(added);
                    }
                }

                if compaction_enabled
                    && let Some(checkpoint) = maybe_compact(CompactionAttempt {
                        provider: &self.provider,
                        model: &model,
                        run_id: &run_id,
                        trajectory: &trajectory,
                        previous: latest_checkpoint.as_ref(),
                        tokens_before: context_tokens,
                        options: &self.options.compaction,
                        store: &self.store,
                        events: &events,
                    })
                    .await?
                {
                    latest_checkpoint = Some(checkpoint);
                }
                let active_messages =
                    build_active_context(&trajectory, latest_checkpoint.as_ref())?;

                events
                    .emit(&RuntimeEvent::new(
                        &run_id,
                        RuntimeEventKind::ModelStarted { step },
                    ))
                    .await?;
                let response = self
                    .provider
                    .complete(
                        ModelRequest {
                            run_id: run_id.clone(),
                            model: model.clone(),
                            system: system.clone(),
                            messages: active_messages,
                            tools: registry.specs(),
                            max_output_tokens,
                        },
                        events.clone(),
                    )
                    .await
                    .with_context(|| format!("{} model call failed", self.provider.name()))?;
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
                let assistant_content = if response.assistant_content.is_empty() {
                    let mut content = Vec::new();
                    if !response.text.is_empty() {
                        content.push(MessageContent::Text {
                            text: response.text.clone(),
                        });
                    }
                    content.extend(response.tool_calls.iter().map(|call| {
                        MessageContent::ToolCall {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                        }
                    }));
                    content
                } else {
                    response.assistant_content.clone()
                };
                let assistant_message = Message {
                    role: Role::Assistant,
                    content: assistant_content,
                };
                context_tokens = response.usage.input_tokens.map(|tokens| {
                    tokens.saturating_add(estimate_message_tokens(&assistant_message))
                });
                let assistant_record = self
                    .store
                    .append_message(&run_id, &assistant_message)
                    .await?;
                trajectory.push(assistant_record);

                if response.tool_calls.is_empty() {
                    if let Some(manager) = &task_manager {
                        let ready = manager.settle_before_finish().await?;
                        if !ready.is_empty() {
                            let added = append_background_results(
                                &self.store,
                                &run_id,
                                &mut trajectory,
                                ready,
                            )
                            .await?;
                            if let Some(tokens) = &mut context_tokens {
                                *tokens = tokens.saturating_add(added);
                            }
                            continue;
                        }
                    }
                    self.finish_success(&run_id, &response.text, events.clone())
                        .await?;
                    return Ok(RunResult {
                        run_id: run_id.clone(),
                        final_output: response.text,
                    });
                }

                for call in response.tool_calls {
                    let tool_message = direct_tools.execute(call).await?;
                    let record = self.store.append_message(&run_id, &tool_message).await?;
                    if let Some(tokens) = &mut context_tokens {
                        *tokens = tokens.saturating_add(estimate_message_tokens(&tool_message));
                    }
                    trajectory.push(record);
                }
            }

            bail!("run exceeded the maximum of {max_steps} model steps")
        }
        .await;
        if outcome.is_err()
            && let Some(manager) = &task_manager
        {
            manager
                .abort_and_settle("parent run ended before background task completion")
                .await;
        }
        outcome
    }

    async fn finish_success(
        &self,
        run_id: &str,
        final_output: &str,
        events: SharedEventSink,
    ) -> Result<()> {
        self.store.write_final(run_id, final_output).await?;
        self.hooks
            .run(
                HookEvent::RunEnd,
                json!({ "run_id": run_id, "final_output": final_output }),
                &self.workspace,
            )
            .await?;
        self.store.update_state(run_id, RunState::Completed).await?;
        events
            .emit(&RuntimeEvent::new(
                run_id,
                RuntimeEventKind::RunCompleted {
                    final_output: final_output.to_owned(),
                },
            ))
            .await
    }
}

fn allowed(allowlist: &Option<Vec<String>>, name: &str) -> bool {
    allowlist
        .as_ref()
        .is_none_or(|items| items.iter().any(|item| item == name))
}

async fn append_background_results(
    store: &RunDirStore,
    run_id: &str,
    trajectory: &mut Vec<TrajectoryMessage>,
    records: Vec<BackgroundTaskRecord>,
) -> Result<u64> {
    let mut estimated_tokens = 0_u64;
    for record in records {
        let status = record.status().to_owned();
        let content = record.model_content();
        let message = Message {
            role: Role::User,
            content: vec![MessageContent::BackgroundTaskResult {
                task_id: record.id,
                name: record.name,
                status,
                content,
            }],
        };
        let trajectory_record = store.append_message(run_id, &message).await?;
        estimated_tokens = estimated_tokens.saturating_add(estimate_message_tokens(&message));
        trajectory.push(trajectory_record);
    }
    Ok(estimated_tokens)
}
