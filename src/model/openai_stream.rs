use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::Value;

use super::{
    MessageContent, ModelResponse, ModelUsage,
    common::{ToolCallBuilder, emit_text, ensure_success, merge_usage},
    openai_compatible::OpenAiProtocol,
};
use crate::events::SharedEventSink;

pub(crate) async fn complete_request(
    builder: reqwest::RequestBuilder,
    protocol: OpenAiProtocol,
    run_id: &str,
    events: SharedEventSink,
) -> Result<ModelResponse> {
    let response = ensure_success(
        builder
            .send()
            .await
            .context("failed to send OpenAI request")?,
        "OpenAI",
    )
    .await?;
    complete_response(response, protocol, run_id, events).await
}

pub(crate) async fn complete_response(
    response: reqwest::Response,
    protocol: OpenAiProtocol,
    run_id: &str,
    events: SharedEventSink,
) -> Result<ModelResponse> {
    let mut stream = response.bytes_stream().eventsource();
    let mut accumulator = OpenAiAccumulator::default();

    while let Some(event) = stream.next().await {
        let event = event.context("invalid OpenAI SSE stream")?;
        if event.data.trim() == "[DONE]" {
            break;
        }
        let value: Value = serde_json::from_str(&event.data)
            .with_context(|| format!("invalid OpenAI SSE JSON for event `{}`", event.event))?;
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(event.event.as_str());
        reject_failure_event(event_type, &value)?;
        let deltas = match protocol {
            OpenAiProtocol::Responses => accumulator.handle_responses(event_type, &value)?,
            OpenAiProtocol::ChatCompletions => accumulator.handle_chat(&value)?,
        };
        for delta in deltas {
            emit_text(&events, run_id, &delta).await?;
        }
    }

    accumulator.finish(protocol)
}

