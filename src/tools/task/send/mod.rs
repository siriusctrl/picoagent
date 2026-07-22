use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    agent::task::{TaskManager, TaskSendMode},
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

pub(super) struct SendTool {
    manager: Arc<TaskManager>,
}

impl SendTool {
    pub(super) fn new(manager: Arc<TaskManager>) -> Self {
        Self { manager }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskSendArgs {
    task_id: String,
    message: String,
    mode: SendMode,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SendMode {
    Steer,
    Followup,
}

impl From<SendMode> for TaskSendMode {
    fn from(mode: SendMode) -> Self {
        match mode {
            SendMode::Steer => Self::Steer,
            SendMode::Followup => Self::Followup,
        }
    }
}

#[async_trait]
impl Tool for SendTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: TaskSendArgs =
            serde_json::from_value(arguments).context("invalid task_send arguments")?;
        Ok(RawToolOutput::text(serde_json::to_string(
            &self
                .manager
                .send(&args.task_id, args.message, args.mode.into())
                .await?,
        )?))
    }
}
