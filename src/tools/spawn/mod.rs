use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    agent::task::TaskManager,
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

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
        let mut spec = crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!());
        let properties = spec
            .input_schema
            .get_mut("properties")
            .and_then(Value::as_object_mut)
            .expect("spawn tool.yaml must define input_schema.properties");
        for property in ["kind", "tool", "arguments", "prompt"] {
            properties
                .get(property)
                .and_then(Value::as_object)
                .unwrap_or_else(|| {
                    panic!("spawn tool.yaml must define an object `{property}` property")
                });
        }
        let kind = properties
            .get_mut("kind")
            .and_then(Value::as_object_mut)
            .expect("spawn tool.yaml must define a kind property");
        if spawnable_tools.is_empty() {
            kind.insert("enum".to_owned(), json!(["agent"]));
            properties.remove("tool");
            properties.remove("arguments");
        } else {
            kind.insert("enum".to_owned(), json!(["tool", "agent"]));
            properties
                .get_mut("tool")
                .and_then(Value::as_object_mut)
                .expect("spawn tool.yaml must define a tool property")
                .insert("enum".to_owned(), json!(spawnable_tools));
        }
        spec
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
