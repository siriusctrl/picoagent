use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    agent::task::{BackgroundTaskRecord, TaskManager},
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

const DEFAULT_INSPECT_LIMIT: usize = 6;
const MAX_INSPECT_LIMIT: usize = 20;

pub struct TaskTool {
    manager: Arc<TaskManager>,
}

impl TaskTool {
    pub fn new(manager: Arc<TaskManager>) -> Self {
        Self { manager }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskArgs {
    action: String,
    #[serde(default)]
    task_ids: Vec<String>,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    before_seq: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl Tool for TaskTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: TaskArgs = serde_json::from_value(arguments).context("invalid task arguments")?;
        let output = match args.action.as_str() {
            "status" => task_records(self.manager.status(&args.task_ids).await?),
            "wait" => task_records(self.manager.wait(&args.task_ids).await?),
            "inspect" => {
                let task_id = args.task_id.context("task inspect requires `task_id`")?;
                let limit = args.limit.unwrap_or(DEFAULT_INSPECT_LIMIT);
                if !(1..=MAX_INSPECT_LIMIT).contains(&limit) {
                    bail!("task inspect limit must be between 1 and {MAX_INSPECT_LIMIT}");
                }
                self.manager
                    .inspect(&task_id, args.before_seq, limit)
                    .await?
            }
            "steer" => {
                let task_id = args.task_id.context("task steer requires `task_id`")?;
                self.manager
                    .steer(
                        &task_id,
                        args.message.context("task steer requires `message`")?,
                    )
                    .await?
            }
            "stop" => {
                let task_id = args.task_id.context("task stop requires `task_id`")?;
                task_records(vec![self.manager.stop(&task_id).await?])
            }
            action => bail!("invalid task action `{action}`"),
        };
        Ok(RawToolOutput::text(serde_json::to_string(&output)?))
    }
}

fn task_records(records: Vec<BackgroundTaskRecord>) -> Value {
    let tasks = records
        .into_iter()
        .map(|record| {
            json!({
                "task_id": record.id,
                "kind": record.kind,
                "name": record.name,
                "status": record.status(),
            })
        })
        .collect::<Vec<_>>();
    json!({ "tasks": tasks })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_limits_match_runtime_constants() {
        let spec = crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!());
        assert_eq!(
            spec.input_schema.pointer("/properties/limit/maximum"),
            Some(&json!(MAX_INSPECT_LIMIT))
        );
        assert!(spec.description.contains(&format!(
            "latest {DEFAULT_INSPECT_LIMIT} messages by default"
        )));
    }

    #[test]
    fn task_status_is_structured_without_explanatory_messages() {
        let record = BackgroundTaskRecord::queued_tool("t1".to_owned(), "bash".to_owned());

        assert_eq!(
            task_records(vec![record]),
            json!({
                "tasks": [{
                    "task_id": "t1",
                    "kind": "tool",
                    "name": "bash",
                    "status": "queued"
                }]
            })
        );
    }
}
