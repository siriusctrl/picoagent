use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::{
    model::ImageAttachment,
    tools::{RawToolOutput, ToolContext},
};

mod preview;

pub use preview::{PreviewInfo, PreviewLimitation};
use preview::{file_preview, textual_preview, unavailable_preview};

const MODEL_INSPECTION_INSTRUCTION: &str = include_str!("artifact/model-instruction.md");

/// Controls when a tool result is replaced with a small model-facing preview.
#[derive(Debug, Clone)]
pub struct ArtifactPolicy {
    pub inline_limit_bytes: usize,
    pub preview_head_bytes: usize,
    pub preview_tail_bytes: usize,
}

impl Default for ArtifactPolicy {
    fn default() -> Self {
        Self {
            inline_limit_bytes: 32 * 1024,
            preview_head_bytes: 8 * 1024,
            preview_tail_bytes: 8 * 1024,
        }
    }
}

/// A stable reference to the complete result of one tool call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    pub version: u32,
    /// Immutable content identity. Equal bytes produce the same artifact id.
    pub artifact_id: String,
    pub run_id: String,
    pub call_id: String,
    /// Workspace-relative when the workspace contains the artifact.
    pub path: String,
    pub media_type: String,
    pub bytes: u64,
    pub sha256: String,
}

/// Local execution metadata for one completed result. It is persisted beside
/// Chat-compatible messages, never inside their model-facing content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResultMetadata {
    pub artifact: Option<ArtifactRef>,
}

