use std::fmt;

use anyhow::{Result, ensure};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{artifact::ResultMetadata, events::SharedEventSink};

mod common;
pub(crate) mod openai_chat;
mod openai_oauth_credentials;
mod openai_oauth_device;
mod openai_request;
mod openai_stream;
pub(crate) mod runtime;

pub mod anthropic_compatible;
pub mod openai_compatible;
pub mod openai_oauth;

pub use anthropic_compatible::{AnthropicCompatibleOptions, AnthropicCompatibleProvider};
pub use openai_compatible::{OpenAiCompatibleOptions, OpenAiCompatibleProvider, OpenAiProtocol};
pub use openai_oauth::{
    DEFAULT_OPENAI_OAUTH_BASE_URL, DeviceCode, OAuthCredentials, OpenAiOAuthOptions,
    OpenAiOAuthProvider,
};
pub(crate) use runtime::{background_task_started_reminder, render_background_task_content};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum ModelModality {
    Text,
    Image,
}

impl ModelModality {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
        }
    }
}

impl fmt::Display for ModelModality {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

pub mod echo;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageContent {
    /// Synthetic runtime context prepended to the first user request.
    RuntimeReminder {
        text: String,
    },
    Text {
        text: String,
    },
    /// A model-facing image input. The base64 payload is kept separate from
    /// text so provider adapters can use their native multimodal shapes.
    Image {
        attachment: ImageAttachment,
    },
    /// Reasoning text explicitly returned by a compatible provider.
    /// Replayed separately from visible assistant content when supported.
    Reasoning {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
    },
    ToolResult {
        call_id: String,
        content: String,
        is_error: bool,
        metadata: ResultMetadata,
    },
    /// Provider-owned continuation material, such as encrypted OpenAI reasoning.
    ProviderItem {
        provider: String,
        item: Value,
    },
    BackgroundTask {
        task_id: String,
        name: String,
        /// Terminal state when this notice carries a result artifact. A
        /// status-less notice only reports that the task remains active.
        status: Option<String>,
        content: String,
        metadata: ResultMetadata,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImageAttachment {
    pub media_type: String,
    /// Standard padded base64 without a data-URL prefix.
    pub data: String,
}

impl ImageAttachment {
    pub fn from_bytes(media_type: impl Into<String>, bytes: &[u8]) -> Self {
        Self {
            media_type: media_type.into(),
            data: STANDARD.encode(bytes),
        }
    }

    pub(crate) fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.media_type, self.data)
    }

    pub(crate) fn from_data_url(value: &str) -> Result<Self> {
        let value = value
            .strip_prefix("data:")
            .ok_or_else(|| anyhow::anyhow!("image attachment is not a data URL"))?;
        let (media_type, data) = value
            .split_once(";base64,")
            .ok_or_else(|| anyhow::anyhow!("image attachment is not base64 encoded"))?;
        anyhow::ensure!(!media_type.is_empty(), "image attachment has no media type");
        STANDARD
            .decode(data)
            .map_err(|error| anyhow::anyhow!("invalid image attachment base64: {error}"))?;
        Ok(Self {
            media_type: media_type.to_owned(),
            data: data.to_owned(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<MessageContent>,
}

impl Message {
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            content: vec![MessageContent::Text { text: text.into() }],
        }
    }

    pub fn assistant(content: Vec<MessageContent>) -> Self {
        Self {
            role: Role::Assistant,
            content,
        }
    }

    pub fn visible_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                MessageContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn tool_calls(&self) -> Vec<ToolCall> {
        self.content
            .iter()
            .filter_map(|block| match block {
                MessageContent::ToolCall {
                    id,
                    name,
                    arguments,
                } => Some(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                }),
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub run_id: String,
    pub model: String,
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_output_tokens: Option<u32>,
    /// Maximum silence while opening or advancing a streaming response.
    /// Streaming providers should restart this interval after each valid event.
    pub stream_idle_timeout: std::time::Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResponse {
    /// The one completed assistant message returned by the provider.
    ///
    /// Content ordering is provider-significant: opaque continuation items,
    /// visible text, reasoning, and tool calls must remain in wire order.
    pub assistant: Message,
    pub usage: ModelUsage,
}

impl ModelResponse {
    pub fn new(assistant: Message, usage: ModelUsage) -> Self {
        assert_eq!(
            assistant.role,
            Role::Assistant,
            "a completed model response must contain an assistant message"
        );
        Self { assistant, usage }
    }

    /// Validate responses assembled outside [`ModelResponse::new`].
    ///
    /// The runner should call this once after every provider completion so a
    /// custom provider cannot persist a user or tool message as model output.
    pub fn validate_completed(&self) -> Result<()> {
        ensure!(
            self.assistant.role == Role::Assistant,
            "model provider returned a completed response with role `{:?}`; expected `assistant`",
            self.assistant.role
        );
        Ok(())
    }

    pub fn text(&self) -> String {
        self.assistant.visible_text()
    }

    pub fn tool_calls(&self) -> Vec<ToolCall> {
        self.assistant.tool_calls()
    }
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &str;

    /// Stable, non-secret identity for provider settings that affect request
    /// and continuation wire formats. Runs should persist this value and
    /// require an exact match before resume.
    fn resume_fingerprint(&self) -> String {
        stable_resume_fingerprint(self.name(), &[])
    }

    async fn complete(
        &self,
        request: ModelRequest,
        events: SharedEventSink,
    ) -> Result<ModelResponse>;
}

pub(crate) fn stable_resume_fingerprint(provider: &str, fields: &[(&str, &str)]) -> String {
    let payload = serde_json::to_vec(&("picoagent-provider-resume-v1", provider, fields))
        .expect("provider resume fingerprint fields must serialize");
    format!("sha256:{:x}", Sha256::digest(payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_response_validation_rejects_non_assistant_messages() {
        let response = ModelResponse {
            assistant: Message::text(Role::User, "not an assistant response"),
            usage: ModelUsage::default(),
        };

        let error = response.validate_completed().unwrap_err().to_string();
        assert!(error.contains("expected `assistant`"), "{error}");
    }

    #[test]
    fn visible_text_matches_chat_projection_for_multiple_text_blocks() {
        let message = Message::assistant(vec![
            MessageContent::Text {
                text: "first".to_owned(),
            },
            MessageContent::Reasoning {
                text: "between".to_owned(),
            },
            MessageContent::Text {
                text: "second".to_owned(),
            },
        ]);

        assert_eq!(message.visible_text(), "first\nsecond");
    }

    #[test]
    fn default_resume_fingerprint_is_stable_and_provider_specific() {
        let first = stable_resume_fingerprint("one", &[]);
        assert_eq!(first, stable_resume_fingerprint("one", &[]));
        assert_ne!(first, stable_resume_fingerprint("two", &[]));
        assert!(first.starts_with("sha256:"));
    }
}
