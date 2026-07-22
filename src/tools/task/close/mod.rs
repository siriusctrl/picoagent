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

use super::result::task_record;

pub(super) struct CloseTool {
    manager: Arc<TaskManager>,
}

impl CloseTool {
    pub(super) fn new(manager: Arc<TaskManager>) -> Self {
        Self { manager }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskCloseArgs {
    task_id: String,
}

#[async_trait]
impl Tool for CloseTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: TaskCloseArgs =
            serde_json::from_value(arguments).context("invalid task_close arguments")?;
        Ok(RawToolOutput::text(serde_json::to_string(&task_record(
            self.manager.close(&args.task_id).await?,
        ))?))
    }
}
