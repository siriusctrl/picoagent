use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;

use crate::{
    model::ToolSpec,
    skills::SkillRegistry,
    tools::{RawToolOutput, Tool, ToolContext},
};

#[derive(Clone)]
pub struct LoadSkillTool {
    registry: Arc<SkillRegistry>,
}

impl LoadSkillTool {
    pub fn new(registry: Arc<SkillRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for LoadSkillTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let name = arguments
            .get("name")
            .and_then(Value::as_str)
            .context("`name` is required")?;
        Ok(RawToolOutput::text(self.registry.load(name)?))
    }
}
