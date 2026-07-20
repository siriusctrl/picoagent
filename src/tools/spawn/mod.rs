use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    agent::task::TaskManager,
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

const DESCRIPTION: &str = include_str!("description.md");

pub struct SpawnTool {
    manager: Arc<TaskManager>,
}

impl SpawnTool {
    pub fn new(manager: Arc<TaskManager>) -> Self {
        Self { manager }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnArgs {
    kind: String,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    arguments: Option<Value>,
    #[serde(default)]
    prompt: Option<String>,
}

#[async_trait]
impl Tool for SpawnTool {
    fn spec(&self) -> ToolSpec {
        let spawnable_tools = self.manager.spawnable_tool_names();
        let mut kinds = vec!["agent"];
        let mut properties = Map::from_iter([
            (
                "kind".to_owned(),
                json!({ "type": "string", "enum": kinds }),
            ),
            (
                "prompt".to_owned(),
                json!({ "type": "string", "description": "Complete delegated task when kind=agent" }),
            ),
        ]);
        if !spawnable_tools.is_empty() {
            kinds.insert(0, "tool");
            properties["kind"] = json!({ "type": "string", "enum": kinds });
            properties.insert(
                "tool".to_owned(),
                json!({
                    "type": "string",
                    "enum": spawnable_tools,
                    "description": "Allowed tool name when kind=tool"
                }),
            );
            properties.insert(
                "arguments".to_owned(),
                json!({ "type": "object", "description": "Tool arguments when kind=tool" }),
            );
        }
        ToolSpec {
            name: "spawn".to_owned(),
            description: DESCRIPTION.trim().to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": properties,
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
                    .spawn_agent(args.prompt.context("spawn kind=agent requires `prompt`")?)
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
