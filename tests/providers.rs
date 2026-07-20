use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use async_trait::async_trait;
use picoagent::{
    events::{EventSink, NoopEventSink, RuntimeEvent, RuntimeEventKind},
    model::{
        AnthropicCompatibleOptions, AnthropicCompatibleProvider, Message, ModelProvider,
        ModelRequest, OAuthCredentials, OpenAiCompatibleOptions, OpenAiCompatibleProvider,
        OpenAiOAuthOptions, OpenAiOAuthProvider, OpenAiProtocol, Role,
    },
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, header, method, path},
};

#[derive(Default)]
struct RecordedDeltas {
    text: Vec<String>,
    reasoning: Vec<String>,
}

#[derive(Default)]
struct RecordingSink(Mutex<RecordedDeltas>);

#[async_trait]
impl EventSink for RecordingSink {
    async fn emit(&self, event: &RuntimeEvent) -> Result<()> {
        let mut recorded = self.0.lock().expect("recording lock poisoned");
        match &event.kind {
            RuntimeEventKind::ModelDelta { text } => recorded.text.push(text.clone()),
            RuntimeEventKind::ModelReasoningDelta { text } => {
                recorded.reasoning.push(text.clone());
            }
            _ => {}
        }
        Ok(())
    }
}

fn request() -> ModelRequest {
    ModelRequest {
        run_id: "run-test".to_owned(),
        model: "test-model".to_owned(),
        system: "Be concise.".to_owned(),
        messages: vec![Message::text(Role::User, "hello")],
        tools: Vec::new(),
        max_output_tokens: Some(128),
        stream_idle_timeout: Duration::from_secs(30),
    }
}

#[tokio::test]
async fn responses_streams_text_and_usage() {
    let server = MockServer::start().await;
    let body = concat!(
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello \"}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"world\"}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":12,\"output_tokens\":2,\"input_tokens_details\":{\"cached_tokens\":8},\"output_tokens_details\":{\"reasoning_tokens\":1}}}}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(body_partial_json(serde_json::json!({
            "reasoning": {"effort": "high"},
            "max_output_tokens": 128
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let options = OpenAiCompatibleOptions::new(
        format!("{}/v1", server.uri()),
        "test-key",
        OpenAiProtocol::Responses,
    );
    let provider = OpenAiCompatibleProvider::with_options(options).with_reasoning_effort("high");
    let events = Arc::new(RecordingSink::default());
    let response = provider
        .complete(request(), events.clone())
        .await
        .expect("response should parse");

    assert_eq!(response.text(), "hello world");
    assert_eq!(response.usage.input_tokens, Some(12));
    assert_eq!(response.usage.output_tokens, Some(2));
    assert_eq!(response.usage.cached_input_tokens, Some(8));
    assert_eq!(response.usage.reasoning_tokens, Some(1));
    let recorded = events.0.lock().expect("recording lock poisoned");
    assert_eq!(recorded.text, ["hello ", "world"]);
    assert!(recorded.reasoning.is_empty());
}

#[tokio::test]
async fn chat_stream_reassembles_fragmented_tool_arguments_and_supplies_missing_id() {
    let server = MockServer::start().await;
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"inspect \"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"\",\"content\":\"\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"first\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"checking \"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"type\":\"function\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"pa\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"th\\\":\\\"README.md\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":8,\"completion_tokens_details\":{\"reasoning_tokens\":7}}}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(serde_json::json!({
            "reasoning_effort": "low",
            "max_completion_tokens": 128
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let options = OpenAiCompatibleOptions::new(
        format!("{}/v1", server.uri()),
        "test-key",
        OpenAiProtocol::ChatCompletions,
    );
    let provider = OpenAiCompatibleProvider::with_options(options).with_reasoning_effort("low");
    let events = Arc::new(RecordingSink::default());
    let response = provider
        .complete(request(), events.clone())
        .await
        .expect("response should parse");

    let tool_calls = response.tool_calls();
    assert_eq!(tool_calls.len(), 1);
    assert!(tool_calls[0].id.starts_with("call_"));
    assert_eq!(tool_calls[0].name, "read");
    assert_eq!(tool_calls[0].arguments["path"], "README.md");
    assert_eq!(response.text(), "checking ");
    assert_eq!(response.usage.reasoning_tokens, Some(7));
    assert!(matches!(
        &response.assistant.content[0],
        picoagent::model::MessageContent::Reasoning { text } if text == "inspect first"
    ));
    assert!(matches!(
        &response.assistant.content[1],
        picoagent::model::MessageContent::Text { text } if text == "checking "
    ));
    assert!(matches!(
        &response.assistant.content[2],
        picoagent::model::MessageContent::ToolCall { id, .. } if id == &tool_calls[0].id
    ));
    let recorded = events.0.lock().expect("recording lock poisoned");
    assert_eq!(recorded.reasoning, ["inspect ", "first"]);
    assert_eq!(recorded.text, ["checking "]);
}

#[tokio::test]
async fn chat_stream_preserves_provider_tool_call_id() {
    let server = MockServer::start().await;
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_provider_1\",\"type\":\"function\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"README.md\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let options = OpenAiCompatibleOptions::new(
        format!("{}/v1", server.uri()),
        "test-key",
        OpenAiProtocol::ChatCompletions,
    );
    let response = OpenAiCompatibleProvider::with_options(options)
        .complete(request(), Arc::new(NoopEventSink))
        .await
        .expect("response should parse");

    assert_eq!(response.tool_calls()[0].id, "call_provider_1");
    assert!(matches!(
        &response.assistant.content[0],
        picoagent::model::MessageContent::ToolCall { id, .. } if id == "call_provider_1"
    ));
}

#[tokio::test]
async fn anthropic_stream_reassembles_fragmented_tool_input() {
    let server = MockServer::start().await;
    let body = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":9,\"cache_read_input_tokens\":3}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"checking\"}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"bash\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"com\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"mand\\\":\\\"cargo test\\\"}\"}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":7}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let provider = AnthropicCompatibleProvider::new(format!("{}/v1", server.uri()), "test-key");
    let response = provider
        .complete(request(), Arc::new(NoopEventSink))
        .await
        .expect("response should parse");

    assert_eq!(response.text(), "checking");
    assert_eq!(response.tool_calls()[0].name, "bash");
    assert_eq!(response.tool_calls()[0].arguments["command"], "cargo test");
    assert_eq!(response.usage.input_tokens, Some(9));
    assert_eq!(response.usage.output_tokens, Some(7));
    assert_eq!(response.usage.cached_input_tokens, Some(3));
}

