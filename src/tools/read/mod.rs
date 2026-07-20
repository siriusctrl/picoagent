use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::AsyncReadExt;

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
    offset: usize,
    #[serde(default = "default_read_limit")]
    limit: usize,
    #[serde(default = "default_read_bytes")]
    max_bytes: usize,
}

fn default_read_limit() -> usize {
    200
}

fn default_read_bytes() -> usize {
    64 * 1024
}

#[async_trait]
impl Tool for ReadTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: ReadArgs = serde_json::from_value(arguments).context("invalid read arguments")?;
        if args.limit == 0 || args.max_bytes == 0 {
            bail!("read limit and max_bytes must be greater than zero");
        }
        let path = resolve_path(&context.workspace, &args.path);
        let mut file = tokio::fs::File::open(&path)
            .await
            .with_context(|| format!("read UTF-8 file {}", path.display()))?;
        let mut selected = Vec::with_capacity(args.max_bytes.min(64 * 1024));
        let mut buffer = [0_u8; 8 * 1024];
        let mut line_index = 0_usize;
        let mut selected_lines = 0_usize;
        let mut byte_truncated = false;
        'read: loop {
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            for byte in &buffer[..read] {
                if line_index >= args.offset && selected_lines < args.limit {
                    if selected.len() == args.max_bytes {
                        byte_truncated = true;
                        break 'read;
                    }
                    selected.push(*byte);
                }
                if *byte == b'\n' {
                    if line_index >= args.offset {
                        selected_lines += 1;
                        if selected_lines == args.limit {
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
        let mut text = decode_bounded_utf8(selected)
            .with_context(|| format!("read UTF-8 file {}", path.display()))?;
        if byte_truncated {
            text.push_str(
                "\n[read max_bytes reached; reduce limit or continue from a later line offset]",
            );
        }
        Ok(RawToolOutput::text(text))
    }
}

fn decode_bounded_utf8(bytes: Vec<u8>) -> Result<String> {
    match String::from_utf8(bytes) {
        Ok(text) => Ok(text),
        Err(error) if error.utf8_error().error_len().is_none() => {
            let valid = error.utf8_error().valid_up_to();
            Ok(String::from_utf8(error.into_bytes()[..valid].to_vec())?)
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
            spec.input_schema.pointer("/properties/offset/default"),
            Some(&json!(usize::default()))
        );
        assert_eq!(
            spec.input_schema.pointer("/properties/limit/default"),
            Some(&json!(default_read_limit()))
        );
        assert_eq!(
            spec.input_schema.pointer("/properties/max_bytes/default"),
            Some(&json!(default_read_bytes()))
        );
    }
}
