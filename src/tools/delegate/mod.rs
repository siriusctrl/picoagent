use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    agent::task::TaskManager,
    model::ToolSpec,
    storage::DelegateContext,
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
    name: String,
    prompt: String,
    context: DelegateContext,
}

#[async_trait]
impl Tool for DelegateTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: DelegateArgs =
            serde_json::from_value(arguments).context("invalid delegate arguments")?;
        let record = self
            .manager
            .delegate(args.name, args.prompt, args.context, &context.call_id)
            .await?;
        Ok(RawToolOutput::text(
            crate::model::background_task_started_reminder(&record.id, &record.name),
        ))
    }
}
