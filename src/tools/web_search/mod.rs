use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

const DEFAULT_BRAVE_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
const MAX_SEARCH_RESULTS: usize = 20;

#[derive(Clone)]
pub struct WebSearchTool {
    client: Client,
    endpoint: String,
    api_key: String,
    default_count: usize,
}

impl WebSearchTool {
    pub fn brave(api_key: impl Into<String>, default_count: usize) -> Self {
        Self::with_endpoint(DEFAULT_BRAVE_ENDPOINT, api_key, default_count)
    }

    pub fn with_endpoint(
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        default_count: usize,
    ) -> Self {
        Self {
            client: Client::new(),
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            default_count: default_count.clamp(1, MAX_SEARCH_RESULTS),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SearchArgs {
    query: String,
    count: Option<usize>,
    country: Option<String>,
    search_lang: Option<String>,
}

#[derive(Debug, Serialize)]
struct SearchResult {
    title: String,
    url: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    age: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    extra_snippets: Vec<String>,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: SearchArgs =
            serde_json::from_value(arguments).context("invalid web_search arguments")?;
        let query = args.query.trim();
        if query.is_empty() {
            bail!("web_search query must not be empty");
        }
        let count = args
            .count
            .unwrap_or(self.default_count)
            .clamp(1, MAX_SEARCH_RESULTS);
        let mut url = url::Url::parse(&self.endpoint).context("invalid web search endpoint")?;
        {
            let mut query_pairs = url.query_pairs_mut();
            query_pairs.append_pair("q", query);
            query_pairs.append_pair("count", &count.to_string());
            if let Some(country) = args.country.as_deref() {
                query_pairs.append_pair("country", country);
            }
            if let Some(language) = args.search_lang.as_deref() {
                query_pairs.append_pair("search_lang", language);
            }
        }
        let request = self
            .client
            .get(url)
            .header("accept", "application/json")
            .header("x-subscription-token", &self.api_key);
        let response = request.send().await.context("send Brave web search")?;
        let status = response.status();
        let bytes = response.bytes().await.context("read Brave web search")?;
        if !status.is_success() {
            let body = String::from_utf8_lossy(&bytes);
            bail!(
                "web_search failed with HTTP {status}: {}",
                body.chars().take(4_000).collect::<String>()
            );
        }
        let payload: Value = serde_json::from_slice(&bytes).context("decode Brave web search")?;
        let results = payload
            .pointer("/web/results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(count)
            .map(|item| SearchResult {
                title: string_field(item, "title"),
                url: string_field(item, "url"),
                description: string_field(item, "description"),
                age: item.get("age").and_then(Value::as_str).map(str::to_owned),
                extra_snippets: item
                    .get("extra_snippets")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            })
            .collect::<Vec<_>>();
        Ok(RawToolOutput {
            content: serde_json::to_vec_pretty(&json!({
                "query": query,
                "results": results,
            }))?,
            source_path: None,
            media_type: "application/json".to_owned(),
            is_error: false,
        })
    }
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_limit_matches_runtime_limit() {
        let spec = crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!());
        assert_eq!(
            spec.input_schema.pointer("/properties/count/maximum"),
            Some(&json!(MAX_SEARCH_RESULTS))
        );
    }
}
