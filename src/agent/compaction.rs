use std::{sync::Arc, time::Duration};

use anyhow::{Result, anyhow, bail, ensure};

use crate::{
    agent::CompactionOptions,
    events::{NoopEventSink, RuntimeEvent, RuntimeEventKind, SharedEventSink},
    model::{Message, MessageContent, ModelProvider, ModelRequest, Role, ToolSpec},
    prompts::agent_prompts,
    storage::RunDirStore,
    trajectory::{CompactionMessage, CompactionState, TrajectoryMessage, message_ref},
};

pub(crate) struct CompactionAttempt<'a> {
    pub provider: &'a Arc<dyn ModelProvider>,
    pub model: &'a str,
    pub run_id: &'a str,
    pub system: &'a str,
    pub tools: &'a [ToolSpec],
    pub trajectory: &'a [TrajectoryMessage],
    pub tokens_before: u64,
    pub options: &'a CompactionOptions,
    pub store: &'a RunDirStore,
    pub events: &'a SharedEventSink,
    pub model_slots: &'a tokio::sync::Semaphore,
    pub stream_idle_timeout_seconds: u64,
    pub request_deadline_seconds: u64,
}

pub(crate) struct CompletedCompaction {
    pub records: [TrajectoryMessage; 2],
    pub estimated_context_tokens: u64,
}

const MAX_INVALID_COMPACTION_RESPONSES: usize = 2;

