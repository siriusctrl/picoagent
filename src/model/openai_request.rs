use serde_json::{Value, json};

use super::{
    Message, MessageContent, ModelRequest, Role, ToolSpec, common::content_text,
    openai_chat::project_chat_message,
};

pub(crate) fn responses_body(request: &ModelRequest, reasoning_effort: Option<&str>) -> Value {
    let mut body = json!({
        "model": request.model,
        "input": responses_input(&request.messages),
        "stream": true,
        "store": false,
        "include": ["reasoning.encrypted_content"],
    });
    if !request.tools.is_empty() {
        body["tools"] = response_tools(&request.tools);
    }
    if !request.system.is_empty() {
        body["instructions"] = json!(request.system);
    }
    if let Some(max_tokens) = request.max_output_tokens {
        body["max_output_tokens"] = json!(max_tokens);
    }
    if let Some(effort) = reasoning_effort {
        body["reasoning"] = json!({"effort": effort});
    }
    body
}

pub(crate) fn chat_body(request: &ModelRequest, reasoning_effort: Option<&str>) -> Value {
    let mut messages = Vec::new();
    if !request.system.is_empty() {
        messages.push(json!({"role": "system", "content": request.system}));
    }
    messages.extend(
        request
            .messages
            .iter()
            .map(|message| json!(project_chat_message(message))),
    );
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if !request.tools.is_empty() {
        body["tools"] = chat_tools(&request.tools);
    }
    if let Some(max_tokens) = request.max_output_tokens {
        let field = if reasoning_effort.is_some() {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        body[field] = json!(max_tokens);
    }
    if let Some(effort) = reasoning_effort {
        body["reasoning_effort"] = json!(effort);
    }
    body
}

fn response_tools(tools: &[ToolSpec]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                    "strict": false,
                })
            })
            .collect(),
    )
}

fn responses_input(messages: &[Message]) -> Value {
    let mut input = Vec::new();
    for message in messages {
        match &message.role {
            Role::User => input.push(json!({
                "role": "user",
                "content": [{"type": "input_text", "text": content_text(&message.content)}]
            })),
            Role::Assistant => {
                for block in &message.content {
                    match block {
                        MessageContent::Text { text } if !text.is_empty() => input.push(json!({
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": text}]
                        })),
                        MessageContent::ToolCall {
                            id,
                            name,
                            arguments,
                        } => input.push(json!({
                            "type": "function_call",
                            "call_id": id,
                            "name": name,
                            "arguments": arguments.to_string(),
                        })),
                        MessageContent::ProviderItem { provider, item } if provider == "openai" => {
                            input.push(item.clone())
                        }
                        MessageContent::BackgroundTaskResult { .. } => {}
                        _ => {}
                    }
                }
            }
            Role::Tool => {
                for block in &message.content {
                    if let MessageContent::ToolResult {
                        call_id, content, ..
                    } = block
                    {
                        input.push(json!({
                            "type": "function_call_output",
                            "call_id": call_id,
                            "output": content,
                        }));
                    }
                }
            }
        }
    }
    Value::Array(input)
}

