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

pub(super) struct ListTool {
    manager: Arc<TaskManager>,
}

impl ListTool {
    pub(super) fn new(manager: Arc<TaskManager>) -> Self {
        Self { manager }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskListArgs {
    #[serde(default)]
    include_closed: bool,
}

#[async_trait]
impl Tool for ListTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: TaskListArgs =
            serde_json::from_value(arguments).context("invalid task_list arguments")?;
        let mut records = self.manager.list_agents().await?;
        if !args.include_closed {
            records.retain(|record| record.status() != "closed");
        }
        Ok(RawToolOutput::text(serde_json::to_string(&task_records(
            records,
        ))?))
    }
}
