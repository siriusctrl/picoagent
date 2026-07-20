use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use regex::Regex;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::artifact::ArtifactRef;

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
            verified_artifacts: HashSet::new(),
        })
    }
}

pub(super) struct LocalArtifactSearch {
    workspace: PathBuf,
    run_id: String,
    verified_artifacts: HashSet<(String, String)>,
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
        if artifact.run_id != self.run_id {
            bail!(
                "artifact ref belongs to run `{}` instead of `{}`",
                artifact.run_id,
                self.run_id
            );
        }
        if artifact.version != 1 {
            bail!("unsupported artifact ref version {}", artifact.version);
        }
        let artifact_directory = self
            .workspace
            .join(".pico")
            .join("runs")
            .join(safe_component(&self.run_id))
            .join("artifacts");
        let canonical_directory = tokio::fs::canonicalize(&artifact_directory)
            .await
            .with_context(|| format!("resolve {}", artifact_directory.display()))?;
        let artifact_path = resolve_artifact_path(&self.workspace, &artifact.path);
        let canonical_path = tokio::fs::canonicalize(&artifact_path)
            .await
            .with_context(|| format!("resolve artifact {}", artifact_path.display()))?;
        if !canonical_path.starts_with(&canonical_directory) {
            bail!(
                "artifact path escapes current run directory: {}",
                artifact.path
            );
        }
        let metadata = tokio::fs::metadata(&canonical_path)
            .await
            .with_context(|| format!("inspect artifact {}", canonical_path.display()))?;
        if !metadata.is_file() {
            bail!(
                "artifact is not a regular file: {}",
                canonical_path.display()
            );
        }
        if metadata.len() != artifact.bytes {
            bail!(
                "artifact byte length changed for {}: sidecar={}, file={}",
                artifact.path,
                artifact.bytes,
                metadata.len()
            );
        }
        let expected_artifact_id = format!("sha256:{}", artifact.sha256);
        if artifact.artifact_id != expected_artifact_id {
            bail!(
                "artifact identity disagrees with its sha256 for {}",
                artifact.path
            );
        }

        let verification_key = (artifact.artifact_id.clone(), artifact.path.clone());
        if !self.verified_artifacts.contains(&verification_key) {
            let actual_sha256 = hash_file(&canonical_path).await?;
            if actual_sha256 != artifact.sha256 {
                bail!(
                    "artifact content hash changed for {}: sidecar={}, file={}",
                    artifact.path,
                    artifact.sha256,
                    actual_sha256
                );
            }
            self.verified_artifacts.insert(verification_key);
        }

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

async fn hash_file(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("open artifact for integrity check {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .with_context(|| format!("hash artifact {}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn resolve_artifact_path(workspace: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_owned()
    } else {
        workspace.join(path)
    }
}

fn safe_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_owned()
    } else {
        sanitized
    }
}

fn is_textual(media_type: &str) -> bool {
    media_type.starts_with("text/")
        || media_type.contains("json")
        || media_type.contains("xml")
        || media_type.contains("yaml")
}
