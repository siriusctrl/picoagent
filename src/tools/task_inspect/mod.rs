use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    agent::task::TaskManager,
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

const DEFAULT_LIMIT: usize = 6;
const MAX_LIMIT: usize = 20;

pub struct TaskInspectTool {
    manager: Arc<TaskManager>,
}

impl TaskInspectTool {
    pub fn new(manager: Arc<TaskManager>) -> Self {
        Self { manager }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskInspectArgs {
    task_id: String,
    #[serde(default)]
    before_seq: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl Tool for TaskInspectTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: TaskInspectArgs =
            serde_json::from_value(arguments).context("invalid task_inspect arguments")?;
        if args.before_seq == Some(0) {
            bail!("task_inspect before_seq must be greater than zero");
        }
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            bail!("task_inspect limit must be between 1 and {MAX_LIMIT}");
        }
        Ok(RawToolOutput::text(serde_json::to_string(
            &self
                .manager
                .inspect(&args.task_id, args.before_seq, limit)
                .await?,
        )?))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn manifest_limits_match_runtime_constants() {
        let spec = crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!());
        assert_eq!(
            spec.input_schema.pointer("/properties/limit/maximum"),
            Some(&json!(MAX_LIMIT))
        );
        assert_eq!(
            spec.input_schema.pointer("/properties/limit/default"),
            Some(&json!(DEFAULT_LIMIT))
        );
    }
}
