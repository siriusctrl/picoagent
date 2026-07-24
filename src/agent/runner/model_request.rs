use std::time::Duration;

use anyhow::{Context, Result};

use crate::{
    events::{RuntimeEvent, RuntimeEventKind, SharedEventSink},
    model::{Message, MessageContent, ModelRequest, ModelResponse, Role},
};

use super::AgentRunner;

const INCOMPLETE_REPAIR_REMINDER: &str = "<runtime-reminder>\nThe previous model response ended before completion and was discarded. Retry the intended assistant response completely; do not continue partial text. Emit complete JSON for every tool call.\n</runtime-reminder>";

impl AgentRunner {
    pub(super) async fn complete_model_step(
        &self,
        run_id: &str,
        step: usize,
        mut request: ModelRequest,
        events: SharedEventSink,
    ) -> Result<ModelResponse> {
        for attempt in 0..=1 {
            let model_permit = self
                .model_slots
                .acquire()
                .await
                .context("model concurrency limiter closed")?;
            events
                .emit(&RuntimeEvent::new(
                    run_id,
                    RuntimeEventKind::ModelStarted { step },
                ))
                .await?;
            let response = tokio::time::timeout(
                Duration::from_secs(self.options.model_request_deadline_seconds),
                self.provider.complete(request.clone(), events.clone()),
            )
            .await;

            let response = match response {
                Ok(Ok(response)) => response,
                Ok(Err(error)) => {
                    let should_repair =
                        attempt == 0 && crate::model::is_incomplete_response(&error);
                    let usage = crate::model::incomplete_response_usage(&error)
                        .cloned()
                        .unwrap_or_default();
                    let error =
                        error.context(format!("{} model call failed", self.provider.name()));
                    events
                        .emit(&RuntimeEvent::new(
                            run_id,
                            RuntimeEventKind::ModelFailed {
                                step,
                                error: format!("{error:#}"),
                                input_tokens: usage.input_tokens,
                                output_tokens: usage.output_tokens,
                                cached_input_tokens: usage.cached_input_tokens,
                                reasoning_tokens: usage.reasoning_tokens,
                            },
                        ))
                        .await?;
                    drop(model_permit);
                    if should_repair {
                        request.messages.push(Message::new(
                            Role::User,
                            vec![MessageContent::RuntimeReminder {
                                text: INCOMPLETE_REPAIR_REMINDER.to_owned(),
                            }],
                        ));
                        continue;
                    }
                    return Err(error);
                }
                Err(_) => {
                    let error = anyhow::anyhow!(
                        "{} model request deadline exceeded {} seconds",
                        self.provider.name(),
                        self.options.model_request_deadline_seconds
                    );
                    events
                        .emit(&RuntimeEvent::new(
                            run_id,
                            RuntimeEventKind::ModelFailed {
                                step,
                                error: error.to_string(),
                                input_tokens: None,
                                output_tokens: None,
                                cached_input_tokens: None,
                                reasoning_tokens: None,
                            },
                        ))
                        .await?;
                    drop(model_permit);
                    return Err(error);
                }
            };
            if let Err(error) = response.validate_completed() {
                events
                    .emit(&RuntimeEvent::new(
                        run_id,
                        RuntimeEventKind::ModelFailed {
                            step,
                            error: format!("{error:#}"),
                            input_tokens: None,
                            output_tokens: None,
                            cached_input_tokens: None,
                            reasoning_tokens: None,
                        },
                    ))
                    .await?;
                drop(model_permit);
                return Err(error);
            }
            events
                .emit(&RuntimeEvent::new(
                    run_id,
                    RuntimeEventKind::ModelCompleted {
                        step,
                        input_tokens: response.usage.input_tokens,
                        output_tokens: response.usage.output_tokens,
                        cached_input_tokens: response.usage.cached_input_tokens,
                        reasoning_tokens: response.usage.reasoning_tokens,
                    },
                ))
                .await?;
            drop(model_permit);
            return Ok(response);
        }
        unreachable!("incomplete response repair loop always returns")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use anyhow::Result;
    use async_trait::async_trait;
    use tempfile::tempdir;

    use crate::{
        agent::runner::{AgentRunnerConfig, RunRequest, RunnerOptions},
        artifact::ArtifactStore,
        events::{NoopEventSink, SharedEventSink},
        hooks::HookPipeline,
        model::{Message, ModelProvider, ModelRequest, ModelResponse, ModelUsage, Role},
        storage::RunDirStore,
        tools::ToolRegistry,
    };

    use super::*;

    struct RepairProvider {
        failures: usize,
        calls: AtomicUsize,
        requests: Mutex<Vec<ModelRequest>>,
    }

    #[async_trait]
    impl ModelProvider for RepairProvider {
        fn name(&self) -> &str {
            "repair-test"
        }

        async fn complete(
            &self,
            request: ModelRequest,
            _events: SharedEventSink,
        ) -> Result<ModelResponse> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            self.requests.lock().unwrap().push(request);
            if call < self.failures {
                return Err(crate::model::incomplete_response_with_usage(
                    "test provider",
                    "output limit",
                    ModelUsage {
                        input_tokens: Some(100),
                        output_tokens: Some(20),
                        cached_input_tokens: Some(80),
                        reasoning_tokens: Some(10),
                    },
                ));
            }
            Ok(ModelResponse::new(
                Message::text(Role::Assistant, "complete"),
                ModelUsage::default(),
            ))
        }
    }

