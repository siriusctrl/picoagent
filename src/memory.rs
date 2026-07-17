use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ulid::Ulid;

use crate::{
    agent::runner::{AgentRunner, RunRequest},
    events::{RuntimeEvent, RuntimeEventKind, SharedEventSink},
    model::ToolSpec,
    prompts::agent_prompts,
    storage::{RunDirStore, RunState},
    tools::{RawToolOutput, Tool, ToolContext},
};

const MEMORY_UPDATE_DESCRIPTION: &str = include_str!("memory/descriptions/memory_update.md");
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    User,
    Project,
}

#[derive(Debug, Clone)]
pub struct MemoryPaths {
    pub user: PathBuf,
    pub project: PathBuf,
}

impl MemoryPaths {
    pub fn new(home: impl Into<PathBuf>, workspace: impl Into<PathBuf>) -> Self {
        Self {
            user: home.into().join("memory/user"),
            project: workspace.into().join(".pico/memory/project"),
        }
    }

    pub fn root(&self, scope: MemoryScope) -> &std::path::Path {
        match scope {
            MemoryScope::User => &self.user,
            MemoryScope::Project => &self.project,
        }
    }

    pub fn runtime_reminder_section(&self, can_update: bool) -> String {
        let guidance = if can_update {
            "Use `memory_update` when durable knowledge should be added, corrected, merged, or removed; do not edit memory directly during the main task."
        } else {
            "This profile has no `memory_update` capability. Treat these memory files as read-only during this run; do not modify them directly."
        };
        format!(
            "user: {}\nproject: {}\n\nUse `read` and `bash` to inspect these ordinary Markdown files. {guidance}",
            self.user.display(),
            self.project.display()
        )
    }
}

pub struct MemoryUpdateTool {
    runner: Arc<AgentRunner>,
    paths: MemoryPaths,
    parent_run_id: String,
    parent_depth: usize,
    events: SharedEventSink,
}

struct ChildRunGuard {
    store: RunDirStore,
    events: SharedEventSink,
    parent_run_id: String,
    child_run_id: String,
    armed: bool,
}

impl ChildRunGuard {
    fn new(
        store: RunDirStore,
        events: SharedEventSink,
        parent_run_id: String,
        child_run_id: String,
    ) -> Self {
        Self {
            store,
            events,
            parent_run_id,
            child_run_id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ChildRunGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let store = self.store.clone();
        let events = self.events.clone();
        let parent_run_id = self.parent_run_id.clone();
        let child_run_id = self.child_run_id.clone();
        tokio::spawn(async move {
            if let Ok(run) = store.load_run(&child_run_id).await
                && matches!(run.state, RunState::Queued | RunState::Running)
            {
                let _ = store.update_state(&child_run_id, RunState::Failed).await;
            }
            let _ = events
                .emit(&RuntimeEvent::new(
                    parent_run_id,
                    RuntimeEventKind::SubagentFailed {
                        child_run_id,
                        error: "memory update was cancelled or timed out".to_owned(),
                    },
                ))
                .await;
        });
    }
}

impl MemoryUpdateTool {
    pub fn new(
        runner: Arc<AgentRunner>,
        paths: MemoryPaths,
        parent_run_id: String,
        parent_depth: usize,
        events: SharedEventSink,
    ) -> Self {
        Self {
            runner,
            paths,
            parent_run_id,
            parent_depth,
            events,
        }
    }
}

#[derive(Debug, Deserialize)]
struct MemoryUpdateArgs {
    scope: MemoryScope,
    instruction: String,
}

#[async_trait]
impl Tool for MemoryUpdateTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_update".to_owned(),
            description: MEMORY_UPDATE_DESCRIPTION.trim().to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "scope": { "type": "string", "enum": ["user", "project"] },
                    "instruction": { "type": "string", "description": "What durable knowledge should be recorded, corrected, merged, or forgotten" }
                },
                "required": ["scope", "instruction"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: MemoryUpdateArgs =
            serde_json::from_value(arguments).context("invalid memory_update arguments")?;
        if args.instruction.trim().is_empty() {
            bail!("memory_update instruction must not be empty");
        }
        let root = self.paths.root(args.scope);
        tokio::fs::create_dir_all(root).await?;
        let child_run_id = Ulid::new().to_string();
        self.events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::SubagentStarted {
                    child_run_id: child_run_id.clone(),
                    task: format!("memory_update:{:?}", args.scope),
                },
            ))
            .await?;
        let prompt = format!(
            "Update durable {:?} memory stored at {}.\n\nInstruction from the parent agent:\n{}\n\nRead and search the existing Markdown files before changing them. Use write with targeted edits when possible. Decide semantically whether to add, update, merge, or remove information; the harness should not make that judgment. Keep memory concise, factual, and easy to search. Return a short summary listing changed files and what changed.",
            args.scope,
            root.display(),
            args.instruction.trim()
        );
        let mut guard = ChildRunGuard::new(
            self.runner.store().clone(),
            self.events.clone(),
            self.parent_run_id.clone(),
            child_run_id.clone(),
        );
        let result = self
            .runner
            .run_with_id(
                RunRequest::memory_maintenance_child(
                    prompt,
                    self.parent_run_id.clone(),
                    self.parent_depth + 1,
                    agent_prompts().memory_maintenance.clone(),
                ),
                child_run_id.clone(),
            )
            .await;
        guard.disarm();
        match result {
            Ok(result) => {
                self.events
                    .emit(&RuntimeEvent::new(
                        &self.parent_run_id,
                        RuntimeEventKind::SubagentCompleted {
                            child_run_id: child_run_id.clone(),
                        },
                    ))
                    .await?;
                Ok(RawToolOutput::text(
                    json!({ "run_id": child_run_id, "summary": result.final_output }).to_string(),
                ))
            }
            Err(error) => {
                let error = format!("{error:#}");
                self.events
                    .emit(&RuntimeEvent::new(
                        &self.parent_run_id,
                        RuntimeEventKind::SubagentFailed {
                            child_run_id: child_run_id.clone(),
                            error: error.clone(),
                        },
                    ))
                    .await?;
                bail!("memory update subagent {child_run_id} failed: {error}")
            }
        }
    }
}
