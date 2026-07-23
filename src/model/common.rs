use std::{error::Error, fmt, time::Duration};

use anyhow::{Context, Result};
use eventsource_stream::Event;
use futures_util::{Stream, StreamExt};
use reqwest::{RequestBuilder, Response, StatusCode};

use crate::{
    events::{RuntimeEvent, RuntimeEventKind, SharedEventSink},
    model::{MessageContent, ModelUsage, ToolArguments, ToolCall},
};

pub(crate) const ERROR_BODY_LIMIT: usize = 16 * 1024;

#[derive(Debug)]
struct IncompleteModelResponse {
    provider: String,
    reason: String,
    usage: ModelUsage,
}

impl fmt::Display for IncompleteModelResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} response ended before completion: {}",
            self.provider, self.reason
        )
    }
}

impl Error for IncompleteModelResponse {}

pub(crate) fn incomplete_response_with_usage(
    provider: impl Into<String>,
    reason: impl Into<String>,
    usage: ModelUsage,
) -> anyhow::Error {
    IncompleteModelResponse {
        provider: provider.into(),
        reason: reason.into(),
        usage,
    }
    .into()
}

pub(crate) fn is_incomplete_response(error: &anyhow::Error) -> bool {
    error.downcast_ref::<IncompleteModelResponse>().is_some()
}

pub(crate) fn incomplete_response_usage(error: &anyhow::Error) -> Option<&ModelUsage> {
    error
        .downcast_ref::<IncompleteModelResponse>()
        .map(|incomplete| &incomplete.usage)
}

#[derive(Debug)]
struct HttpStatusError {
    provider: String,
    status: StatusCode,
    body: String,
}

impl fmt::Display for HttpStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} request failed with HTTP {}: {}",
            self.provider, self.status, self.body
        )
    }
}

impl Error for HttpStatusError {}

pub(crate) fn http_status(error: &anyhow::Error) -> Option<StatusCode> {
    error
        .downcast_ref::<HttpStatusError>()
        .map(|error| error.status)
}

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
    Err(HttpStatusError {
        provider: provider.to_owned(),
        status,
        body,
    }
    .into())
}

pub(crate) async fn send_streaming_request(
    builder: RequestBuilder,
    provider: &str,
    idle_timeout: Duration,
) -> Result<Response> {
    tokio::time::timeout(idle_timeout, builder.send())
        .await
        .with_context(|| {
            format!(
                "{provider} response headers exceeded the stream idle timeout ({idle_timeout:?})"
            )
        })?
        .with_context(|| format!("failed to send {provider} request"))
}

pub(crate) async fn next_sse_event<S, E>(
    stream: &mut S,
    provider: &str,
    idle_timeout: Duration,
) -> Result<Option<Event>>
where
    S: Stream<Item = std::result::Result<Event, E>> + Unpin,
    E: Error + Send + Sync + 'static,
{
    tokio::time::timeout(idle_timeout, stream.next())
        .await
        .with_context(|| format!("{provider} stream idle timeout exceeded ({idle_timeout:?})"))?
        .transpose()
        .with_context(|| format!("invalid {provider} SSE stream"))
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
    pub fn finish(self) -> ToolCall {
        let Self {
            id,
            name,
            arguments,
        } = self;
        ToolCall {
            id: if id.trim().is_empty() {
                format!("call_{}", ulid::Ulid::new())
            } else {
                id
            },
            name,
            arguments: ToolArguments::from_raw(arguments),
        }
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
    if let Some(rendered) = super::render_runtime_handle_content(content) {
        return rendered;
    }
    let mut rendered = String::new();
    let mut previous_was_reminder = false;
    for block in content {
        let (text, is_reminder) = match block {
            MessageContent::RuntimeReminder { text } => (Some(text.clone()), true),
            MessageContent::Text { text } => (Some(text.clone()), false),
            MessageContent::RuntimeHandle { .. } => (None, false),
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
        .finish();

        assert_eq!(call.id, "provider-call");
    }

    #[test]
    fn tool_call_builder_supplies_an_id_when_a_compatible_endpoint_omits_it() {
        let call = ToolCallBuilder {
            id: String::new(),
            name: "read".into(),
            arguments: "{}".into(),
        }
        .finish();

        assert!(call.id.starts_with("call_"));
        assert!(call.id.len() > "call_".len());
    }
}
