use std::{fmt, time::Duration};

use anyhow::Result;
use async_trait::async_trait;
use reqwest::{Client, RequestBuilder, StatusCode};
use serde::{Deserialize, Serialize};

use super::{
    ModelProvider, ModelRequest, ModelResponse,
    common::{http_status, join_url},
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
    rate_limit_retries: usize,
    rate_limit_backoff: Duration,
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
            rate_limit_retries: 3,
            rate_limit_backoff: Duration::from_secs(2),
        }
    }

    pub fn with_client(options: OpenAiCompatibleOptions, client: Client) -> Self {
        Self {
            client,
            options,
            reasoning_effort: None,
            rate_limit_retries: 3,
            rate_limit_backoff: Duration::from_secs(2),
        }
    }

    pub fn with_reasoning_effort(mut self, effort: impl Into<String>) -> Self {
        self.reasoning_effort = Some(effort.into());
        self
    }

    pub fn with_rate_limit_retry(mut self, retries: usize, base_delay: Duration) -> Self {
        self.rate_limit_retries = retries;
        self.rate_limit_backoff = base_delay;
        self
    }
}

#[async_trait]
impl ModelProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        "openai-compatible"
    }

    fn resume_fingerprint(&self) -> String {
        let protocol = match self.options.protocol {
            OpenAiProtocol::Responses => "responses",
            OpenAiProtocol::ChatCompletions => "chat-completions",
        };
        super::stable_resume_fingerprint(
            self.name(),
            &[
                ("base_url", self.options.base_url.trim_end_matches('/')),
                ("protocol", protocol),
                (
                    "reasoning_effort",
                    self.reasoning_effort.as_deref().unwrap_or(""),
                ),
            ],
        )
    }

    async fn complete(
        &self,
        request: ModelRequest,
        events: SharedEventSink,
    ) -> Result<ModelResponse> {
        for attempt in 0..=self.rate_limit_retries {
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
            let result = openai_stream::complete_request(
                bearer(builder, &self.options.api_key),
                self.options.protocol,
                &request.run_id,
                events.clone(),
                request.stream_idle_timeout,
            )
            .await;
            match result {
                Err(error)
                    if attempt < self.rate_limit_retries
                        && http_status(&error) == Some(StatusCode::TOO_MANY_REQUESTS) =>
                {
                    let multiplier = 1_u32.checked_shl(attempt as u32).unwrap_or(u32::MAX);
                    tokio::time::sleep(self.rate_limit_backoff.saturating_mul(multiplier)).await;
                }
                result => return result,
            }
        }
        unreachable!("rate-limit retry loop always returns")
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
