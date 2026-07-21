use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;

use super::create_run;
use crate::{
    agent::{
        runner::AgentRunner,
        types::{AgentRunnerConfig, RunnerOptions},
    },
    artifact::ArtifactStore,
    events::{NoopEventSink, SharedEventSink},
    hooks::HookPipeline,
    model::{
        Message, MessageContent, ModelProvider, ModelRequest, ModelResponse, ModelUsage, Role,
    },
    storage::{DelegateContext, RunDirStore, RunRecord, RunState},
    tools::ToolRegistry,
    trajectory::{CompactionMessage, CompactionState},
};

#[derive(Default)]
struct CapturingResumeProvider {
    requests: Mutex<Vec<ModelRequest>>,
}

#[async_trait]
impl ModelProvider for CapturingResumeProvider {
    fn name(&self) -> &str {
        "capturing-resume"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _events: SharedEventSink,
    ) -> Result<ModelResponse> {
        self.requests.lock().unwrap().push(request);
        Ok(ModelResponse::new(
            Message::text(Role::Assistant, "resumed fork completed"),
            ModelUsage::default(),
        ))
    }
}

#[tokio::test]
async fn resumed_compacted_fork_pins_its_exact_local_assignment_without_persisting_a_copy() {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    create_run(&store, "parent", None).await;
    let inherited = store
        .append_message(
            "parent",
            &Message::text(Role::User, "root workflow must edit source files"),
        )
        .await
        .unwrap();
    let provider = Arc::new(CapturingResumeProvider::default());
    store
        .create_run(
            &RunRecord::new(
                "fork-child",
                "inspect only; do not edit",
                provider.name(),
                "test-model",
                store.workspace().to_path_buf(),
                Some("parent".to_owned()),
            )
            .with_execution_context("general_task_leaf", 1, None, 0)
            .with_delegate_context(DelegateContext::Fork, Some(1))
            .with_provider_resume_fingerprint(provider.resume_fingerprint()),
        )
        .await
        .unwrap();
    store
        .append_forked_message("fork-child", &inherited)
        .await
        .unwrap();
    let assignment = Message {
        role: Role::User,
        content: vec![
            MessageContent::RuntimeReminder {
                text: "<runtime-reminder>fork child</runtime-reminder>".to_owned(),
            },
            MessageContent::Text {
                text: "inspect only; do not edit".to_owned(),
            },
        ],
    };
    store
        .append_message("fork-child", &assignment)
        .await
        .unwrap();
    store
        .append_message(
            "fork-child",
            &Message::text(Role::Assistant, "old child work"),
        )
        .await
        .unwrap();
    store
        .append_message(
            "fork-child",
            &Message::text(Role::Assistant, "recent child work"),
        )
        .await
        .unwrap();
    store
        .append_compaction_message(
            "fork-child",
            &Message::text(Role::User, "compact now"),
            CompactionMessage::Request,
        )
        .await
        .unwrap();
    store
        .append_compaction_message(
            "fork-child",
            &Message::text(
                Role::Assistant,
                "# Compacted state\nroot workflow summarized",
            ),
            CompactionMessage::State {
                state: CompactionState {
                    covered_through_message_ref: "m3".to_owned(),
                    first_kept_message_ref: "m4".to_owned(),
                },
            },
        )
        .await
        .unwrap();
    store
        .update_state("fork-child", RunState::Running)
        .await
        .unwrap();

    let runner = AgentRunner::new(AgentRunnerConfig {
        provider: provider.clone(),
        model: "test-model".to_owned(),
        workspace: workspace.path().to_path_buf(),
        skill_catalog: String::new(),
        tools: ToolRegistry::default(),
        artifacts: ArtifactStore::default(),
        store: store.clone(),
        hooks: HookPipeline::new(),
        memory: None,
        extra_events: Arc::new(NoopEventSink),
        options: RunnerOptions::default(),
    });
    let result = runner
        .resume_child("fork-child".to_owned(), "parent")
        .await
        .unwrap();
    assert_eq!(result.final_output, "resumed fork completed");

    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let messages = &requests[0].messages;
    assert_eq!(
        messages[0].visible_text(),
        "root workflow must edit source files"
    );
    assert_eq!(
        messages[1].visible_text(),
        "# Compacted state\nroot workflow summarized"
    );
    assert!(messages[2].content.iter().any(|content| {
        matches!(content, MessageContent::RuntimeReminder { text } if text.contains("not a final answer"))
    }));
    assert_eq!(
        serde_json::to_value(&messages[3]).unwrap(),
        serde_json::to_value(&assignment).unwrap()
    );
    assert_eq!(messages[4].visible_text(), "recent child work");
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.content.iter().any(|content| {
                matches!(content, MessageContent::Text { text } if text == "inspect only; do not edit")
            }))
            .count(),
        1
    );
    drop(requests);

    let persisted = store.load_trajectory("fork-child").await.unwrap();
    assert_eq!(persisted.len(), 7);
    assert_eq!(
        persisted
            .iter()
            .filter(|record| {
                record.compaction.is_none()
                    && record.message.content.iter().any(|content| {
                        matches!(content, MessageContent::Text { text } if text == "inspect only; do not edit")
                    })
            })
            .count(),
        1
    );
    assert!(!persisted.iter().any(|record| {
        record.compaction.is_none()
            && record.message.content.iter().any(|content| {
                matches!(content, MessageContent::RuntimeReminder { text } if text.contains("not a final answer"))
            })
    }));
}
