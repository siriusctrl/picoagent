use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::{Result, bail, ensure};
use async_trait::async_trait;
use picoagent::{
    agent::runner::{AgentRunner, AgentRunnerConfig, RunnerOptions},
    artifact::ArtifactStore,
    events::{NoopEventSink, SharedEventSink},
    hooks::HookPipeline,
    model::{
        Message, MessageContent, ModelProvider, ModelRequest, ModelResponse, ModelUsage, Role,
    },
    storage::{RunDirStore, RunRecord, RunState},
    tools::ToolRegistry,
};
use serde_json::json;
use tempfile::TempDir;

const PARENT_RUN_ID: &str = "delegate-crash-parent";
const CHILD_RUN_ID: &str = "delegate-crash-child";
const DELEGATE_CALL_ID: &str = "delegate-crash-call";

struct CrashRecoveryProvider {
    parent_calls: Arc<AtomicUsize>,
    child_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ModelProvider for CrashRecoveryProvider {
    fn name(&self) -> &str {
        "delegate-crash-recovery"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        if first_user_text(&request) == "child work" {
            self.child_calls.fetch_add(1, Ordering::SeqCst);
            return Ok(text_response("child result"));
        }

        self.parent_calls.fetch_add(1, Ordering::SeqCst);
        let acknowledgements = request
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|content| match content {
                MessageContent::ToolResult {
                    call_id,
                    content,
                    is_error,
                    ..
                } if call_id == DELEGATE_CALL_ID => Some((content, is_error)),
                _ => None,
            })
            .collect::<Vec<_>>();
        ensure!(
            acknowledgements.len() == 1,
            "expected exactly one recovered delegate acknowledgement"
        );
        let (acknowledgement, is_error) = acknowledgements[0];
        if *is_error
            || !acknowledgement.contains("<background_task task_id=\"t1\"")
            || !acknowledgement.contains("name=\"inspect_child\"")
            || acknowledgement.contains("status=")
        {
            bail!("recovered delegate acknowledgement is invalid: {acknowledgement}");
        }

        let terminal_results = request
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter(|content| {
                matches!(
                    content,
                    MessageContent::BackgroundTask {
                        task_id,
                        status: Some(status),
                        ..
                    } if task_id == "t1" && status == "completed"
                )
            })
            .count();
        ensure!(terminal_results <= 1, "delegate result was delivered twice");
        Ok(text_response(if terminal_results == 1 {
            "parent consumed child result"
        } else {
            "parent waiting for child"
        }))
    }
}

