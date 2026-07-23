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

use super::result::handle_snapshots;

pub(super) struct WaitTool {
    handles: Arc<RuntimeHandleManager>,
}

impl WaitTool {
    pub(super) fn new(handles: Arc<RuntimeHandleManager>) -> Self {
        Self { handles }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitArgs {
    #[serde(default)]
    handles: Vec<String>,
}

#[async_trait]
impl Tool for WaitTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: WaitArgs = serde_json::from_value(arguments).context("invalid wait arguments")?;
        Ok(RawToolOutput::text(serde_json::to_string(
            &handle_snapshots(self.handles.wait(&args.handles).await?),
        )?))
    }
}
