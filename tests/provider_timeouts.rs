use std::{sync::Arc, time::Duration};

use fiasco::{
    events::NoopEventSink,
    model::{
        AnthropicCompatibleProvider, Message, ModelProvider, ModelRequest,
        OpenAiCompatibleProvider, OpenAiProtocol, Role,
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
    time::Instant,
};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

fn request(stream_idle_timeout: Duration) -> ModelRequest {
    ModelRequest {
        run_id: "run-timeout".to_owned(),
        model: "test-model".to_owned(),
        system: String::new(),
        messages: vec![Message::text(Role::User, "hello")],
        tools: Vec::new(),
        max_output_tokens: Some(128),
        stream_idle_timeout,
    }
}

async fn delayed_sse_server(chunks: Vec<(Duration, &'static str)>) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let body_bytes = chunks.iter().map(|(_, chunk)| chunk.len()).sum::<usize>();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 8 * 1024];
        let _ = socket.read(&mut request).await;
        let headers = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {body_bytes}\r\nconnection: close\r\n\r\n"
        );
        if socket.write_all(headers.as_bytes()).await.is_err() {
            return;
        }
        for (delay, chunk) in chunks {
            tokio::time::sleep(delay).await;
            if socket.write_all(chunk.as_bytes()).await.is_err() {
                return;
            }
            let _ = socket.flush().await;
        }
    });
    (format!("http://{address}/v1"), task)
}

#[tokio::test]
async fn openai_stream_idle_timeout_restarts_after_each_valid_event() {
    let chunks = vec![
        (
            Duration::ZERO,
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
        ),
        (
            Duration::from_millis(600),
            "event: response.in_progress\ndata: {\"type\":\"response.in_progress\"}\n\n",
        ),
        (
            Duration::from_millis(600),
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\" world\"}\n\nevent: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{}}\n\ndata: [DONE]\n\n",
        ),
    ];
    let (base_url, server) = delayed_sse_server(chunks).await;
    let provider = OpenAiCompatibleProvider::new(base_url, "test-key", OpenAiProtocol::Responses);
    let started = Instant::now();

    let response = provider
        .complete(request(Duration::from_secs(1)), Arc::new(NoopEventSink))
        .await
        .expect("sub-timeout event gaps should keep the stream alive");

    assert_eq!(response.text(), "hello world");
    assert!(started.elapsed() >= Duration::from_millis(1_100));
    server.await.unwrap();
}

#[tokio::test]
async fn openai_stream_idle_timeout_rejects_a_stalled_event_gap() {
    let chunks = vec![
        (
            Duration::ZERO,
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
        ),
        (
            Duration::from_millis(1_500),
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{}}\n\n",
        ),
    ];
    let (base_url, server) = delayed_sse_server(chunks).await;
    let provider = OpenAiCompatibleProvider::new(base_url, "test-key", OpenAiProtocol::Responses);

    let error = provider
        .complete(request(Duration::from_secs(1)), Arc::new(NoopEventSink))
        .await
        .expect_err("a silent event gap should time out");

    assert!(
        format!("{error:#}").contains("OpenAI stream idle timeout exceeded (1s)"),
        "{error:#}"
    );
    server.abort();
}

#[tokio::test]
async fn anthropic_stream_idle_timeout_covers_response_headers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(1_500))
                .insert_header("content-type", "text/event-stream")
                .set_body_string("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"),
        )
        .mount(&server)
        .await;
    let provider = AnthropicCompatibleProvider::new(format!("{}/v1", server.uri()), "test-key");

    let error = provider
        .complete(request(Duration::from_secs(1)), Arc::new(NoopEventSink))
        .await
        .expect_err("waiting for response headers should use the idle timeout");

    assert!(
        format!("{error:#}")
            .contains("Anthropic response headers exceeded the stream idle timeout (1s)"),
        "{error:#}"
    );
}
