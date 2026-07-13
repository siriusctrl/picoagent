use std::{path::Path, sync::Arc, time::Duration};

use anyhow::Result;
use serde_json::json;
use tokio::sync::Mutex;

use crate::{
    artifact::ArtifactStore,
    events::{RuntimeEvent, RuntimeEventKind, SharedEventSink},
    hooks::{HookEvent, HookPipeline},
    model::{Message, MessageContent, Role, ToolCall},
    tools::{RawToolOutput, ToolContext, ToolRegistry},
};

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
        let before = self
            .hooks
            .run(
                HookEvent::ToolBefore,
                json!({
                    "run_id": self.run_id,
                    "call_id": call.id,
                    "name": call.name,
                    "arguments": call.arguments,
                }),
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
        let raw = match self.registry.get(&call.name) {
            Some(tool) => match tokio::time::timeout(
                Duration::from_secs(self.timeout_seconds.max(1)),
                tool.execute(context.clone(), arguments),
            )
            .await
            {
                Ok(Ok(output)) => output,
                Ok(Err(error)) => failed_tool_output(&call.name, &format!("{error:#}")),
                Err(_) => failed_tool_output(
                    &call.name,
                    "direct tool call exceeded its execution timeout",
                ),
            },
            None => failed_tool_output(&call.name, "unknown tool"),
        };
        let mut preview_budget = self.preview_budget.lock().await;
        let output = self
            .artifacts
            .persist_output_with_budget(&context, raw, *preview_budget)
            .await?;
        *preview_budget = preview_budget.saturating_sub(output.preview.len());
        drop(preview_budget);
        if let Some(artifact) = &output.artifact {
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
        self.hooks
            .run(
                HookEvent::ToolAfter,
                json!({
                    "run_id": self.run_id,
                    "call_id": call.id,
                    "name": call.name,
                    "truncated": output.truncated,
                    "artifact": output.artifact,
                    "is_error": output.is_error,
                }),
                self.workspace,
            )
            .await?;

        Ok(Message {
            role: Role::Tool,
            content: vec![MessageContent::ToolResult {
                call_id: call.id,
                content: output.model_content(),
                is_error: output.is_error,
            }],
        })
    }
}

fn failed_tool_output(name: &str, error: &str) -> RawToolOutput {
    RawToolOutput {
        content: format!("tool `{name}` failed: {error}").into_bytes(),
        source_path: None,
        media_type: "text/plain; charset=utf-8".to_owned(),
        is_error: true,
    }
}
