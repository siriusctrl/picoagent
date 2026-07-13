use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::events::SharedEventSink;

mod common;
mod openai_oauth_credentials;
mod openai_oauth_device;
mod openai_request;
mod openai_stream;

pub mod anthropic_compatible;
pub mod openai_compatible;
pub mod openai_oauth;

pub use anthropic_compatible::{AnthropicCompatibleOptions, AnthropicCompatibleProvider};
pub use openai_compatible::{OpenAiCompatibleOptions, OpenAiCompatibleProvider, OpenAiProtocol};
pub use openai_oauth::{
    DEFAULT_OPENAI_OAUTH_BASE_URL, DeviceCode, OAuthCredentials, OpenAiOAuthOptions,
    OpenAiOAuthProvider,
};

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
    Text {
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
    },
    /// Provider-owned continuation material, such as encrypted OpenAI reasoning.
    ProviderItem {
        provider: String,
        item: Value,
    },
    BackgroundTaskResult {
        task_id: String,
        name: String,
        status: String,
        content: String,
    },
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
}

#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub run_id: String,
    pub model: String,
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    /// Ordered assistant content used for exact provider continuation replay.
    /// Providers without opaque continuation items may leave this empty.
    pub assistant_content: Vec<MessageContent>,
    pub usage: ModelUsage,
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn complete(
        &self,
        request: ModelRequest,
        events: SharedEventSink,
    ) -> Result<ModelResponse>;
}
