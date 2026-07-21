use std::{
    future::{Future, poll_fn},
    path::Path,
    pin::Pin,
    sync::Arc,
    task::Poll,
    time::Duration,
};

use crate::{
    agent::task::TaskManager,
    artifact::{ArtifactStore, ResultMetadata, ToolOutput},
    events::{RuntimeEvent, RuntimeEventKind, SharedEventSink},
    hooks::{HookEvent, HookPipeline},
    model::{ImageAttachment, Message, MessageContent, Role, ToolCall},
    tools::{RawToolOutput, ToolContext, ToolRegistry},
};
use anyhow::{Result, anyhow};
use serde_json::{Map, Value, json};

pub(crate) type ToolExecutionFuture =
    Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + 'static>>;

/// Runs the lifecycle shared by direct and background ordinary-tool calls.
/// Task state and scheduling remain the responsibility of `TaskManager`.
pub(crate) struct ToolExecutor<'a> {
    registry: &'a ToolRegistry,
    hooks: &'a HookPipeline,
    artifacts: &'a ArtifactStore,
    events: &'a SharedEventSink,
    workspace: &'a Path,
    run_id: &'a str,
}

impl<'a> ToolExecutor<'a> {
    pub(crate) fn new(
        registry: &'a ToolRegistry,
        hooks: &'a HookPipeline,
        artifacts: &'a ArtifactStore,
        events: &'a SharedEventSink,
        workspace: &'a Path,
        run_id: &'a str,
    ) -> Self {
        Self {
            registry,
            hooks,
            artifacts,
            events,
            workspace,
            run_id,
        }
    }

    pub(crate) async fn execute(&self, call: ToolCall) -> Result<ToolOutput> {
        let arguments = match call.arguments.parse() {
            Ok(arguments) => arguments,
            Err(error) => {
                self.events
                    .emit(&RuntimeEvent::new(
                        self.run_id,
                        RuntimeEventKind::ToolStarted {
                            call_id: call.id.clone(),
                            name: call.name.clone(),
                        },
                    ))
                    .await?;
                return self
                    .finish_failure(
                        &call,
                        &ToolContext {
                            run_id: self.run_id.to_owned(),
                            call_id: call.id.clone(),
                            workspace: self.workspace.to_owned(),
                        },
                        error.context("the tool was not executed"),
                    )
                    .await;
            }
        };
        let mut before_payload = hook_payload(self.run_id, &call);
        before_payload.insert("arguments".to_owned(), arguments.clone());
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
            .unwrap_or(arguments);
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
                .finish_failure(&call, &context, anyhow!("unknown tool"))
                .await;
        };
        match tool.execute(context.clone(), arguments).await {
            Ok(raw) => {
                let output = self.persist_output(&context, raw).await?;
                self.finish_lifecycle(&call, Some(&output), output.is_error)
                    .await?;
                Ok(output)
            }
            Err(error) => self.finish_failure(&call, &context, error).await,
        }
    }

    pub(crate) async fn persist_output(
        &self,
        context: &ToolContext,
        raw: RawToolOutput,
    ) -> Result<ToolOutput> {
        self.artifacts.persist_output(context, raw).await
    }

    async fn finish_failure(
        &self,
        call: &ToolCall,
        context: &ToolContext,
        error: anyhow::Error,
    ) -> Result<ToolOutput> {
        let output = self
            .persist_output(
                context,
                failed_tool_output(&call.name, &format!("{error:#}")),
            )
            .await?;
        self.finish_lifecycle(call, Some(&output), true).await?;
        Ok(output)
    }

    async fn finish_lifecycle(
        &self,
        call: &ToolCall,
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
        let mut after_payload = hook_payload(self.run_id, call);
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
        self.events
            .emit(&RuntimeEvent::new(
                self.run_id,
                RuntimeEventKind::ToolCompleted {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                },
            ))
            .await?;
        Ok(())
    }
}

pub struct DirectToolRuntime<'a> {
    pub registry: &'a ToolRegistry,
    pub hooks: &'a HookPipeline,
    pub artifacts: &'a ArtifactStore,
    pub events: &'a SharedEventSink,
    pub workspace: &'a Path,
    pub run_id: &'a str,
    pub manager: Arc<TaskManager>,
    pub foreground_timeout_seconds: u64,
}

