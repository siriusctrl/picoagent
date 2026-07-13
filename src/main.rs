use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use clap::Parser;
use picoagent::{
    agent::runner::{AgentRunner, AgentRunnerConfig, RunRequest, RunnerOptions},
    artifact::{ArtifactPolicy, ArtifactStore},
    config::{AppConfig, OpenAiProtocol as ConfigOpenAiProtocol, ProviderConfig},
    events::{NdjsonEventSink, NoopEventSink, SharedEventSink},
    hooks::{CommandHook, HookEvent, HookPipeline},
    mcp::{McpStdioClient, McpStdioConfig},
    memory::{MemoryPaths, MemoryScope},
    model::{
        ModelProvider,
        anthropic_compatible::{AnthropicCompatibleOptions, AnthropicCompatibleProvider},
        echo::EchoProvider,
        openai_compatible::{OpenAiCompatibleProvider, OpenAiProtocol},
        openai_oauth::{DEFAULT_OPENAI_OAUTH_BASE_URL, OpenAiOAuthProvider},
    },
    skills::{LoadSkillTool, SkillRegistry},
    storage::RunDirStore,
    tools::{ToolRegistry, builtin},
};

mod cli;

use cli::{AuthCommand, Cli, Command, MemoryCommand, OutputFormat, SkillsCommand};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let workspace = dunce_canonicalize(&cli.workspace)?;
    let config = AppConfig::load(&workspace, cli.config.as_deref())?;
    let pico_home = pico_home()?;

    match cli.command {
        Command::Run { prompt, output } => {
            run_task(&workspace, &pico_home, config, prompt, output).await
        }
        Command::Inspect { run_id } => inspect_run(&workspace, &run_id).await,
        Command::Auth {
            command: AuthCommand::Login,
        } => login(&pico_home, &config).await,
        Command::Memory { command } => {
            memory_command(&workspace, &pico_home, config, command).await
        }
        Command::Skills {
            command: SkillsCommand::List,
        } => list_skills(&workspace),
    }
}

async fn run_task(
    workspace: &Path,
    pico_home: &Path,
    config: AppConfig,
    prompt: String,
    output: OutputFormat,
) -> Result<()> {
    run_request(
        workspace,
        pico_home,
        config,
        RunRequest::root(prompt),
        output,
    )
    .await
}

