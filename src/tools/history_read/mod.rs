use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
    trajectory::{HistoryReadRequest, TrajectoryReader},
};

const DESCRIPTION: &str = include_str!("description.md");
const DEFAULT_CONTEXT_MESSAGES: usize = 2;
const MAX_CONTEXT_MESSAGES: usize = 10;

pub struct HistoryReadTool {
    reader: Arc<dyn TrajectoryReader>,
}

impl HistoryReadTool {
    pub fn new(reader: Arc<dyn TrajectoryReader>) -> Self {
        Self { reader }
    }
}

#[derive(Debug, Deserialize)]
struct HistoryReadArgs {
    #[serde(rename = "ref")]
    message_ref: String,
    #[serde(default = "default_context_messages")]
    before: usize,
    #[serde(default = "default_context_messages")]
    after: usize,
}

fn default_context_messages() -> usize {
    DEFAULT_CONTEXT_MESSAGES
}

#[async_trait]
impl Tool for HistoryReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "history_read".to_owned(),
            description: DESCRIPTION.trim().to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "ref": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Ref from history_search"
                    },
                    "before": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": MAX_CONTEXT_MESSAGES,
                        "default": DEFAULT_CONTEXT_MESSAGES
                    },
                    "after": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": MAX_CONTEXT_MESSAGES,
                        "default": DEFAULT_CONTEXT_MESSAGES
                    }
                },
                "required": ["ref"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: HistoryReadArgs =
            serde_json::from_value(arguments).context("invalid history_read arguments")?;
        if args.message_ref.is_empty() {
            bail!("history_read ref must not be empty");
        }
        if args.before > MAX_CONTEXT_MESSAGES || args.after > MAX_CONTEXT_MESSAGES {
            bail!("history_read before and after must not exceed {MAX_CONTEXT_MESSAGES} messages");
        }
        let result = self
            .reader
            .read(HistoryReadRequest {
                run_id: context.run_id,
                message_ref: args.message_ref,
                before: args.before,
                after: args.after,
            })
            .await?;

        let first_ref = result
            .messages
            .first()
            .map(|message| message.message_ref.as_str());
        let last_ref = result
            .messages
            .last()
            .map(|message| message.message_ref.as_str());
        let mut records = Vec::with_capacity(result.messages.len() + 1);
        records.push(serde_json::to_string(&json!({
            "type": "history_window",
            "anchor_ref": result.anchor_ref,
            "requested_before": args.before,
            "requested_after": args.after,
            "returned": result.messages.len(),
            "first_ref": first_ref,
            "last_ref": last_ref,
            "tool_pairs_expanded": result.tool_pairs_expanded,
        }))?);
        for record in result.messages {
            records.push(serde_json::to_string(&json!({
                "type": "message",
                "message_ref": record.message_ref,
                "seq": record.seq,
                "created_at": record.created_at,
                "anchor": record.message_ref == result.anchor_ref,
                "role": record.message.role,
                "content": record.message.content,
            }))?);
        }

        Ok(RawToolOutput {
            content: records.join("\n").into_bytes(),
            source_path: None,
            media_type: "application/x-ndjson; charset=utf-8".to_owned(),
            is_error: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{Message, MessageContent, Role},
        trajectory::{
            HistoryReadResult, HistorySearchRequest, HistorySearchResult, TrajectoryMessage,
            TrajectoryReader,
        },
    };
    use chrono::Utc;
    use tempfile::tempdir;

    struct StubReader;

    #[async_trait]
    impl TrajectoryReader for StubReader {
        async fn search(&self, _request: HistorySearchRequest) -> Result<HistorySearchResult> {
            unreachable!()
        }

        async fn read(&self, request: HistoryReadRequest) -> Result<HistoryReadResult> {
            Ok(HistoryReadResult {
                anchor_ref: request.message_ref.clone(),
                messages: vec![TrajectoryMessage {
                    message_ref: request.message_ref,
                    seq: 7,
                    created_at: Utc::now(),
                    message: Message {
                        role: Role::Assistant,
                        content: vec![MessageContent::Text {
                            text: "remembered".to_owned(),
                        }],
                    },
                }],
                tool_pairs_expanded: false,
            })
        }
    }

    #[tokio::test]
    async fn returns_an_anchor_aware_jsonl_window() {
        let workspace = tempdir().unwrap();
        let tool = HistoryReadTool::new(Arc::new(StubReader));
        let output = tool
            .execute(
                ToolContext {
                    run_id: "run".to_owned(),
                    call_id: "call".to_owned(),
                    workspace: workspace.path().to_owned(),
                },
                json!({"ref": "msg-7", "before": 1, "after": 3}),
            )
            .await
            .unwrap();
        let lines = String::from_utf8(output.content).unwrap();
        assert!(lines.contains(r#""anchor_ref":"msg-7""#));
        assert!(lines.contains(r#""anchor":true"#));
        assert!(lines.contains("remembered"));
        assert_eq!(output.media_type, "application/x-ndjson; charset=utf-8");
    }
}
