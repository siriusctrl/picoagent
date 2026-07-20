use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    agent::task::TaskManager,
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

pub struct TaskSteerTool {
    manager: Arc<TaskManager>,
}

impl TaskSteerTool {
    pub fn new(manager: Arc<TaskManager>) -> Self {
        Self { manager }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskSteerArgs {
    task_id: String,
    message: String,
}

#[async_trait]
impl Tool for TaskSteerTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: TaskSteerArgs =
            serde_json::from_value(arguments).context("invalid task_steer arguments")?;
        Ok(RawToolOutput::text(serde_json::to_string(
            &self.manager.steer(&args.task_id, args.message).await?,
        )?))
    }
}
