use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

use super::{
    model::{
        GraphDocument, GraphListing, GraphStatus, GraphSummary, InvalidGraph, graph_id_from_path,
    },
    store::GraphStore,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphListArgs {}

#[derive(Clone)]
pub struct GraphListTool {
    store: Arc<GraphStore>,
}

impl GraphListTool {
    pub(super) fn new(store: Arc<GraphStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for GraphListTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let _: GraphListArgs =
            serde_json::from_value(arguments).context("invalid graph_list arguments")?;
        let listing = list_graphs(&self.store, &context).await?;
        Ok(RawToolOutput::text(
            serde_json::to_string_pretty(&listing).context("serialize graph listing")?,
        ))
    }
}

async fn list_graphs(store: &GraphStore, context: &ToolContext) -> Result<GraphListing> {
    let _guard = store.lock().await;
    let directory = GraphStore::directory(context)?;
    let mut listing = GraphListing::default();
    if !tokio::fs::try_exists(&directory).await? {
        return Ok(listing);
    }

    let mut reader = tokio::fs::read_dir(&directory)
        .await
        .with_context(|| format!("read graph directory {}", directory.display()))?;
    let mut paths = Vec::new();
    while let Some(entry) = reader.next_entry().await? {
        let file_type = entry.file_type().await?;
        if file_type.is_file()
            && entry.path().extension().and_then(|value| value.to_str()) == Some("yaml")
        {
            paths.push(entry.path());
        }
    }
    paths.sort_by_key(|path| {
        graph_id_from_path(path)
            .map(|(_, number)| (0_u8, number, String::new()))
            .unwrap_or_else(|_| (1, 0, path.to_string_lossy().into_owned()))
    });

    for path in paths {
        classify_graph(context, path, &mut listing).await;
    }
    Ok(listing)
}

async fn classify_graph(context: &ToolContext, path: PathBuf, listing: &mut GraphListing) {
    let display_path = GraphStore::display_path(context, &path);
    let parsed_id = graph_id_from_path(&path);
    let id = parsed_id.as_ref().ok().map(|(id, _)| id.clone());
    let result = async {
        let (id, _) = parsed_id?;
        let source = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("read graph `{id}`"))?;
        let graph = GraphDocument::parse(&source)?;
        let derived = graph.validate()?;
        Ok::<_, anyhow::Error>((
            graph.status,
            GraphSummary {
                id,
                path: display_path.clone(),
                goal: graph.goal,
                resolved: derived.resolved,
                unresolved: derived.unresolved,
                ready: derived.ready,
                summary: graph.summary,
                abort_reason: graph.abort_reason,
            },
        ))
    }
    .await;

    match result {
        Ok((GraphStatus::Wip, summary)) => listing.wip.push(summary),
        Ok((GraphStatus::Completed, summary)) => listing.completed.push(summary),
        Ok((GraphStatus::Aborted, summary)) => listing.aborted.push(summary),
        Err(error) => listing.invalid.push(InvalidGraph {
            id,
            path: display_path,
            error: format!("{error:#}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    fn context(workspace: &Path) -> ToolContext {
        ToolContext {
            run_id: "run-1".to_owned(),
            call_id: "list".to_owned(),
            workspace: workspace.to_owned(),
        }
    }

    async fn write_graph(workspace: &Path, name: &str, source: &str) {
        let directory = workspace.join(".fiasco/runs/run-1/graphs");
        tokio::fs::create_dir_all(&directory).await.unwrap();
        tokio::fs::write(directory.join(name), source)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn returns_empty_groups_before_any_graph_exists() {
        let workspace = tempdir().unwrap();
        let tool = GraphListTool::new(Arc::new(GraphStore::default()));
        let output = tool
            .execute(context(workspace.path()), json!({}))
            .await
            .unwrap();
        let listing: Value = serde_json::from_slice(&output.content).unwrap();
        for group in ["wip", "completed", "aborted", "invalid"] {
            assert_eq!(listing[group], json!([]));
        }
    }

    #[tokio::test]
    async fn groups_graphs_and_derives_ready_nodes() {
        let workspace = tempdir().unwrap();
        write_graph(
            workspace.path(),
            "g1.yaml",
            "version: 1\nstatus: wip\ngoal: implement\nnodes:\n  inspect:\n    objective: inspect\n    resolution:\n      summary: done\n      evidence: [docs/report.md]\n  implement:\n    objective: implement\n    depends_on: [inspect]\n    resolution: null\n",
        )
        .await;
        write_graph(
            workspace.path(),
            "g2.yaml",
            "version: 1\nstatus: completed\ngoal: done\nsummary: shipped\nnodes: {}\n",
        )
        .await;
        write_graph(
            workspace.path(),
            "g3.yaml",
            "version: 1\nstatus: aborted\ngoal: old\nabort_reason: superseded\nnodes:\n  unused:\n    objective: no longer needed\n    resolution: null\n",
        )
        .await;

        let listing = list_graphs(&GraphStore::default(), &context(workspace.path()))
            .await
            .unwrap();
        assert_eq!(listing.wip[0].id, "g1");
        assert_eq!(listing.wip[0].resolved, 1);
        assert_eq!(listing.wip[0].unresolved, 1);
        assert_eq!(listing.wip[0].ready, ["implement"]);
        assert_eq!(listing.completed[0].summary.as_deref(), Some("shipped"));
        assert_eq!(
            listing.aborted[0].abort_reason.as_deref(),
            Some("superseded")
        );
        assert!(listing.aborted[0].ready.is_empty());
        assert!(listing.invalid.is_empty());
    }

    #[tokio::test]
    async fn reports_parse_name_dependency_cycle_and_completion_errors_as_invalid() {
        let workspace = tempdir().unwrap();
        for (name, source) in [
            ("bad.yaml", "not: [valid"),
            (
                "g2.yaml",
                "version: 1\nstatus: wip\ngoal: x\nnodes:\n  a:\n    objective: a\n    depends_on: [missing]\n    resolution: null\n",
            ),
            (
                "g3.yaml",
                "version: 1\nstatus: wip\ngoal: x\nnodes:\n  a:\n    objective: a\n    depends_on: [b]\n    resolution: null\n  b:\n    objective: b\n    depends_on: [a]\n    resolution: null\n",
            ),
            (
                "g4.yaml",
                "version: 1\nstatus: completed\ngoal: x\nsummary: premature\nnodes:\n  a:\n    objective: a\n    resolution: null\n",
            ),
        ] {
            write_graph(workspace.path(), name, source).await;
        }

        let listing = list_graphs(&GraphStore::default(), &context(workspace.path()))
            .await
            .unwrap();
        assert_eq!(listing.invalid.len(), 4);
        let errors = listing
            .invalid
            .iter()
            .map(|graph| graph.error.as_str())
            .collect::<Vec<_>>();
        assert!(errors.iter().any(|error| error.contains("file name")));
        assert!(errors.iter().any(|error| error.contains("unknown node")));
        assert!(errors.iter().any(|error| error.contains("cycle")));
        assert!(errors.iter().any(|error| error.contains("unresolved")));
    }

    #[tokio::test]
    async fn rejects_arguments_but_does_not_fail_the_whole_list_for_a_bad_graph() {
        let workspace = tempdir().unwrap();
        write_graph(workspace.path(), "g1.yaml", "bad: yaml").await;
        let tool = GraphListTool::new(Arc::new(GraphStore::default()));
        let output = tool
            .execute(context(workspace.path()), json!({}))
            .await
            .unwrap();
        assert!(
            String::from_utf8(output.content)
                .unwrap()
                .contains("invalid")
        );
        assert!(
            tool.execute(context(workspace.path()), json!({"unexpected": true}))
                .await
                .is_err()
        );
    }
}
