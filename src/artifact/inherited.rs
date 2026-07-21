use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::model::{Message, MessageContent};

use super::{ArtifactRef, extension_for_media_type, hash_file, safe_component, write_sidecar};

pub(crate) fn message_artifact_refs(message: &Message) -> Vec<&ArtifactRef> {
    message
        .content
        .iter()
        .filter_map(|content| match content {
            MessageContent::ToolResult { metadata, .. }
            | MessageContent::BackgroundTask { metadata, .. } => metadata.artifact.as_ref(),
            _ => None,
        })
        .collect()
}

/// Resolve an artifact as owned by `current_run_id`. Forked messages retain
/// their original refs so their provider-facing prefix stays byte-identical;
/// inherited refs therefore resolve to a deterministic copy inside the
/// current run instead of following the original run path.
pub(crate) async fn verified_artifact_path_for_run(
    workspace: &Path,
    current_run_id: &str,
    artifact: &ArtifactRef,
) -> Result<PathBuf> {
    validate_artifact_ref(artifact)?;
    let path = artifact_path_for_run(workspace, current_run_id, artifact)?;
    let directory = artifact_directory_for_run(workspace, current_run_id);
    let canonical_directory = tokio::fs::canonicalize(&directory)
        .await
        .with_context(|| format!("resolve {}", directory.display()))?;
    let canonical_path = tokio::fs::canonicalize(&path)
        .await
        .with_context(|| format!("resolve artifact {}", path.display()))?;
    if !canonical_path.starts_with(&canonical_directory) {
        anyhow::bail!(
            "artifact path escapes current run directory: {}",
            artifact.path
        );
    }
    verify_artifact_file(&canonical_path, artifact).await?;
    Ok(canonical_path)
}

/// Copy every artifact referenced by one inherited message into the child run.
/// The message itself is not changed: its native Chat line, local metadata, and
/// model-visible artifact path remain the exact parent prefix.
pub(crate) async fn snapshot_message_artifacts(
    workspace: &Path,
    source_run_id: &str,
    child_run_id: &str,
    message: &Message,
) -> Result<()> {
    anyhow::ensure!(
        source_run_id != child_run_id,
        "forked artifacts cannot inherit into their source run"
    );
    for artifact in message_artifact_refs(message) {
        let source = verified_artifact_path_for_run(workspace, source_run_id, artifact).await?;
        let destination = inherited_artifact_path(workspace, child_run_id, artifact)?;
        let directory = artifact_directory_for_run(workspace, child_run_id);
        tokio::fs::create_dir_all(&directory)
            .await
            .with_context(|| format!("create artifact directory {}", directory.display()))?;

        if tokio::fs::try_exists(&destination).await? {
            verify_artifact_file(&destination, artifact).await?;
        } else {
            copy_artifact(&source, &destination, &directory, artifact).await?;
        }

        let relative_path = destination
            .strip_prefix(workspace)
            .unwrap_or(&destination)
            .to_string_lossy()
            .into_owned();
        let local_ref = ArtifactRef {
            run_id: child_run_id.to_owned(),
            path: relative_path,
            ..artifact.clone()
        };
        let stable_name = destination
            .file_stem()
            .and_then(|name| name.to_str())
            .context("inherited artifact path has no UTF-8 stem")?;
        write_sidecar(&directory, stable_name, &local_ref).await?;
    }
    Ok(())
}

async fn copy_artifact(
    source: &Path,
    destination: &Path,
    directory: &Path,
    artifact: &ArtifactRef,
) -> Result<()> {
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .context("inherited artifact path has no UTF-8 file name")?;
    let temporary = directory.join(format!(".{file_name}.fork.tmp"));
    tokio::fs::copy(source, &temporary).await.with_context(|| {
        format!(
            "copy inherited artifact {} to {}",
            source.display(),
            temporary.display()
        )
    })?;
    if let Err(error) = verify_artifact_file(&temporary, artifact).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    tokio::fs::rename(&temporary, destination)
        .await
        .with_context(|| {
            format!(
                "commit inherited artifact {} to {}",
                temporary.display(),
                destination.display()
            )
        })
}

fn artifact_path_for_run(
    workspace: &Path,
    current_run_id: &str,
    artifact: &ArtifactRef,
) -> Result<PathBuf> {
    if artifact.run_id == current_run_id {
        let path = Path::new(&artifact.path);
        return Ok(if path.is_absolute() {
            path.to_owned()
        } else {
            workspace.join(path)
        });
    }
    inherited_artifact_path(workspace, current_run_id, artifact)
}

fn inherited_artifact_path(
    workspace: &Path,
    current_run_id: &str,
    artifact: &ArtifactRef,
) -> Result<PathBuf> {
    let identity = serde_json::to_vec(artifact).context("serialize inherited artifact identity")?;
    let identity_sha256 = format!("{:x}", Sha256::digest(identity));
    Ok(
        artifact_directory_for_run(workspace, current_run_id).join(format!(
            "inherited-{identity_sha256}.{}",
            extension_for_media_type(&artifact.media_type)
        )),
    )
}

fn artifact_directory_for_run(workspace: &Path, run_id: &str) -> PathBuf {
    workspace
        .join(".pico/runs")
        .join(safe_component(run_id))
        .join("artifacts")
}

fn validate_artifact_ref(artifact: &ArtifactRef) -> Result<()> {
    anyhow::ensure!(
        artifact.version == 1,
        "unsupported artifact ref version {}",
        artifact.version
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

#[cfg(test)]
mod tests {
    use crate::{
        artifact::{ArtifactStore, ResultMetadata},
        model::{Message, MessageContent, Role},
        tools::{RawToolOutput, ToolContext},
    };

    use super::*;

    #[tokio::test]
    async fn nested_fork_copies_from_the_immediate_local_snapshot() {
        let workspace = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::default();
        let original = artifacts
            .persist_artifact(
                &ToolContext {
                    run_id: "root".to_owned(),
                    call_id: "large-result".to_owned(),
                    workspace: workspace.path().to_owned(),
                },
                RawToolOutput::text("nested fork artifact"),
            )
            .await
            .unwrap()
            .artifact
            .unwrap();
        let message = Message {
            role: Role::Tool,
            content: vec![MessageContent::ToolResult {
                call_id: original.call_id.clone(),
                content: original.path.clone(),
                is_error: false,
                metadata: ResultMetadata {
                    artifact: Some(original.clone()),
                },
            }],
        };

        snapshot_message_artifacts(workspace.path(), "root", "child-a", &message)
            .await
            .unwrap();
        tokio::fs::remove_dir_all(workspace.path().join(".pico/runs/root"))
            .await
            .unwrap();
        snapshot_message_artifacts(workspace.path(), "child-a", "child-b", &message)
            .await
            .unwrap();

        let child_b = verified_artifact_path_for_run(workspace.path(), "child-b", &original)
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(child_b).await.unwrap(),
            "nested fork artifact"
        );
        assert_eq!(message_artifact_refs(&message), vec![&original]);
    }
}
