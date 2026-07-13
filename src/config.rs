use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub provider: ProviderConfig,
    pub runtime: RuntimeConfig,
    pub tasks: TaskConfig,
    pub agents: AgentProfilesConfig,
    pub artifacts: ArtifactConfig,
    pub memory: MemoryConfig,
    pub web_search: WebSearchConfig,
    pub mcp: BTreeMap<String, McpServerConfig>,
    pub hooks: HookConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ProviderConfig {
    OpenaiOauth {
        model: String,
        #[serde(default)]
        base_url: Option<String>,
        #[serde(default)]
        auth_file: Option<PathBuf>,
    },
    OpenaiCompatible {
        model: String,
        base_url: String,
        #[serde(default = "default_openai_key_env")]
        api_key_env: String,
        #[serde(default)]
        protocol: OpenAiProtocol,
    },
    AnthropicCompatible {
        model: String,
        base_url: String,
        #[serde(default = "default_anthropic_key_env")]
        api_key_env: String,
        #[serde(default)]
        anthropic_version: Option<String>,
    },
    Echo {
        #[serde(default = "default_echo_model")]
        model: String,
    },
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self::Echo {
            model: default_echo_model(),
        }
    }
}

impl ProviderConfig {
    pub fn model(&self) -> &str {
        match self {
            Self::OpenaiOauth { model, .. }
            | Self::OpenaiCompatible { model, .. }
            | Self::AnthropicCompatible { model, .. }
            | Self::Echo { model } => model,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpenAiProtocol {
    Responses,
    #[default]
    ChatCompletions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    pub max_steps: usize,
    pub max_subagent_depth: usize,
    pub max_parallel_tasks: usize,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskConfig {
    pub default_execution_timeout_seconds: u64,
    pub default_wait_timeout_seconds: u64,
    pub max_execution_timeout_seconds: u64,
    pub direct_tool_timeout_seconds: u64,
}

impl Default for TaskConfig {
    fn default() -> Self {
        Self {
            default_execution_timeout_seconds: 300,
            default_wait_timeout_seconds: 30,
            max_execution_timeout_seconds: 1_800,
            direct_tool_timeout_seconds: 300,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentProfilesConfig {
    pub general_task: GeneralTaskConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralTaskConfig {
    pub model: Option<String>,
    pub max_steps: usize,
    pub max_output_tokens: Option<u32>,
}

impl Default for GeneralTaskConfig {
    fn default() -> Self {
        Self {
            model: None,
            max_steps: 8,
            max_output_tokens: Some(4_096),
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            max_steps: 32,
            max_subagent_depth: 1,
            max_parallel_tasks: 4,
            max_output_tokens: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtifactConfig {
    pub inline_bytes: usize,
    pub max_inline_bytes_per_run: usize,
    pub preview_head_bytes: usize,
    pub preview_tail_bytes: usize,
}

impl Default for ArtifactConfig {
    fn default() -> Self {
        Self {
            inline_bytes: 32 * 1024,
            max_inline_bytes_per_run: 128 * 1024,
            preview_head_bytes: 8 * 1024,
            preview_tail_bytes: 8 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    pub enabled: bool,
    pub global_root: Option<PathBuf>,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            global_root: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebSearchConfig {
    pub enabled: bool,
    pub endpoint: String,
    pub api_key_env: String,
    pub default_count: usize,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "https://api.search.brave.com/res/v1/web/search".to_owned(),
            api_key_env: "BRAVE_SEARCH_API_KEY".to_owned(),
            default_count: 8,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HookConfig {
    pub run_start: Vec<String>,
    pub run_end: Vec<String>,
    pub tool_before: Vec<String>,
    pub tool_after: Vec<String>,
}

impl AppConfig {
    pub fn load(workspace: &Path, explicit: Option<&Path>) -> Result<Self> {
        if let Some(path) = explicit {
            return read_config(path);
        }

        let workspace_path = workspace.join(".pico/config.toml");
        if workspace_path.is_file() {
            return read_config(&workspace_path);
        }

        if let Some(home) = env::var_os("HOME") {
            let user_path = PathBuf::from(home).join(".pico/config.toml");
            if user_path.is_file() {
                return read_config(&user_path);
            }
        }

        Ok(Self::default())
    }
}

fn read_config(path: &Path) -> Result<AppConfig> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    toml::from_str(&source).with_context(|| format!("invalid config {}", path.display()))
}

fn default_openai_key_env() -> String {
    "OPENAI_API_KEY".to_owned()
}
fn default_anthropic_key_env() -> String {
    "ANTHROPIC_API_KEY".to_owned()
}
fn default_echo_model() -> String {
    "echo".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openai_compatible_config() {
        let config: AppConfig = toml::from_str(
            r#"
            [provider]
            kind = "openai-compatible"
            model = "local-model"
            base_url = "http://localhost:8000/v1"
            protocol = "chat-completions"

            [runtime]
            max_steps = 12
            "#,
        )
        .unwrap();

        assert_eq!(config.provider.model(), "local-model");
        assert_eq!(config.runtime.max_steps, 12);
    }
}