#[tokio::test]
async fn provider_errors_include_http_status_and_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(
        format!("{}/v1", server.uri()),
        "test-key",
        OpenAiProtocol::Responses,
    )
    .with_rate_limit_retry(0, Duration::ZERO);
    let error = provider
        .complete(request(), Arc::new(NoopEventSink))
        .await
        .expect_err("429 must fail")
        .to_string();

    assert!(error.contains("429"), "{error}");
    assert!(error.contains("rate limited"), "{error}");
}

#[tokio::test]
async fn openai_compatible_retries_initial_rate_limits() {
    let server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));
    let responder_attempts = attempts.clone();
    let success = concat!(
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{}}\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(move |_: &wiremock::Request| {
            if responder_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(429).set_body_string("rate limited")
            } else {
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(success)
            }
        })
        .expect(2)
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(
        format!("{}/v1", server.uri()),
        "test-key",
        OpenAiProtocol::Responses,
    )
    .with_rate_limit_retry(1, Duration::ZERO);
    let response = provider
        .complete(request(), Arc::new(NoopEventSink))
        .await
        .unwrap();

    assert_eq!(response.text(), "ok");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn openai_compatible_does_not_retry_a_non_429_body_that_mentions_429() {
    let server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));
    let responder_attempts = attempts.clone();
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(move |_: &wiremock::Request| {
            responder_attempts.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(500).set_body_string("upstream returned HTTP 429")
        })
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(
        format!("{}/v1", server.uri()),
        "test-key",
        OpenAiProtocol::Responses,
    )
    .with_rate_limit_retry(1, Duration::ZERO);
    let error = provider
        .complete(request(), Arc::new(NoopEventSink))
        .await
        .expect_err("HTTP 500 must not be retried as a rate limit");

    assert!(error.to_string().contains("HTTP 500"));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[test]
