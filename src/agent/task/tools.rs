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

const SPAWN_DESCRIPTION: &str = include_str!("descriptions/spawn.md");
const WAIT_DESCRIPTION: &str = include_str!("descriptions/wait.md");

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
            description: SPAWN_DESCRIPTION.trim().to_owned(),
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
#[serde(deny_unknown_fields)]
struct WaitArgs {
    #[serde(default)]
    task_ids: Vec<String>,
}

#[async_trait]
impl Tool for WaitTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "wait".to_owned(),
            description: WAIT_DESCRIPTION.trim().to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_ids": { "type": "array", "items": { "type": "string" }, "description": "Task ids to join; empty means all" }
                },
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: WaitArgs = serde_json::from_value(arguments).context("invalid wait arguments")?;
        let records = self.manager.wait(&args.task_ids).await?;
        let output = records
            .iter()
            .map(|record| {
                json!({
                    "task_id": record.id,
                    "kind": record.kind,
                    "name": record.name,
                    "status": record.status(),
                    "child_run_id": record.child_run_id,
                    "message": if record.state.is_terminal() {
                        "Terminal result will be delivered as a background-result message."
                    } else {
                        "Task is still running."
                    },
                })
            })
            .collect::<Vec<_>>();
        Ok(RawToolOutput::text(serde_json::to_string(
            &json!({ "tasks": output }),
        )?))
    }
}
