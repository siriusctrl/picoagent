use std::sync::Arc;

use anyhow::Result;

use crate::{agent::task::TaskManager, skills::SkillRegistry, trajectory::TrajectoryReader};

use super::{
    BashTool, DelegateTool, LoadSkillTool, ReadTool, ToolRegistry, WebSearchTool, WriteTool,
    history, task,
};

/// Assemble the process-wide tools. Run-scoped history and task controls are
/// added later by `RunToolAssembly`.
pub fn build_app_tools(
    skills: Arc<SkillRegistry>,
    web_search: Option<WebSearchTool>,
    image_enabled: bool,
) -> Result<ToolRegistry> {
    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(ReadTool::new(image_enabled)))?;
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
        history::register(&mut registry, reader, history_search_max_matches)?;
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
        task::register_controls(&mut self.registry, manager)?;
        Ok(self.registry)
    }
}