async fn run_request(
    workspace: &Path,
    pico_home: &Path,
    config: AppConfig,
    request: RunRequest,
    output: OutputFormat,
) -> Result<()> {
    let provider = build_provider(&config.provider, pico_home)?;
    let memory_home = config.memory.global_root.as_deref().unwrap_or(pico_home);
    let memory = config
        .memory
        .enabled
        .then(|| MemoryPaths::new(memory_home, workspace));
    let home = env::var_os("HOME").map(PathBuf::from);
    let skills = Arc::new(SkillRegistry::discover(workspace, home.as_deref())?);
    let mut tools = ToolRegistry::default();
    builtin::register_all(&mut tools)?;
    tools.register(Arc::new(LoadSkillTool::new(skills.clone())))?;
    if config.web_search.enabled {
        let api_key = env::var(&config.web_search.api_key_env).with_context(|| {
            format!(
                "web_search is enabled but `{}` is not set",
                config.web_search.api_key_env
            )
        })?;
        tools.register(Arc::new(builtin::WebSearchTool::with_endpoint(
            &config.web_search.endpoint,
            api_key,
            config.web_search.default_count,
        )))?;
    }

    let mut mcp_clients = Vec::new();
    for (name, server) in &config.mcp {
        let client = McpStdioClient::connect(McpStdioConfig {
            name: name.clone(),
            command: server.command.clone(),
            args: server.args.clone(),
            env: server
                .env
                .iter()
                .map(|(name, value)| Ok((name.clone(), expand_env_reference(value)?)))
                .collect::<Result<_>>()?,
            cwd: Some(workspace.to_path_buf()),
        })
        .await?;
        client.register_tools(&mut tools).await?;
        mcp_clients.push(client);
    }

    let hooks = build_hooks(&config, workspace)?;
    let extra_events: SharedEventSink = match output {
        OutputFormat::Text => Arc::new(NoopEventSink),
        OutputFormat::Ndjson => Arc::new(NdjsonEventSink),
    };
    let runner = AgentRunner::new(AgentRunnerConfig {
        provider,
        model: config.provider.model().to_owned(),
        workspace: workspace.to_path_buf(),
        skill_catalog: skills.prompt_index(),
        tools,
        artifacts: ArtifactStore::new(ArtifactPolicy {
            inline_limit_bytes: config.artifacts.inline_bytes,
            max_inline_bytes_per_run: config.artifacts.max_inline_bytes_per_run,
            preview_head_bytes: config.artifacts.preview_head_bytes,
            preview_tail_bytes: config.artifacts.preview_tail_bytes,
        }),
        store: RunDirStore::new(workspace),
        hooks,
        memory,
        extra_events,
        options: RunnerOptions {
            max_steps: config.runtime.max_steps,
            max_subagent_depth: config.runtime.max_subagent_depth,
            max_parallel_tasks: config.runtime.max_parallel_tasks,
            max_output_tokens: config.runtime.max_output_tokens,
            direct_tool_timeout_seconds: config.tasks.direct_tool_timeout_seconds,
            task_execution_timeout_seconds: config.tasks.default_execution_timeout_seconds,
            task_wait_timeout_seconds: config.tasks.default_wait_timeout_seconds,
            task_max_timeout_seconds: config.tasks.max_execution_timeout_seconds,
            general_task: picoagent::agent::GeneralTaskProfile {
                model: config.agents.general_task.model.clone(),
                max_steps: config.agents.general_task.max_steps,
                max_output_tokens: config.agents.general_task.max_output_tokens,
            },
        },
    });
    let result = runner.run(request).await;
    let mut shutdown_error = None;
    for client in mcp_clients {
        if let Err(error) = client.shutdown().await
            && shutdown_error.is_none()
        {
            shutdown_error = Some(error);
        }
    }
    let result = result?;
    if let Some(error) = shutdown_error {
        return Err(error);
    }
    if matches!(output, OutputFormat::Text) {
        println!("{}", result.final_output);
        eprintln!("run: {}", result.run_id);
    }
    Ok(())
}

fn build_provider(config: &ProviderConfig, pico_home: &Path) -> Result<Arc<dyn ModelProvider>> {
    let provider: Arc<dyn ModelProvider> = match config {
        ProviderConfig::Echo { .. } => Arc::new(EchoProvider),
        ProviderConfig::OpenaiOauth {
            base_url,
            auth_file,
            ..
        } => {
            let base_url = base_url
                .clone()
                .unwrap_or_else(|| DEFAULT_OPENAI_OAUTH_BASE_URL.to_owned());
            let auth_path = auth_file
                .clone()
                .unwrap_or_else(|| pico_home.join("auth.json"));
            Arc::new(OpenAiOAuthProvider::new(base_url, auth_path))
        }
        ProviderConfig::OpenaiCompatible {
            base_url,
            api_key_env,
            protocol,
            ..
        } => {
            let api_key = env::var(api_key_env)
                .with_context(|| format!("missing provider credential `{api_key_env}`"))?;
            let protocol = match protocol {
                ConfigOpenAiProtocol::Responses => OpenAiProtocol::Responses,
                ConfigOpenAiProtocol::ChatCompletions => OpenAiProtocol::ChatCompletions,
            };
            Arc::new(OpenAiCompatibleProvider::new(base_url, api_key, protocol))
        }
        ProviderConfig::AnthropicCompatible {
            base_url,
            api_key_env,
            anthropic_version,
            ..
        } => {
            let api_key = env::var(api_key_env)
                .with_context(|| format!("missing provider credential `{api_key_env}`"))?;
            let mut options = AnthropicCompatibleOptions::new(base_url, api_key);
            if let Some(version) = anthropic_version {
                options.anthropic_version.clone_from(version);
            }
            Arc::new(AnthropicCompatibleProvider::with_options(options))
        }
    };
    Ok(provider)
}

