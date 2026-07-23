use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

pub(super) const GRAPH_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum GraphStatus {
    Wip,
    Completed,
    Aborted,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GraphDocument {
    version: u32,
    pub(super) status: GraphStatus,
    pub(super) goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) abort_reason: Option<String>,
    nodes: BTreeMap<String, GraphNode>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GraphNode {
    pub(super) objective: String,
    #[serde(default)]
    pub(super) depends_on: Vec<String>,
    #[serde(default)]
    pub(super) resolution: Option<GraphResolution>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GraphResolution {
    summary: String,
    #[serde(default)]
    evidence: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct DerivedState {
    pub(super) resolved: usize,
    pub(super) unresolved: usize,
    pub(super) ready: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct GraphSummary {
    pub(super) id: String,
    pub(super) path: String,
    pub(super) goal: String,
    pub(super) resolved: usize,
    pub(super) unresolved: usize,
    pub(super) ready: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) abort_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct InvalidGraph {
    pub(super) id: Option<String>,
    pub(super) path: String,
    pub(super) error: String,
}

#[derive(Debug, Default, Serialize)]
pub(super) struct GraphListing {
    pub(super) wip: Vec<GraphSummary>,
    pub(super) completed: Vec<GraphSummary>,
    pub(super) aborted: Vec<GraphSummary>,
    pub(super) invalid: Vec<InvalidGraph>,
}

impl GraphDocument {
    pub(super) fn parse(source: &str) -> Result<Self> {
        serde_yaml_ng::from_str(source).context("parse graph YAML")
    }

    pub(super) fn validate_initial(&self) -> Result<DerivedState> {
        ensure!(
            self.status == GraphStatus::Wip,
            "initial graph status must be `wip`"
        );
        ensure!(
            !self.nodes.is_empty(),
            "initial graph must contain at least one node"
        );
        self.validate()
    }

    pub(super) fn validate(&self) -> Result<DerivedState> {
        ensure!(
            self.version == GRAPH_VERSION,
            "unsupported graph version {}; expected {GRAPH_VERSION}",
            self.version
        );
        ensure_nonempty("goal", &self.goal)?;

        for (id, node) in &self.nodes {
            validate_node_id(id)?;
            ensure_nonempty(&format!("node `{id}` objective"), &node.objective)?;
            let mut dependencies = BTreeSet::new();
            for dependency in &node.depends_on {
                ensure!(
                    dependencies.insert(dependency),
                    "node `{id}` repeats dependency `{dependency}`"
                );
                ensure!(
                    self.nodes.contains_key(dependency),
                    "node `{id}` depends on unknown node `{dependency}`"
                );
            }
            if let Some(resolution) = &node.resolution {
                ensure_nonempty(
                    &format!("node `{id}` resolution summary"),
                    &resolution.summary,
                )?;
                for (index, evidence) in resolution.evidence.iter().enumerate() {
                    validate_evidence_path(id, index, evidence)?;
                }
                for dependency in &node.depends_on {
                    ensure!(
                        self.nodes
                            .get(dependency)
                            .is_some_and(|node| node.resolution.is_some()),
                        "node `{id}` is resolved but dependency `{dependency}` is unresolved"
                    );
                }
            }
        }
        ensure_acyclic(&self.nodes)?;

        let resolved = self
            .nodes
            .values()
            .filter(|node| node.resolution.is_some())
            .count();
        let unresolved = self.nodes.len() - resolved;
        let ready = if self.status == GraphStatus::Wip {
            self.nodes
                .iter()
                .filter(|(_, node)| {
                    node.resolution.is_none()
                        && node.depends_on.iter().all(|dependency| {
                            self.nodes
                                .get(dependency)
                                .is_some_and(|node| node.resolution.is_some())
                        })
                })
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        match self.status {
            GraphStatus::Wip => {
                ensure!(self.summary.is_none(), "wip graph must not have `summary`");
                ensure!(
                    self.abort_reason.is_none(),
                    "wip graph must not have `abort_reason`"
                );
            }
            GraphStatus::Completed => {
                ensure!(
                    unresolved == 0,
                    "completed graph has {unresolved} unresolved node(s)"
                );
                ensure_optional_nonempty("completed graph summary", self.summary.as_deref())?;
                ensure!(
                    self.abort_reason.is_none(),
                    "completed graph must not have `abort_reason`"
                );
            }
            GraphStatus::Aborted => {
                ensure_optional_nonempty("aborted graph reason", self.abort_reason.as_deref())?;
                ensure!(
                    self.summary.is_none(),
                    "aborted graph must not have `summary`"
                );
            }
        }

        Ok(DerivedState {
            resolved,
            unresolved,
            ready,
        })
    }
}

pub(super) fn graph_id_from_path(path: &Path) -> Result<(String, u64)> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("graph path has a non-UTF-8 file name")?;
    let number = file_name
        .strip_prefix('g')
        .and_then(|rest| rest.strip_suffix(".yaml"))
        .and_then(|rest| rest.parse::<u64>().ok())
        .filter(|number| *number > 0)
        .context("graph file name must be `g<N>.yaml` with positive N")?;
    let id = format!("g{number}");
    ensure!(
        file_name == format!("{id}.yaml"),
        "graph id is not canonical"
    );
    Ok((id, number))
}

fn ensure_nonempty(label: &str, value: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "{label} must not be empty");
    Ok(())
}

fn ensure_optional_nonempty(label: &str, value: Option<&str>) -> Result<()> {
    let value = value.with_context(|| format!("{label} is required"))?;
    ensure_nonempty(label, value)
}

fn validate_node_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty() && id.trim() == id,
        "node id must be non-empty and have no boundary whitespace"
    );
    Ok(())
}

fn validate_evidence_path(node_id: &str, index: usize, value: &str) -> Result<()> {
    ensure_nonempty(&format!("node `{node_id}` evidence[{index}]"), value)?;
    let path = Path::new(value);
    ensure!(
        !path.is_absolute()
            && path.components().all(|component| {
                !matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            }),
        "node `{node_id}` evidence[{index}] must be a project-relative path without `..`"
    );
    Ok(())
}

fn ensure_acyclic(nodes: &BTreeMap<String, GraphNode>) -> Result<()> {
    let mut remaining_dependencies = nodes
        .iter()
        .map(|(id, node)| {
            (
                id.as_str(),
                node.depends_on
                    .iter()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    loop {
        let resolved = remaining_dependencies
            .iter()
            .filter(|(_, dependencies)| dependencies.is_empty())
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        if resolved.is_empty() {
            break;
        }
        for id in &resolved {
            remaining_dependencies.remove(id);
        }
        for dependencies in remaining_dependencies.values_mut() {
            for id in &resolved {
                dependencies.remove(id);
            }
        }
    }
    if let Some(id) = remaining_dependencies.keys().next() {
        bail!("graph contains a dependency cycle through `{id}`");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> GraphDocument {
        GraphDocument::parse(source).unwrap()
    }

    #[test]
    fn derives_ready_nodes_from_resolved_dependencies() {
        let graph = parse(
            r#"
version: 1
status: wip
goal: Build the feature
nodes:
  inspect:
    objective: Inspect the implementation
    depends_on: []
    resolution:
      summary: Inspection complete
      evidence:
        - docs/report.md
  implement:
    objective: Implement it
    depends_on: [inspect]
    resolution: null
  review:
    objective: Review it
    depends_on: [implement]
    resolution: null
"#,
        );

        assert_eq!(
            graph.validate().unwrap(),
            DerivedState {
                resolved: 1,
                unresolved: 2,
                ready: vec!["implement".to_owned()],
            }
        );
    }

    #[test]
    fn rejects_unknown_dependencies_cycles_and_duplicate_dependencies() {
        for (source, expected) in [
            (
                "version: 1\nstatus: wip\ngoal: x\nnodes:\n  a:\n    objective: a\n    depends_on: [missing]\n    resolution: null\n",
                "unknown node",
            ),
            (
                "version: 1\nstatus: wip\ngoal: x\nnodes:\n  a:\n    objective: a\n    depends_on: [b]\n    resolution: null\n  b:\n    objective: b\n    depends_on: [a]\n    resolution: null\n",
                "cycle",
            ),
            (
                "version: 1\nstatus: wip\ngoal: x\nnodes:\n  a:\n    objective: a\n    depends_on: []\n    resolution: null\n  b:\n    objective: b\n    depends_on: [a, a]\n    resolution: null\n",
                "repeats dependency",
            ),
        ] {
            assert!(graph_error(source).contains(expected));
        }
    }

    #[test]
    fn rejects_a_resolution_whose_direct_dependency_is_unresolved() {
        let error = graph_error(
            "version: 1\nstatus: wip\ngoal: x\nnodes:\n  inspect:\n    objective: inspect\n    resolution: null\n  implement:\n    objective: implement\n    depends_on: [inspect]\n    resolution:\n      summary: implemented\n",
        );

        assert!(
            error.contains("`implement` is resolved") && error.contains("`inspect` is unresolved"),
            "{error}"
        );
    }

    #[test]
    fn validates_terminal_graph_contracts() {
        assert!(
            graph_error(
                "version: 1\nstatus: completed\ngoal: x\nsummary: done\nnodes:\n  a:\n    objective: a\n    resolution: null\n"
            )
            .contains("unresolved")
        );
        assert!(
            graph_error("version: 1\nstatus: completed\ngoal: x\nnodes: {}\n")
                .contains("summary is required")
        );
        assert!(
            graph_error("version: 1\nstatus: aborted\ngoal: x\nnodes: {}\n")
                .contains("reason is required")
        );

        parse("version: 1\nstatus: completed\ngoal: x\nsummary: done\nnodes: {}\n")
            .validate()
            .unwrap();
        let aborted = parse(
            "version: 1\nstatus: aborted\ngoal: x\nabort_reason: superseded\nnodes:\n  unused:\n    objective: no longer needed\n    resolution: null\n",
        )
        .validate()
        .unwrap();
        assert!(aborted.ready.is_empty());
    }

    #[test]
    fn rejects_unsafe_evidence_paths_and_unknown_fields() {
        assert!(
            graph_error(
                "version: 1\nstatus: wip\ngoal: x\nnodes:\n  a:\n    objective: a\n    resolution:\n      summary: done\n      evidence: [../secret]\n"
            )
            .contains("project-relative")
        );
        assert!(
            GraphDocument::parse("version: 1\nstatus: wip\ngoal: x\nnodes: {}\nextra: x\n")
                .is_err()
        );
    }

    #[test]
    fn accepts_only_canonical_short_graph_file_names() {
        assert_eq!(
            graph_id_from_path(Path::new("g12.yaml")).unwrap(),
            ("g12".to_owned(), 12)
        );
        for path in ["g0.yaml", "g01.yaml", "graph1.yaml", "g1.yml"] {
            assert!(graph_id_from_path(Path::new(path)).is_err(), "{path}");
        }
    }

    fn graph_error(source: &str) -> String {
        let graph = GraphDocument::parse(source).unwrap();
        graph.validate().unwrap_err().to_string()
    }
}
