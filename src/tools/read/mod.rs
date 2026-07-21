use std::{io::Cursor, path::Path};

use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use image::ImageFormat;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};

use crate::{
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

use super::paths::resolve_path;

#[derive(Debug)]
pub struct ReadTool {
    image_enabled: bool,
}

impl ReadTool {
    pub fn new(image_enabled: bool) -> Self {
        Self { image_enabled }
    }
}

impl Default for ReadTool {
    fn default() -> Self {
        Self::new(false)
    }
}

#[derive(Debug, Deserialize)]
struct ReadArgs {
    path: String,
    #[serde(default)]
    line_offset: usize,
    #[serde(default)]
    byte_offset: u64,
}

const MAX_READ_LINES: usize = 400;
const MAX_READ_BYTES: usize = 64 * 1024;

#[async_trait]
impl Tool for ReadTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: ReadArgs = serde_json::from_value(arguments).context("invalid read arguments")?;
        if args.line_offset != 0 && args.byte_offset != 0 {
            bail!("read line_offset and byte_offset are mutually exclusive");
        }
        let path = resolve_path(&context.workspace, &args.path);
        if let Some(image) = image_kind(&path) {
            ensure!(
                self.image_enabled,
                "configured model cannot inspect images; add `image` to `provider.modalities` only when the selected model supports it"
            );
            ensure!(
                args.line_offset == 0 && args.byte_offset == 0,
                "read image attachments do not accept line_offset or byte_offset"
            );
            return read_image(&path, image).await;
        }
        let mut file = tokio::fs::File::open(&path)
            .await
            .with_context(|| format!("read UTF-8 file {}", path.display()))?;
        let total_bytes = file
            .metadata()
            .await
            .with_context(|| format!("inspect UTF-8 file {}", path.display()))?
            .len();
        if args.byte_offset != 0 {
            file.seek(SeekFrom::Start(args.byte_offset)).await?;
        }
        let mut selected = Vec::with_capacity(MAX_READ_BYTES);
        let mut buffer = [0_u8; 8 * 1024];
        let mut line_index = 0_usize;
        let mut selected_lines = 0_usize;
        let mut absolute_position = args.byte_offset;
        let mut selected_start = None;
        let mut byte_truncated = false;
        let mut line_limit_reached = false;
        let mut line_limit_next_byte = 0_u64;
        let mut buffered_remainder = false;
        'read: loop {
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            for (position, byte) in buffer[..read].iter().enumerate() {
                if line_index >= args.line_offset && selected_lines < MAX_READ_LINES {
                    if selected.len() == MAX_READ_BYTES {
                        byte_truncated = true;
                        break 'read;
                    }
                    selected_start.get_or_insert(absolute_position);
                    selected.push(*byte);
                }
                absolute_position += 1;
                if *byte == b'\n' {
                    if line_index >= args.line_offset {
                        selected_lines += 1;
                        if selected_lines == MAX_READ_LINES {
                            line_limit_reached = true;
                            line_limit_next_byte = absolute_position;
                            buffered_remainder = position + 1 < read;
                            break 'read;
                        }
                    }
                    line_index += 1;
                }
            }
        }
        let rounded_byte_continuation = if byte_truncated {
            selected.iter().rposition(|byte| *byte == b'\n').map(|end| {
                selected.truncate(end + 1);
                selected_start.unwrap_or(args.byte_offset) + end as u64 + 1
            })
        } else {
            None
        };
        if selected.last() == Some(&b'\n') {
            selected.pop();
        }
        let (mut text, valid_bytes) = decode_bounded_utf8(selected)
            .with_context(|| format!("read UTF-8 file {}", path.display()))?;
        if byte_truncated {
            if let Some(next_byte) = rounded_byte_continuation {
                if args.byte_offset == 0 {
                    text.push_str(&format!(
                        "\n[read truncated: internal byte limit reached after a complete line; total_bytes={total_bytes}; continue with line_offset={}]",
                        args.line_offset + selected_lines
                    ));
                } else {
                    text.push_str(&format!(
                        "\n[read truncated: internal byte limit reached after a complete line; total_bytes={total_bytes}; continue with byte_offset={next_byte}]"
                    ));
                }
            } else {
                let next = selected_start.unwrap_or(args.byte_offset) + valid_bytes as u64;
                text.push_str(&format!(
                    "\n[read truncated: internal byte limit reached; total_bytes={total_bytes}; continue with byte_offset={next}]"
                ));
            }
        } else if line_limit_reached {
            let has_unread_tail = if buffered_remainder {
                true
            } else {
                let mut peek = [0_u8; 1];
                file.read(&mut peek).await? != 0
            };
            if has_unread_tail {
                if args.byte_offset == 0 {
                    text.push_str(&format!(
                        "\n[read truncated: line limit reached; total_bytes={total_bytes}; continue with line_offset={}]",
                        args.line_offset + selected_lines
                    ));
                } else {
                    text.push_str(&format!(
                        "\n[read truncated: line limit reached; total_bytes={total_bytes}; continue with byte_offset={line_limit_next_byte}]"
                    ));
                }
            }
        }
        Ok(RawToolOutput::text(text))
    }
}

