use std::sync::Arc;

use anyhow::Result;

use crate::agent::task::TaskManager;

use super::ToolRegistry;

mod close;
mod inspect;
mod list;
mod result;
mod send;
mod status;
mod stop;
mod wait;

pub(super) fn register_controls(
    registry: &mut ToolRegistry,
    manager: Arc<TaskManager>,
) -> Result<()> {
    registry.register(Arc::new(close::CloseTool::new(manager.clone())))?;
    registry.register(Arc::new(inspect::InspectTool::new(manager.clone())))?;
    registry.register(Arc::new(list::ListTool::new(manager.clone())))?;
    registry.register(Arc::new(send::SendTool::new(manager.clone())))?;
    registry.register(Arc::new(status::StatusTool::new(manager.clone())))?;
    registry.register(Arc::new(stop::StopTool::new(manager.clone())))?;
    registry.register(Arc::new(wait::WaitTool::new(manager)))?;
    Ok(())
}
