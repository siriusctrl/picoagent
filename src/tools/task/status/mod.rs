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

use super::result::task_records;

pub(super) struct StatusTool {
    manager: Arc<TaskManager>,
}

impl StatusTool {
    pub(super) fn new(manager: Arc<TaskManager>) -> Self {
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
impl Tool for StatusTool {
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
