use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde_json::Value;

use crate::model::ToolSpec;

mod assembly;
pub mod bash;
pub mod history_read;
pub mod history_search;
pub mod load_skill;
mod manifest;
mod paths;
pub mod read;
pub mod spawn;
pub mod task;
pub mod web_search;
pub mod write;

pub use assembly::{RunToolAssembly, build_app_tools};
pub use bash::BashTool;
pub use history_read::HistoryReadTool;
pub use history_search::HistorySearchTool;
pub use load_skill::LoadSkillTool;
pub(crate) use manifest::embedded_tool_spec;
pub use read::ReadTool;
pub use spawn::SpawnTool;
pub use task::TaskTool;
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
    tools: BTreeMap<String, RegisteredTool>,
}

#[derive(Clone)]
struct RegisteredTool {
    implementation: Arc<dyn Tool>,
    spec: ToolSpec,
    explicit_spawn: ExplicitSpawn,
}

/// Whether the model may start this tool directly through `spawn(kind=tool)`.
/// Foreground calls may still be promoted when their foreground window elapses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplicitSpawn {
    Allowed,
    Denied,
}

impl ToolRegistry {
    pub fn register(
        &mut self,
        implementation: Arc<dyn Tool>,
        explicit_spawn: ExplicitSpawn,
    ) -> Result<()> {
        let spec = implementation.spec();
        let name = spec.name.clone();
        if self.tools.contains_key(&name) {
            bail!("tool `{name}` is already registered");
        }
        self.tools.insert(
            name,
            RegisteredTool {
                implementation,
                spec,
                explicit_spawn,
            },
        );
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).map(|tool| tool.implementation.clone())
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|tool| tool.spec.clone()).collect()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(String::as_str)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    pub fn explicit_spawn_names(&self) -> Vec<String> {
        self.tools
            .iter()
            .filter(|(_, tool)| tool.explicit_spawn == ExplicitSpawn::Allowed)
            .map(|(name, _)| name.clone())
            .collect()
    }

    pub(crate) fn explicitly_spawnable(&self) -> Self {
        Self {
            tools: self
                .tools
                .iter()
                .filter(|(_, tool)| tool.explicit_spawn == ExplicitSpawn::Allowed)
                .map(|(name, tool)| (name.clone(), tool.clone()))
                .collect(),
        }
    }
}

#[cfg(test)]
mod registry_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::*;

    struct CountingTool {
        name: &'static str,
        spec_calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Tool for CountingTool {
        fn spec(&self) -> ToolSpec {
            self.spec_calls.fetch_add(1, Ordering::SeqCst);
            ToolSpec {
                name: self.name.to_owned(),
                description: "test tool".to_owned(),
                input_schema: json!({"type": "object"}),
            }
        }

        async fn execute(&self, _context: ToolContext, _arguments: Value) -> Result<RawToolOutput> {
            Ok(RawToolOutput::text("ok"))
        }
    }

    #[test]
    fn registry_freezes_specs_once_and_tracks_spawnability_explicitly() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = ToolRegistry::default();
        registry
            .register(
                Arc::new(CountingTool {
                    name: "backgroundable",
                    spec_calls: calls.clone(),
                }),
                ExplicitSpawn::Allowed,
            )
            .unwrap();
        registry
            .register(
                Arc::new(CountingTool {
                    name: "control",
                    spec_calls: calls.clone(),
                }),
                ExplicitSpawn::Denied,
            )
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            registry
                .specs()
                .into_iter()
                .map(|spec| spec.name)
                .collect::<Vec<_>>(),
            ["backgroundable", "control"]
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(registry.explicit_spawn_names(), ["backgroundable"]);
    }
}
