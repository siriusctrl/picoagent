use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

use crate::{
    artifact::ArtifactStore,
    events::SharedEventSink,
    hooks::HookPipeline,
    memory::MemoryPaths,
    model::{ModelModality, ModelProvider},
    storage::{RunDirStore, RunRecord},
    tools::ToolRegistry,
};

#[derive(Debug, Clone)]
pub struct GeneralTaskProfile {
    pub model: Option<String>,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct RunnerOptions {
    pub model_modalities: BTreeSet<ModelModality>,
    pub max_subagent_depth: usize,
    pub max_parallel_subagents: usize,
    pub max_parallel_model_calls: usize,
    pub model_stream_idle_timeout_seconds: u64,
    pub model_request_deadline_seconds: u64,
    pub max_output_tokens: Option<u32>,
    pub foreground_tool_timeout_seconds: u64,
    pub handle_wait_timeout_seconds: u64,
    pub compaction: CompactionOptions,
    pub general_task: GeneralTaskProfile,
}

#[derive(Debug, Clone)]
pub struct CompactionOptions {
    pub compact_at_tokens: Option<u64>,
    pub context_window_tokens: Option<u64>,
    pub keep_recent_tokens: u64,
    pub summary_max_output_tokens: u32,
    pub history_search_max_matches: usize,
}

impl Default for CompactionOptions {
    fn default() -> Self {
        Self {
            compact_at_tokens: None,
            context_window_tokens: None,
            keep_recent_tokens: 20_000,
            summary_max_output_tokens: 4_096,
            history_search_max_matches: 50,
        }
    }
}

impl Default for RunnerOptions {
    fn default() -> Self {
        Self {
            model_modalities: BTreeSet::from([ModelModality::Text]),
            max_subagent_depth: 1,
            max_parallel_subagents: 4,
            max_parallel_model_calls: 1,
            model_stream_idle_timeout_seconds: 300,
            model_request_deadline_seconds: 3_600,
            max_output_tokens: None,
            foreground_tool_timeout_seconds: 30,
            handle_wait_timeout_seconds: 10,
            compaction: CompactionOptions::default(),
            general_task: GeneralTaskProfile {
                model: None,
                max_output_tokens: None,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunRequest {
    pub(crate) name: String,
    pub(crate) prompt: String,
    pub(crate) parent_run_id: Option<String>,
    pub(crate) profile: RunProfile,
    /// Frozen for durable runs. `None` is used only by a new root request,
    /// whose initial value comes from `RunnerOptions` before the run is stored.
    pub(crate) remaining_delegation_depth: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunProfile {
    Root,
    GeneralTask,
}

impl RunRequest {
    pub fn root(prompt: impl Into<String>) -> Self {
        Self {
            name: "root".to_owned(),
            prompt: prompt.into(),
            parent_run_id: None,
            profile: RunProfile::Root,
            remaining_delegation_depth: None,
        }
    }

    pub(crate) fn general_task(
        name: impl Into<String>,
        prompt: impl Into<String>,
        parent_run_id: String,
        remaining_delegation_depth: usize,
    ) -> Self {
        Self {
            name: name.into(),
            prompt: prompt.into(),
            parent_run_id: Some(parent_run_id),
            profile: RunProfile::GeneralTask,
            remaining_delegation_depth: Some(remaining_delegation_depth),
        }
    }

    pub(crate) fn from_stored(record: &RunRecord) -> anyhow::Result<Self> {
        let profile = match record.profile.as_str() {
            "root" => RunProfile::Root,
            "general_task" => RunProfile::GeneralTask,
            value => anyhow::bail!("unknown stored run profile `{value}`"),
        };
        match profile {
            RunProfile::Root => anyhow::ensure!(
                record.parent_run_id.is_none(),
                "stored root run has a parent"
            ),
            RunProfile::GeneralTask => anyhow::ensure!(
                record.parent_run_id.is_some(),
                "stored GeneralTask run has no parent"
            ),
        }
        Ok(Self {
            name: record.name.clone(),
            prompt: record.prompt.clone(),
            parent_run_id: record.parent_run_id.clone(),
            profile,
            remaining_delegation_depth: Some(record.remaining_delegation_depth),
        })
    }
}

impl RunProfile {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::GeneralTask => "general_task",
        }
    }

    pub(crate) fn runtime_role(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::GeneralTask => "general_task",
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
