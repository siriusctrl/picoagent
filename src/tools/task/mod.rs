use std::sync::Arc;

use anyhow::Result;

use crate::agent::task::TaskManager;

use super::ToolRegistry;

mod inspect;
mod result;
mod status;
mod steer;
mod stop;
mod wait;

pub(super) fn register_controls(
    registry: &mut ToolRegistry,
    manager: Arc<TaskManager>,
) -> Result<()> {
    registry.register(Arc::new(inspect::InspectTool::new(manager.clone())))?;
    registry.register(Arc::new(status::StatusTool::new(manager.clone())))?;
    registry.register(Arc::new(steer::SteerTool::new(manager.clone())))?;
    registry.register(Arc::new(stop::StopTool::new(manager.clone())))?;
    registry.register(Arc::new(wait::WaitTool::new(manager)))?;
    Ok(())
}
