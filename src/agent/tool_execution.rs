use std::{path::Path, sync::Arc, time::Duration};

use anyhow::{Result, anyhow};
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;

use crate::{
    artifact::{ArtifactStore, ToolOutput},
    events::{RuntimeEvent, RuntimeEventKind, SharedEventSink},
    hooks::{HookEvent, HookPipeline},
    model::{Message, MessageContent, Role, ToolCall},
    tools::{RawToolOutput, ToolContext, ToolRegistry},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolExecutionMode {
    Direct,
    Background,
}

pub(crate) enum ToolExecutionOutcome {
    Completed(Box<ToolOutput>),
    Failed(anyhow::Error),
    TimedOut,
}

/// Runs the lifecycle shared by direct and background ordinary-tool calls.
/// Task state and scheduling remain the responsibility of `TaskManager`.
pub(crate) struct ToolExecutor<'a> {
    registry: &'a ToolRegistry,
    hooks: &'a HookPipeline,
    artifacts: &'a ArtifactStore,
    preview_budget: &'a Arc<Mutex<usize>>,
    events: &'a SharedEventSink,
    workspace: &'a Path,
    run_id: &'a str,
}

impl<'a> ToolExecutor<'a> {
    pub(crate) fn new(
        registry: &'a ToolRegistry,
        hooks: &'a HookPipeline,
        artifacts: &'a ArtifactStore,
        preview_budget: &'a Arc<Mutex<usize>>,
        events: &'a SharedEventSink,
        workspace: &'a Path,
        run_id: &'a str,
    ) -> Self {
        Self {
            registry,
            hooks,
            artifacts,
            preview_budget,
            events,
            workspace,
            run_id,
        }
    }

    pub(crate) async fn execute(
        &self,
        call: ToolCall,
        timeout: Duration,
        mode: ToolExecutionMode,
    ) -> Result<ToolExecutionOutcome> {
        let mut before_payload = hook_payload(self.run_id, &call, mode);
        before_payload.insert("arguments".to_owned(), call.arguments.clone());
        let before = self
            .hooks
            .run(
                HookEvent::ToolBefore,
                Value::Object(before_payload),
                self.workspace,
            )
            .await?;
        let arguments = before
            .payload
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| call.arguments.clone());
        self.events
            .emit(&RuntimeEvent::new(
                self.run_id,
                RuntimeEventKind::ToolStarted {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                },
            ))
            .await?;