#[derive(Debug, Clone, Copy)]
enum ImageKind {
    Passthrough(&'static str),
    NormalizeToPng(ImageFormat),
}

fn image_kind(path: &Path) -> Option<ImageKind> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "jpg" | "jpeg" => Some(ImageKind::Passthrough("image/jpeg")),
        "png" => Some(ImageKind::Passthrough("image/png")),
        "webp" => Some(ImageKind::Passthrough("image/webp")),
        // OpenAI accepts only non-animated GIF and does not accept BMP. A PNG
        // first frame gives every provider the same predictable attachment.
        "gif" => Some(ImageKind::NormalizeToPng(ImageFormat::Gif)),
        "bmp" => Some(ImageKind::NormalizeToPng(ImageFormat::Bmp)),
        _ => None,
    }
}

async fn read_image(path: &Path, kind: ImageKind) -> Result<RawToolOutput> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("read image {}", path.display()))?;
    match kind {
        ImageKind::Passthrough(media_type) => Ok(RawToolOutput::image(bytes, media_type)),
        ImageKind::NormalizeToPng(format) => {
            let normalized = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
                let image = image::load_from_memory_with_format(&bytes, format)
                    .context("decode image attachment")?;
                let mut output = Cursor::new(Vec::new());
                image
                    .write_to(&mut output, ImageFormat::Png)
                    .context("encode image attachment as PNG")?;
                Ok(output.into_inner())
            })
            .await
            .context("join image normalization task")??;
            Ok(RawToolOutput::image(normalized, "image/png"))
        }
    }
}

fn decode_bounded_utf8(bytes: Vec<u8>) -> Result<(String, usize)> {
    match String::from_utf8(bytes) {
        Ok(text) => {
            let valid_bytes = text.len();
            Ok((text, valid_bytes))
        }
        Err(error) if error.utf8_error().error_len().is_none() => {
            let valid = error.utf8_error().valid_up_to();
            Ok((
                String::from_utf8(error.into_bytes()[..valid].to_vec())?,
                valid,
            ))
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn manifest_defaults_match_runtime_defaults() {
        let spec = ReadTool::default().spec();
        assert_eq!(
            spec.input_schema.pointer("/properties/line_offset/default"),
            Some(&json!(usize::default()))
        );
        assert_eq!(
            spec.input_schema.pointer("/properties/byte_offset/default"),
            Some(&json!(u64::default()))
        );
        assert!(spec.input_schema.pointer("/properties/limit").is_none());
        assert!(spec.input_schema.pointer("/properties/max_bytes").is_none());
        assert!(spec.description.contains("400 lines"));
        assert!(spec.description.contains("65,536 bytes"));
        assert!(spec.description.contains("total_bytes"));
    }
}
