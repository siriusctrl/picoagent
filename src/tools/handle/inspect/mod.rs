use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    agent::handle::RuntimeHandleManager,
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

const DEFAULT_LIMIT: usize = 6;
const MAX_LIMIT: usize = 20;

pub(super) struct InspectTool {
    handles: Arc<RuntimeHandleManager>,
}

impl InspectTool {
    pub(super) fn new(handles: Arc<RuntimeHandleManager>) -> Self {
        Self { handles }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectArgs {
    handle: String,
    #[serde(default)]
    before_seq: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl Tool for InspectTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: InspectArgs =
            serde_json::from_value(arguments).context("invalid inspect arguments")?;
        if args.before_seq == Some(0) {
            bail!("inspect before_seq must be greater than zero");
        }
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            bail!("inspect limit must be between 1 and {MAX_LIMIT}");
        }
        Ok(RawToolOutput::text(serde_json::to_string(
            &self
                .handles
                .inspect(&args.handle, args.before_seq, limit)
                .await?,
        )?))
    }
}