fn reject_failure_event(event_type: &str, value: &Value) -> Result<()> {
    if matches!(
        event_type,
        "error" | "response.error" | "response.failed" | "response.incomplete"
    ) {
        let message = value
            .pointer("/error/message")
            .or_else(|| value.pointer("/response/error/message"))
            .or_else(|| value.pointer("/response/incomplete_details/reason"))
            .or_else(|| value.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("response did not complete");
        bail!("OpenAI stream ended with `{event_type}`: {message}")
    }
    Ok(())
}

#[derive(Default)]
struct OpenAiAccumulator {
    text: String,
    texts: BTreeMap<usize, String>,
    tools: BTreeMap<usize, ToolCallBuilder>,
    item_indexes: BTreeMap<String, usize>,
    usage: ModelUsage,
    provider_items: BTreeMap<usize, MessageContent>,
    completed: bool,
}

impl OpenAiAccumulator {
    fn handle_responses(&mut self, event_type: &str, value: &Value) -> Result<Vec<String>> {
        match event_type {
            "response.output_text.delta" => self.handle_text_delta(value),
            "response.output_item.added" | "response.output_item.done" => {
                self.handle_output_item(event_type, value)
            }
            "response.function_call_arguments.delta" => {
                self.handle_arguments_delta(value);
                Ok(Vec::new())
            }
            "response.completed" => {
                self.completed = true;
                if let Some(usage) = value.pointer("/response/usage") {
                    merge_usage(&mut self.usage, usage);
                }
                Ok(Vec::new())
            }
            _ => Ok(Vec::new()),
        }
    }

    fn handle_text_delta(&mut self, value: &Value) -> Result<Vec<String>> {
        let delta = value
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.text.push_str(delta);
        let index = value
            .get("output_index")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(usize::MAX);
        self.texts.entry(index).or_default().push_str(delta);
        Ok(if delta.is_empty() {
            Vec::new()
        } else {
            vec![delta.to_owned()]
        })
    }

    fn handle_output_item(&mut self, event_type: &str, value: &Value) -> Result<Vec<String>> {
        let Some(item) = value.get("item") else {
            return Ok(Vec::new());
        };
        if item.get("type").and_then(Value::as_str) == Some("reasoning") {
            if event_type == "response.output_item.done" {
                let index = output_index(value, self.provider_items.len());
                self.provider_items.insert(
                    index,
                    MessageContent::ProviderItem {
                        provider: "openai".to_owned(),
                        item: item.clone(),
                    },
                );
            }
            return Ok(Vec::new());
        }
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return Ok(Vec::new());
        }
        let index = output_index(value, self.tools.len());
        let builder = self.tools.entry(index).or_default();
        if let Some(id) = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
        {
            builder.id = id.to_owned();
            if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                self.item_indexes.insert(item_id.to_owned(), index);
            }
        }
        if let Some(name) = item.get("name").and_then(Value::as_str) {
            builder.name = name.to_owned();
        }
        if let Some(arguments) = item.get("arguments").and_then(Value::as_str)
            && (event_type == "response.output_item.done" || builder.arguments.is_empty())
        {
            builder.arguments = arguments.to_owned();
        }
        Ok(Vec::new())
    }

    fn handle_arguments_delta(&mut self, value: &Value) {
        let index = value
            .get("output_index")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .or_else(|| {
                value
                    .get("item_id")
                    .and_then(Value::as_str)
                    .and_then(|id| self.item_indexes.get(id).copied())
            })
            .unwrap_or(0);
        if let Some(delta) = value.get("delta").and_then(Value::as_str) {
            self.tools
                .entry(index)
                .or_default()
                .arguments
                .push_str(delta);
        }
    }

    fn handle_chat(&mut self, value: &Value) -> Result<Vec<String>> {
        if let Some(usage) = value.get("usage") {
            merge_usage(&mut self.usage, usage);
        }
        let Some(delta) = value.pointer("/choices/0/delta") else {
            return Ok(Vec::new());
        };
        if let Some(reason) = value
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
        {
            if !matches!(reason, "stop" | "tool_calls" | "function_call") {
                bail!("OpenAI chat response stopped with `{reason}` before completion");
            }
            self.completed = true;
        }
        let mut emitted = Vec::new();
        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            self.text.push_str(text);
            emitted.push(text.to_owned());
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let builder = self.tools.entry(index).or_default();
                if let Some(id) = call.get("id").and_then(Value::as_str) {
                    builder.id = id.to_owned();
                }
                if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                    builder.name.push_str(name);
                }
                if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str)
                {
                    builder.arguments.push_str(arguments);
                }
            }
        }
        Ok(emitted)
    }

    fn finish(self, protocol: OpenAiProtocol) -> Result<ModelResponse> {
        if !self.completed {
            bail!("OpenAI stream ended before a completion event");
        }
        if protocol == OpenAiProtocol::ChatCompletions {
            let mut assistant_content = Vec::new();
            if !self.text.is_empty() {
                assistant_content.push(MessageContent::Text {
                    text: self.text.clone(),
                });
            }
            let mut tool_calls = Vec::new();
            for builder in self.tools.into_values() {
                let call = builder.finish()?;
                assistant_content.push(MessageContent::ToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                });
                tool_calls.push(call);
            }
            return Ok(ModelResponse {
                text: self.text,
                tool_calls,
                assistant_content,
                usage: self.usage,
            });
        }

        let mut assistant_items = self.provider_items;
        for (index, text) in self.texts {
            if !text.is_empty() {
                assistant_items.insert(index, MessageContent::Text { text });
            }
        }
        let mut tool_calls = Vec::new();
        for (index, builder) in self.tools {
            let call = builder.finish()?;
            assistant_items.insert(
                index,
                MessageContent::ToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                },
            );
            tool_calls.push(call);
        }
        Ok(ModelResponse {
            text: self.text,
            tool_calls,
            assistant_content: assistant_items.into_values().collect(),
            usage: self.usage,
        })
    }
}

fn output_index(value: &Value, fallback: usize) -> usize {
    value
        .get("output_index")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn responses_preserve_reasoning_before_function_calls() {
        let mut accumulator = OpenAiAccumulator::default();
        accumulator
            .handle_responses(
                "response.output_item.done",
                &json!({
                    "output_index": 0,
                    "item": {"type": "reasoning", "id": "rs_1", "encrypted_content": "opaque"}
                }),
            )
            .unwrap();
        accumulator
            .handle_responses(
                "response.output_item.done",
                &json!({
                    "output_index": 1,
                    "item": {"type": "function_call", "call_id": "call_1", "name": "read", "arguments": "{}"}
                }),
            )
            .unwrap();
        accumulator
            .handle_responses("response.completed", &json!({"response": {}}))
            .unwrap();
        let response = accumulator.finish(OpenAiProtocol::Responses).unwrap();
        assert!(matches!(
            response.assistant_content[0],
            MessageContent::ProviderItem { .. }
        ));
        assert!(matches!(
            response.assistant_content[1],
            MessageContent::ToolCall { .. }
        ));
    }
}