impl DirectToolRuntime<'_> {
    #[cfg(test)]
    pub async fn execute(&self, call: ToolCall) -> Result<Message> {
        Ok(self
            .execute_batch(vec![call])
            .await?
            .into_iter()
            .next()
            .expect("single direct tool execution must return one result"))
    }

    /// Executes one assistant tool-call batch concurrently under one shared
    /// foreground deadline. Results remain in the assistant's original call
    /// order even when their executions finish in a different order.
    pub async fn execute_batch(&self, calls: Vec<ToolCall>) -> Result<Vec<Message>> {
        let mut pending = calls
            .into_iter()
            .map(|call| self.start(call))
            .collect::<Vec<_>>();
        if pending.is_empty() {
            return Ok(Vec::new());
        }

        let deadline = tokio::time::sleep(Duration::from_secs(self.foreground_timeout_seconds));
        tokio::pin!(deadline);
        tokio::select! {
            // Prefer collecting ready executions at the deadline boundary so
            // only genuinely pending futures are promoted.
            biased;
            () = settle_all(&mut pending) => {}
            () = &mut deadline => {}
        }

        let mut results = (0..pending.len())
            .map(|_| None)
            .collect::<Vec<Option<Result<DirectToolResult>>>>();
        let mut promotions = Vec::new();
        for (index, mut execution) in pending.into_iter().enumerate() {
            match execution.outcome.take() {
                Some(Ok(output)) => {
                    results[index] = Some(self.completed_result(execution.call_id, output));
                }
                Some(Err(error)) => results[index] = Some(Err(error)),
                None => {
                    let future = execution
                        .future
                        .take()
                        .expect("pending direct tool execution must retain its future");
                    match self
                        .manager
                        .prepare_tool_promotion(execution.name, execution.call_id.clone(), future)
                        .await
                    {
                        Ok(promotion) => {
                            promotions.push((index, execution.call_id, promotion));
                        }
                        Err(error) => results[index] = Some(Err(error)),
                    }
                }
            }
        }
        // Every pending exact future is now running independently. Announcing
        // one promotion can therefore wait on a resource held by another
        // future without deadlocking the batch.
        for (index, call_id, promotion) in promotions {
            results[index] = Some(
                self.manager
                    .announce_tool_promotion(promotion)
                    .await
                    .and_then(|(task_id, name)| self.promoted_result(call_id, task_id, name)),
            );
        }

        let mut messages = Vec::with_capacity(results.len());
        let mut attachments = Vec::new();
        let mut first_error = None;
        for result in results {
            match result.expect("every direct tool call must produce a result or promotion") {
                Ok(result) => {
                    messages.push(result.message);
                    if let Some(attachment) = result.attachment {
                        attachments.push(attachment);
                    }
                }
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        if !attachments.is_empty() {
            let call_ids = attachments
                .iter()
                .map(|(call_id, _)| call_id.as_str())
                .collect::<Vec<_>>();
            let mut content = vec![MessageContent::RuntimeReminder {
                text: format!(
                    "<runtime-reminder>\nImages attached from tool results in this order: {}\n</runtime-reminder>",
                    serde_json::to_string(&call_ids)?
                ),
            }];
            content.extend(
                attachments
                    .into_iter()
                    .map(|(_, attachment)| MessageContent::Image { attachment }),
            );
            messages.push(Message {
                role: Role::User,
                content,
            });
        }
        Ok(messages)
    }

    fn start(&self, call: ToolCall) -> PendingDirectTool {
        let call_id = call.id.clone();
        let name = call.name.clone();
        let registry = self.registry.clone();
        let hooks = self.hooks.clone();
        let artifacts = self.artifacts.clone();
        let events = self.events.clone();
        let workspace = self.workspace.to_owned();
        let run_id = self.run_id.to_owned();
        let execution: ToolExecutionFuture = Box::pin(async move {
            ToolExecutor::new(&registry, &hooks, &artifacts, &events, &workspace, &run_id)
                .execute(call)
                .await
        });

        PendingDirectTool {
            call_id,
            name,
            future: Some(execution),
            outcome: None,
        }
    }

    fn completed_result(
        &self,
        call_id: String,
        mut output: ToolOutput,
    ) -> Result<DirectToolResult> {
        let attachment = output
            .attachment
            .take()
            .map(|attachment| (call_id.clone(), attachment));
        let metadata = output.result_metadata();
        Ok(DirectToolResult {
            message: Message {
                role: Role::Tool,
                content: vec![MessageContent::ToolResult {
                    call_id,
                    content: output.model_content(),
                    is_error: output.is_error,
                    metadata,
                }],
            },
            attachment,
        })
    }

    fn promoted_result(
        &self,
        call_id: String,
        task_id: String,
        name: String,
    ) -> Result<DirectToolResult> {
        Ok(DirectToolResult {
            message: Message {
                role: Role::Tool,
                content: vec![MessageContent::ToolResult {
                    call_id,
                    content: crate::model::background_task_started_reminder(&task_id, &name),
                    is_error: false,
                    metadata: ResultMetadata::empty(),
                }],
            },
            attachment: None,
        })
    }
}

struct DirectToolResult {
    message: Message,
    attachment: Option<(String, ImageAttachment)>,
}

struct PendingDirectTool {
    call_id: String,
    name: String,
    future: Option<ToolExecutionFuture>,
    outcome: Option<Result<ToolOutput>>,
}

async fn settle_all(pending: &mut [PendingDirectTool]) {
    poll_fn(|context| {
        let mut all_settled = true;
        for execution in pending.iter_mut() {
            if execution.outcome.is_some() {
                continue;
            }
            let future = execution
                .future
                .as_mut()
                .expect("unsettled direct tool execution must retain its future");
            match future.as_mut().poll(context) {
                Poll::Ready(outcome) => {
                    execution.outcome = Some(outcome);
                    execution.future = None;
                }
                Poll::Pending => all_settled = false,
            }
        }
        if all_settled {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
}

fn hook_payload(run_id: &str, call: &ToolCall) -> Map<String, Value> {
    Map::from_iter([
        ("run_id".to_owned(), json!(run_id)),
        ("call_id".to_owned(), json!(call.id)),
        ("name".to_owned(), json!(call.name)),
    ])
}

pub(crate) fn failed_tool_output(name: &str, error: &str) -> RawToolOutput {
    RawToolOutput {
        content: format!("tool `{name}` failed: {error}").into_bytes(),
        source_path: None,
        media_type: "text/plain; charset=utf-8".to_owned(),
        is_error: true,
        attach_to_model: false,
    }
}