#[tokio::test]
async fn resume_recovers_delegate_ack_child_and_delivery_exactly_once() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let parent_calls = Arc::new(AtomicUsize::new(0));
    let child_calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(CrashRecoveryProvider {
        parent_calls: parent_calls.clone(),
        child_calls: child_calls.clone(),
    });
    create_crash_window(&store, workspace.path(), provider.as_ref()).await;

    let runner = AgentRunner::new(AgentRunnerConfig {
        provider,
        model: "scripted".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools: ToolRegistry::default(),
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions {
            max_parallel_model_calls: 2,
            task_wait_timeout_seconds: 1,
            ..RunnerOptions::default()
        },
    });

    let result = runner.resume(PARENT_RUN_ID).await.unwrap();
    assert_eq!(result.final_output, "parent consumed child result");
    assert_eq!(child_calls.load(Ordering::SeqCst), 1);
    assert!((1..=2).contains(&parent_calls.load(Ordering::SeqCst)));
    assert_eq!(
        store.load_run(CHILD_RUN_ID).await.unwrap().state,
        RunState::Completed
    );

    let messages = store.load_messages(PARENT_RUN_ID).await.unwrap();
    assert_eq!(
        messages
            .iter()
            .flat_map(|message| &message.content)
            .filter(|content| matches!(
                content,
                MessageContent::ToolResult { call_id, .. } if call_id == DELEGATE_CALL_ID
            ))
            .count(),
        1
    );
    assert!(
        !messages
            .iter()
            .any(|message| message.visible_text().contains("side effects are unknown"))
    );
    let terminal = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|content| match content {
            MessageContent::BackgroundTask {
                task_id,
                status: Some(status),
                content,
                ..
            } if task_id == "t1" => Some((status, content)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].0, "completed");
    assert_eq!(terminal[0].1, "child result");

    let task: serde_json::Value = serde_json::from_slice(
        &tokio::fs::read(store.paths(PARENT_RUN_ID).directory.join("tasks/t1.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(task["state"], "completed");
    assert_eq!(task["origin_call_id"], DELEGATE_CALL_ID);
    let run_count = std::fs::read_dir(workspace.path().join(".pico/runs"))
        .unwrap()
        .count();
    assert_eq!(run_count, 2);

    let events = tokio::fs::read_to_string(store.paths(PARENT_RUN_ID).events)
        .await
        .unwrap();
    assert_eq!(events.matches("\"type\":\"subagent_completed\"").count(), 1);
    assert_eq!(
        events
            .matches("\"type\":\"background_task_delivered\"")
            .count(),
        1
    );
}

async fn create_crash_window(
    store: &RunDirStore,
    workspace: &Path,
    provider: &CrashRecoveryProvider,
) {
    store
        .create_run(
            &RunRecord::new(
                PARENT_RUN_ID,
                "parent work",
                provider.name(),
                "scripted",
                workspace.to_path_buf(),
                None,
            )
            .with_provider_resume_fingerprint(provider.resume_fingerprint()),
        )
        .await
        .unwrap();
    store
        .update_state(PARENT_RUN_ID, RunState::Running)
        .await
        .unwrap();
    store
        .append_message(PARENT_RUN_ID, &Message::text(Role::User, "parent work"))
        .await
        .unwrap();
    store
        .append_checkpoint(
            PARENT_RUN_ID,
            &[
                Message::assistant(vec![MessageContent::ToolCall {
                    id: DELEGATE_CALL_ID.to_owned(),
                    name: "delegate".to_owned(),
                    arguments: json!({
                        "name": "inspect_child",
                        "prompt": "child work"
                    })
                    .into(),
                }]),
                Message {
                    role: Role::Tool,
                    content: vec![MessageContent::ToolResult {
                        call_id: DELEGATE_CALL_ID.to_owned(),
                        content: "<runtime-reminder>\n<background_task task_id=\"t1\" name=\"inspect_child\">\nThe task is now running in the background.\n</background_task>\n</runtime-reminder>".to_owned(),
                        is_error: false,
                        metadata: picoagent::artifact::ResultMetadata::empty(),
                    }],
                },
            ],
        )
        .await
        .unwrap();

    store
        .create_run(
            &RunRecord::new(
                CHILD_RUN_ID,
                "child work",
                provider.name(),
                "scripted",
                workspace.to_path_buf(),
                Some(PARENT_RUN_ID.to_owned()),
            )
            .with_execution_context("general_task_leaf", 1, None, 0)
            .with_provider_resume_fingerprint(provider.resume_fingerprint()),
        )
        .await
        .unwrap();
    store
        .update_state(CHILD_RUN_ID, RunState::Running)
        .await
        .unwrap();
    store
        .append_message(CHILD_RUN_ID, &Message::text(Role::User, "child work"))
        .await
        .unwrap();

    let tasks = store.paths(PARENT_RUN_ID).directory.join("tasks");
    tokio::fs::create_dir_all(&tasks).await.unwrap();
    tokio::fs::write(
        tasks.join("t1.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 10,
            "id": "t1",
            "kind": "agent",
            "name": "inspect_child",
            "origin_call_id": DELEGATE_CALL_ID,
            "state": "running",
            "result": null,
            "error": null,
            "child_run_id": CHILD_RUN_ID,
            "child_remaining_delegation_depth": 0,
            "prompt": "child work",
            "created_at": chrono::Utc::now() - chrono::Duration::seconds(1)
        }))
        .unwrap(),
    )
    .await
    .unwrap();
}

fn first_user_text(request: &ModelRequest) -> &str {
    request
        .messages
        .iter()
        .find(|message| message.role == Role::User)
        .and_then(|message| {
            message.content.iter().find_map(|content| match content {
                MessageContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
        })
        .unwrap_or_default()
}

fn text_response(text: &str) -> ModelResponse {
    ModelResponse::new(Message::text(Role::Assistant, text), ModelUsage::default())
}
