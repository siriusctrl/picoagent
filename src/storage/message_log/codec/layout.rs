use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    artifact::ResultMetadata,
    model::{
        ImageAttachment, Message, MessageContent, Role,
        openai_chat::{
            ChatMessage, ChatToolCall, ChatToolCallKind, ChatUserContent, ChatUserContentPart,
        },
    },
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ContentLayout {
    RuntimeContext {
        start: usize,
        end: usize,
    },
    Text {
        start: usize,
        end: usize,
    },
    Image {
        part_index: usize,
    },
    Reasoning {
        start: usize,
        end: usize,
    },
    ToolCall {
        index: usize,
    },
    ToolResult {
        is_error: bool,
        metadata: ResultMetadata,
    },
    ProviderItem {
        provider: String,
        item: Value,
    },
    BackgroundTask {
        task_id: String,
        name: String,
        status: Option<String>,
        start: usize,
        end: usize,
        content_start: usize,
        content_end: usize,
        metadata: ResultMetadata,
    },
}

pub(super) fn encode(message: &Message, native: &ChatMessage) -> Result<Vec<ContentLayout>> {
    match (&message.role, native) {
        (Role::User, ChatMessage::User { content }) => encode_user(message, content),
        (
            Role::Assistant,
            ChatMessage::Assistant {
                content,
                reasoning_content,
                tool_calls,
            },
        ) => encode_assistant(
            message,
            content,
            reasoning_content.as_deref(),
            tool_calls.len(),
        ),
        (Role::Tool, ChatMessage::Tool { .. }) => encode_tool(message),
        _ => bail!("projected OpenAI Chat role disagrees with the internal message role"),
    }
}

fn encode_user(message: &Message, expected: &ChatUserContent) -> Result<Vec<ContentLayout>> {
    let expected_text = user_text(expected)?;
    if crate::model::render_background_task_content(&message.content).is_some() {
        ensure!(
            matches!(expected, ChatUserContent::Text(_)),
            "background task reminders cannot contain image parts"
        );
        return encode_background_tasks(message, expected_text);
    }
    let image_part_indexes = match expected {
        ChatUserContent::Text(_) => Vec::new(),
        ChatUserContent::Parts(parts) => parts
            .iter()
            .enumerate()
            .filter_map(|(index, part)| {
                matches!(part, ChatUserContentPart::ImageUrl { .. }).then_some(index)
            })
            .collect(),
    };
    let mut rendered = String::new();
    let mut previous_was_reminder = false;
    let mut image_index = 0_usize;
    let mut layout = Vec::with_capacity(message.content.len());
    for block in &message.content {
        match block {
            MessageContent::RuntimeReminder { text } => {
                let (start, end) =
                    append_visible(&mut rendered, text, true, &mut previous_was_reminder);
                layout.push(ContentLayout::RuntimeContext { start, end });
            }
            MessageContent::Text { text } => {
                let (start, end) =
                    append_visible(&mut rendered, text, false, &mut previous_was_reminder);
                layout.push(ContentLayout::Text { start, end });
            }
            MessageContent::Image { attachment } => {
                let part_index = *image_part_indexes
                    .get(image_index)
                    .context("user image is missing from its Chat projection")?;
                let ChatUserContent::Parts(parts) = expected else {
                    bail!("user image requires Chat content parts");
                };
                let ChatUserContentPart::ImageUrl { image_url } = &parts[part_index] else {
                    bail!("user image layout references a non-image Chat part");
                };
                ensure!(
                    image_url.url == attachment.data_url(),
                    "user image disagrees with its Chat projection"
                );
                layout.push(ContentLayout::Image { part_index });
                image_index += 1;
            }
            _ => bail!(
                "user messages contain only runtime context, text, images, or background results"
            ),
        }
    }
    ensure!(
        rendered == expected_text,
        "user layout disagrees with Chat projection"
    );
    ensure!(
        image_index == image_part_indexes.len(),
        "Chat projection contains an image absent from user layout"
    );
    Ok(layout)
}

fn encode_background_tasks(message: &Message, expected: &str) -> Result<Vec<ContentLayout>> {
    let mut cursor = "<runtime-reminder>\n".len();
    let mut layout = Vec::with_capacity(message.content.len());
    for (index, block) in message.content.iter().enumerate() {
        let MessageContent::BackgroundTask {
            task_id,
            name,
            status,
            content,
            metadata,
        } = block
        else {
            bail!("background task notices cannot be mixed with other user content");
        };
        if index > 0 {
            cursor += 2;
        }
        let rendered = crate::model::runtime::render_background_task_block(
            task_id,
            name,
            status.as_deref(),
            content,
        );
        let content_offset = rendered
            .find('\n')
            .map(|offset| offset + 1)
            .context("background task rendering omitted its opening line")?;
        let start = cursor;
        let end = start + rendered.len();
        let content_start = start + content_offset;
        let escaped_content = crate::model::runtime::escape_xml_text(content);
        layout.push(ContentLayout::BackgroundTask {
            task_id: task_id.clone(),
            name: name.clone(),
            status: status.clone(),
            start,
            end,
            content_start,
            content_end: content_start + escaped_content.len(),
            metadata: metadata.clone(),
        });
        cursor = end;
    }
    ensure!(
        crate::model::render_background_task_content(&message.content).as_deref() == Some(expected),
        "background task layout disagrees with Chat projection"
    );
    Ok(layout)
}