pub(crate) async fn maybe_compact(
    attempt: CompactionAttempt<'_>,
) -> Result<Option<CompletedCompaction>> {
    let CompactionAttempt {
        provider,
        model,
        run_id,
        system,
        tools,
        trajectory,
        tokens_before,
        options,
        store,
        events,
        model_slots,
        stream_idle_timeout_seconds,
        request_deadline_seconds,
    } = attempt;
    let Some(compact_at_tokens) = options.compact_at_tokens else {
        return Ok(None);
    };
    if tokens_before < compact_at_tokens || ordinary_messages(trajectory).count() < 3 {
        return Ok(None);
    }

    let previous = latest_compaction(trajectory);
    let Some(plan) = plan_compaction(trajectory, previous, options.keep_recent_tokens)? else {
        return Ok(None);
    };
    let state_message_ref = message_ref(
        trajectory
            .last()
            .ok_or_else(|| anyhow!("compaction trajectory is empty"))?
            .seq
            .saturating_add(2),
    );
    events
        .emit(&RuntimeEvent::new(
            run_id,
            RuntimeEventKind::CompactionStarted {
                state_message_ref: state_message_ref.clone(),
                tokens_before,
            },
        ))
        .await?;

    let compaction_request = Message::text(
        Role::User,
        agent_prompts().compaction_request.trim().to_owned(),
    );
    let request = ModelRequest {
        run_id: run_id.to_owned(),
        model: model.to_owned(),
        system: system.to_owned(),
        messages: compaction_input(trajectory, previous, &plan, &compaction_request)?,
        tools: tools.to_vec(),
        max_output_tokens: Some(options.summary_max_output_tokens.max(1)),
        stream_idle_timeout: Duration::from_secs(stream_idle_timeout_seconds),
    };
    if let Some(context_window) = options.context_window_tokens {
        let input_tokens = estimate_request_input_tokens(system, &request.messages, tools);
        let estimated_total = input_tokens.saturating_add(options.summary_max_output_tokens as u64);
        if estimated_total >= context_window {
            events
                .emit(&RuntimeEvent::new(
                    run_id,
                    RuntimeEventKind::CompactionFailed {
                        state_message_ref,
                        error: format!(
                            "estimated compaction context is {estimated_total} tokens ({input_tokens} input + {} output), at or above context_window_tokens={context_window}",
                            options.summary_max_output_tokens
                        ),
                    },
                ))
                .await?;
            return Ok(None);
        }
    }
    let model_permit = model_slots
        .acquire()
        .await
        .map_err(|_| anyhow!("model concurrency limiter closed"))?;
    let mut accepted = None;
    for attempt_index in 1..=MAX_INVALID_COMPACTION_RESPONSES {
        let response = tokio::time::timeout(
            Duration::from_secs(request_deadline_seconds),
            provider.complete(request.clone(), Arc::new(NoopEventSink)),
        )
        .await
        .map_err(|_| {
            anyhow!("compaction model request deadline exceeded {request_deadline_seconds} seconds")
        })
        .and_then(|response| response);
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                events
                    .emit(&RuntimeEvent::new(
                        run_id,
                        RuntimeEventKind::CompactionFailed {
                            state_message_ref,
                            error: format!("{error:#}"),
                        },
                    ))
                    .await?;
                drop(model_permit);
                return Ok(None);
            }
        };
        if let Err(error) = response.validate_completed() {
            events
                .emit(&RuntimeEvent::new(
                    run_id,
                    RuntimeEventKind::CompactionFailed {
                        state_message_ref,
                        error: format!("{error:#}"),
                    },
                ))
                .await?;
            drop(model_permit);
            return Ok(None);
        }
        let invalid = if !response.tool_calls().is_empty() {
            Some("compaction model returned tool calls")
        } else if response.text().trim().is_empty() {
            Some("compaction model returned an empty state")
        } else {
            None
        };
        let Some(error) = invalid else {
            accepted = Some(response);
            break;
        };
        events
            .emit(&RuntimeEvent::new(
                run_id,
                RuntimeEventKind::CompactionFailed {
                    state_message_ref: state_message_ref.clone(),
                    error: format!(
                        "{error} (attempt {attempt_index}/{MAX_INVALID_COMPACTION_RESPONSES})"
                    ),
                },
            ))
            .await?;
    }
    drop(model_permit);
    let Some(response) = accepted else {
        return Ok(None);
    };

    let state = CompactionState {
        covered_through_message_ref: plan.covered_through.message_ref.clone(),
        first_kept_message_ref: plan.first_kept.message_ref.clone(),
    };
    let removed_tokens = plan
        .to_compact
        .iter()
        .map(|record| estimate_message_tokens(&record.message))
        .sum::<u64>()
        .saturating_add(
            previous
                .map(|(record, _)| estimate_message_tokens(&record.message))
                .unwrap_or_default(),
        );
    let estimated_context_tokens = tokens_before
        .saturating_sub(removed_tokens)
        .saturating_add(estimate_message_tokens(&response.assistant));

    // The assistant state is the commit marker. A crash after the request
    // append but before this append leaves an inert, auditable request that is
    // excluded from normal context and can be retried on resume.
    let request_record = store
        .append_compaction_message(run_id, &compaction_request, CompactionMessage::Request)
        .await?;
    let state_record = store
        .append_compaction_message(
            run_id,
            &response.assistant,
            CompactionMessage::State {
                state: state.clone(),
            },
        )
        .await?;
    ensure!(
        state_record.message_ref == state_message_ref,
        "compacted state ref changed from planned `{state_message_ref}` to `{}`",
        state_record.message_ref
    );
    events
        .emit(&RuntimeEvent::new(
            run_id,
            RuntimeEventKind::CompactionCompleted {
                state_message_ref,
                covered_through_message_ref: state.covered_through_message_ref,
                first_kept_message_ref: state.first_kept_message_ref,
                input_tokens: response.usage.input_tokens,
                output_tokens: response.usage.output_tokens,
            },
        ))
        .await?;
    Ok(Some(CompletedCompaction {
        records: [request_record, state_record],
        estimated_context_tokens,
    }))
}

