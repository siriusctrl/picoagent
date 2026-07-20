use std::{path::PathBuf, sync::Arc};

use crate::{
    artifact::ArtifactStore, events::SharedEventSink, hooks::HookPipeline, memory::MemoryPaths,
    model::ModelProvider, storage::RunDirStore, tools::ToolRegistry,
};

#[derive(Debug, Clone)]
pub struct GeneralTaskProfile {
    pub model: Option<String>,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct RunnerOptions {
    pub max_subagent_depth: usize,
    pub max_parallel_tasks: usize,
    pub max_parallel_model_calls: usize,
    pub model_stream_idle_timeout_seconds: u64,
    pub model_request_deadline_seconds: u64,
    pub max_output_tokens: Option<u32>,
    pub foreground_tool_timeout_seconds: u64,
    pub task_wait_timeout_seconds: u64,
    pub compaction: CompactionOptions,
    pub general_task: GeneralTaskProfile,
}

#[derive(Debug, Clone)]
pub struct CompactionOptions {
    pub trigger_tokens: Option<u64>,
    pub keep_recent_tokens: u64,
    pub summary_max_output_tokens: u32,
    pub history_search_max_matches: usize,
}

impl Default for CompactionOptions {
    fn default() -> Self {
        Self {
            trigger_tokens: None,
            keep_recent_tokens: 20_000,
            summary_max_output_tokens: 4_096,
            history_search_max_matches: 50,
        }
    }
}

impl Default for RunnerOptions {
    fn default() -> Self {
        Self {
            max_subagent_depth: 1,
            max_parallel_tasks: 4,
            max_parallel_model_calls: 1,
            model_stream_idle_timeout_seconds: 300,
            model_request_deadline_seconds: 3_600,
            max_output_tokens: None,
            foreground_tool_timeout_seconds: 300,
            task_wait_timeout_seconds: 30,
            compaction: CompactionOptions::default(),
            general_task: GeneralTaskProfile {
                model: None,
                max_output_tokens: Some(4_096),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunRequest {
    pub(crate) prompt: String,
    pub(crate) parent_run_id: Option<String>,
    pub(crate) depth: usize,
    pub(crate) additional_instructions: Option<String>,
    pub(crate) profile: RunProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunProfile {
    Root,
    GeneralTaskDelegating,
    GeneralTaskLeaf,
}

impl RunRequest {
    pub fn root(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            parent_run_id: None,
            depth: 0,
            additional_instructions: None,
            profile: RunProfile::Root,
        }
    }

    pub(crate) fn general_task(
        prompt: impl Into<String>,
        parent_run_id: String,
        depth: usize,
        additional_instructions: String,
        can_delegate: bool,
    ) -> Self {
        Self {
            prompt: prompt.into(),
            parent_run_id: Some(parent_run_id),
            depth,
            additional_instructions: Some(additional_instructions),
            profile: if can_delegate {
                RunProfile::GeneralTaskDelegating
            } else {
                RunProfile::GeneralTaskLeaf
            },
        }
    }

    pub(crate) fn from_stored(
        prompt: String,
        parent_run_id: Option<String>,
        depth: usize,
        additional_instructions: Option<String>,
        profile: &str,
    ) -> anyhow::Result<Self> {
        let profile = match profile {
            "root" => RunProfile::Root,
            "general_task_delegating" => RunProfile::GeneralTaskDelegating,
            "general_task_leaf" => RunProfile::GeneralTaskLeaf,
            value => anyhow::bail!("unknown stored run profile `{value}`"),
        };
        Ok(Self {
            prompt,
            parent_run_id,
            depth,
            additional_instructions,
            profile,
        })
    }
}

impl RunProfile {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::GeneralTaskDelegating => "general_task_delegating",
            Self::GeneralTaskLeaf => "general_task_leaf",
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
