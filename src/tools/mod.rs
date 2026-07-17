use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde_json::Value;

use crate::model::ToolSpec;

pub mod bash;
pub mod history_read;
pub mod history_search;
mod paths;
pub mod read;
pub mod web_search;
pub mod write;

pub use bash::BashTool;
pub use history_read::HistoryReadTool;
pub use history_search::HistorySearchTool;
pub use read::ReadTool;
pub use web_search::WebSearchTool;
pub use write::WriteTool;

#[derive(Clone)]
pub struct ToolContext {
    pub run_id: String,
    pub call_id: String,
    pub workspace: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RawToolOutput {
    pub content: Vec<u8>,
    /// Optional file containing the complete output. ArtifactStore consumes it
    /// without loading the whole file into memory.
    pub source_path: Option<PathBuf>,
    pub media_type: String,
    pub is_error: bool,
}

impl RawToolOutput {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into().into_bytes(),
            source_path: None,
            media_type: "text/plain; charset=utf-8".to_owned(),
            is_error: false,
        }
    }

    pub fn file(path: PathBuf, media_type: impl Into<String>, is_error: bool) -> Self {
        Self {
            content: Vec::new(),
            source_path: Some(path),
            media_type: media_type.into(),
            is_error,
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    async fn execute(&self, context: ToolContext, arguments: Value) -> Result<RawToolOutput>;
}

#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<()> {
        let name = tool.spec().name;
        if self.tools.contains_key(&name) {
            bail!("tool `{name}` is already registered");
        }
        self.tools.insert(name, tool);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|tool| tool.spec()).collect()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(String::as_str)
    }
}

pub fn register_defaults(registry: &mut ToolRegistry) -> Result<()> {
    registry.register(Arc::new(ReadTool))?;
    registry.register(Arc::new(WriteTool::default()))?;
    registry.register(Arc::new(BashTool))?;
    Ok(())
}

pub fn register_history_tools(
    registry: &mut ToolRegistry,
    reader: Arc<dyn crate::trajectory::TrajectoryReader>,
    max_matches: usize,
) -> Result<()> {
    if registry.get("history_search").is_some() || registry.get("history_read").is_some() {
        bail!("history tools are already registered");
    }
    let search = HistorySearchTool::new(reader.clone(), max_matches)?;
    registry.register(Arc::new(search))?;
    registry.register(Arc::new(HistoryReadTool::new(reader)))?;
    Ok(())
}
