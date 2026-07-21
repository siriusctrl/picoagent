use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

use super::{
    model::{GraphDocument, GraphNode},
    store::GraphStore,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphInitArgs {
    goal: String,
    nodes: BTreeMap<String, GraphInitNode>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphInitNode {
    objective: String,
    depends_on: Vec<String>,
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
        ensure!(
            !args.nodes.is_empty(),
            "graph must contain at least one node"
        );
        let nodes = args
            .nodes
            .into_iter()
            .map(|(id, node)| {
                let objective = normalize_text(&node.objective, &format!("node `{id}` objective"))?;
                Ok((
                    id,
                    GraphNode {
                        objective,
                        depends_on: node.depends_on,
                        resolution: None,
                    },
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let graph = GraphDocument::initial(goal, nodes);
        let derived = graph.validate().context("validate initial graph")?;
        let content = serde_yaml_ng::to_string(&graph).context("serialize initial graph YAML")?;
        let (id, path) = self.store.create_next(&context, content.as_bytes()).await?;
        Ok(RawToolOutput::text(
            serde_json::to_string_pretty(&json!({
                "id": id,
                "path": GraphStore::display_path(&context, &path),
                "status": "wip",
                "resolved": derived.resolved,
                "unresolved": derived.unresolved,
                "ready": derived.ready,
            }))
            .context("serialize graph_init result")?,
        ))
    }
}

fn normalize_goal(goal: &str) -> Result<String> {
    normalize_text(goal, "graph goal")
}

fn normalize_text(value: &str, label: &str) -> Result<String> {
    ensure!(
        value
            .chars()
            .all(|character| !character.is_control() || character.is_whitespace()),
        "{label} must not contain control characters"
    );
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    ensure!(!normalized.is_empty(), "{label} must not be empty");
    Ok(normalized)
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
    async fn initializes_and_validates_a_complete_topology() {
        let workspace = tempdir().unwrap();
        let tool = GraphInitTool::new(Arc::new(GraphStore::default()));
        let output = tool
            .execute(
                context(workspace.path()),
                json!({
                    "goal": "  Inspect: API # behavior\nand implement it  ",
                    "nodes": {
                        "inspect_api": {
                            "objective": " Inspect the API contract ",
                            "depends_on": []
                        },
                        "implement": {
                            "objective": "Implement the accepted contract",
                            "depends_on": ["inspect_api"]
                        }
                    }
                }),
            )
            .await
            .unwrap();
        let result: Value = serde_json::from_slice(&output.content).unwrap();
        assert_eq!(result["id"], "g1");
        assert_eq!(result["path"], ".fiasco/runs/run-1/graphs/g1.yaml");
        assert_eq!(result["resolved"], 0);
        assert_eq!(result["unresolved"], 2);
        assert_eq!(result["ready"], json!(["inspect_api"]));
        let source =
            tokio::fs::read_to_string(workspace.path().join(result["path"].as_str().unwrap()))
                .await
                .unwrap();
        assert_eq!(source.matches("resolution: null").count(), 2);
        assert!(!source.contains("summary:"));
        assert!(!source.contains("abort_reason:"));
        let graph = GraphDocument::parse(&source).unwrap();
        assert_eq!(graph.goal, "Inspect: API # behavior and implement it");
        graph.validate().unwrap();
    }

    #[tokio::test]
    async fn rejects_empty_goals_and_unknown_arguments() {
        let workspace = tempdir().unwrap();
        let tool = GraphInitTool::new(Arc::new(GraphStore::default()));
        for arguments in [
            json!({"goal": " \n ", "nodes": {"a": {"objective": "a", "depends_on": []}}}),
            json!({"goal": "x", "nodes": {}, "extra": true}),
            json!({"goal": "x", "nodes": {}}),
            json!({"goal": "x", "nodes": {"a": {"objective": "a", "depends_on": ["missing"]}}}),
            json!({"goal": "x", "nodes": {"a": {"objective": "a", "depends_on": ["a"]}}}),
            json!({"goal": "x", "nodes": {"a": {"objective": "a", "depends_on": ["b", "b"]}, "b": {"objective": "b", "depends_on": []}}}),
            json!({"goal": "x", "nodes": {" bad ": {"objective": "a", "depends_on": []}}}),
            json!({"goal": "x", "nodes": {"a": {"objective": " \n ", "depends_on": []}}}),
        ] {
            assert!(
                tool.execute(context(workspace.path()), arguments)
                    .await
                    .is_err()
            );
        }
        assert!(!workspace.path().join(".fiasco/runs/run-1/graphs").exists());
    }
}
