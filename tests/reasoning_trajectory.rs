use std::sync::Arc;

use picoagent::{
    agent::runner::{AgentRunner, AgentRunnerConfig, RunRequest, RunnerOptions},
    artifact::ArtifactStore,
    events::NoopEventSink,
    hooks::HookPipeline,
    model::{MessageContent, OpenAiCompatibleOptions, OpenAiCompatibleProvider, OpenAiProtocol},
    storage::RunDirStore,
    tools::ToolRegistry,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, method, path},
};

#[tokio::test]
async fn chat_reasoning_is_persisted_as_a_separate_trajectory_channel() {
    let server = MockServer::start().await;
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"inspect \"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"\",\"content\":\"\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"first\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"PICO_REASONING_OK\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":7,\"completion_tokens_details\":{\"reasoning_tokens\":5}}}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(json!({
            "reasoning_effort": "high",
            "max_completion_tokens": 128
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .expect(1)
        .mount(&server)
        .await;

    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let options = OpenAiCompatibleOptions::new(
        format!("{}/v1", server.uri()),
        "test-key",
        OpenAiProtocol::ChatCompletions,
    );
    let provider = OpenAiCompatibleProvider::with_options(options).with_reasoning_effort("high");
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: Arc::new(provider),
        model: "reasoning-model".into(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools: ToolRegistry::default(),
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions {
            max_output_tokens: Some(128),
            ..RunnerOptions::default()
        },
    });

    let result = runner
        .run(RunRequest::root("test reasoning"))
        .await
        .unwrap();
    assert_eq!(result.final_output, "PICO_REASONING_OK");

    let messages = store.load_messages(&result.run_id).await.unwrap();
    assert!(matches!(
        &messages[1].content[0],
        MessageContent::Reasoning { text } if text == "inspect first"
    ));
    assert!(matches!(
        &messages[1].content[1],
        MessageContent::Text { text } if text == "PICO_REASONING_OK"
    ));

    let paths = store.paths(&result.run_id);
    let events = tokio::fs::read_to_string(paths.events).await.unwrap();
    let events: Vec<Value> = events
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "model_reasoning_delta")
            .map(|event| event["text"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["inspect ", "first"]
    );
    assert!(events.iter().all(|event| {
        event["type"] != "model_delta"
            || event["text"].as_str().is_some_and(|text| !text.is_empty())
    }));
    let completed = events
        .iter()
        .find(|event| event["type"] == "model_completed")
        .unwrap();
    assert_eq!(completed["reasoning_tokens"], 5);
    assert_eq!(
        tokio::fs::read_to_string(paths.final_output).await.unwrap(),
        "PICO_REASONING_OK"
    );
}
