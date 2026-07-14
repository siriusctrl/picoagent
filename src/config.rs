use std::{
    collections::BTreeMap,
    env, fmt, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const DEFAULT_OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";

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

#[derive(Clone, Serialize, Deserialize)]
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
        #[serde(default, skip_serializing)]
        api_key: Option<String>,
        #[serde(default)]
        api_key_env: Option<String>,
        #[serde(default)]
        protocol: OpenAiProtocol,
        #[serde(default)]
        reasoning_effort: Option<String>,
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

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenaiOauth {
                model,
                base_url,
                auth_file,
            } => formatter
                .debug_struct("OpenaiOauth")
                .field("model", model)
                .field("base_url", base_url)
                .field("auth_file", auth_file)
                .finish(),
            Self::OpenaiCompatible {
                model,
                base_url,
                api_key,
                api_key_env,
                protocol,
                reasoning_effort,
            } => formatter
                .debug_struct("OpenaiCompatible")
                .field("model", model)
                .field("base_url", base_url)
                .field("api_key", &api_key.as_ref().map(|_| "[REDACTED]"))
                .field("api_key_env", api_key_env)
                .field("protocol", protocol)
                .field("reasoning_effort", reasoning_effort)
                .finish(),
            Self::AnthropicCompatible {
                model,
                base_url,
                api_key_env,
                anthropic_version,
            } => formatter
                .debug_struct("AnthropicCompatible")
                .field("model", model)
                .field("base_url", base_url)
                .field("api_key_env", api_key_env)
                .field("anthropic_version", anthropic_version)
                .finish(),
            Self::Echo { model } => formatter
                .debug_struct("Echo")
                .field("model", model)
                .finish(),
        }
    }
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

pub fn resolve_env_reference(value: &str) -> Result<String> {
    let name = value
        .strip_prefix("${")
        .and_then(|value| value.strip_suffix('}'))
        .or_else(|| value.strip_prefix('$'));
    match name {
        Some("") => bail!("environment reference must include a variable name"),
        Some(name) => env::var(name).with_context(|| format!("missing environment value `{name}`")),
        None => Ok(value.to_owned()),
    }
}

pub fn resolve_openai_api_key(api_key: Option<&str>, api_key_env: Option<&str>) -> Result<String> {
    match (api_key, api_key_env) {
        (Some(_), Some(_)) => {
            bail!("set only one of `provider.api_key` or deprecated `provider.api_key_env`")
        }
        (Some(value), None) => resolve_env_reference(value),
        (None, Some("")) => bail!("`provider.api_key_env` must include a variable name"),
        (None, Some(name)) => {
            env::var(name).with_context(|| format!("missing provider credential `{name}`"))
        }
        (None, None) => env::var(DEFAULT_OPENAI_API_KEY_ENV)
            .with_context(|| format!("missing provider credential `{DEFAULT_OPENAI_API_KEY_ENV}`")),
    }
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
            api_key = "inline-token"
            protocol = "chat-completions"
            reasoning_effort = "high"

            [runtime]
            max_steps = 12
            "#,
        )
        .unwrap();

        assert_eq!(config.provider.model(), "local-model");
        let ProviderConfig::OpenaiCompatible {
            api_key,
            reasoning_effort,
            ..
        } = &config.provider
        else {
            panic!("expected openai-compatible provider");
        };
        assert_eq!(api_key.as_deref(), Some("inline-token"));
        assert_eq!(reasoning_effort.as_deref(), Some("high"));
        assert_eq!(config.runtime.max_steps, 12);
    }

    #[test]
    fn parses_legacy_openai_api_key_env() {
        let config: AppConfig = toml::from_str(
            r#"
            [provider]
            kind = "openai-compatible"
            model = "legacy-model"
            base_url = "http://localhost:8000/v1"
            api_key_env = "LEGACY_OPENAI_API_KEY"
            "#,
        )
        .unwrap();

        let ProviderConfig::OpenaiCompatible {
            api_key,
            api_key_env,
            ..
        } = config.provider
        else {
            panic!("expected openai-compatible provider");
        };
        assert!(api_key.is_none());
        assert_eq!(api_key_env.as_deref(), Some("LEGACY_OPENAI_API_KEY"));
    }

    #[test]
    fn resolves_literal_and_braced_environment_values() {
        assert_eq!(
            resolve_env_reference("inline-token").unwrap(),
            "inline-token"
        );
        assert_eq!(
            resolve_env_reference("${PATH}").unwrap(),
            env::var("PATH").unwrap()
        );
        assert!(resolve_env_reference("${}").is_err());
    }

    #[test]
    fn resolves_new_and_legacy_openai_credentials_without_ambiguity() {
        assert_eq!(
            resolve_openai_api_key(Some("inline-token"), None).unwrap(),
            "inline-token"
        );
        assert_eq!(
            resolve_openai_api_key(Some("${PATH}"), None).unwrap(),
            env::var("PATH").unwrap()
        );
        assert_eq!(
            resolve_openai_api_key(None, Some("PATH")).unwrap(),
            env::var("PATH").unwrap()
        );
        match env::var(DEFAULT_OPENAI_API_KEY_ENV) {
            Ok(expected) => {
                assert_eq!(resolve_openai_api_key(None, None).unwrap(), expected);
            }
            Err(_) => {
                let error = resolve_openai_api_key(None, None).unwrap_err().to_string();
                assert!(error.contains(DEFAULT_OPENAI_API_KEY_ENV), "{error}");
            }
        }
        assert!(resolve_openai_api_key(Some("token"), Some("PATH")).is_err());
    }

    #[test]
    fn provider_debug_and_serialization_redact_literal_api_keys() {
        let config: AppConfig = toml::from_str(
            r#"
            [provider]
            kind = "openai-compatible"
            model = "model"
            base_url = "http://localhost:8000/v1"
            api_key = "super-secret-token"
            "#,
        )
        .unwrap();

        let debug = format!("{:?}", config.provider);
        let serialized = toml::to_string(&config.provider).unwrap();
        assert!(!debug.contains("super-secret-token"));
        assert!(debug.contains("[REDACTED]"));
        assert!(!serialized.contains("super-secret-token"));
    }
}
