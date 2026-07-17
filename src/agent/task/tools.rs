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
const TASK_DESCRIPTION: &str = include_str!("descriptions/task.md");
const DEFAULT_INSPECT_LIMIT: usize = 6;
const MAX_INSPECT_LIMIT: usize = 20;

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
                    "prompt": { "type": "string", "description": "Complete delegated task when kind=agent" }
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
                    .spawn_tool(name, args.arguments.unwrap_or_else(|| json!({})))
                    .await?
            }
            "agent" => {
                self.manager
                    .spawn_agent(
                        args.profile.unwrap_or_else(|| "general-task".to_owned()),
                        args.prompt.context("spawn kind=agent requires `prompt`")?,
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
            "message": "Background task started. Continue independent work or call task wait before using its result."
        }))?))
    }
}

pub struct TaskTool {
    manager: Arc<TaskManager>,
}

impl TaskTool {
    pub fn new(manager: Arc<TaskManager>) -> Self {
        Self { manager }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskArgs {
    action: String,
    #[serde(default)]
    task_ids: Vec<String>,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    before_seq: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl Tool for TaskTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "task".to_owned(),
            description: TASK_DESCRIPTION.trim().to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["status", "wait", "inspect", "steer", "stop"] },
                    "task_ids": { "type": "array", "items": { "type": "string" }, "description": "Task ids for status/wait; empty means all" },
                    "task_id": { "type": "string", "description": "One task id for inspect/steer/stop" },
                    "message": { "type": "string", "description": "Steering instruction when action=steer" },
                    "before_seq": { "type": "integer", "minimum": 1, "description": "For inspect, return messages with seq lower than this value" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_INSPECT_LIMIT, "description": "For inspect, number of recent messages to return (default 6)" }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: TaskArgs = serde_json::from_value(arguments).context("invalid task arguments")?;
        let output = match args.action.as_str() {
            "status" => task_records(self.manager.status(&args.task_ids).await?),
            "wait" => task_records(self.manager.wait(&args.task_ids).await?),
            "inspect" => {
                let task_id = args.task_id.context("task inspect requires `task_id`")?;
                let limit = args.limit.unwrap_or(DEFAULT_INSPECT_LIMIT);
                if !(1..=MAX_INSPECT_LIMIT).contains(&limit) {
                    bail!("task inspect limit must be between 1 and {MAX_INSPECT_LIMIT}");
                }
                self.manager
                    .inspect(&task_id, args.before_seq, limit)
                    .await?
            }
            "steer" => {
                let task_id = args.task_id.context("task steer requires `task_id`")?;
                self.manager
                    .steer(
                        &task_id,
                        args.message.context("task steer requires `message`")?,
                    )
                    .await?
            }
            "stop" => {
                let task_id = args.task_id.context("task stop requires `task_id`")?;
                task_records(vec![self.manager.stop(&task_id).await?])
            }
            action => bail!("invalid task action `{action}`"),
        };
        Ok(RawToolOutput::text(serde_json::to_string(&output)?))
    }
}

fn task_records(records: Vec<super::BackgroundTaskRecord>) -> Value {
    let tasks = records
        .into_iter()
        .map(|record| {
            json!({
                "task_id": record.id,
                "kind": record.kind,
                "name": record.name,
                "status": record.status(),
                "child_run_id": record.child_run_id,
                "message": if record.state.is_terminal() {
                    "Terminal result is delivered separately as a durable background-result message."
                } else {
                    "Task is still running."
                },
            })
        })
        .collect::<Vec<_>>();
    json!({ "tasks": tasks })
}
