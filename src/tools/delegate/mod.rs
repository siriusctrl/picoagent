use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    agent::task::TaskManager,
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

pub struct DelegateTool {
    manager: Arc<TaskManager>,
}

impl DelegateTool {
    pub fn new(manager: Arc<TaskManager>) -> Self {
        Self { manager }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DelegateArgs {
    prompt: String,
}

#[async_trait]
impl Tool for DelegateTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: DelegateArgs =
            serde_json::from_value(arguments).context("invalid delegate arguments")?;
        let record = self.manager.delegate(args.prompt).await?;
        Ok(RawToolOutput::text(serde_json::to_string(&json!({
            "task_id": record.id,
            "status": record.status(),
        }))?))
    }
}
