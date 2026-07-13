use std::{path::PathBuf, sync::Arc};

use crate::{
    artifact::ArtifactStore, events::SharedEventSink, hooks::HookPipeline, memory::MemoryPaths,
    model::ModelProvider, storage::RunDirStore, tools::ToolRegistry,
};

#[derive(Debug, Clone)]
pub struct GeneralTaskProfile {
    pub model: Option<String>,
    pub max_steps: usize,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct RunnerOptions {
    pub max_steps: usize,
    pub max_subagent_depth: usize,
    pub max_parallel_tasks: usize,
    pub max_output_tokens: Option<u32>,
    pub direct_tool_timeout_seconds: u64,
    pub task_execution_timeout_seconds: u64,
    pub task_wait_timeout_seconds: u64,
    pub task_max_timeout_seconds: u64,
    pub general_task: GeneralTaskProfile,
}

impl Default for RunnerOptions {
    fn default() -> Self {
        Self {
            max_steps: 32,
            max_subagent_depth: 1,
            max_parallel_tasks: 4,
            max_output_tokens: None,
            direct_tool_timeout_seconds: 300,
            task_execution_timeout_seconds: 300,
            task_wait_timeout_seconds: 30,
            task_max_timeout_seconds: 1_800,
            general_task: GeneralTaskProfile {
                model: None,
                max_steps: 8,
                max_output_tokens: Some(4_096),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunRequest {
    pub prompt: String,
    pub parent_run_id: Option<String>,
    pub depth: usize,
    pub additional_instructions: Option<String>,
    pub tool_allowlist: Option<Vec<String>>,
    pub use_general_task_profile: bool,
}

impl RunRequest {
    pub fn root(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            parent_run_id: None,
            depth: 0,
            additional_instructions: None,
            tool_allowlist: None,
            use_general_task_profile: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunResult {
    pub run_id: String,
    pub final_output: String,
}

pub struct AgentRunnerConfig {
    pub provider: Arc<dyn ModelProvider>,
    pub model: String,
    pub workspace: PathBuf,
    pub skill_catalog: String,
    pub tools: ToolRegistry,
    pub artifacts: ArtifactStore,
    pub store: RunDirStore,
    pub hooks: HookPipeline,
    pub memory: Option<MemoryPaths>,
    pub extra_events: SharedEventSink,
    pub options: RunnerOptions,
}
