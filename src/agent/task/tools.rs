use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

use super::TaskManager;

pub struct SpawnTool {
    manager: Arc<TaskManager>,
}

impl SpawnTool {
    pub fn new(manager: Arc<TaskManager>) -> Self {
        Self { manager }
    }
}

#[derive(Debug, Deserialize)]
struct SpawnArgs {
    kind: String,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    arguments: Option<Value>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

#[async_trait]
impl Tool for SpawnTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "spawn".to_owned(),
            description: "Start one tool call or general-task subagent in the background and return immediately with a task_id. Direct tool calls are synchronous, so use spawn only when work is independent and the current task can continue safely. Do not background a mutation that later work depends on; call it directly or use wait before consuming its result. Completed results are automatically appended at the next model boundary, or can be joined explicitly with wait.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": ["tool", "agent"] },
                    "tool": { "type": "string", "description": "Tool name when kind=tool" },
                    "arguments": { "type": "object", "description": "Tool arguments when kind=tool" },
                    "profile": { "type": "string", "enum": ["general-task"], "description": "Agent profile when kind=agent" },
                    "prompt": { "type": "string", "description": "Complete delegated task when kind=agent" },
                    "timeout_seconds": { "type": "integer", "minimum": 1, "description": "Hard execution timeout; values above the runtime maximum are clamped" }
                },
                "required": ["kind"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: SpawnArgs =
            serde_json::from_value(arguments).context("invalid spawn arguments")?;
        let record = match args.kind.as_str() {
            "tool" => {
                let name = args.tool.context("spawn kind=tool requires `tool`")?;
                self.manager
                    .spawn_tool(
                        name,
                        args.arguments.unwrap_or_else(|| json!({})),
                        args.timeout_seconds,
                    )
                    .await?
            }
            "agent" => {
                self.manager
                    .spawn_agent(
                        args.profile.unwrap_or_else(|| "general-task".to_owned()),
                        args.prompt.context("spawn kind=agent requires `prompt`")?,
                        args.timeout_seconds,
                    )
                    .await?
            }
            kind => bail!("invalid spawn kind `{kind}`"),
        };
        Ok(RawToolOutput::text(serde_json::to_string(&json!({
            "task_id": record.id,
            "kind": record.kind,
            "name": record.name,
            "status": record.status(),
            "message": "Background task started. Continue independent work or call wait before using its result."
        }))?))
    }
}

pub struct WaitTool {
    manager: Arc<TaskManager>,
}

impl WaitTool {
    pub fn new(manager: Arc<TaskManager>) -> Self {
        Self { manager }
    }
}

#[derive(Debug, Deserialize)]
struct WaitArgs {
    #[serde(default)]
    task_ids: Vec<String>,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

#[async_trait]
impl Tool for WaitTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "wait".to_owned(),
            description: "Wait for selected background tasks up to a bounded timeout and return their current states and terminal results. Pass task_ids=[] to wait for all tasks created by this run. A wait timeout does not cancel tasks; call wait again if needed. Results returned by wait are marked delivered and will not be appended again automatically.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_ids": { "type": "array", "items": { "type": "string" }, "description": "Task ids to join; empty means all" },
                    "timeout_seconds": { "type": "integer", "minimum": 1, "description": "Maximum time to wait in this call" }
                },
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: WaitArgs = serde_json::from_value(arguments).context("invalid wait arguments")?;
        let records = self
            .manager
            .wait(&args.task_ids, args.timeout_seconds)
            .await?;
        let output = records
            .iter()
            .map(|record| {
                json!({
                    "task_id": record.id,
                    "kind": record.kind,
                    "name": record.name,
                    "status": record.status(),
                    "result": record.result,
                    "error": record.error,
                    "child_run_id": record.child_run_id,
                })
            })
            .collect::<Vec<_>>();
        Ok(RawToolOutput::text(serde_json::to_string(
            &json!({ "tasks": output }),
        )?))
    }
}