pub(crate) fn build_active_context(trajectory: &[TrajectoryMessage]) -> Result<Vec<Message>> {
    let Some((state_record, state)) = latest_compaction(trajectory) else {
        return Ok(ordinary_messages(trajectory)
            .map(|record| record.message.clone())
            .collect());
    };
    let initial = ordinary_messages(trajectory)
        .next()
        .ok_or_else(|| anyhow!("compaction trajectory has no initial message"))?;
    let first_kept = trajectory
        .iter()
        .position(|record| record.message_ref == state.first_kept_message_ref)
        .ok_or_else(|| {
            anyhow!(
                "compacted state `{}` references missing message `{}`",
                state_record.message_ref,
                state.first_kept_message_ref
            )
        })?;
    if trajectory[first_kept].seq <= initial.seq {
        bail!("compaction cannot replace the initial runtime message")
    }

    let mut active = Vec::with_capacity(trajectory.len() - first_kept + 3);
    active.push(initial.message.clone());
    active.push(state_record.message.clone());
    active.push(Message {
        role: Role::User,
        content: vec![MessageContent::RuntimeReminder {
            text: format!(
                "<runtime-reminder>\n{}\n</runtime-reminder>",
                agent_prompts().compaction_resume.trim()
            ),
        }],
    });
    active.extend(
        trajectory[first_kept..]
            .iter()
            .filter(|record| record.compaction.is_none())
            .map(|record| record.message.clone()),
    );
    Ok(active)
}

fn latest_compaction(
    trajectory: &[TrajectoryMessage],
) -> Option<(&TrajectoryMessage, &CompactionState)> {
    trajectory
        .iter()
        .rev()
        .find_map(|record| record.compaction_state().map(|state| (record, state)))
}

fn ordinary_messages(trajectory: &[TrajectoryMessage]) -> impl Iterator<Item = &TrajectoryMessage> {
    trajectory
        .iter()
        .filter(|record| record.compaction.is_none())
}

struct CompactionPlan<'a> {
    to_compact: Vec<&'a TrajectoryMessage>,
    covered_through: &'a TrajectoryMessage,
    first_kept: &'a TrajectoryMessage,
}

fn plan_compaction<'a>(
    trajectory: &'a [TrajectoryMessage],
    previous: Option<(&'a TrajectoryMessage, &'a CompactionState)>,
    keep_recent_tokens: u64,
) -> Result<Option<CompactionPlan<'a>>> {
    let messages = ordinary_messages(trajectory).collect::<Vec<_>>();
    let active_start = match previous {
        Some((state_record, state)) => messages
            .iter()
            .position(|record| record.message_ref == state.first_kept_message_ref)
            .ok_or_else(|| {
                anyhow!(
                    "compacted state `{}` references missing message `{}`",
                    state_record.message_ref,
                    state.first_kept_message_ref
                )
            })?,
        None => 1,
    };
    if active_start >= messages.len().saturating_sub(1) {
        return Ok(None);
    }

    let mut kept_tokens = 0_u64;
    let mut first_kept = messages.len() - 1;
    for index in (active_start..messages.len()).rev() {
        let message_tokens = estimate_message_tokens(&messages[index].message);
        if kept_tokens > 0 && kept_tokens.saturating_add(message_tokens) > keep_recent_tokens {
            first_kept = index + 1;
            break;
        }
        kept_tokens = kept_tokens.saturating_add(message_tokens);
        first_kept = index;
    }

    while first_kept > active_start && messages[first_kept].message.role == Role::Tool {
        first_kept -= 1;
    }
    if first_kept <= active_start || first_kept >= messages.len() {
        return Ok(None);
    }
    let to_compact = messages[active_start..first_kept].to_vec();
    Ok(Some(CompactionPlan {
        covered_through: messages[first_kept - 1],
        first_kept: messages[first_kept],
        to_compact,
    }))
}

