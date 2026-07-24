use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::{ArtifactRef, safe_component};

pub(crate) async fn verified_artifact_path_for_run(
    workspace: &Path,
    current_run_id: &str,
    artifact: &ArtifactRef,
) -> Result<PathBuf> {
    let path = Path::new(&artifact.path);
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        workspace.join(path)
    };
    let directory = workspace
        .join(".fiasco/runs")
        .join(safe_component(current_run_id))
        .join("artifacts");
    let canonical_directory = tokio::fs::canonicalize(&directory)
        .await
        .with_context(|| format!("resolve {}", directory.display()))?;
    let canonical_path = tokio::fs::canonicalize(&path)
        .await
        .with_context(|| format!("resolve artifact {}", path.display()))?;
    anyhow::ensure!(
        canonical_path.starts_with(&canonical_directory),
        "artifact path escapes current run directory: {}",
        artifact.path
    );
    let metadata = tokio::fs::metadata(&canonical_path)
        .await
        .with_context(|| format!("inspect artifact {}", canonical_path.display()))?;
    anyhow::ensure!(
        metadata.is_file(),
        "artifact is not a regular file: {}",
        canonical_path.display()
    );
    Ok(canonical_path)
}
