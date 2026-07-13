use anyhow::{Context, Result, bail};
use reqwest::Response;

use crate::{
    events::{RuntimeEvent, RuntimeEventKind, SharedEventSink},
    model::{MessageContent, ModelUsage, ToolCall},
};

pub(crate) const ERROR_BODY_LIMIT: usize = 16 * 1024;

pub(crate) async fn ensure_success(response: Response, provider: &str) -> Result<Response> {
    if response.status().is_success() {
        return Ok(response);
    }

    let status = response.status();
    let mut body = response.text().await.unwrap_or_default();
    if body.len() > ERROR_BODY_LIMIT {
        let mut boundary = ERROR_BODY_LIMIT;
        while !body.is_char_boundary(boundary) {
            boundary -= 1;
        }
        body.truncate(boundary);
        body.push_str("...[truncated]");
    }
    bail!("{provider} request failed with HTTP {status}: {body}")
}

pub(crate) async fn emit_text(events: &SharedEventSink, run_id: &str, text: &str) -> Result<()> {
    events
        .emit(&RuntimeEvent::new(
            run_id,
            RuntimeEventKind::ModelDelta {
                text: text.to_owned(),
            },
        ))
        .await
}

#[derive(Debug, Default)]
pub(crate) struct ToolCallBuilder {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl ToolCallBuilder {
    pub fn finish(self) -> Result<ToolCall> {
        let arguments = if self.arguments.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&self.arguments).with_context(|| {
                format!(
                    "model returned invalid JSON arguments for tool `{}`",
                    self.name
                )
            })?
        };
        Ok(ToolCall {
            id: self.id,
            name: self.name,
            arguments,
        })
    }
}

pub(crate) fn merge_usage(target: &mut ModelUsage, value: &serde_json::Value) {
    target.input_tokens = value
        .get("input_tokens")
        .or_else(|| value.get("prompt_tokens"))
        .and_then(serde_json::Value::as_u64)
        .or(target.input_tokens);
    target.output_tokens = value
        .get("output_tokens")
        .or_else(|| value.get("completion_tokens"))
        .and_then(serde_json::Value::as_u64)
        .or(target.output_tokens);
    target.cached_input_tokens = value
        .pointer("/input_tokens_details/cached_tokens")
        .or_else(|| value.pointer("/prompt_tokens_details/cached_tokens"))
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            value
                .get("cache_read_input_tokens")
                .and_then(serde_json::Value::as_u64)
        })
        .or(target.cached_input_tokens);
}

pub(crate) fn content_text(content: &[MessageContent]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            MessageContent::Text { text } => Some(text.clone()),
            MessageContent::BackgroundTaskResult {
                task_id,
                name,
                status,
                content,
            } => Some(format!(
                "<background_task_result task_id=\"{task_id}\" name=\"{name}\" status=\"{status}\">\n{content}\n</background_task_result>"
            )),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn join_url(base_url: &str, endpoint: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        endpoint.trim_start_matches('/')
    )
}