impl ResultMetadata {
    pub fn empty() -> Self {
        Self { artifact: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolOutput {
    pub preview: String,
    pub artifact: Option<ArtifactRef>,
    pub truncated: bool,
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_info: Option<PreviewInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<ImageAttachment>,
}

impl ToolOutput {
    pub fn result_metadata(&self) -> ResultMetadata {
        ResultMetadata {
            artifact: self.artifact.clone(),
        }
    }

    /// Text sent back to the model. The complete artifact stays out of context.
    pub fn model_content(&self) -> String {
        let Some(artifact) = &self.artifact else {
            return self.preview.clone();
        };
        let info = self
            .preview_info
            .as_ref()
            .expect("artifact-backed tool output must describe its preview");
        let preview = if self.preview.is_empty() {
            String::new()
        } else {
            format!("\n\n[Preview]\n{}", self.preview)
        };
        let limitation = info
            .limitation
            .map(PreviewLimitation::as_str)
            .unwrap_or("none");
        format!(
            "[Tool output]\nis_error: {}\ntruncated: {}\nbytes: total={}; preview_head={}; preview_tail={}; omitted={}\npreview_limitation: {}\nartifact: {}\nmedia_type: {}\nsha256: {}\ninstruction: {}{}",
            self.is_error,
            self.truncated,
            artifact.bytes,
            info.shown_head_bytes,
            info.shown_tail_bytes,
            info.omitted_bytes,
            limitation,
            artifact.path,
            artifact.media_type,
            artifact.sha256,
            MODEL_INSPECTION_INSTRUCTION.trim(),
            preview,
        )
    }
}

#[derive(Debug, Clone)]
pub struct ArtifactStore {
    policy: ArtifactPolicy,
}

impl ArtifactStore {
    pub fn new(policy: ArtifactPolicy) -> Self {
        Self { policy }
    }

    pub async fn persist_output(
        &self,
        context: &ToolContext,
        output: RawToolOutput,
    ) -> Result<ToolOutput> {
        self.persist_output_inner(context, output, false).await
    }

    /// Persist a complete result even when it would normally fit inline.
    /// Background task delivery uses this so its terminal notice can remain a
    /// small, stable artifact reference.
    pub async fn persist_artifact(
        &self,
        context: &ToolContext,
        output: RawToolOutput,
    ) -> Result<ToolOutput> {
        self.persist_output_inner(context, output, true).await
    }

    async fn persist_output_inner(
        &self,
        context: &ToolContext,
        mut output: RawToolOutput,
        force_artifact: bool,
    ) -> Result<ToolOutput> {
        let attachment = output
            .attach_to_model
            .then(|| ImageAttachment::from_bytes(output.media_type.clone(), &output.content));
        if let Some(source_path) = output.source_path.take() {
            anyhow::ensure!(
                attachment.is_none(),
                "model image attachments must provide their bytes directly"
            );
            let bytes = tokio::fs::metadata(&source_path).await?.len();
            if bytes <= self.policy.inline_limit_bytes as u64 && !force_artifact {
                output.content = tokio::fs::read(&source_path).await?;
                tokio::fs::remove_file(source_path).await?;
            } else {
                return self.persist_file(context, output, source_path, bytes).await;
            }
        }
        let textual =
            is_textual(&output.media_type) && std::str::from_utf8(&output.content).is_ok();
        if output.content.len() <= self.policy.inline_limit_bytes && textual && !force_artifact {
            return Ok(ToolOutput {
                preview: String::from_utf8_lossy(&output.content).into_owned(),
                artifact: None,
                truncated: false,
                is_error: output.is_error,
                preview_info: None,
                attachment,
            });
        }

        let extension = extension_for_media_type(&output.media_type);
        let sha256 = format!("{:x}", Sha256::digest(&output.content));
        let content_suffix = &sha256[..12];
        let stable_name = format!("{}-{content_suffix}", safe_component(&context.call_id));
        let file_name = format!("{stable_name}.{extension}");
        let directory = context
            .workspace
            .join(".pico")
            .join("runs")
            .join(safe_component(&context.run_id))
            .join("artifacts");
        tokio::fs::create_dir_all(&directory)
            .await
            .with_context(|| format!("create artifact directory {}", directory.display()))?;

        let absolute_path = directory.join(file_name);
        tokio::fs::write(&absolute_path, &output.content)
            .await
            .with_context(|| format!("write artifact {}", absolute_path.display()))?;

        let relative_path = absolute_path
            .strip_prefix(&context.workspace)
            .unwrap_or(&absolute_path)
            .to_string_lossy()
            .into_owned();
        let artifact = ArtifactRef {
            version: 1,
            artifact_id: format!("sha256:{sha256}"),
            run_id: context.run_id.clone(),
            call_id: context.call_id.clone(),
            path: relative_path,
            media_type: output.media_type,
            bytes: output.content.len() as u64,
            sha256,
        };
        let sidecar_path = directory.join(format!("{stable_name}.artifact.json"));
        let sidecar = serde_json::to_vec_pretty(&artifact).context("serialize artifact sidecar")?;
        tokio::fs::write(&sidecar_path, sidecar)
            .await
            .with_context(|| format!("write artifact sidecar {}", sidecar_path.display()))?;

        let (preview, preview_info) = if textual {
            textual_preview(
                &output.content,
                self.policy.preview_head_bytes,
                self.policy.preview_tail_bytes,
            )
        } else {
            unavailable_preview(output.content.len() as u64)
        };
        let truncated = preview_info.omitted_bytes > 0;
        Ok(ToolOutput {
            preview,
            artifact: Some(artifact),
            truncated,
            is_error: output.is_error,
            preview_info: Some(preview_info),
            attachment,
        })
    }

    async fn persist_file(
        &self,
        context: &ToolContext,
        output: RawToolOutput,
        source_path: PathBuf,
        bytes: u64,
    ) -> Result<ToolOutput> {
        let sha256 = hash_file(&source_path).await?;
        let stable_name = format!("{}-{}", safe_component(&context.call_id), &sha256[..12]);
        let directory = artifact_directory(context);
        tokio::fs::create_dir_all(&directory).await?;
        let absolute_path = directory.join(format!(
            "{stable_name}.{}",
            extension_for_media_type(&output.media_type)
        ));
        if tokio::fs::try_exists(&absolute_path).await? {
            tokio::fs::remove_file(&source_path).await?;
        } else {
            match tokio::fs::rename(&source_path, &absolute_path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
                    tokio::fs::copy(&source_path, &absolute_path).await?;
                    tokio::fs::remove_file(&source_path).await?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        let artifact = artifact_ref(context, &output.media_type, bytes, sha256, &absolute_path);
        write_sidecar(&directory, &stable_name, &artifact).await?;
        let (preview, preview_info) = if is_textual(&output.media_type) {
            file_preview(
                &absolute_path,
                bytes,
                self.policy.preview_head_bytes,
                self.policy.preview_tail_bytes,
            )
            .await?
        } else {
            unavailable_preview(bytes)
        };
        let truncated = preview_info.omitted_bytes > 0;
        Ok(ToolOutput {
            preview,
            artifact: Some(artifact),
            truncated,
            is_error: output.is_error,
            preview_info: Some(preview_info),
            attachment: None,
        })
    }
}

impl Default for ArtifactStore {
    fn default() -> Self {
        Self::new(ArtifactPolicy::default())
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

fn extension_for_media_type(media_type: &str) -> &'static str {
    if media_type.contains("json") {
        "json"
    } else if media_type.starts_with("text/") {
        "txt"
    } else if media_type == "image/jpeg" {
        "jpg"
    } else if media_type == "image/png" {
        "png"
    } else if media_type == "image/gif" {
        "gif"
    } else if media_type == "image/webp" {
        "webp"
    } else if media_type == "image/bmp" {
        "bmp"
    } else {
        "bin"
    }
}

fn is_textual(media_type: &str) -> bool {
    media_type.starts_with("text/")
        || media_type.contains("json")
        || media_type.contains("xml")
        || media_type.contains("yaml")
}

fn artifact_directory(context: &ToolContext) -> PathBuf {
    context
        .workspace
        .join(".pico/runs")
        .join(safe_component(&context.run_id))
        .join("artifacts")
}

fn artifact_ref(
    context: &ToolContext,
    media_type: &str,
    bytes: u64,
    sha256: String,
    absolute_path: &Path,
) -> ArtifactRef {
    ArtifactRef {
        version: 1,
        artifact_id: format!("sha256:{sha256}"),
        run_id: context.run_id.clone(),
        call_id: context.call_id.clone(),
        path: absolute_path
            .strip_prefix(&context.workspace)
            .unwrap_or(absolute_path)
            .to_string_lossy()
            .into_owned(),
        media_type: media_type.to_owned(),
        bytes,
        sha256,
    }
}

async fn write_sidecar(directory: &Path, stable_name: &str, artifact: &ArtifactRef) -> Result<()> {
    let sidecar_path = directory.join(format!("{stable_name}.artifact.json"));
    let sidecar = serde_json::to_vec_pretty(artifact).context("serialize artifact sidecar")?;
    tokio::fs::write(&sidecar_path, sidecar)
        .await
        .with_context(|| format!("write artifact sidecar {}", sidecar_path.display()))
}

async fn hash_file(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