fn compaction_input(
    trajectory: &[TrajectoryMessage],
    previous: Option<(&TrajectoryMessage, &CompactionState)>,
    plan: &CompactionPlan<'_>,
    instruction: &Message,
) -> Result<Vec<Message>> {
    let initial = ordinary_messages(trajectory)
        .next()
        .ok_or_else(|| anyhow!("compaction trajectory has no initial message"))?;
    let mut messages = Vec::with_capacity(plan.to_compact.len() + 3);
    messages.push(initial.message.clone());
    if let Some((state_record, _)) = previous {
        messages.push(state_record.message.clone());
    }
    messages.extend(plan.to_compact.iter().map(|record| record.message.clone()));
    messages.push(instruction.clone());
    Ok(messages)
}

pub(crate) fn estimate_message_tokens(message: &Message) -> u64 {
    let bytes = message
        .content
        .iter()
        .map(|content| match content {
            MessageContent::RuntimeReminder { text } | MessageContent::Text { text } => text.len(),
            MessageContent::Reasoning { text } => text.len(),
            MessageContent::ToolCall {
                id,
                name,
                arguments,
            } => id.len() + name.len() + arguments.to_string().len(),
            MessageContent::ToolResult {
                call_id, content, ..
            } => call_id.len() + content.len(),
            MessageContent::ProviderItem { item, .. } => item.to_string().len(),
            MessageContent::BackgroundTask {
                task_id,
                name,
                status,
                content,
                ..
            } => {
                task_id.len() + name.len() + status.as_ref().map_or(0, String::len) + content.len()
            }
        })
        .sum::<usize>();
    (bytes as u64).div_ceil(4)
}

