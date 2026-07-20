use std::{collections::BTreeMap, fmt};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use reqwest::Client;
use serde_json::{Value, json};

use super::{
    Message, MessageContent, ModelProvider, ModelRequest, ModelResponse, ModelUsage, Role,
    ToolSpec,
    common::{
        ToolCallBuilder, content_text, emit_text, ensure_success, join_url, merge_usage,
        next_sse_event, send_streaming_request,
    },
};
use crate::events::SharedEventSink;

#[derive(Clone)]
pub struct AnthropicCompatibleOptions {
    pub base_url: String,
    pub api_key: String,
    pub anthropic_version: String,
}

impl fmt::Debug for AnthropicCompatibleOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AnthropicCompatibleOptions")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("anthropic_version", &self.anthropic_version)
            .finish()
    }
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

    fn resume_fingerprint(&self) -> String {
        super::stable_resume_fingerprint(
            self.name(),
            &[
                ("base_url", self.options.base_url.trim_end_matches('/')),
                ("anthropic_version", &self.options.anthropic_version),
            ],
        )
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
            send_streaming_request(builder, "Anthropic", request.stream_idle_timeout).await?,
            "Anthropic",
        )
        .await?;
        let mut stream = response.bytes_stream().eventsource();
        let mut accumulator = AnthropicAccumulator::default();
        while let Some(event) =
            next_sse_event(&mut stream, "Anthropic", request.stream_idle_timeout).await?
        {
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
        "messages": anthropic_messages(&request.messages),
        "stream": true,
    });
    if !request.tools.is_empty() {
        body["tools"] = anthropic_tools(&request.tools);
    }
    if !request.system.is_empty() {
        body["system"] = json!(request.system);
    }
    body
}

fn anthropic_messages(messages: &[Message]) -> Vec<Value> {
    let mut serialized = Vec::new();
    for message in messages {
        let next = anthropic_message(message);
        let can_merge = serialized
            .last()
            .is_some_and(|previous: &Value| previous["role"] == next["role"]);
        if can_merge {
            let content = next["content"]
                .as_array()
                .expect("serialized Anthropic content must be an array")
                .clone();
            serialized
                .last_mut()
                .and_then(|previous| previous["content"].as_array_mut())
                .expect("serialized Anthropic content must be an array")
                .extend(content);
        } else {
            serialized.push(next);
        }
    }
    serialized
}

fn anthropic_tool_results(message: &Message) -> Vec<Value> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            MessageContent::ToolResult {
                call_id,
                content,
                is_error,
                ..
            } => Some(json!({
                "type": "tool_result",
                "tool_use_id": call_id,
                "content": content,
                "is_error": is_error,
            })),
            _ => None,
        })
        .collect()
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
        Role::Tool => json!({"role": "user", "content": anthropic_tool_results(message)}),
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
    texts: BTreeMap<usize, String>,
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
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            self.texts.entry(index).or_default().push_str(text);
                        }
                    }
                    Some("tool_use") => {
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
                    _ => {}
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
                        self.texts.entry(index).or_default().push_str(text);
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
        let mut content = BTreeMap::new();
        for (index, text) in self.texts {
            if !text.is_empty() {
                content.insert(index, MessageContent::Text { text });
            }
        }
        for (index, builder) in self.tools {
            let call = builder.finish()?;
            content.insert(
                index,
                MessageContent::ToolCall {
                    id: call.id,
                    name: call.name,
                    arguments: call.arguments,
                },
            );
        }
        Ok(ModelResponse::new(
            Message::assistant(content.into_values().collect()),
            self.usage,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_debug_redacts_resolved_api_key() {
        let options =
            AnthropicCompatibleOptions::new("https://example.test/v1", "anthropic-secret-token");

        let debug = format!("{options:?}");
        assert!(!debug.contains("anthropic-secret-token"));
        assert!(debug.contains("[REDACTED]"));
    }

    fn request_with(messages: Vec<Message>, tools: Vec<ToolSpec>) -> ModelRequest {
        ModelRequest {
            run_id: "run".into(),
            model: "model".into(),
            system: String::new(),
            messages,
            tools,
            max_output_tokens: None,
            stream_idle_timeout: std::time::Duration::from_secs(300),
        }
    }

    #[test]
    fn anthropic_body_omits_empty_tools_and_keeps_nonempty_tools() {
        let empty = request_with(vec![Message::text(Role::User, "hello")], Vec::new());
        assert!(anthropic_body(&empty).get("tools").is_none());

        let with_tool = request_with(
            vec![Message::text(Role::User, "hello")],
            vec![ToolSpec {
                name: "read".into(),
                description: "Read a file".into(),
                input_schema: json!({"type": "object"}),
            }],
        );
        assert_eq!(anthropic_body(&with_tool)["tools"][0]["name"], "read");
    }

    #[test]
    fn consecutive_tool_messages_share_one_anthropic_user_turn() {
        let request = request_with(
            vec![
                Message::text(Role::User, "start"),
                Message {
                    role: Role::Assistant,
                    content: vec![
                        MessageContent::ToolCall {
                            id: "call-1".into(),
                            name: "read".into(),
                            arguments: json!({"path": "one"}),
                        },
                        MessageContent::ToolCall {
                            id: "call-2".into(),
                            name: "read".into(),
                            arguments: json!({"path": "two"}),
                        },
                    ],
                },
                Message {
                    role: Role::Tool,
                    content: vec![MessageContent::ToolResult {
                        call_id: "call-1".into(),
                        content: "one result".into(),
                        is_error: false,
                        metadata: crate::artifact::ResultMetadata::empty(),
                    }],
                },
                Message {
                    role: Role::Tool,
                    content: vec![MessageContent::ToolResult {
                        call_id: "call-2".into(),
                        content: "two result".into(),
                        is_error: true,
                        metadata: crate::artifact::ResultMetadata::empty(),
                    }],
                },
                Message::text(Role::Assistant, "done"),
            ],
            Vec::new(),
        );

        let messages = anthropic_body(&request)["messages"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"].as_array().unwrap().len(), 2);
        assert_eq!(messages[2]["content"][0]["tool_use_id"], "call-1");
        assert_eq!(messages[2]["content"][1]["tool_use_id"], "call-2");
        assert_eq!(messages[2]["content"][1]["is_error"], true);
        assert_eq!(messages[3]["role"], "assistant");
        assert_eq!(messages[3]["content"][0]["text"], "done");
    }

    #[test]
    fn consecutive_user_messages_share_one_anthropic_turn() {
        let request = request_with(
            vec![
                Message::text(Role::User, "initial request"),
                Message::text(Role::User, "compacted history"),
            ],
            Vec::new(),
        );

        let messages = anthropic_body(&request)["messages"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"].as_array().unwrap().len(), 2);
        assert_eq!(messages[0]["content"][0]["text"], "initial request");
        assert_eq!(messages[0]["content"][1]["text"], "compacted history");
    }

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
                    metadata: crate::artifact::ResultMetadata::empty(),
                }],
            }],
            tools: Vec::new(),
            max_output_tokens: None,
            stream_idle_timeout: std::time::Duration::from_secs(300),
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
            stream_idle_timeout: std::time::Duration::from_secs(300),
        };

        let body = anthropic_body(&request);
        assert_eq!(
            body["messages"][0]["content"][0]["text"],
            "<runtime-reminder>context</runtime-reminder>\n\ndo the task"
        );
    }
}
