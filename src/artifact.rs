use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use ulid::Ulid;

use crate::{
    model::ImageAttachment,
    tools::{RawToolOutput, ToolContext},
};

mod preview;
mod verification;

pub use preview::{PreviewInfo, PreviewLimitation};
use preview::{file_preview, textual_preview, unavailable_preview};
pub(crate) use verification::verified_artifact_path_for_run;

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

/// A run-local attachment that may be updated after the originating result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    /// Workspace-relative when the workspace contains the artifact.
    pub path: String,
    pub media_type: String,
}

/// Local execution metadata for one completed result. It is persisted in the
/// provider-neutral result block and omitted from provider-facing projections.
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

    pub(crate) fn artifact_bytes(&self) -> Option<u64> {
        self.artifact.as_ref()?;
        let info = self.preview_info.as_ref()?;
        Some(
            info.shown_head_bytes
                .saturating_add(info.shown_tail_bytes)
                .saturating_add(info.omitted_bytes),
        )
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
        let total_bytes = info
            .shown_head_bytes
            .saturating_add(info.shown_tail_bytes)
            .saturating_add(info.omitted_bytes);
        format!(
            "[Tool output]\nis_error: {}\ntruncated: {}\nbytes: total={}; preview_head={}; preview_tail={}; omitted={}\npreview_limitation: {}\nartifact: {}\nmedia_type: {}\ninstruction: {}{}",
            self.is_error,
            self.truncated,
            total_bytes,
            info.shown_head_bytes,
            info.shown_tail_bytes,
            info.omitted_bytes,
            limitation,
            artifact.path,
            artifact.media_type,
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
        self.persist_output_inner(context, output).await
    }

    async fn persist_output_inner(
        &self,
        context: &ToolContext,
        mut output: RawToolOutput,
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
            if bytes <= self.policy.inline_limit_bytes as u64 {
                output.content = tokio::fs::read(&source_path).await?;
                tokio::fs::remove_file(source_path).await?;
            } else {
                return self.persist_file(context, output, source_path, bytes).await;
            }
        }
        let textual =
            is_textual(&output.media_type) && std::str::from_utf8(&output.content).is_ok();
        if output.content.len() <= self.policy.inline_limit_bytes && textual {
            return Ok(ToolOutput {
                preview: String::from_utf8_lossy(&output.content).into_owned(),
                artifact: None,
                truncated: false,
                is_error: output.is_error,
                preview_info: None,
                attachment,
            });
        }

        let directory = artifact_directory(context);
        tokio::fs::create_dir_all(&directory)
            .await
            .with_context(|| format!("create artifact directory {}", directory.display()))?;

        let absolute_path = directory.join(artifact_file_name(context, &output.media_type));
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&absolute_path)
            .await
            .with_context(|| format!("create artifact {}", absolute_path.display()))?;
        file.write_all(&output.content)
            .await
            .with_context(|| format!("write artifact {}", absolute_path.display()))?;
        file.flush().await?;

        let artifact = artifact_ref(context, &output.media_type, &absolute_path);

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
        let directory = artifact_directory(context);
        tokio::fs::create_dir_all(&directory).await?;
        let absolute_path = directory.join(artifact_file_name(context, &output.media_type));
        match tokio::fs::hard_link(&source_path, &absolute_path).await {
            Ok(()) => {
                tokio::fs::remove_file(&source_path).await?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
                let mut destination = tokio::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&absolute_path)
                    .await?;
                let mut source = tokio::fs::File::open(&source_path).await?;
                tokio::io::copy(&mut source, &mut destination).await?;
                destination.flush().await?;
                tokio::fs::remove_file(&source_path).await?;
            }
            Err(error) => return Err(error.into()),
        }
        let artifact = artifact_ref(context, &output.media_type, &absolute_path);
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
        .join(".fiasco/runs")
        .join(safe_component(&context.run_id))
        .join("artifacts")
}

fn artifact_file_name(context: &ToolContext, media_type: &str) -> String {
    format!(
        "{}-{}.{}",
        safe_component(&context.call_id),
        Ulid::new(),
        extension_for_media_type(media_type)
    )
}

fn artifact_ref(
    context: &ToolContext,
    media_type: &str,
    absolute_path: &std::path::Path,
) -> ArtifactRef {
    ArtifactRef {
        path: absolute_path
            .strip_prefix(&context.workspace)
            .unwrap_or(absolute_path)
            .to_string_lossy()
            .into_owned(),
        media_type: media_type.to_owned(),
    }
}
