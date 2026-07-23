use std::sync::Arc;

use anyhow::Result;

use crate::agent::handle::RuntimeHandleManager;

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
    handles: Arc<RuntimeHandleManager>,
) -> Result<()> {
    registry.register(Arc::new(close::CloseTool::new(handles.clone())))?;
    registry.register(Arc::new(inspect::InspectTool::new(handles.clone())))?;
    registry.register(Arc::new(list::ListHandlesTool::new(handles.clone())))?;
    registry.register(Arc::new(send::SendMessageTool::new(handles.clone())))?;
    registry.register(Arc::new(status::StatusTool::new(handles.clone())))?;
    registry.register(Arc::new(stop::StopTool::new(handles.clone())))?;
    registry.register(Arc::new(wait::WaitTool::new(handles)))?;
    Ok(())
}
