use std::path::PathBuf;

use anyhow::Result;
use regex::Regex;

use crate::artifact::{ArtifactRef, verified_artifact_path_for_run};

mod rg;

/// Searches structured artifact references from the current run's completed
/// result metadata.
pub(super) struct LocalRunArtifactSource {
    workspace: PathBuf,
}

impl LocalRunArtifactSource {
    pub(super) fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
        }
    }

    pub(super) async fn begin_search(&self, run_id: &str) -> Result<LocalArtifactSearch> {
        Ok(LocalArtifactSearch {
            workspace: self.workspace.clone(),
            run_id: run_id.to_owned(),
        })
    }
}

pub(super) struct LocalArtifactSearch {
    workspace: PathBuf,
    run_id: String,
}

pub(super) struct ArtifactMatch {
    pub path: String,
    pub snippet: String,
}

impl LocalArtifactSearch {
    async fn search_artifact(
        &mut self,
        artifact: &ArtifactRef,
        pattern: &Regex,
    ) -> Result<Option<String>> {
        if !is_textual(&artifact.media_type) {
            return Ok(None);
        }
        let canonical_path =
            verified_artifact_path_for_run(&self.workspace, &self.run_id, artifact).await?;
        let metadata = tokio::fs::metadata(&canonical_path).await?;
        rg::search_file(&canonical_path, metadata.len(), pattern).await
    }

    pub(super) async fn find(
        &mut self,
        artifacts: &[&ArtifactRef],
        pattern: &Regex,
    ) -> Result<Option<ArtifactMatch>> {
        for artifact in artifacts {
            if let Some(snippet) = self.search_artifact(artifact, pattern).await? {
                return Ok(Some(ArtifactMatch {
                    path: artifact.path.clone(),
                    snippet,
                }));
            }
        }
        Ok(None)
    }
}

fn is_textual(media_type: &str) -> bool {
    media_type.starts_with("text/")
        || media_type.contains("json")
        || media_type.contains("xml")
        || media_type.contains("yaml")
}
