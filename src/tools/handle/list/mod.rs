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

pub(super) struct ListHandlesTool {
    handles: Arc<RuntimeHandleManager>,
}

impl ListHandlesTool {
    pub(super) fn new(handles: Arc<RuntimeHandleManager>) -> Self {
        Self { handles }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListHandlesArgs {
    #[serde(default)]
    include_closed: bool,
}

#[async_trait]
impl Tool for ListHandlesTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: ListHandlesArgs =
            serde_json::from_value(arguments).context("invalid list_handles arguments")?;
        Ok(RawToolOutput::text(serde_json::to_string(
            &handle_snapshots(self.handles.list_handles(args.include_closed).await?),
        )?))
    }
}