    fn runner(
        workspace: &std::path::Path,
        store: &RunDirStore,
        provider: Arc<RepairProvider>,
    ) -> Arc<AgentRunner> {
        AgentRunner::new(AgentRunnerConfig {
            provider,
            model: "test".to_owned(),
            workspace: workspace.to_owned(),
            skill_catalog: String::new(),
            tools: ToolRegistry::default(),
            artifacts: ArtifactStore::default(),
            store: store.clone(),
            hooks: HookPipeline::new(),
            memory: None,
            extra_events: Arc::new(NoopEventSink),
            options: RunnerOptions::default(),
        })
    }

    #[tokio::test]
    async fn retries_one_incomplete_response_with_an_ephemeral_tail_reminder() {
        let workspace = tempdir().unwrap();
        let store = RunDirStore::new(workspace.path());
        let provider = Arc::new(RepairProvider {
            failures: 1,
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        });
        let runner = runner(workspace.path(), &store, provider.clone());

        let result = runner.run(RunRequest::root("finish once")).await.unwrap();
        assert_eq!(result.final_output, "complete");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
        {
            let requests = provider.requests.lock().unwrap();
            assert_eq!(requests[0].system, requests[1].system);
            assert_eq!(requests[0].tools.len(), requests[1].tools.len());
            assert_eq!(requests[1].messages.len(), requests[0].messages.len() + 1);
            assert!(matches!(
                requests[1].messages.last().unwrap().content.as_slice(),
                [MessageContent::RuntimeReminder { text }]
                    if text.contains("previous model response ended before completion")
            ));
        }

        let messages = store.load_messages(&result.run_id).await.unwrap();
        assert_eq!(messages.len(), 2);
        assert!(!messages.iter().any(|message| {
            message
                .visible_text()
                .contains("previous model response ended before completion")
        }));
        let events = tokio::fs::read_to_string(store.paths(&result.run_id).events)
            .await
            .unwrap();
        let lifecycle = events
            .lines()
            .filter_map(|line| {
                let event: serde_json::Value = serde_json::from_str(line).unwrap();
                match event["type"].as_str() {
                    Some("model_started" | "model_failed" | "model_completed") => {
                        event["type"].as_str().map(str::to_owned)
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(
            lifecycle,
            [
                "model_started",
                "model_failed",
                "model_started",
                "model_completed"
            ]
        );
        let failed: serde_json::Value = events
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .find(|event: &serde_json::Value| event["type"] == "model_failed")
            .unwrap();
        assert_eq!(failed["input_tokens"], 100);
        assert_eq!(failed["output_tokens"], 20);
        assert_eq!(failed["cached_input_tokens"], 80);
        assert_eq!(failed["reasoning_tokens"], 10);
    }

    #[tokio::test]
    async fn stops_after_one_incomplete_response_repair() {
        let workspace = tempdir().unwrap();
        let store = RunDirStore::new(workspace.path());
        let provider = Arc::new(RepairProvider {
            failures: 2,
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        });
        let runner = runner(workspace.path(), &store, provider.clone());

        let error = format!(
            "{:#}",
            runner
                .run(RunRequest::root("cannot finish"))
                .await
                .unwrap_err()
        );
        assert!(error.contains("ended before completion"), "{error}");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    }
}
