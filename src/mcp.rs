use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use rmcp::{
    Peer, RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, JsonObject, Tool as RemoteTool},
    service::RunningService,
    transport::TokioChildProcess,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::{
    model::ToolSpec,
    tools::{ExplicitSpawn, RawToolOutput, Tool, ToolContext, ToolRegistry},
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
/// explicit; tool adapters retain a clone of the lightweight peer handle.
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

    pub async fn tool_adapters(self: &Arc<Self>) -> Result<Vec<Arc<dyn Tool>>> {
        Ok(self
            .list_tools()
            .await?
            .into_iter()
            .map(|remote| Arc::new(McpToolAdapter::new(self.clone(), remote)) as Arc<dyn Tool>)
            .collect())
    }

    pub async fn register_tools(self: &Arc<Self>, registry: &mut ToolRegistry) -> Result<()> {
        for adapter in self.tool_adapters().await? {
            registry.register(adapter, ExplicitSpawn::Allowed)?;
        }
        Ok(())
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

pub struct McpToolAdapter {
    client: Arc<McpStdioClient>,
    remote_name: String,
    spec: ToolSpec,
}

impl McpToolAdapter {
    fn new(client: Arc<McpStdioClient>, remote: RemoteTool) -> Self {
        let remote_name = remote.name.to_string();
        let spec = adapter_spec(client.name(), &remote);
        Self {
            client,
            remote_name,
            spec,
        }
    }

    pub fn remote_name(&self) -> &str {
        &self.remote_name
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let result = self.client.call_tool(&self.remote_name, arguments).await?;
        let is_error = result.is_error.unwrap_or(false);
        Ok(RawToolOutput {
            content: serde_json::to_vec_pretty(&result)?,
            source_path: None,
            media_type: "application/json".to_owned(),
            is_error,
        })
    }
}

fn adapter_spec(server_name: &str, remote: &RemoteTool) -> ToolSpec {
    ToolSpec {
        name: format!(
            "mcp__{}__{}",
            tool_name_part(server_name),
            tool_name_part(&remote.name)
        ),
        description: remote
            .description
            .as_deref()
            .unwrap_or("Tool provided by an MCP server")
            .to_owned(),
        input_schema: Value::Object((*remote.input_schema).clone()),
    }
}

fn tool_name_part(name: &str) -> String {
    let value = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if value.is_empty() {
        "unnamed".to_owned()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn remote_tool_is_adapted_to_stable_local_spec() {
        let remote: RemoteTool = serde_json::from_value(json!({
            "name": "search/code",
            "description": "Search code",
            "inputSchema": {
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"]
            }
        }))
        .unwrap();
        let spec = adapter_spec("GitHub Cloud", &remote);
        assert_eq!(spec.name, "mcp__GitHub_Cloud__search_code");
        assert_eq!(spec.description, "Search code");
        assert_eq!(spec.input_schema["required"], json!(["query"]));
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