fn chat_tools(tools: &[ToolSpec]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|tool| {
                json!({"type": "function", "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                }})
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_with_tools(tools: Vec<ToolSpec>) -> ModelRequest {
        ModelRequest {
            run_id: "run".into(),
            model: "model".into(),
            system: String::new(),
            messages: vec![Message::text(Role::User, "hello")],
            tools,
            max_output_tokens: None,
            stream_idle_timeout: std::time::Duration::from_secs(300),
        }
    }

    #[test]
    fn openai_bodies_omit_empty_tools_and_keep_nonempty_tools() {
        let empty = request_with_tools(Vec::new());
        assert!(responses_body(&empty, None).get("tools").is_none());
        assert!(chat_body(&empty, None).get("tools").is_none());

        let with_tool = request_with_tools(vec![ToolSpec {
            name: "read".into(),
            description: "Read a file".into(),
            input_schema: json!({"type": "object"}),
        }]);
        assert_eq!(responses_body(&with_tool, None)["tools"][0]["name"], "read");
        assert_eq!(
            chat_body(&with_tool, None)["tools"][0]["function"]["name"],
            "read"
        );
    }

    #[test]
    fn oauth_request_replays_encrypted_reasoning_items() {
        let reasoning = json!({
            "type": "reasoning",
            "id": "rs_1",
            "encrypted_content": "opaque"
        });
        let request = ModelRequest {
            run_id: "run".into(),
            model: "reasoning-model".into(),
            system: String::new(),
            messages: vec![Message {
                role: Role::Assistant,
                content: vec![MessageContent::ProviderItem {
                    provider: "openai".into(),
                    item: reasoning.clone(),
                }],
            }],
            tools: Vec::new(),
            max_output_tokens: None,
            stream_idle_timeout: std::time::Duration::from_secs(300),
        };

        let body = responses_body(&request, None);
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(body["input"][0], reasoning);
    }

    #[test]
    fn responses_replay_ordered_reasoning_text_and_tool_items() {
        let request = ModelRequest {
            run_id: "run".into(),
            model: "reasoning-model".into(),
            system: String::new(),
            messages: vec![Message {
                role: Role::Assistant,
                content: vec![
                    MessageContent::ProviderItem {
                        provider: "openai".into(),
                        item: json!({"type": "reasoning", "id": "rs_1", "encrypted_content": "opaque"}),
                    },
                    MessageContent::Text {
                        text: "checked".into(),
                    },
                    MessageContent::ToolCall {
                        id: "call_1".into(),
                        name: "read".into(),
                        arguments: json!({"path": "README.md"}),
                    },
                ],
            }],
            tools: Vec::new(),
            max_output_tokens: None,
            stream_idle_timeout: std::time::Duration::from_secs(300),
        };

        let body = responses_body(&request, None);
        assert_eq!(body["input"][0]["type"], "reasoning");
        assert_eq!(body["input"][1]["content"][0]["text"], "checked");
        assert_eq!(body["input"][2]["type"], "function_call");
    }

    #[test]
    fn chat_keeps_reasoning_separate_from_visible_continuation_context() {
        let message = Message {
            role: Role::Assistant,
            content: vec![
                MessageContent::Reasoning {
                    text: "inspect first".into(),
                },
                MessageContent::Text {
                    text: "checked".into(),
                },
                MessageContent::ToolCall {
                    id: "call_1".into(),
                    name: "read".into(),
                    arguments: json!({"path": "README.md"}),
                },
            ],
        };

        let value = json!(project_chat_message(&message));
        assert_eq!(value["reasoning_content"], "inspect first");
        assert_eq!(value["content"], "checked");
        assert_eq!(value["tool_calls"][0]["id"], "call_1");
    }

    #[test]
    fn background_results_are_new_user_text_for_both_openai_protocols() {
        let request = ModelRequest {
            run_id: "run".into(),
            model: "model".into(),
            system: String::new(),
            messages: vec![Message {
                role: Role::User,
                content: vec![MessageContent::BackgroundTaskResult {
                    task_id: "task-1".into(),
                    name: "bash".into(),
                    status: "completed".into(),
                    content: "done".into(),
                    metadata: crate::artifact::ResultMetadata::empty(),
                }],
            }],
            tools: Vec::new(),
            max_output_tokens: None,
            stream_idle_timeout: std::time::Duration::from_secs(300),
        };

        let responses = responses_body(&request, None);
        let chat = chat_body(&request, None);
        assert_eq!(responses["input"][0]["role"], "user");
        assert!(
            responses["input"][0]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("task-1")
        );
        assert_eq!(chat["messages"][0]["role"], "user");
        assert!(
            chat["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("done")
        );
    }

    #[test]
    fn runtime_reminder_precedes_user_text_for_both_openai_protocols() {
        let request = ModelRequest {
            run_id: "run".into(),
            model: "model".into(),
            system: "stable system".into(),
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

        let expected = "<runtime-reminder>context</runtime-reminder>\n\ndo the task";
        assert_eq!(
            responses_body(&request, None)["input"][0]["content"][0]["text"],
            expected
        );
        assert_eq!(
            chat_body(&request, None)["messages"][1]["content"],
            expected
        );
    }

    #[test]
    fn reasoning_effort_uses_protocol_specific_request_shapes() {
        let request = ModelRequest {
            run_id: "run".into(),
            model: "reasoning-model".into(),
            system: String::new(),
            messages: vec![Message::text(Role::User, "solve this")],
            tools: Vec::new(),
            max_output_tokens: Some(128),
            stream_idle_timeout: std::time::Duration::from_secs(300),
        };

        let responses = responses_body(&request, Some("high"));
        let chat = chat_body(&request, Some("low"));
        let default_chat = chat_body(&request, None);
        assert_eq!(responses["reasoning"], json!({"effort": "high"}));
        assert_eq!(chat["reasoning_effort"], json!("low"));
        assert_eq!(chat["max_completion_tokens"], json!(128));
        assert!(chat.get("max_tokens").is_none());
        assert_eq!(default_chat["max_tokens"], json!(128));
        assert!(default_chat.get("max_completion_tokens").is_none());
        assert!(responses_body(&request, None).get("reasoning").is_none());
        assert!(default_chat.get("reasoning_effort").is_none());
    }
}
