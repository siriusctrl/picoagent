use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    agent::handle::{RuntimeHandleManager, SendMode},
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

pub(super) struct SendMessageTool {
    handles: Arc<RuntimeHandleManager>,
}

impl SendMessageTool {
    pub(super) fn new(handles: Arc<RuntimeHandleManager>) -> Self {
        Self { handles }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SendMessageArgs {
    handle: String,
    message: String,
    mode: InputMode,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum InputMode {
    Steer,
    Followup,
}

impl From<InputMode> for SendMode {
    fn from(mode: InputMode) -> Self {
        match mode {
            InputMode::Steer => Self::Steer,
            InputMode::Followup => Self::Followup,
        }
    }
}

#[async_trait]
impl Tool for SendMessageTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: SendMessageArgs =
            serde_json::from_value(arguments).context("invalid send_message arguments")?;
        Ok(RawToolOutput::text(serde_json::to_string(
            &self
                .handles
                .send(&args.handle, args.message, args.mode.into())
                .await?,
        )?))
    }
}
