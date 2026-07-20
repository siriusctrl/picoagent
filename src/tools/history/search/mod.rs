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

pub(super) struct SearchTool {
    reader: Arc<dyn TrajectoryReader>,
    max_matches: usize,
}

impl SearchTool {
    pub(super) fn new(reader: Arc<dyn TrajectoryReader>, max_matches: usize) -> Result<Self> {
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
impl Tool for SearchTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
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
        Ok(RawToolOutput {
            content: serde_json::to_vec(&json!({
                "matches": result.matches,
                "truncated": result.truncated,
                "instruction": instruction,
            }))?,
            source_path: None,
            media_type: "application/json".to_owned(),
            is_error: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trajectory::{
        HistoryMatch, HistoryMatchSource, HistoryReadRequest, HistoryReadResult,
        HistorySearchResult, TrajectoryReader,
    };
    use tempfile::tempdir;

    struct StubReader;

    #[async_trait]
    impl TrajectoryReader for StubReader {
        async fn search(&self, _request: HistorySearchRequest) -> Result<HistorySearchResult> {
            Ok(HistorySearchResult {
                matches: vec![HistoryMatch {
                    message_ref: "m7".to_owned(),
                    match_source: HistoryMatchSource::Message,
                    artifact: None,
                    snippet: "alpha".to_owned(),
                }],
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
        let tool = SearchTool::new(Arc::new(StubReader), 50).unwrap();
        let description = tool.spec().description;
        assert!(description.contains("`ref`: `m<N>`"));
        assert!(description.contains("`source`: `message`"));
        assert!(description.contains("`artifact`"));
        assert!(description.contains("newest-first"));
        let output = tool
            .execute(context.clone(), json!({"pattern": "alpha"}))
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&output.content).unwrap();
        assert_eq!(
            value["matches"][0],
            json!({"ref": "m7", "source": "message", "snippet": "alpha"})
        );
        assert_eq!(value["truncated"], true);
        assert!(value["instruction"].as_str().unwrap().contains("refine"));

        let error = tool
            .execute(context, json!({"pattern": "["}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("invalid history_search regex"));
    }
}
