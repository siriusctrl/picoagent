use serde::{Deserialize, Serialize};

use super::{Message, MessageContent, Role, common::content_text};

/// The OpenAI Chat-compatible representation of one provider-neutral message.
///
/// `reasoning_content` is an extension implemented by reasoning-capable
/// OpenAI-compatible endpoints. It is deliberately optional so ordinary OpenAI
/// Chat messages retain their native shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "role", rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum ChatMessage {
    User {
        content: ChatUserContent,
    },
    Assistant {
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ChatToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub(crate) enum ChatUserContent {
    Text(String),
    Parts(Vec<ChatUserContentPart>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ChatUserContentPart {
    Text { text: String },
    ImageUrl { image_url: ChatImageUrl },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChatImageUrl {
    pub(crate) url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChatToolCall {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) kind: ChatToolCallKind,
    pub(crate) function: ChatFunctionCall,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum ChatToolCallKind {
    #[serde(rename = "function")]
    Function,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChatFunctionCall {
    pub(crate) name: String,
    /// OpenAI Chat represents function arguments as a JSON-encoded string.
    pub(crate) arguments: String,
}

pub(crate) fn project_chat_message(message: &Message) -> ChatMessage {
    match message.role {
        Role::User => ChatMessage::User {
            content: project_user_content(&message.content),
        },
        Role::Assistant => ChatMessage::Assistant {
            content: content_text(&message.content),
            reasoning_content: message.reasoning_content.clone(),
            tool_calls: message
                .content
                .iter()
                .filter_map(|block| match block {
                    MessageContent::ToolCall(call) => Some(ChatToolCall {
                        id: call.id.clone(),
                        kind: ChatToolCallKind::Function,
                        function: ChatFunctionCall {
                            name: call.name.clone(),
                            arguments: call.arguments.as_raw().to_owned(),
                        },
                    }),
                    _ => None,
                })
                .collect(),
        },
        Role::Tool => {
            let (tool_call_id, content) = message
                .content
                .iter()
                .find_map(|block| match block {
                    MessageContent::ToolResult {
                        call_id, content, ..
                    } => Some((call_id.clone(), content.clone())),
                    _ => None,
                })
                .unwrap_or_default();
            ChatMessage::Tool {
                tool_call_id,
                content,
            }
        }
    }
}

fn project_user_content(content: &[MessageContent]) -> ChatUserContent {
    let images = content
        .iter()
        .filter_map(|block| match block {
            MessageContent::Image { attachment } => Some(attachment),
            _ => None,
        })
        .collect::<Vec<_>>();
    let text = content_text(content);
    if images.is_empty() {
        return ChatUserContent::Text(text);
    }
    let mut parts = Vec::with_capacity(images.len() + usize::from(!text.is_empty()));
    if !text.is_empty() {
        parts.push(ChatUserContentPart::Text { text });
    }
    parts.extend(
        images
            .into_iter()
            .map(|attachment| ChatUserContentPart::ImageUrl {
                image_url: ChatImageUrl {
                    url: attachment.data_url(),
                },
            }),
    );
    ChatUserContent::Parts(parts)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::model::ToolCall;

    #[test]
    fn user_message_serializes_as_native_role_and_content() {
        let projected = project_chat_message(&Message::new(
            Role::User,
            vec![
                MessageContent::RuntimeReminder {
                    text: "<runtime-reminder>context</runtime-reminder>".into(),
                },
                MessageContent::Text {
                    text: "do the task".into(),
                },
            ],
        ));

        assert_eq!(
            serde_json::to_value(projected).unwrap(),
            json!({
                "role": "user",
                "content": "<runtime-reminder>context</runtime-reminder>\n\ndo the task"
            })
        );
    }

    #[test]
    fn user_images_use_native_chat_content_parts() {
        let projected = project_chat_message(&Message::new(
            Role::User,
            vec![
                MessageContent::Text {
                    text: "inspect the attachment".into(),
                },
                MessageContent::Image {
                    attachment: super::super::ImageAttachment::from_bytes("image/png", b"png"),
                },
            ],
        ));

        assert_eq!(
            serde_json::to_value(projected).unwrap(),
            json!({
                "role": "user",
                "content": [
                    {"type": "text", "text": "inspect the attachment"},
                    {
                        "type": "image_url",
                        "image_url": {"url": "data:image/png;base64,cG5n"}
                    }
                ]
            })
        );
    }

    #[test]
    fn assistant_message_keeps_reasoning_and_stringifies_tool_arguments() {
        let projected = project_chat_message(&Message {
            role: Role::Assistant,
            reasoning_content: Some("inspect\n\n  confirm ".into()),
            content: vec![
                MessageContent::Text {
                    text: "checked".into(),
                },
                MessageContent::ToolCall(ToolCall {
                    id: "call_1".into(),
                    name: "read".into(),
                    arguments: json!({"path": "README.md"}).into(),
                }),
            ],
        });

        let value = serde_json::to_value(&projected).unwrap();
        assert_eq!(value["role"], "assistant");
        assert_eq!(value["content"], "checked");
        assert_eq!(value["reasoning_content"], "inspect\n\n  confirm ");
        assert_eq!(value["tool_calls"][0]["type"], "function");
        assert_eq!(
            value["tool_calls"][0]["function"]["arguments"],
            r#"{"path":"README.md"}"#
        );
        assert!(
            value["tool_calls"][0]["function"]["arguments"]
                .as_str()
                .is_some()
        );

        let decoded: ChatMessage = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, projected);
    }

    #[test]
    fn assistant_omits_absent_extension_fields() {
        let value = serde_json::to_value(project_chat_message(&Message::text(
            Role::Assistant,
            "done",
        )))
        .unwrap();

        assert_eq!(value, json!({"role": "assistant", "content": "done"}));
    }

    #[test]
    fn tool_message_round_trips() {
        let value = json!({
            "role": "tool",
            "tool_call_id": "call_1",
            "content": "file contents"
        });
        let decoded: ChatMessage = serde_json::from_value(value.clone()).unwrap();

        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    }

    #[test]
    fn deserialization_rejects_non_function_tool_calls() {
        let value: Value = json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "call_1",
                "type": "custom",
                "function": {"name": "read", "arguments": "{}"}
            }]
        });

        assert!(serde_json::from_value::<ChatMessage>(value).is_err());
    }

    #[test]
    fn deserialization_rejects_unknown_message_fields() {
        let value = json!({
            "role": "user",
            "content": "hello",
            "message_id": "fiasco-private-field"
        });

        assert!(serde_json::from_value::<ChatMessage>(value).is_err());
    }
}
