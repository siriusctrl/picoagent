pub mod bash;
mod paths;
pub mod read;
pub mod web_search;
pub mod write;

use anyhow::Result;

use crate::tools::ToolRegistry;

pub use bash::BashTool;
pub use read::ReadTool;
pub use web_search::WebSearchTool;
pub use write::WriteTool;

pub fn register_all(registry: &mut ToolRegistry) -> Result<()> {
    registry.register(std::sync::Arc::new(ReadTool))?;
    registry.register(std::sync::Arc::new(WriteTool::default()))?;
    registry.register(std::sync::Arc::new(BashTool))?;
    Ok(())
}