fn build_hooks(config: &AppConfig, workspace: &Path) -> Result<HookPipeline> {
    let mut pipeline = HookPipeline::new();
    let groups = [
        (HookEvent::RunStart, "run_start", &config.hooks.run_start),
        (HookEvent::RunEnd, "run_end", &config.hooks.run_end),
        (
            HookEvent::ToolBefore,
            "tool_before",
            &config.hooks.tool_before,
        ),
        (HookEvent::ToolAfter, "tool_after", &config.hooks.tool_after),
    ];
    for (event, prefix, commands) in groups {
        for (index, command) in commands.iter().enumerate() {
            pipeline.register(
                CommandHook::from_command_line(format!("{prefix}_{index}"), event, command)?
                    .with_cwd(workspace),
            );
        }
    }
    Ok(pipeline)
}

async fn inspect_run(workspace: &Path, run_id: &str) -> Result<()> {
    let store = RunDirStore::new(workspace);
    let run = store.load_run(run_id).await?;
    println!("{}", serde_json::to_string_pretty(&run)?);
    let final_path = store.paths(run_id).final_output;
    if let Ok(final_output) = tokio::fs::read_to_string(&final_path).await {
        println!("\n--- final ---\n{final_output}");
    }
    Ok(())
}

async fn login(pico_home: &Path, config: &AppConfig) -> Result<()> {
    let ProviderConfig::OpenaiOauth {
        base_url,
        auth_file,
        ..
    } = &config.provider
    else {
        bail!("active provider is not `openai-oauth`");
    };
    let provider = OpenAiOAuthProvider::new(
        base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_OPENAI_OAUTH_BASE_URL.to_owned()),
        auth_file
            .clone()
            .unwrap_or_else(|| pico_home.join("auth.json")),
    );
    let device = provider.request_device_code().await?;
    println!(
        "Open {} and enter code {}",
        device.verification_url, device.user_code
    );
    provider.poll_device_code(&device).await?;
    println!("OpenAI OAuth login complete.");
    Ok(())
}

async fn memory_command(
    workspace: &Path,
    pico_home: &Path,
    config: AppConfig,
    command: MemoryCommand,
) -> Result<()> {
    match command {
        MemoryCommand::Consolidate { scope } => {
            let memory_home = config.memory.global_root.as_deref().unwrap_or(pico_home);
            let paths = MemoryPaths::new(memory_home, workspace);
            let target = match scope.map(Into::into) {
                Some(MemoryScope::User) => {
                    format!("global user memory at {}", paths.user.display())
                }
                Some(MemoryScope::Project) => {
                    format!("project memory at {}", paths.project.display())
                }
                None => format!(
                    "global user memory at {} and project memory at {}",
                    paths.user.display(),
                    paths.project.display()
                ),
            };
            let prompt = format!(
                "Semantically consolidate {target}. Read the existing Markdown files, remove stale duplication, merge related durable facts, preserve useful provenance, and keep the result concise and searchable. Do not use mechanical similarity as a substitute for judgment. Return a short summary of changed files."
            );
            run_request(
                workspace,
                pico_home,
                config,
                RunRequest {
                    prompt,
                    parent_run_id: None,
                    depth: 0,
                    additional_instructions: Some(
                        "This is a memory consolidation job. Only edit the named memory directories."
                            .to_owned(),
                    ),
                    tool_allowlist: Some(vec!["read".into(), "write".into(), "bash".into()]),
                    use_general_task_profile: true,
                },
                OutputFormat::Text,
            )
            .await?;
        }
    }
    Ok(())
}

fn list_skills(workspace: &Path) -> Result<()> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let skills = SkillRegistry::discover(workspace, home.as_deref())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&skills.list().collect::<Vec<_>>())?
    );
    Ok(())
}

fn pico_home() -> Result<PathBuf> {
    if let Some(path) = env::var_os("PICO_HOME") {
        return Ok(PathBuf::from(path));
    }
    let home = env::var_os("HOME").context("HOME is not set; set PICO_HOME explicitly")?;
    Ok(PathBuf::from(home).join(".pico"))
}

fn expand_env_reference(value: &str) -> Result<String> {
    let name = value
        .strip_prefix("${")
        .and_then(|value| value.strip_suffix('}'))
        .or_else(|| value.strip_prefix('$'));
    match name {
        Some(name) => {
            env::var(name).with_context(|| format!("missing MCP environment value `{name}`"))
        }
        None => Ok(value.to_owned()),
    }
}

fn dunce_canonicalize(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path)
        .with_context(|| format!("workspace does not exist: {}", path.display()))
}