pub(crate) fn estimate_request_input_tokens(
    system: &str,
    messages: &[Message],
    tools: &[ToolSpec],
) -> u64 {
    let system_tokens = (system.len() as u64).div_ceil(4);
    let message_tokens = messages
        .iter()
        .map(estimate_message_tokens)
        .sum::<u64>()
        .saturating_add(messages.len() as u64 * 4);
    let tool_tokens = serde_json::to_vec(tools)
        .map(|tools| (tools.len() as u64).div_ceil(4))
        .unwrap_or_default();
    system_tokens
        .saturating_add(message_tokens)
        .saturating_add(tool_tokens)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;

    use super::*;

    fn record(seq: u64, role: Role, content: MessageContent) -> TrajectoryMessage {
        TrajectoryMessage {
            message_ref: format!("m{seq}"),
            seq,
            created_at: Utc::now(),
            message: Message {
                role,
                content: vec![content],
            },
            pending_input_id: None,
            compaction: None,
        }
    }

    fn state_record(seq: u64, first_kept_message_ref: &str, summary: &str) -> TrajectoryMessage {
        TrajectoryMessage {
            message_ref: format!("m{seq}"),
            seq,
            created_at: Utc::now(),
            message: Message::text(Role::Assistant, summary),
            pending_input_id: None,
            compaction: Some(CompactionMessage::State {
                state: CompactionState {
                    covered_through_message_ref: "m2".to_owned(),
                    first_kept_message_ref: first_kept_message_ref.to_owned(),
                },
            }),
        }
    }

    #[test]
    fn plan_keeps_tool_calls_with_their_results() {
        let trajectory = vec![
            record(
                1,
                Role::User,
                MessageContent::Text {
                    text: "start".into(),
                },
            ),
            record(
                2,
                Role::Assistant,
                MessageContent::ToolCall {
                    id: "call-1".into(),
                    name: "bash".into(),
                    arguments: json!({}),
                },
            ),
            record(
                3,
                Role::Tool,
                MessageContent::ToolResult {
                    call_id: "call-1".into(),
                    content: "old".repeat(100),
                    is_error: false,
                    metadata: crate::artifact::ResultMetadata::empty(),
                },
            ),
            record(
                4,
                Role::Assistant,
                MessageContent::ToolCall {
                    id: "call-2".into(),
                    name: "bash".into(),
                    arguments: json!({}),
                },
            ),
            record(
                5,
                Role::Tool,
                MessageContent::ToolResult {
                    call_id: "call-2".into(),
                    content: "recent".repeat(100),
                    is_error: false,
                    metadata: crate::artifact::ResultMetadata::empty(),
                },
            ),
        ];
        let plan = plan_compaction(&trajectory, None, 160).unwrap().unwrap();
        assert_eq!(plan.first_kept.message_ref, "m4");
        assert_eq!(plan.covered_through.message_ref, "m3");
    }

    #[test]
    fn active_context_reuses_exact_state_and_omits_control_messages() {
        let mut request = record(
            4,
            Role::User,
            MessageContent::Text {
                text: "compact now".into(),
            },
        );
        request.compaction = Some(CompactionMessage::Request);
        let state = state_record(5, "m3", "# Compacted state\nold work summarized");
        let trajectory = vec![
            record(
                1,
                Role::User,
                MessageContent::Text {
                    text: "start".into(),
                },
            ),
            record(
                2,
                Role::Assistant,
                MessageContent::Text { text: "old".into() },
            ),
            record(
                3,
                Role::Assistant,
                MessageContent::Text {
                    text: "recent".into(),
                },
            ),
            request,
            state.clone(),
        ];

        let active = build_active_context(&trajectory).unwrap();
        assert_eq!(active.len(), 4);
        assert_eq!(active[0].visible_text(), "start");
        assert_eq!(active[1].visible_text(), state.message.visible_text());
        assert_eq!(active[1].role, Role::Assistant);
        assert_eq!(active[2].role, Role::User);
        let MessageContent::RuntimeReminder { text } = &active[2].content[0] else {
            panic!("compaction continuation must be a runtime reminder")
        };
        assert!(text.contains("not a final answer"));
        assert!(text.starts_with("<runtime-reminder>"));
        assert_eq!(active[3].visible_text(), "recent");
        assert!(
            !active
                .iter()
                .any(|message| message.visible_text() == "compact now")
        );
    }

    #[test]
    fn orphan_compaction_request_is_not_replayed() {
        let mut request = record(
            3,
            Role::User,
            MessageContent::Text {
                text: "compact now".into(),
            },
        );
        request.compaction = Some(CompactionMessage::Request);
        let trajectory = vec![
            record(
                1,
                Role::User,
                MessageContent::Text {
                    text: "start".into(),
                },
            ),
            record(
                2,
                Role::Assistant,
                MessageContent::Text {
                    text: "working".into(),
                },
            ),
            request,
        ];

        let active = build_active_context(&trajectory).unwrap();
        assert_eq!(active.len(), 2);
        assert!(
            !active
                .iter()
                .any(|message| message.visible_text() == "compact now")
        );
    }

    #[test]
    fn repeated_compaction_uses_previous_state_before_new_native_messages() {
        let state = state_record(5, "m3", "first state");
        let trajectory = vec![
            record(
                1,
                Role::User,
                MessageContent::Text {
                    text: "start".into(),
                },
            ),
            record(
                2,
                Role::Assistant,
                MessageContent::Text { text: "old".into() },
            ),
            record(
                3,
                Role::Assistant,
                MessageContent::Text {
                    text: "middle".into(),
                },
            ),
            state.clone(),
            record(
                6,
                Role::Assistant,
                MessageContent::Text {
                    text: "recent".repeat(100),
                },
            ),
            record(
                7,
                Role::User,
                MessageContent::Text {
                    text: "latest".repeat(100),
                },
            ),
        ];
        let previous = latest_compaction(&trajectory);
        let plan = plan_compaction(&trajectory, previous, 200)
            .unwrap()
            .unwrap();
        let instruction = Message::text(Role::User, "compact");
        let input = compaction_input(&trajectory, previous, &plan, &instruction).unwrap();

        assert_eq!(input[0].visible_text(), "start");
        assert_eq!(input[1].visible_text(), state.message.visible_text());
        assert_eq!(input[2].visible_text(), "middle");
        assert_eq!(input.last().unwrap().visible_text(), "compact");
        assert!(
            !input
                .iter()
                .any(|message| message.visible_text().starts_with("latest"))
        );
    }
}
