use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};

use crate::{
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

use super::paths::resolve_path;

#[derive(Debug, Default)]
pub struct ReadTool;

#[derive(Debug, Deserialize)]
struct ReadArgs {
    path: String,
    #[serde(default)]
    line_offset: usize,
    #[serde(default)]
    byte_offset: u64,
}

const MAX_READ_LINES: usize = 200;
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
        let mut file = tokio::fs::File::open(&path)
            .await
            .with_context(|| format!("read UTF-8 file {}", path.display()))?;
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
        if selected.last() == Some(&b'\n') {
            selected.pop();
        }
        let (mut text, valid_bytes) = decode_bounded_utf8(selected)
            .with_context(|| format!("read UTF-8 file {}", path.display()))?;
        if byte_truncated {
            let next = selected_start.unwrap_or(args.byte_offset) + valid_bytes as u64;
            text.push_str(&format!(
                "\n[read truncated: internal byte limit reached; continue with byte_offset={next}]"
            ));
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
                        "\n[read truncated: line limit reached; continue with line_offset={}]",
                        args.line_offset + selected_lines
                    ));
                } else {
                    text.push_str(&format!(
                        "\n[read truncated: line limit reached; continue with byte_offset={line_limit_next_byte}]"
                    ));
                }
            }
        }
        Ok(RawToolOutput::text(text))
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
        let spec = ReadTool.spec();
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
    }
}
