use std::fmt;

use anyhow::Result;
use async_trait::async_trait;
use reqwest::{Client, RequestBuilder};
use serde::{Deserialize, Serialize};

use super::{
    ModelProvider, ModelRequest, ModelResponse,
    common::join_url,
    openai_request::{chat_body, responses_body},
    openai_stream,
};
use crate::events::SharedEventSink;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OpenAiProtocol {
    #[default]
    Responses,
    ChatCompletions,
}

#[derive(Clone)]
pub struct OpenAiCompatibleOptions {
    pub base_url: String,
    pub api_key: String,
    pub protocol: OpenAiProtocol,
}

impl fmt::Debug for OpenAiCompatibleOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiCompatibleOptions")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("protocol", &self.protocol)
            .finish()
    }
}

impl OpenAiCompatibleOptions {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        protocol: OpenAiProtocol,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            protocol,
        }
    }
}

#[derive(Clone)]
pub struct OpenAiCompatibleProvider {
    client: Client,
    options: OpenAiCompatibleOptions,
    reasoning_effort: Option<String>,
}

impl OpenAiCompatibleProvider {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        protocol: OpenAiProtocol,
    ) -> Self {
        Self::with_options(OpenAiCompatibleOptions::new(base_url, api_key, protocol))
    }

    pub fn with_options(options: OpenAiCompatibleOptions) -> Self {
        Self {
            client: Client::new(),
            options,
            reasoning_effort: None,
        }
    }

    pub fn with_client(options: OpenAiCompatibleOptions, client: Client) -> Self {
        Self {
            client,
            options,
            reasoning_effort: None,
        }
    }

    pub fn with_reasoning_effort(mut self, effort: impl Into<String>) -> Self {
        self.reasoning_effort = Some(effort.into());
        self
    }
}

#[async_trait]
impl ModelProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        "openai-compatible"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let builder = match self.options.protocol {
            OpenAiProtocol::Responses => self
                .client
                .post(join_url(&self.options.base_url, "responses"))
                .json(&responses_body(&request, self.reasoning_effort.as_deref())),
            OpenAiProtocol::ChatCompletions => self
                .client
                .post(join_url(&self.options.base_url, "chat/completions"))
                .json(&chat_body(&request, self.reasoning_effort.as_deref())),
        };
        openai_stream::complete_request(
            bearer(builder, &self.options.api_key),
            self.options.protocol,
            &request.run_id,
            events,
        )
        .await
    }
}

fn bearer(builder: RequestBuilder, token: &str) -> RequestBuilder {
    if token.trim().is_empty() {
        builder
    } else {
        builder.bearer_auth(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_debug_redacts_resolved_api_key() {
        let options = OpenAiCompatibleOptions::new(
            "https://example.test/v1",
            "super-secret-token",
            OpenAiProtocol::Responses,
        );

        let debug = format!("{options:?}");
        assert!(!debug.contains("super-secret-token"));
        assert!(debug.contains("[REDACTED]"));
    }
}
