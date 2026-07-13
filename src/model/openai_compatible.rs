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

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleOptions {
    pub base_url: String,
    pub api_key: String,
    pub protocol: OpenAiProtocol,
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
        }
    }

    pub fn with_client(options: OpenAiCompatibleOptions, client: Client) -> Self {
        Self { client, options }
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
                .json(&responses_body(&request)),
            OpenAiProtocol::ChatCompletions => self
                .client
                .post(join_url(&self.options.base_url, "chat/completions"))
                .json(&chat_body(&request)),
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
