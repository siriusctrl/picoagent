use std::sync::Arc;

use anyhow::{Result, bail};

use crate::trajectory::TrajectoryReader;

use super::ToolRegistry;

mod read;
mod search;

pub(super) fn register(
    registry: &mut ToolRegistry,
    reader: Arc<dyn TrajectoryReader>,
    search_max_matches: usize,
) -> Result<()> {
    if registry.contains("history_search") || registry.contains("history_read") {
        bail!("history tools are already registered");
    }
    registry.register(Arc::new(search::SearchTool::new(
        reader.clone(),
        search_max_matches,
    )?))?;
    registry.register(Arc::new(read::ReadTool::new(reader)))?;
    Ok(())
}
