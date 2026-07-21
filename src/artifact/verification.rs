use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::{ArtifactRef, hash_file, safe_component};

pub(crate) async fn verified_artifact_path_for_run(
    workspace: &Path,
    current_run_id: &str,
    artifact: &ArtifactRef,
) -> Result<PathBuf> {
    validate_artifact_ref(current_run_id, artifact)?;
    let path = Path::new(&artifact.path);
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        workspace.join(path)
    };
    let directory = workspace
        .join(".pico/runs")
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
    verify_artifact_file(&canonical_path, artifact).await?;
    Ok(canonical_path)
}

fn validate_artifact_ref(current_run_id: &str, artifact: &ArtifactRef) -> Result<()> {
    anyhow::ensure!(
        artifact.version == 1,
        "unsupported artifact ref version {}",
        artifact.version
    );
    anyhow::ensure!(
        artifact.run_id == current_run_id,
        "artifact belongs to run `{}`, not current run `{current_run_id}`",
        artifact.run_id
    );
    anyhow::ensure!(
        artifact.artifact_id == format!("sha256:{}", artifact.sha256),
        "artifact identity disagrees with its sha256 for {}",
        artifact.path
    );
    Ok(())
}

async fn verify_artifact_file(path: &Path, artifact: &ArtifactRef) -> Result<()> {
    let metadata = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("inspect artifact {}", path.display()))?;
    anyhow::ensure!(
        metadata.is_file(),
        "artifact is not a regular file: {}",
        path.display()
    );
    anyhow::ensure!(
        metadata.len() == artifact.bytes,
        "artifact byte length changed for {}: sidecar={}, file={}",
        artifact.path,
        artifact.bytes,
        metadata.len()
    );
    let actual_sha256 = hash_file(path).await?;
    anyhow::ensure!(
        actual_sha256 == artifact.sha256,
        "artifact content hash changed for {}: sidecar={}, file={}",
        artifact.path,
        artifact.sha256,
        actual_sha256
    );
    Ok(())
}
