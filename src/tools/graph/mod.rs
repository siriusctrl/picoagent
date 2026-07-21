use std::sync::Arc;

use anyhow::Result;

use super::ToolRegistry;

mod init;
mod list;
mod model;
mod store;

use init::GraphInitTool;
use list::GraphListTool;
use store::GraphStore;

pub(crate) fn register(registry: &mut ToolRegistry) -> Result<()> {
    let store = Arc::new(GraphStore::default());
    registry.register(Arc::new(GraphInitTool::new(store.clone())))?;
    registry.register(Arc::new(GraphListTool::new(store)))?;
    Ok(())
}