fn encode_assistant(
    message: &Message,
    expected_content: &str,
    expected_reasoning: Option<&str>,
    expected_tool_calls: usize,
) -> Result<Vec<ContentLayout>> {
    let mut content = String::new();
    let mut previous_was_reminder = false;
    let mut reasoning = String::new();
    let mut reasoning_count = 0_usize;
    let mut tool_call_index = 0_usize;
    let mut layout = Vec::with_capacity(message.content.len());
    for block in &message.content {
        match block {
            MessageContent::Text { text } => {
                let (start, end) =
                    append_visible(&mut content, text, false, &mut previous_was_reminder);
                layout.push(ContentLayout::Text { start, end });
            }
            MessageContent::Reasoning { text } => {
                if reasoning_count > 0 {
                    reasoning.push('\n');
                }
                let start = reasoning.len();
                reasoning.push_str(text);
                let end = reasoning.len();
                reasoning_count += 1;
                layout.push(ContentLayout::Reasoning { start, end });
            }
            MessageContent::ToolCall { .. } => {
                layout.push(ContentLayout::ToolCall {
                    index: tool_call_index,
                });
                tool_call_index += 1;
            }
            MessageContent::ProviderItem { provider, item } => {
                layout.push(ContentLayout::ProviderItem {
                    provider: provider.clone(),
                    item: item.clone(),
                });
            }
            _ => bail!(
                "assistant messages contain only text, reasoning, tool calls, or provider items"
            ),
        }
    }
    ensure!(
        content == expected_content,
        "assistant text layout disagrees with Chat projection"
    );
    ensure!(
        (reasoning_count > 0).then_some(reasoning.as_str()) == expected_reasoning,
        "assistant reasoning layout disagrees with Chat projection"
    );
    ensure!(
        tool_call_index == expected_tool_calls,
        "assistant tool layout disagrees with Chat projection"
    );
    Ok(layout)
}

fn encode_tool(message: &Message) -> Result<Vec<ContentLayout>> {
    let [
        MessageContent::ToolResult {
            call_id,
            is_error,
            metadata,
            ..
        },
    ] = message.content.as_slice()
    else {
        bail!("tool messages require exactly one tool result");
    };
    validate_result_metadata(metadata, call_id)?;
    Ok(vec![ContentLayout::ToolResult {
        is_error: *is_error,
        metadata: metadata.clone(),
    }])
}

pub(super) fn decode(native: &ChatMessage, layout: Vec<ContentLayout>) -> Result<Message> {
    let (role, content) = match native {
        ChatMessage::User { content } => (Role::User, decode_user(content, layout)?),
        ChatMessage::Assistant {
            content,
            reasoning_content,
            tool_calls,
        } => (
            Role::Assistant,
            decode_assistant(content, reasoning_content.as_deref(), tool_calls, layout)?,
        ),
        ChatMessage::Tool {
            tool_call_id,
            content,
        } => (Role::Tool, decode_tool(tool_call_id, content, layout)?),
    };
    Ok(Message { role, content })
}

