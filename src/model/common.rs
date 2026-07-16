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

pub(crate) async fn emit_reasoning(
    events: &SharedEventSink,
    run_id: &str,
    text: &str,
) -> Result<()> {
    events
        .emit(&RuntimeEvent::new(
            run_id,
            RuntimeEventKind::ModelReasoningDelta {
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
        let Self {
            id,
            name,
            arguments,
        } = self;
        let arguments = if arguments.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&arguments).with_context(|| {
                format!("model returned invalid JSON arguments for tool `{name}`")
            })?
        };
        Ok(ToolCall {
            id: if id.trim().is_empty() {
                format!("call_{}", ulid::Ulid::new())
            } else {
                id
            },
            name,
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
    target.reasoning_tokens = value
        .pointer("/output_tokens_details/reasoning_tokens")
        .or_else(|| value.pointer("/completion_tokens_details/reasoning_tokens"))
        .and_then(serde_json::Value::as_u64)
        .or(target.reasoning_tokens);
}

pub(crate) fn content_text(content: &[MessageContent]) -> String {
    let mut rendered = String::new();
    let mut previous_was_reminder = false;
    for block in content {
        let (text, is_reminder) = match block {
            MessageContent::RuntimeReminder { text } => (Some(text.clone()), true),
            MessageContent::Text { text } => (Some(text.clone()), false),
            MessageContent::BackgroundTaskResult {
                task_id,
                name,
                status,
                content,
            } => (
                Some(format!(
                    "<background_task_result task_id=\"{task_id}\" name=\"{name}\" status=\"{status}\">\n{content}\n</background_task_result>"
                )),
                false,
            ),
            _ => (None, false),
        };
        let Some(text) = text else {
            continue;
        };
        if !rendered.is_empty() {
            rendered.push_str(if previous_was_reminder || is_reminder {
                "\n\n"
            } else {
                "\n"
            });
        }
        rendered.push_str(&text);
        previous_was_reminder = is_reminder;
    }
    rendered
}

pub(crate) fn join_url(base_url: &str, endpoint: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        endpoint.trim_start_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_builder_preserves_provider_ids() {
        let call = ToolCallBuilder {
            id: "provider-call".into(),
            name: "read".into(),
            arguments: "{}".into(),
        }
        .finish()
        .unwrap();

        assert_eq!(call.id, "provider-call");
    }

    #[test]
    fn tool_call_builder_supplies_an_id_when_a_compatible_endpoint_omits_it() {
        let call = ToolCallBuilder {
            id: String::new(),
            name: "read".into(),
            arguments: "{}".into(),
        }
        .finish()
        .unwrap();

        assert!(call.id.starts_with("call_"));
        assert!(call.id.len() > "call_".len());
    }
}
