mod artifact;
mod command;
mod configured;

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use rmcp::{
    Peer, RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, ContentBlock, JsonObject, Tool as RemoteTool},
    service::RunningService,
    transport::TokioChildProcess,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::{
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

pub use artifact::{McpArtifact, write_catalog};
pub use command::{CompiledMcpCall, McpArtifactRegistry};
pub use configured::{
    call_configured, capture_configured, check_configured, compile_configured, configured_server,
    connect_configured, load_configured_artifacts,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStdioConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
}

/// A live MCP stdio child client. It owns rmcp's running service so shutdown is
/// explicit while runtime and authoring calls share its lightweight peer.
pub struct McpStdioClient {
    name: String,
    peer: Peer<RoleClient>,
    service: Mutex<Option<RunningService<RoleClient, ()>>>,
}

impl McpStdioClient {
    pub async fn connect(config: McpStdioConfig) -> Result<Arc<Self>> {
        if config.name.trim().is_empty() {
            bail!("MCP server name must not be empty");
        }
        if config.command.trim().is_empty() {
            bail!("MCP server command must not be empty");
        }
        let mut command = tokio::process::Command::new(&config.command);
        command
            .args(&config.args)
            .envs(&config.env)
            .kill_on_drop(true);
        if let Some(cwd) = &config.cwd {
            command.current_dir(cwd);
        }
        let transport = TokioChildProcess::new(command)
            .with_context(|| format!("failed to start MCP server `{}`", config.name))?;
        let service = ()
            .serve(transport)
            .await
            .with_context(|| format!("failed to initialize MCP server `{}`", config.name))?;
        let peer = service.peer().clone();
        Ok(Arc::new(Self {
            name: config.name,
            peer,
            service: Mutex::new(Some(service)),
        }))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub async fn list_tools(&self) -> Result<Vec<RemoteTool>> {
        self.peer
            .list_all_tools()
            .await
            .with_context(|| format!("failed to list tools from MCP server `{}`", self.name))
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<CallToolResult> {
        let arguments = match arguments {
            Value::Null => JsonObject::new(),
            Value::Object(arguments) => arguments,
            _ => bail!("MCP tool arguments must be a JSON object"),
        };
        self.peer
            .call_tool(CallToolRequestParams::new(name.to_owned()).with_arguments(arguments))
            .await
            .with_context(|| format!("MCP tool `{}` on server `{}` failed", name, self.name))
    }

    pub async fn shutdown(&self) -> Result<()> {
        let Some(mut service) = self.service.lock().await.take() else {
            return Ok(());
        };
        service
            .close()
            .await
            .context("MCP client task failed during shutdown")?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct McpRuntime {
    artifacts: McpArtifactRegistry,
    clients: BTreeMap<String, Arc<McpStdioClient>>,
}

impl McpRuntime {
    pub fn new(
        artifacts: McpArtifactRegistry,
        clients: impl IntoIterator<Item = Arc<McpStdioClient>>,
    ) -> Result<Self> {
        let mut by_name = BTreeMap::new();
        for client in clients {
            let name = client.name().to_owned();
            if by_name.insert(name.clone(), client).is_some() {
                bail!("MCP client `{name}` is already registered");
            }
        }
        for name in by_name.keys() {
            if artifacts.get(name).is_none() {
                bail!("MCP client `{name}` has no artifact");
            }
        }
        Ok(Self {
            artifacts,
            clients: by_name,
        })
    }

    pub fn prompt_index(&self) -> String {
        self.artifacts.prompt_index()
    }

    pub fn compile(&self, command: &str) -> Result<CompiledMcpCall> {
        self.artifacts.compile(command)
    }

    pub async fn call(&self, command: &str) -> Result<RawToolOutput> {
        let compiled = self.compile(command)?;
        let client = self
            .clients
            .get(&compiled.source)
            .with_context(|| format!("MCP source `{}` is not connected", compiled.source))?;
        let result = client.call_tool(&compiled.tool, compiled.arguments).await?;
        render_call_result(result)
    }
}

pub struct McpTool {
    runtime: Arc<McpRuntime>,
}

impl McpTool {
    pub fn new(runtime: Arc<McpRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("mcp/tool.yaml"), module_path!())
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let command = arguments
            .get("command")
            .and_then(Value::as_str)
            .context("`command` is required")?;
        self.runtime.call(command).await
    }
}

pub fn render_call_result(result: CallToolResult) -> Result<RawToolOutput> {
    let is_error = result.is_error.unwrap_or(false);
    let all_text = result
        .content
        .iter()
        .all(|content| matches!(content, ContentBlock::Text(_)));
    if all_text && !result.content.is_empty() {
        let text = result
            .content
            .iter()
            .filter_map(|content| match content {
                ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        return Ok(RawToolOutput {
            content: text.into_bytes(),
            source_path: None,
            media_type: "text/plain; charset=utf-8".to_owned(),
            is_error,
            attach_to_model: false,
        });
    }
    if result.content.is_empty()
        && let Some(structured) = &result.structured_content
    {
        return Ok(RawToolOutput {
            content: serde_json::to_vec_pretty(structured)?,
            source_path: None,
            media_type: "application/json".to_owned(),
            is_error,
            attach_to_model: false,
        });
    }
    Ok(RawToolOutput {
        content: serde_json::to_vec_pretty(&result)?,
        source_path: None,
        media_type: "application/json".to_owned(),
        is_error,
        attach_to_model: false,
    })
}

#[cfg(test)]
mod tests {
    use rmcp::model::ContentBlock;
    use serde_json::json;

    use super::*;

    #[test]
    fn fixed_mcp_manifest_exposes_only_one_command_string() {
        let runtime = Arc::new(
            McpRuntime::new(
                McpArtifactRegistry::default(),
                Vec::<Arc<McpStdioClient>>::new(),
            )
            .unwrap(),
        );
        let spec = McpTool::new(runtime).spec();
        assert_eq!(spec.name, "mcp");
        assert_eq!(spec.input_schema["required"], json!(["command"]));
        assert_eq!(spec.input_schema["additionalProperties"], false);
    }

    #[test]
    fn text_results_are_returned_without_a_json_envelope() {
        let result = CallToolResult::success(vec![
            ContentBlock::text("first"),
            ContentBlock::text("second"),
        ]);
        let output = render_call_result(result).unwrap();
        assert_eq!(output.content, b"first\n\nsecond");
        assert_eq!(output.media_type, "text/plain; charset=utf-8");
        assert!(!output.is_error);
    }

    #[test]
    fn structured_only_results_preserve_the_remote_value() {
        let result: CallToolResult = serde_json::from_value(json!({
            "content": [],
            "structuredContent": {"count": 2},
            "isError": true
        }))
        .unwrap();
        let output = render_call_result(result).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&output.content).unwrap(),
            json!({"count": 2})
        );
        assert_eq!(output.media_type, "application/json");
        assert!(output.is_error);
    }

    #[test]
    fn invalid_config_is_rejected_before_spawn() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime.block_on(McpStdioClient::connect(McpStdioConfig {
            name: String::new(),
            command: "never-run".into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
        }));
        assert!(result.is_err());
    }
}
