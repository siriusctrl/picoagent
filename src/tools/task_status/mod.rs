use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    agent::task::TaskManager,
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext, task_result::task_records},
};

pub struct TaskStatusTool {
    manager: Arc<TaskManager>,
}

impl TaskStatusTool {
    pub fn new(manager: Arc<TaskManager>) -> Self {
        Self { manager }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskStatusArgs {
    #[serde(default)]
    task_ids: Vec<String>,
}

#[async_trait]
impl Tool for TaskStatusTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: TaskStatusArgs =
            serde_json::from_value(arguments).context("invalid task_status arguments")?;
        Ok(RawToolOutput::text(serde_json::to_string(&task_records(
            self.manager.status(&args.task_ids).await?,
        ))?))
    }
}
