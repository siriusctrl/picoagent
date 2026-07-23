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

pub(super) struct StatusTool {
    handles: Arc<RuntimeHandleManager>,
}

impl StatusTool {
    pub(super) fn new(handles: Arc<RuntimeHandleManager>) -> Self {
        Self { handles }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusArgs {
    #[serde(default)]
    handles: Vec<String>,
}

#[async_trait]
impl Tool for StatusTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: StatusArgs =
            serde_json::from_value(arguments).context("invalid status arguments")?;
        Ok(RawToolOutput::text(serde_json::to_string(
            &handle_snapshots(self.handles.status(&args.handles).await?),
        )?))
    }
}