        let context = ToolContext {
            run_id: self.run_id.to_owned(),
            call_id: call.id.clone(),
            workspace: self.workspace.to_owned(),
        };
        let Some(tool) = self.registry.get(&call.name) else {
            return self
                .finish_failure(&call, &context, mode, anyhow!("unknown tool"))
                .await;
        };
        match tokio::time::timeout(timeout, tool.execute(context.clone(), arguments)).await {
            Ok(Ok(raw)) => {
                let output = self.persist_output(&context, raw).await?;
                self.finish_lifecycle(&call, mode, Some(&output), output.is_error)
                    .await?;
                Ok(ToolExecutionOutcome::Completed(Box::new(output)))
            }
            Ok(Err(error)) => self.finish_failure(&call, &context, mode, error).await,
            Err(_) if mode == ToolExecutionMode::Direct => {
                let raw = failed_tool_output(
                    &call.name,
                    "direct tool call exceeded its execution timeout",
                );
                let output = self.persist_output(&context, raw).await?;
                self.finish_lifecycle(&call, mode, Some(&output), true)
                    .await?;
                Ok(ToolExecutionOutcome::Completed(Box::new(output)))
            }
            Err(_) => {
                // A timeout remains a task timeout even if a best-effort after
                // hook or event sink fails while reporting it.
                let _ = self.finish_lifecycle(&call, mode, None, true).await;
                Ok(ToolExecutionOutcome::TimedOut)
            }
        }
    }

    pub(crate) async fn persist_output(
        &self,
        context: &ToolContext,
        raw: RawToolOutput,
    ) -> Result<ToolOutput> {
        let mut preview_budget = self.preview_budget.lock().await;
        let output = self
            .artifacts
            .persist_output_with_budget(context, raw, *preview_budget)
            .await?;
        *preview_budget = preview_budget.saturating_sub(output.preview.len());
        Ok(output)
    }

    async fn finish_failure(
        &self,
        call: &ToolCall,
        context: &ToolContext,
        mode: ToolExecutionMode,
        error: anyhow::Error,
    ) -> Result<ToolExecutionOutcome> {
        if mode == ToolExecutionMode::Direct {
            let output = self
                .persist_output(
                    context,
                    failed_tool_output(&call.name, &format!("{error:#}")),
                )
                .await?;
            self.finish_lifecycle(call, mode, Some(&output), true)
                .await?;
            return Ok(ToolExecutionOutcome::Completed(Box::new(output)));
        }

        // Preserve the original tool failure as the task result. Reporting
        // failures from the after lifecycle must not change the task state.
        let _ = self.finish_lifecycle(call, mode, None, true).await;
        Ok(ToolExecutionOutcome::Failed(error))
    }

    async fn finish_lifecycle(
        &self,
        call: &ToolCall,
        mode: ToolExecutionMode,
        output: Option<&ToolOutput>,
        is_error: bool,
    ) -> Result<()> {
        if let Some(artifact) = output.and_then(|output| output.artifact.as_ref()) {
            self.events
                .emit(&RuntimeEvent::new(
                    self.run_id,
                    RuntimeEventKind::ArtifactCreated {
                        call_id: call.id.clone(),
                        path: artifact.path.clone(),
                        bytes: artifact.bytes,
                    },
                ))
                .await?;
        }
        self.events
            .emit(&RuntimeEvent::new(
                self.run_id,
                RuntimeEventKind::ToolCompleted {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                },
            ))
            .await?;
        let mut after_payload = hook_payload(self.run_id, call, mode);
        after_payload.insert(
            "truncated".to_owned(),
            Value::Bool(output.is_some_and(|output| output.truncated)),
        );
        after_payload.insert(
            "artifact".to_owned(),
            output
                .and_then(|output| output.artifact.as_ref())
                .map_or(Value::Null, |artifact| json!(artifact)),
        );
        after_payload.insert("is_error".to_owned(), Value::Bool(is_error));
        self.hooks
            .run(
                HookEvent::ToolAfter,
                Value::Object(after_payload),
                self.workspace,
            )
            .await?;
        Ok(())
    }
}

pub struct DirectToolRuntime<'a> {
    pub registry: &'a ToolRegistry,
    pub hooks: &'a HookPipeline,
    pub artifacts: &'a ArtifactStore,
    pub preview_budget: &'a Arc<Mutex<usize>>,
    pub events: &'a SharedEventSink,
    pub workspace: &'a Path,
    pub run_id: &'a str,
    pub timeout_seconds: u64,
}

impl DirectToolRuntime<'_> {
    pub async fn execute(&self, call: ToolCall) -> Result<Message> {
        let call_id = call.id.clone();
        let outcome = ToolExecutor::new(
            self.registry,
            self.hooks,
            self.artifacts,
            self.preview_budget,
            self.events,
            self.workspace,
            self.run_id,
        )
        .execute(
            call,
            Duration::from_secs(self.timeout_seconds.max(1)),
            ToolExecutionMode::Direct,
        )
        .await?;
        let ToolExecutionOutcome::Completed(output) = outcome else {
            return Err(anyhow!("direct tool execution did not produce a result"));
        };
        let metadata = output.result_metadata();
        Ok(Message {
            role: Role::Tool,
            content: vec![MessageContent::ToolResult {
                call_id,
                content: output.model_content(),
                is_error: output.is_error,
                metadata,
            }],
        })
    }
}

fn hook_payload(run_id: &str, call: &ToolCall, mode: ToolExecutionMode) -> Map<String, Value> {
    let mut payload = Map::from_iter([
        ("run_id".to_owned(), json!(run_id)),
        ("call_id".to_owned(), json!(call.id)),
        ("name".to_owned(), json!(call.name)),
    ]);
    if mode == ToolExecutionMode::Background {
        payload.insert("background".to_owned(), Value::Bool(true));
    }
    payload
}

fn failed_tool_output(name: &str, error: &str) -> RawToolOutput {
    RawToolOutput {
        content: format!("tool `{name}` failed: {error}").into_bytes(),
        source_path: None,
        media_type: "text/plain; charset=utf-8".to_owned(),
        is_error: true,
    }
}
