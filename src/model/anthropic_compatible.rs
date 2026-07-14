use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{Value, json};

use super::{
    Message, MessageContent, ModelProvider, ModelRequest, ModelResponse, ModelUsage, Role,
    ToolCall, ToolSpec,
    common::{ToolCallBuilder, content_text, emit_text, ensure_success, join_url, merge_usage},
};
use crate::events::SharedEventSink;

#[derive(Debug, Clone)]
pub struct AnthropicCompatibleOptions {
    pub base_url: String,
    pub api_key: String,
    pub anthropic_version: String,
}

impl AnthropicCompatibleOptions {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            anthropic_version: "2023-06-01".to_owned(),
        }
    }
}

#[derive(Clone)]
pub struct AnthropicCompatibleProvider {
    client: Client,
    options: AnthropicCompatibleOptions,
}

impl AnthropicCompatibleProvider {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self::with_options(AnthropicCompatibleOptions::new(base_url, api_key))
    }

    pub fn with_options(options: AnthropicCompatibleOptions) -> Self {
        Self {
            client: Client::new(),
            options,
        }
    }

    pub fn with_client(options: AnthropicCompatibleOptions, client: Client) -> Self {
        Self { client, options }
    }
}

#[async_trait]
impl ModelProvider for AnthropicCompatibleProvider {
    fn name(&self) -> &str {
        "anthropic-compatible"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let url = join_url(&self.options.base_url, "messages");
        let mut builder = self
            .client
            .post(url)
            .header("anthropic-version", &self.options.anthropic_version)
            .json(&anthropic_body(&request));
        if !self.options.api_key.trim().is_empty() {
            builder = builder.header("x-api-key", &self.options.api_key);
        }
        let response = ensure_success(
            builder
                .send()
                .await
                .context("failed to send Anthropic request")?,
            "Anthropic",
        )
        .await?;
        let mut stream = response.bytes_stream().eventsource();
        let mut accumulator = AnthropicAccumulator::default();
        while let Some(event) = stream.next().await {
            let event = event.context("invalid Anthropic SSE stream")?;
            let value: Value = serde_json::from_str(&event.data).with_context(|| {
                format!("invalid Anthropic SSE JSON for event `{}`", event.event)
            })?;
            let event_type = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or(event.event.as_str());
            if event_type == "error" {
                let message = value
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown streaming error");
                bail!("Anthropic stream error: {message}")
            }
            if let Some(delta) = accumulator.handle(event_type, &value)? {
                emit_text(&events, &request.run_id, &delta).await?;
            }
        }
        accumulator.finish()
    }
}

fn anthropic_body(request: &ModelRequest) -> Value {
    let mut body = json!({
        "model": request.model,
        "max_tokens": request.max_output_tokens.unwrap_or(4096),
        "messages": request.messages.iter().map(anthropic_message).collect::<Vec<_>>(),
        "tools": anthropic_tools(&request.tools),
        "stream": true,
    });
    if !request.system.is_empty() {
        body["system"] = json!(request.system);
    }
    body
}

fn anthropic_message(message: &Message) -> Value {
    match &message.role {
        Role::User => json!({
            "role": "user",
            "content": [{"type": "text", "text": content_text(&message.content)}]
        }),
        Role::Assistant => {
            let content: Vec<_> = message
                .content
                .iter()
                .map(|block| match block {
                    MessageContent::Text { text } => json!({"type": "text", "text": text}),
                    MessageContent::ToolCall {
                        id,
                        name,
                        arguments,
                    } => {
                        json!({"type": "tool_use", "id": id, "name": name, "input": arguments})
                    }
                    MessageContent::RuntimeReminder { .. }
                    | MessageContent::ToolResult { .. }
                    | MessageContent::Reasoning { .. }
                    | MessageContent::ProviderItem { .. }
                    | MessageContent::BackgroundTaskResult { .. } => Value::Null,
                })
                .filter(|value| !value.is_null())
                .collect();
            json!({"role": "assistant", "content": content})
        }
        Role::Tool => {
            let content: Vec<_> = message
                .content
                .iter()
                .filter_map(|block| match block {
                    MessageContent::ToolResult {
                        call_id,
                        content,
                        is_error,
                    } => Some(json!({
                        "type": "tool_result",
                        "tool_use_id": call_id,
                        "content": content,
                        "is_error": is_error,
                    })),
                    _ => None,
                })
                .collect();
            json!({"role": "user", "content": content})
        }
    }
}

