use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

use super::{model::GraphDocument, store::GraphStore};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphInitArgs {
    graph: GraphDocument,
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
        let derived = args
            .graph
            .validate_initial()
            .context("validate initial graph")?;
        let content =
            serde_yaml_ng::to_string(&args.graph).context("serialize initial graph YAML")?;
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
    async fn initializes_and_validates_a_complete_graph_document() {
        let workspace = tempdir().unwrap();
        let tool = GraphInitTool::new(Arc::new(GraphStore::default()));
        let output = tool
            .execute(
                context(workspace.path()),
                json!({
                    "graph": {
                        "version": 1,
                        "status": "wip",
                        "goal": "Inspect and implement the API",
                        "nodes": {
                            "inspect_api": {
                                "objective": "Inspect the API contract",
                                "depends_on": [],
                                "resolution": {
                                    "summary": "The API contract is accepted",
                                    "evidence": ["docs/api.md"]
                                }
                            },
                            "implement": {
                                "objective": "Implement the accepted contract",
                                "depends_on": ["inspect_api"],
                                "resolution": null
                            }
                        }
                    }
                }),
            )
            .await
            .unwrap();
        let result: Value = serde_json::from_slice(&output.content).unwrap();
        assert_eq!(result["id"], "g1");
        assert_eq!(result["path"], ".fiasco/runs/run-1/graphs/g1.yaml");
        assert_eq!(result["resolved"], 1);
        assert_eq!(result["unresolved"], 1);
        assert_eq!(result["ready"], json!(["implement"]));
        let source =
            tokio::fs::read_to_string(workspace.path().join(result["path"].as_str().unwrap()))
                .await
                .unwrap();
        assert_eq!(source.matches("resolution: null").count(), 1);
        assert!(source.contains("summary: The API contract is accepted"));
        assert!(source.contains("- docs/api.md"));
        assert!(!source.contains("abort_reason:"));
        let graph = GraphDocument::parse(&source).unwrap();
        assert_eq!(graph.goal, "Inspect and implement the API");
        graph.validate_initial().unwrap();
    }

    #[tokio::test]
    async fn rejects_empty_goals_and_unknown_arguments() {
        let workspace = tempdir().unwrap();
        let tool = GraphInitTool::new(Arc::new(GraphStore::default()));
        for arguments in [
            json!({"goal": "x", "nodes": {"a": {"objective": "a", "depends_on": []}}}),
            json!({"graph": {"version": 1, "status": "wip", "goal": " \n ", "nodes": {"a": {"objective": "a", "depends_on": [], "resolution": null}}}}),
            json!({"graph": {"version": 1, "status": "wip", "goal": "x", "nodes": {}, "extra": true}}),
            json!({"graph": {"version": 1, "status": "completed", "goal": "x", "summary": "done", "nodes": {}}}),
            json!({"graph": {"version": 1, "status": "wip", "goal": "x", "nodes": {"a": {"objective": "a", "depends_on": ["missing"], "resolution": null}}}}),
            json!({"graph": {"version": 1, "status": "wip", "goal": "x", "nodes": {"a": {"objective": "a", "depends_on": ["a"], "resolution": null}}}}),
            json!({"graph": {"version": 1, "status": "wip", "goal": "x", "nodes": {"a": {"objective": "a", "depends_on": ["b", "b"], "resolution": null}, "b": {"objective": "b", "depends_on": [], "resolution": null}}}}),
            json!({"graph": {"version": 1, "status": "wip", "goal": "x", "nodes": {" bad ": {"objective": "a", "depends_on": [], "resolution": null}}}}),
            json!({"graph": {"version": 1, "status": "wip", "goal": "x", "nodes": {"a": {"objective": " \n ", "depends_on": [], "resolution": null}}}}),
            json!({"graph": {"version": 1, "status": "wip", "goal": "x", "nodes": {"inspect": {"objective": "inspect", "depends_on": [], "resolution": null}, "implement": {"objective": "implement", "depends_on": ["inspect"], "resolution": {"summary": "done", "evidence": []}}}}}),
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
