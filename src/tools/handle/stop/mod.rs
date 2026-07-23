use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    agent::handle::RuntimeHandleManager,
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

use super::result::handle_snapshot;

pub(super) struct StopTool {
    handles: Arc<RuntimeHandleManager>,
}

impl StopTool {
    pub(super) fn new(handles: Arc<RuntimeHandleManager>) -> Self {
        Self { handles }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StopArgs {
    handle: String,
}

#[async_trait]
impl Tool for StopTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: StopArgs = serde_json::from_value(arguments).context("invalid stop arguments")?;
        Ok(RawToolOutput::text(serde_json::to_string(
            &handle_snapshot(self.handles.stop(&args.handle).await?),
        )?))
    }
}