fn anthropic_tools(tools: &[ToolSpec]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.input_schema,
                })
            })
            .collect(),
    )
}

#[derive(Default)]
struct AnthropicAccumulator {
    text: String,
    tools: BTreeMap<usize, ToolCallBuilder>,
    usage: ModelUsage,
    completed: bool,
}

impl AnthropicAccumulator {
    fn handle(&mut self, event_type: &str, value: &Value) -> Result<Option<String>> {
        match event_type {
            "message_start" => {
                if let Some(usage) = value.pointer("/message/usage") {
                    merge_usage(&mut self.usage, usage);
                }
            }
            "content_block_start" => {
                let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let block = &value["content_block"];
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    let builder = self.tools.entry(index).or_default();
                    builder.id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    builder.name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    if let Some(input) = block.get("input").filter(|input| !input.is_null()) {
                        let initial = input.to_string();
                        if initial != "{}" {
                            builder.arguments = initial;
                        }
                    }
                }
            }
            "content_block_delta" => {
                let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                match value.pointer("/delta/type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        let text = value
                            .pointer("/delta/text")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        self.text.push_str(text);
                        return Ok(Some(text.to_owned()));
                    }
                    Some("input_json_delta") => {
                        if let Some(json) =
                            value.pointer("/delta/partial_json").and_then(Value::as_str)
                        {
                            self.tools
                                .entry(index)
                                .or_default()
                                .arguments
                                .push_str(json);
                        }
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(reason) = value.pointer("/delta/stop_reason").and_then(Value::as_str)
                    && !matches!(reason, "end_turn" | "tool_use" | "stop_sequence")
                {
                    bail!("Anthropic response stopped with `{reason}` before completion");
                }
                if let Some(usage) = value.get("usage") {
                    merge_usage(&mut self.usage, usage);
                }
            }
            "message_stop" => self.completed = true,
            _ => {}
        }
        Ok(None)
    }

    fn finish(self) -> Result<ModelResponse> {
        if !self.completed {
            bail!("Anthropic stream ended before `message_stop`");
        }
        let tool_calls = self
            .tools
            .into_values()
            .map(ToolCallBuilder::finish)
            .collect::<Result<Vec<ToolCall>>>()?;
        Ok(ModelResponse {
            text: self.text,
            tool_calls,
            assistant_content: Vec::new(),
            usage: self.usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_results_are_serialized_as_anthropic_user_text() {
        let request = ModelRequest {
            run_id: "run".into(),
            model: "model".into(),
            system: String::new(),
            messages: vec![Message {
                role: Role::User,
                content: vec![MessageContent::BackgroundTaskResult {
                    task_id: "task-1".into(),
                    name: "general-task".into(),
                    status: "completed".into(),
                    content: "done".into(),
                }],
            }],
            tools: Vec::new(),
            max_output_tokens: None,
        };

        let body = anthropic_body(&request);
        assert_eq!(body["messages"][0]["role"], "user");
        let text = body["messages"][0]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("task-1"));
        assert!(text.contains("done"));
    }

    #[test]
    fn runtime_reminder_precedes_anthropic_user_text() {
        let request = ModelRequest {
            run_id: "run".into(),
            model: "model".into(),
            system: String::new(),
            messages: vec![Message {
                role: Role::User,
                content: vec![
                    MessageContent::RuntimeReminder {
                        text: "<runtime-reminder>context</runtime-reminder>".into(),
                    },
                    MessageContent::Text {
                        text: "do the task".into(),
                    },
                ],
            }],
            tools: Vec::new(),
            max_output_tokens: None,
        };

        let body = anthropic_body(&request);
        assert_eq!(
            body["messages"][0]["content"][0]["text"],
            "<runtime-reminder>context</runtime-reminder>\n\ndo the task"
        );
    }
}