fn decode_user(
    content: &ChatUserContent,
    layout: Vec<ContentLayout>,
) -> Result<Vec<MessageContent>> {
    let text = user_text(content)?;
    let decoded = layout
        .into_iter()
        .map(|entry| match entry {
            ContentLayout::RuntimeContext { start, end } => Ok(MessageContent::RuntimeReminder {
                text: span(text, start, end)?.to_owned(),
            }),
            ContentLayout::Text { start, end } => Ok(MessageContent::Text {
                text: span(text, start, end)?.to_owned(),
            }),
            ContentLayout::Image { part_index } => {
                let ChatUserContent::Parts(parts) = content else {
                    bail!("user image metadata requires Chat content parts");
                };
                let ChatUserContentPart::ImageUrl { image_url } = parts
                    .get(part_index)
                    .context("user image metadata references a missing Chat part")?
                else {
                    bail!("user image metadata references a non-image Chat part");
                };
                Ok(MessageContent::Image {
                    attachment: ImageAttachment::from_data_url(&image_url.url)?,
                })
            }
            ContentLayout::BackgroundTask {
                task_id,
                name,
                status,
                start,
                end,
                content_start,
                content_end,
                metadata,
            } => {
                ensure!(
                    start <= content_start && content_end <= end,
                    "background result content span lies outside its block"
                );
                let encoded = span(text, content_start, content_end)?;
                let raw = crate::model::runtime::unescape_xml_text(encoded);
                let expected = crate::model::runtime::render_background_task_block(
                    &task_id,
                    &name,
                    status.as_deref(),
                    &raw,
                );
                ensure!(
                    span(text, start, end)? == expected,
                    "background result envelope disagrees with its metadata"
                );
                Ok(MessageContent::BackgroundTask {
                    task_id,
                    name,
                    status,
                    content: raw,
                    metadata,
                })
            }
            _ => bail!("user message metadata contains a non-user layout entry"),
        })
        .collect::<Result<Vec<_>>>()?;
    if decoded
        .iter()
        .any(|entry| matches!(entry, MessageContent::BackgroundTask { .. }))
    {
        ensure!(
            crate::model::render_background_task_content(&decoded).as_deref() == Some(text),
            "background task reminder envelope disagrees with its metadata"
        );
    }
    Ok(decoded)
}

fn user_text(content: &ChatUserContent) -> Result<&str> {
    match content {
        ChatUserContent::Text(text) => Ok(text),
        ChatUserContent::Parts(parts) => {
            let mut text = parts.iter().filter_map(|part| match part {
                ChatUserContentPart::Text { text } => Some(text.as_str()),
                ChatUserContentPart::ImageUrl { .. } => None,
            });
            let first = text.next().unwrap_or_default();
            ensure!(
                text.next().is_none(),
                "picoagent Chat projection supports at most one user text part"
            );
            Ok(first)
        }
    }
}

fn decode_assistant(
    content: &str,
    reasoning: Option<&str>,
    tool_calls: &[ChatToolCall],
    layout: Vec<ContentLayout>,
) -> Result<Vec<MessageContent>> {
    layout
        .into_iter()
        .map(|entry| match entry {
            ContentLayout::Text { start, end } => Ok(MessageContent::Text {
                text: span(content, start, end)?.to_owned(),
            }),
            ContentLayout::Reasoning { start, end } => {
                let reasoning = reasoning.context(
                    "assistant metadata references reasoning_content that is not present",
                )?;
                Ok(MessageContent::Reasoning {
                    text: span(reasoning, start, end)?.to_owned(),
                })
            }
            ContentLayout::ToolCall { index } => {
                let call = tool_calls
                    .get(index)
                    .with_context(|| format!("assistant metadata references tool call {index}"))?;
                ensure!(
                    call.kind == ChatToolCallKind::Function,
                    "only function tool calls are supported"
                );
                Ok(MessageContent::ToolCall {
                    id: call.id.clone(),
                    name: call.function.name.clone(),
                    arguments: crate::model::ToolArguments::from_raw(
                        call.function.arguments.clone(),
                    ),
                })
            }
            ContentLayout::ProviderItem { provider, item } => {
                Ok(MessageContent::ProviderItem { provider, item })
            }
            _ => bail!("assistant message metadata contains a non-assistant layout entry"),
        })
        .collect()
}

fn decode_tool(
    call_id: &str,
    content: &str,
    layout: Vec<ContentLayout>,
) -> Result<Vec<MessageContent>> {
    let [ContentLayout::ToolResult { is_error, metadata }] = layout.as_slice() else {
        bail!("tool message metadata must contain exactly one tool result");
    };
    validate_result_metadata(metadata, call_id)?;
    Ok(vec![MessageContent::ToolResult {
        call_id: call_id.to_owned(),
        content: content.to_owned(),
        is_error: *is_error,
        metadata: metadata.clone(),
    }])
}

fn validate_result_metadata(metadata: &ResultMetadata, call_id: &str) -> Result<()> {
    if let Some(artifact) = &metadata.artifact {
        ensure!(
            artifact.call_id == call_id,
            "result artifact call id `{}` does not match `{call_id}`",
            artifact.call_id
        );
    }
    Ok(())
}

fn append_visible(
    rendered: &mut String,
    text: &str,
    is_reminder: bool,
    previous_was_reminder: &mut bool,
) -> (usize, usize) {
    if !rendered.is_empty() {
        rendered.push_str(if *previous_was_reminder || is_reminder {
            "\n\n"
        } else {
            "\n"
        });
    }
    let start = rendered.len();
    rendered.push_str(text);
    let end = rendered.len();
    *previous_was_reminder = is_reminder;
    (start, end)
}

fn span(value: &str, start: usize, end: usize) -> Result<&str> {
    ensure!(start <= end, "message span starts after it ends");
    value
        .get(start..end)
        .context("message span is outside a UTF-8 field boundary")
}
