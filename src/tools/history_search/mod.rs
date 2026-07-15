use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
    trajectory::{HistorySearchRequest, TrajectoryReader},
};

const DESCRIPTION: &str = include_str!("description.md");

pub struct HistorySearchTool {
    reader: Arc<dyn TrajectoryReader>,
    max_matches: usize,
}

impl HistorySearchTool {
    pub fn new(reader: Arc<dyn TrajectoryReader>, max_matches: usize) -> Result<Self> {
        if max_matches == 0 {
            bail!("history search max_matches must be greater than zero");
        }
        Ok(Self {
            reader,
            max_matches,
        })
    }
}

#[derive(Debug, Deserialize)]
struct HistorySearchArgs {
    pattern: String,
}

#[async_trait]
impl Tool for HistorySearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "history_search".to_owned(),
            description: DESCRIPTION.trim().to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Rust regex"
                    }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: HistorySearchArgs =
            serde_json::from_value(arguments).context("invalid history_search arguments")?;
        if args.pattern.is_empty() {
            bail!("history_search pattern must not be empty");
        }
        let pattern = Regex::new(&args.pattern).context("invalid history_search regex")?;
        let result = self
            .reader
            .search(HistorySearchRequest {
                run_id: context.run_id,
                pattern,
                max_matches: self.max_matches,
            })
            .await?;

        let instruction = result.truncated.then(|| {
            format!(
                "Only the newest {} matching messages are shown; refine the regex to inspect omitted older matches.",
                result.matches.len()
            )
        });
        let mut records = Vec::with_capacity(result.matches.len() + 1);
        records.push(serde_json::to_string(&json!({
            "type": "search_summary",
            "returned": result.matches.len(),
            "truncated": result.truncated,
            "truncation_reason": result.truncated.then_some("max_matches"),
            "omitted": result.truncated.then_some("older_matches"),
            "instruction": instruction,
        }))?);
        for found in result.matches {
            records.push(serde_json::to_string(&json!({
                "type": "match",
                "message_ref": found.message_ref,
                "seq": found.seq,
                "created_at": found.created_at,
                "role": found.role,
                "kind": found.kind,
                "tool_name": found.tool_name,
                "match_source": found.match_source,
                "snippet": found.snippet,
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
    use crate::trajectory::{
        HistoryReadRequest, HistoryReadResult, HistorySearchResult, TrajectoryReader,
    };
    use tempfile::tempdir;

    struct StubReader;

    #[async_trait]
    impl TrajectoryReader for StubReader {
        async fn search(&self, _request: HistorySearchRequest) -> Result<HistorySearchResult> {
            Ok(HistorySearchResult {
                matches: Vec::new(),
                truncated: true,
            })
        }

        async fn read(&self, _request: HistoryReadRequest) -> Result<HistoryReadResult> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn reports_truncation_and_rejects_invalid_regex() {
        let workspace = tempdir().unwrap();
        let context = ToolContext {
            run_id: "run".to_owned(),
            call_id: "call".to_owned(),
            workspace: workspace.path().to_owned(),
        };
        let tool = HistorySearchTool::new(Arc::new(StubReader), 50).unwrap();
        let output = tool
            .execute(context.clone(), json!({"pattern": "alpha"}))
            .await
            .unwrap();
        let text = String::from_utf8(output.content).unwrap();
        assert!(text.contains(r#""truncated":true"#));
        assert!(text.contains("refine the regex"));

        let error = tool
            .execute(context, json!({"pattern": "["}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("invalid history_search regex"));
    }
}