fn provider_resume_fingerprints_are_stable_wire_identities_without_credentials() {
    let compatible_a = OpenAiCompatibleProvider::new(
        "https://example.test/v1/",
        "secret-a",
        OpenAiProtocol::Responses,
    );
    let compatible_b = OpenAiCompatibleProvider::new(
        "https://example.test/v1",
        "secret-b",
        OpenAiProtocol::Responses,
    );
    let chat = OpenAiCompatibleProvider::new(
        "https://example.test/v1",
        "secret-a",
        OpenAiProtocol::ChatCompletions,
    );
    let reasoning = OpenAiCompatibleProvider::new(
        "https://example.test/v1",
        "secret-a",
        OpenAiProtocol::Responses,
    )
    .with_reasoning_effort("high");

    let fingerprint = compatible_a.resume_fingerprint();
    assert_eq!(fingerprint, compatible_b.resume_fingerprint());
    assert_ne!(fingerprint, chat.resume_fingerprint());
    assert_ne!(fingerprint, reasoning.resume_fingerprint());
    assert!(!fingerprint.contains("secret-a"));
    assert!(!fingerprint.contains("secret-b"));

    let anthropic_a =
        AnthropicCompatibleProvider::new("https://anthropic.example/v1/", "anthropic-secret-a");
    let anthropic_b =
        AnthropicCompatibleProvider::new("https://anthropic.example/v1", "anthropic-secret-b");
    assert_eq!(
        anthropic_a.resume_fingerprint(),
        anthropic_b.resume_fingerprint()
    );
    let mut older_anthropic_options =
        AnthropicCompatibleOptions::new("https://anthropic.example/v1", "anthropic-secret-a");
    older_anthropic_options.anthropic_version = "2022-01-01".to_owned();
    let older_anthropic = AnthropicCompatibleProvider::with_options(older_anthropic_options);
    assert_ne!(
        anthropic_a.resume_fingerprint(),
        older_anthropic.resume_fingerprint()
    );

    let first_home = tempfile::tempdir().unwrap();
    let second_home = tempfile::tempdir().unwrap();
    let oauth_a = OpenAiOAuthProvider::new(
        "https://oauth.example/v1/",
        first_home.path().join("auth.json"),
    );
    let oauth_b = OpenAiOAuthProvider::new(
        "https://oauth.example/v1",
        second_home.path().join("auth.json"),
    );
    assert_eq!(oauth_a.resume_fingerprint(), oauth_b.resume_fingerprint());
}

#[tokio::test]
async fn responses_rejects_incomplete_generation() {
    let server = MockServer::start().await;
    let body = concat!(
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
        "event: response.incomplete\n",
        "data: {\"type\":\"response.incomplete\",\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(
        format!("{}/v1", server.uri()),
        "test-key",
        OpenAiProtocol::Responses,
    );
    let error = provider
        .complete(request(), Arc::new(NoopEventSink))
        .await
        .expect_err("incomplete responses must not be persisted as successful")
        .to_string();
    assert!(error.contains("max_output_tokens"), "{error}");
}

#[tokio::test]
async fn chat_rejects_length_finish_reason() {
    let server = MockServer::start().await;
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(
        format!("{}/v1", server.uri()),
        "test-key",
        OpenAiProtocol::ChatCompletions,
    );
    let error = provider
        .complete(request(), Arc::new(NoopEventSink))
        .await
        .expect_err("length-limited chat completions must fail")
        .to_string();
    assert!(error.contains("length"), "{error}");
}

#[tokio::test]
async fn anthropic_rejects_max_tokens_stop_reason() {
    let server = MockServer::start().await;
    let body = concat!(
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let provider = AnthropicCompatibleProvider::new(format!("{}/v1", server.uri()), "test-key");
    let error = provider
        .complete(request(), Arc::new(NoopEventSink))
        .await
        .expect_err("max_tokens must not be treated as a complete answer")
        .to_string();
    assert!(error.contains("max_tokens"), "{error}");
}

#[tokio::test]
async fn oauth_refreshes_once_after_unauthorized_and_retries() {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    let auth_path = home.path().join("auth.json");
    let credentials = OAuthCredentials {
        access_token: "stale-token".into(),
        refresh_token: "refresh-token".into(),
        expires_at: u64::MAX,
        account_id: Some("account-1".into()),
    };
    tokio::fs::write(&auth_path, serde_json::to_vec(&credentials).unwrap())
        .await
        .unwrap();

    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(header("authorization", "Bearer stale-token"))
        .respond_with(ResponseTemplate::new(401).set_body_string("expired"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "fresh-token",
            "refresh_token": "fresh-refresh-token",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;
    let success = concat!(
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"ok\"}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{}}\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(header("authorization", "Bearer fresh-token"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(success),
        )
        .expect(1)
        .mount(&server)
        .await;

    let mut options = OpenAiOAuthOptions::new(format!("{}/v1", server.uri()), &auth_path);
    options.auth_base_url = server.uri();
    options.codex_auth_path = None;
    let provider = OpenAiOAuthProvider::with_options(options);
    let response = provider
        .complete(request(), Arc::new(NoopEventSink))
        .await
        .unwrap();
    assert_eq!(response.text(), "ok");
    let stored: OAuthCredentials =
        serde_json::from_slice(&tokio::fs::read(auth_path).await.unwrap()).unwrap();
    assert_eq!(stored.access_token, "fresh-token");
    assert_eq!(stored.account_id.as_deref(), Some("account-1"));
}
