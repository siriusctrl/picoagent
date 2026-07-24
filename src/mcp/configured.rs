use std::{path::Path, sync::Arc};

use anyhow::{Context, Result, ensure};

use crate::{
    config::{AppConfig, McpServerConfig, resolve_env_reference},
    tools::RawToolOutput,
};

use super::{
    CompiledMcpCall, McpArtifact, McpArtifactRegistry, McpRuntime, McpStdioClient, McpStdioConfig,
    write_catalog,
};

pub fn configured_server<'a>(config: &'a AppConfig, name: &str) -> Result<&'a McpServerConfig> {
    config
        .mcp
        .get(name)
        .with_context(|| format!("unknown configured MCP source `{name}`"))
}

pub fn load_configured_artifacts(
    workspace: &Path,
    config: &AppConfig,
) -> Result<McpArtifactRegistry> {
    let mut artifacts = McpArtifactRegistry::default();
    for (name, server) in &config.mcp {
        artifacts.register(McpArtifact::load(workspace, name, &server.artifact)?)?;
    }
    Ok(artifacts)
}

pub async fn connect_configured(
    workspace: &Path,
    name: &str,
    server: &McpServerConfig,
) -> Result<Arc<McpStdioClient>> {
    McpStdioClient::connect(McpStdioConfig {
        name: name.to_owned(),
        command: server.command.clone(),
        args: server.args.clone(),
        env: server
            .env
            .iter()
            .map(|(name, value)| Ok((name.clone(), resolve_env_reference(value)?)))
            .collect::<Result<_>>()?,
        cwd: Some(workspace.to_path_buf()),
    })
    .await
}

pub async fn capture_configured(
    workspace: &Path,
    name: &str,
    server: &McpServerConfig,
) -> Result<(usize, std::path::PathBuf)> {
    let client = connect_configured(workspace, name, server).await?;
    let result: Result<_> = async {
        let tools = client.list_tools().await?;
        let directory = resolve_workspace_path(workspace, &server.artifact);
        let path = write_catalog(&directory, &tools)?;
        Ok((tools.len(), path))
    }
    .await;
    let shutdown = client.shutdown().await;
    let summary = result?;
    shutdown?;
    Ok(summary)
}

pub async fn check_configured(
    workspace: &Path,
    name: &str,
    server: &McpServerConfig,
    live: bool,
) -> Result<McpArtifact> {
    let artifact = McpArtifact::load(workspace, name, &server.artifact)?;
    if live {
        let client = connect_configured(workspace, name, server).await?;
        let result: Result<()> = async {
            let mut expected = artifact.tools().cloned().collect::<Vec<_>>();
            let mut observed = client.list_tools().await?;
            expected.sort_by(|left, right| left.name.cmp(&right.name));
            observed.sort_by(|left, right| left.name.cmp(&right.name));
            ensure!(
                expected == observed,
                "live MCP catalog for `{name}` differs from catalog.json; recapture and review the artifact"
            );
            Ok(())
        }
        .await;
        let shutdown = client.shutdown().await;
        result?;
        shutdown?;
    }
    Ok(artifact)
}

pub fn compile_configured(
    workspace: &Path,
    config: &AppConfig,
    command: &str,
) -> Result<CompiledMcpCall> {
    load_configured_artifacts(workspace, config)?.compile(command)
}

pub async fn call_configured(
    workspace: &Path,
    config: &AppConfig,
    command: &str,
) -> Result<RawToolOutput> {
    let artifacts = load_configured_artifacts(workspace, config)?;
    let compiled = artifacts.compile(command)?;
    let server = configured_server(config, &compiled.source)?;
    let client = connect_configured(workspace, &compiled.source, server).await?;
    let runtime = McpRuntime::new(artifacts, [client.clone()])?;
    let result = runtime.call(command).await;
    let shutdown = client.shutdown().await;
    let output = result?;
    shutdown?;
    Ok(output)
}

fn resolve_workspace_path(workspace: &Path, path: &Path) -> std::path::PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        workspace.join(path)
    }
}
