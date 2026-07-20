use std::sync::Arc;

use anyhow::{Result, bail};

use crate::{agent::task::TaskManager, skills::SkillRegistry, trajectory::TrajectoryReader};

use super::{
    BashTool, DelegateTool, HistoryReadTool, HistorySearchTool, LoadSkillTool, ReadTool,
    TaskInspectTool, TaskStatusTool, TaskSteerTool, TaskStopTool, TaskWaitTool, ToolRegistry,
    WebSearchTool, WriteTool,
};

/// Assemble the process-wide tools. Run-scoped history and task controls are
/// added later by `RunToolAssembly`.
pub fn build_app_tools(
    skills: Arc<SkillRegistry>,
    web_search: Option<WebSearchTool>,
) -> Result<ToolRegistry> {
    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(ReadTool))?;
    registry.register(Arc::new(WriteTool::default()))?;
    registry.register(Arc::new(BashTool))?;
    registry.register(Arc::new(LoadSkillTool::new(skills)))?;
    if let Some(web_search) = web_search {
        registry.register(Arc::new(web_search))?;
    }
    Ok(registry)
}

/// The one assembly path for the frozen set of tools exposed by an agent run.
pub struct RunToolAssembly {
    registry: ToolRegistry,
}

impl RunToolAssembly {
    pub fn new(
        mut registry: ToolRegistry,
        reader: Arc<dyn TrajectoryReader>,
        history_search_max_matches: usize,
    ) -> Result<Self> {
        if registry.contains("history_search") || registry.contains("history_read") {
            bail!("history tools are already registered");
        }
        let search = HistorySearchTool::new(reader.clone(), history_search_max_matches)?;
        registry.register(Arc::new(search))?;
        registry.register(Arc::new(HistoryReadTool::new(reader)))?;
        Ok(Self { registry })
    }

    pub fn contains(&self, name: &str) -> bool {
        self.registry.contains(name)
    }

    pub fn finish(mut self, manager: Arc<TaskManager>, may_delegate: bool) -> Result<ToolRegistry> {
        if may_delegate {
            self.registry
                .register(Arc::new(DelegateTool::new(manager.clone())))?;
        }
        self.registry
            .register(Arc::new(TaskInspectTool::new(manager.clone())))?;
        self.registry
            .register(Arc::new(TaskStatusTool::new(manager.clone())))?;
        self.registry
            .register(Arc::new(TaskSteerTool::new(manager.clone())))?;
        self.registry
            .register(Arc::new(TaskStopTool::new(manager.clone())))?;
        self.registry
            .register(Arc::new(TaskWaitTool::new(manager)))?;
        Ok(self.registry)
    }
}
