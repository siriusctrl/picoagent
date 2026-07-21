use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

use super::{model::GRAPH_VERSION, store::GraphStore};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphInitArgs {
    goal: String,
}

#[derive(Clone)]
pub struct GraphInitTool {
    store: Arc<GraphStore>,
}

impl GraphInitTool {
    pub(super) fn new(store: Arc<GraphStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for GraphInitTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: GraphInitArgs =
            serde_json::from_value(arguments).context("invalid graph_init arguments")?;
        let goal = normalize_goal(&args.goal)?;
        let content = initial_document(&goal);
        let (id, path) = self.store.create_next(&context, content.as_bytes()).await?;
        Ok(RawToolOutput::text(
            serde_json::to_string_pretty(&json!({
                "id": id,
                "path": GraphStore::display_path(&context, &path),
                "status": "wip",
            }))
            .context("serialize graph_init result")?,
        ))
    }
}

fn normalize_goal(goal: &str) -> Result<String> {
    ensure!(
        goal.chars()
            .all(|character| !character.is_control() || character.is_whitespace()),
        "graph goal must not contain control characters"
    );
    let normalized = goal.split_whitespace().collect::<Vec<_>>().join(" ");
    ensure!(!normalized.is_empty(), "graph goal must not be empty");
    Ok(normalized)
}

fn initial_document(goal: &str) -> String {
    let mut document = format!("version: {GRAPH_VERSION}\nstatus: wip\ngoal: >-\n");
    for line in fold_words(goal, 88) {
        document.push_str("  ");
        document.push_str(&line);
        document.push('\n');
    }
    document.push_str("nodes: {}\n");
    document
}

fn fold_words(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split(' ') {
        if current.is_empty() {
            current.push_str(word);
        } else if current.len() + 1 + word.len() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(current);
            current = word.to_owned();
        }
    }
    lines.push(current);
    lines
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;
    use tempfile::tempdir;

    use crate::tools::graph::model::GraphDocument;

    use super::*;

    fn context(workspace: &Path) -> ToolContext {
        ToolContext {
            run_id: "run-1".to_owned(),
            call_id: "init".to_owned(),
            workspace: workspace.to_owned(),
        }
    }

    #[tokio::test]
    async fn initializes_a_safe_folded_yaml_skeleton() {
        let workspace = tempdir().unwrap();
        let tool = GraphInitTool::new(Arc::new(GraphStore::default()));
        let output = tool
            .execute(
                context(workspace.path()),
                json!({"goal": "  Inspect: API # behavior\nand implement it  "}),
            )
            .await
            .unwrap();
        let result: Value = serde_json::from_slice(&output.content).unwrap();
        assert_eq!(result["id"], "g1");
        assert_eq!(result["path"], ".pico/runs/run-1/graphs/g1.yaml");
        let source =
            tokio::fs::read_to_string(workspace.path().join(result["path"].as_str().unwrap()))
                .await
                .unwrap();
        assert!(source.contains("goal: >-\n"));
        let graph = GraphDocument::parse(&source).unwrap();
        assert_eq!(graph.goal, "Inspect: API # behavior and implement it");
        graph.validate().unwrap();
    }

    #[tokio::test]
    async fn rejects_empty_goals_and_unknown_arguments() {
        let workspace = tempdir().unwrap();
        let tool = GraphInitTool::new(Arc::new(GraphStore::default()));
        for arguments in [json!({"goal": " \n "}), json!({"goal": "x", "extra": true})] {
            assert!(
                tool.execute(context(workspace.path()), arguments)
                    .await
                    .is_err()
            );
        }
    }
}
