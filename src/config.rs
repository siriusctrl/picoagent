use std::{
    collections::{BTreeMap, BTreeSet},
    env, fmt, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub use crate::model::{ModelModality, OpenAiProtocol};

const DEFAULT_OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub provider: ProviderConfig,
    pub runtime: RuntimeConfig,
    pub compaction: CompactionConfig,
    pub handles: HandleConfig,
    pub agents: AgentProfilesConfig,
    pub artifacts: ArtifactConfig,
    pub memory: MemoryConfig,
    pub web_search: WebSearchConfig,
    pub mcp: BTreeMap<String, McpServerConfig>,
    pub hooks: HookConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CompactionConfig {
    /// Tracked input-token threshold that enables automatic compaction.
    /// `None` keeps compaction disabled because provider context windows vary.
    pub compact_at_tokens: Option<u64>,
    /// Nominal full context window checked with a provider-neutral estimate.
    pub context_window_tokens: Option<u64>,
    pub keep_recent_tokens: u64,
    pub summary_max_output_tokens: u32,
    pub history_search_max_matches: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            compact_at_tokens: None,
            context_window_tokens: None,
            keep_recent_tokens: 20_000,
            summary_max_output_tokens: 4_096,
            history_search_max_matches: 50,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ProviderConfig {
    OpenaiOauth {
        model: String,
        #[serde(default = "default_model_modalities")]
        modalities: BTreeSet<ModelModality>,
        #[serde(default)]
        base_url: Option<String>,
        #[serde(default)]
        auth_file: Option<PathBuf>,
    },
    OpenaiCompatible {
        model: String,
        #[serde(default = "default_model_modalities")]
        modalities: BTreeSet<ModelModality>,
        base_url: String,
        #[serde(default, skip_serializing)]
        api_key: Option<String>,
        #[serde(default = "default_openai_protocol")]
        protocol: OpenAiProtocol,
        #[serde(default)]
        reasoning_effort: Option<String>,
    },
    AnthropicCompatible {
        model: String,
        #[serde(default = "default_model_modalities")]
        modalities: BTreeSet<ModelModality>,
        base_url: String,
        #[serde(default = "default_anthropic_key_env")]
        api_key_env: String,
        #[serde(default)]
        anthropic_version: Option<String>,
    },
    Echo {
        #[serde(default = "default_echo_model")]
        model: String,
        #[serde(default = "default_model_modalities")]
        modalities: BTreeSet<ModelModality>,
    },
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenaiOauth {
                model,
                modalities,
                base_url,
                auth_file,
            } => formatter
                .debug_struct("OpenaiOauth")
                .field("model", model)
                .field("modalities", modalities)
                .field("base_url", base_url)
                .field("auth_file", auth_file)
                .finish(),
            Self::OpenaiCompatible {
                model,
                modalities,
                base_url,
                api_key,
                protocol,
                reasoning_effort,
            } => formatter
                .debug_struct("OpenaiCompatible")
                .field("model", model)
                .field("modalities", modalities)
                .field("base_url", base_url)
                .field("api_key", &api_key.as_ref().map(|_| "[REDACTED]"))
                .field("protocol", protocol)
                .field("reasoning_effort", reasoning_effort)
                .finish(),
            Self::AnthropicCompatible {
                model,
                modalities,
                base_url,
                api_key_env,
                anthropic_version,
            } => formatter
                .debug_struct("AnthropicCompatible")
                .field("model", model)
                .field("modalities", modalities)
                .field("base_url", base_url)
                .field("api_key_env", api_key_env)
                .field("anthropic_version", anthropic_version)
                .finish(),
            Self::Echo { model, modalities } => formatter
                .debug_struct("Echo")
                .field("model", model)
                .field("modalities", modalities)
                .finish(),
        }
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self::Echo {
            model: default_echo_model(),
            modalities: default_model_modalities(),
        }
    }
}

impl ProviderConfig {
    pub fn model(&self) -> &str {
        match self {
            Self::OpenaiOauth { model, .. }
            | Self::OpenaiCompatible { model, .. }
            | Self::AnthropicCompatible { model, .. }
            | Self::Echo { model, .. } => model,
        }
    }

    pub fn modalities(&self) -> &BTreeSet<ModelModality> {
        match self {
            Self::OpenaiOauth { modalities, .. }
            | Self::OpenaiCompatible { modalities, .. }
            | Self::AnthropicCompatible { modalities, .. }
            | Self::Echo { modalities, .. } => modalities,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeConfig {
    pub max_subagent_depth: usize,
    pub max_parallel_subagents: usize,
    pub max_parallel_model_calls: usize,
    pub model_stream_idle_timeout_seconds: u64,
    pub model_request_deadline_seconds: u64,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HandleConfig {
    pub foreground_tool_timeout_seconds: u64,
    pub wait_timeout_seconds: u64,
}

impl Default for HandleConfig {
    fn default() -> Self {
        Self {
            foreground_tool_timeout_seconds: 30,
            wait_timeout_seconds: 10,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentProfilesConfig {
    pub general_task: GeneralTaskConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneralTaskConfig {
    pub model: Option<String>,
    pub max_output_tokens: Option<u32>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            max_subagent_depth: 1,
            max_parallel_subagents: 4,
            max_parallel_model_calls: 1,
            model_stream_idle_timeout_seconds: 300,
            model_request_deadline_seconds: 3_600,
            max_output_tokens: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ArtifactConfig {
    pub inline_bytes: usize,
    pub preview_head_bytes: usize,
    pub preview_tail_bytes: usize,
}

impl Default for ArtifactConfig {
    fn default() -> Self {
        Self {
            inline_bytes: 32 * 1024,
            preview_head_bytes: 8 * 1024,
            preview_tail_bytes: 8 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
#[serde(default, deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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

        let workspace_path = workspace.join(".fiasco/config.toml");
        if workspace_path.is_file() {
            return read_config(&workspace_path);
        }

        if let Some(home) = env::var_os("HOME") {
            let user_path = PathBuf::from(home).join(".fiasco/config.toml");
            if user_path.is_file() {
                return read_config(&user_path);
            }
        }

        Ok(Self::default())
    }

    fn validate(&self) -> Result<()> {
        if !self.provider.modalities().contains(&ModelModality::Text) {
            bail!("`provider.modalities` must include `text`")
        }
        if self.runtime.max_parallel_subagents == 0 {
            bail!("`runtime.max_parallel_subagents` must be greater than zero")
        }
        if self.runtime.max_parallel_model_calls == 0 {
            bail!("`runtime.max_parallel_model_calls` must be greater than zero")
        }
        if self.runtime.model_stream_idle_timeout_seconds == 0 {
            bail!("`runtime.model_stream_idle_timeout_seconds` must be greater than zero")
        }
        if self.runtime.model_request_deadline_seconds == 0 {
            bail!("`runtime.model_request_deadline_seconds` must be greater than zero")
        }
        if self.runtime.max_output_tokens == Some(0) {
            bail!("`runtime.max_output_tokens` must be greater than zero")
        }
        if self.agents.general_task.max_output_tokens == Some(0) {
            bail!("`agents.general_task.max_output_tokens` must be greater than zero")
        }
        for (name, value) in [
            (
                "handles.foreground_tool_timeout_seconds",
                self.handles.foreground_tool_timeout_seconds,
            ),
            (
                "handles.wait_timeout_seconds",
                self.handles.wait_timeout_seconds,
            ),
        ] {
            if value == 0 {
                bail!("`{name}` must be greater than zero")
            }
        }
        if self.handles.wait_timeout_seconds >= self.handles.foreground_tool_timeout_seconds {
            bail!(
                "`handles.wait_timeout_seconds` must be strictly less than `handles.foreground_tool_timeout_seconds`"
            )
        }
        if self.compaction.compact_at_tokens == Some(0) {
            bail!("`compaction.compact_at_tokens` must be greater than zero")
        }
        if self.compaction.context_window_tokens == Some(0) {
            bail!("`compaction.context_window_tokens` must be greater than zero")
        }
        if self.compaction.keep_recent_tokens == 0 {
            bail!("`compaction.keep_recent_tokens` must be greater than zero")
        }
        if let (Some(compact_at), Some(context_window)) = (
            self.compaction.compact_at_tokens,
            self.compaction.context_window_tokens,
        ) && compact_at >= context_window
        {
            bail!(
                "`compaction.compact_at_tokens` must be less than `compaction.context_window_tokens`"
            )
        }
        if self
            .compaction
            .context_window_tokens
            .is_some_and(|window| self.compaction.keep_recent_tokens >= window)
        {
            bail!(
                "`compaction.keep_recent_tokens` must be less than `compaction.context_window_tokens`"
            )
        }
        if self.compaction.context_window_tokens.is_some()
            && self.runtime.max_output_tokens.is_none()
        {
            bail!(
                "`runtime.max_output_tokens` must be set when `compaction.context_window_tokens` is configured"
            )
        }
        if self.compaction.summary_max_output_tokens == 0 {
            bail!("`compaction.summary_max_output_tokens` must be greater than zero")
        }
        if self.compaction.history_search_max_matches == 0 {
            bail!("`compaction.history_search_max_matches` must be greater than zero")
        }
        if self
            .memory
            .global_root
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
        {
            bail!("`memory.global_root` must be an absolute path")
        }
        Ok(())
    }
}

fn read_config(path: &Path) -> Result<AppConfig> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    let config: AppConfig =
        toml::from_str(&source).with_context(|| format!("invalid config {}", path.display()))?;
    config
        .validate()
        .with_context(|| format!("invalid config {}", path.display()))?;
    Ok(config)
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

pub fn resolve_openai_api_key(api_key: Option<&str>) -> Result<String> {
    match api_key {
        Some(value) => resolve_env_reference(value),
        None => env::var(DEFAULT_OPENAI_API_KEY_ENV)
            .with_context(|| format!("missing provider credential `{DEFAULT_OPENAI_API_KEY_ENV}`")),
    }
}
fn default_openai_protocol() -> OpenAiProtocol {
    OpenAiProtocol::ChatCompletions
}
fn default_anthropic_key_env() -> String {
    "ANTHROPIC_API_KEY".to_owned()
}
fn default_echo_model() -> String {
    "echo".to_owned()
}
fn default_model_modalities() -> BTreeSet<ModelModality> {
    BTreeSet::from([ModelModality::Text])
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
            modalities = ["text", "image"]
            base_url = "http://localhost:8000/v1"
            api_key = "inline-token"
            protocol = "chat-completions"
            reasoning_effort = "high"

            "#,
        )
        .unwrap();

        assert_eq!(config.provider.model(), "local-model");
        assert_eq!(
            config.provider.modalities(),
            &BTreeSet::from([ModelModality::Text, ModelModality::Image])
        );
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
    }

    #[test]
    fn model_modalities_default_to_text_and_require_text() {
        let default: AppConfig = toml::from_str("").unwrap();
        assert_eq!(
            default.provider.modalities(),
            &BTreeSet::from([ModelModality::Text])
        );

        let image_only: AppConfig = toml::from_str(
            r#"
            [provider]
            kind = "echo"
            modalities = ["image"]
            "#,
        )
        .unwrap();
        assert!(image_only.validate().is_err());
        assert!(
            toml::from_str::<AppConfig>(
                r#"
                [provider]
                kind = "echo"
                modalities = ["video"]
                "#,
            )
            .is_err()
        );
    }

    #[test]
    fn parses_compaction_settings_and_keeps_safe_defaults() {
        let configured: AppConfig = toml::from_str(
            r#"
            [compaction]
            compact_at_tokens = 120000
            context_window_tokens = 131072
            keep_recent_tokens = 16000
            summary_max_output_tokens = 2048
            history_search_max_matches = 25
            "#,
        )
        .unwrap();
        assert_eq!(configured.compaction.compact_at_tokens, Some(120_000));
        assert_eq!(configured.compaction.context_window_tokens, Some(131_072));
        assert_eq!(configured.compaction.keep_recent_tokens, 16_000);
        assert_eq!(configured.compaction.summary_max_output_tokens, 2_048);
        assert_eq!(configured.compaction.history_search_max_matches, 25);

        let defaults: AppConfig = toml::from_str("").unwrap();
        assert_eq!(defaults.compaction.compact_at_tokens, None);
        assert_eq!(defaults.compaction.context_window_tokens, None);
        assert_eq!(defaults.compaction.keep_recent_tokens, 20_000);
        assert_eq!(defaults.compaction.summary_max_output_tokens, 4_096);
        assert_eq!(defaults.compaction.history_search_max_matches, 50);
        assert_eq!(defaults.agents.general_task.max_output_tokens, None);
    }

    #[test]
    fn rejects_zero_compaction_limits() {
        for source in [
            "[compaction]\ncompact_at_tokens = 0",
            "[compaction]\ncontext_window_tokens = 0",
            "[compaction]\nkeep_recent_tokens = 0",
            "[compaction]\nsummary_max_output_tokens = 0",
            "[compaction]\nhistory_search_max_matches = 0",
        ] {
            let config: AppConfig = toml::from_str(source).unwrap();
            assert!(config.validate().is_err(), "accepted {source}");
        }
    }

    #[test]
    fn rejects_compaction_trigger_at_or_above_context_window() {
        for source in [
            "[runtime]\nmax_output_tokens = 10\n[compaction]\ncompact_at_tokens = 100\ncontext_window_tokens = 100",
            "[runtime]\nmax_output_tokens = 10\n[compaction]\ncompact_at_tokens = 101\ncontext_window_tokens = 100",
            "[runtime]\nmax_output_tokens = 10\n[compaction]\nkeep_recent_tokens = 100\ncontext_window_tokens = 100",
        ] {
            let config: AppConfig = toml::from_str(source).unwrap();
            assert!(config.validate().is_err(), "accepted {source}");
        }
    }

    #[test]
    fn context_window_requires_a_root_output_limit() {
        let config: AppConfig =
            toml::from_str("[compaction]\ncompact_at_tokens = 100\ncontext_window_tokens = 200")
                .unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_deprecated_openai_api_key_env_and_unknown_fields() {
        for source in [
            r#"
            [provider]
            kind = "openai-compatible"
            model = "legacy-model"
            base_url = "http://localhost:8000/v1"
            api_key_env = "LEGACY_OPENAI_API_KEY"
            "#,
            "[runtime]\nmax_step = 12",
            "unknown_section = true",
            r#"
            [mcp.example]
            command = "example"
            arguments = []
            "#,
        ] {
            assert!(
                toml::from_str::<AppConfig>(source).is_err(),
                "accepted unknown field in {source}"
            );
        }
    }

    #[test]
    fn openai_compatible_defaults_to_chat_completions() {
        let config: AppConfig = toml::from_str(
            r#"
            [provider]
            kind = "openai-compatible"
            model = "model"
            base_url = "http://localhost:8000/v1"
            api_key = "token"
            "#,
        )
        .unwrap();

        let ProviderConfig::OpenaiCompatible { protocol, .. } = config.provider else {
            panic!("expected openai-compatible provider");
        };
        assert_eq!(protocol, OpenAiProtocol::ChatCompletions);
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
    fn resolves_openai_literal_environment_and_default_credentials() {
        assert_eq!(
            resolve_openai_api_key(Some("inline-token")).unwrap(),
            "inline-token"
        );
        assert_eq!(
            resolve_openai_api_key(Some("${PATH}")).unwrap(),
            env::var("PATH").unwrap()
        );
        match env::var(DEFAULT_OPENAI_API_KEY_ENV) {
            Ok(expected) => {
                assert_eq!(resolve_openai_api_key(None).unwrap(), expected);
            }
            Err(_) => {
                let error = resolve_openai_api_key(None).unwrap_err().to_string();
                assert!(error.contains(DEFAULT_OPENAI_API_KEY_ENV), "{error}");
            }
        }
    }

    #[test]
    fn rejects_zero_runtime_and_handle_limits() {
        for source in [
            "[runtime]\nmax_parallel_subagents = 0",
            "[runtime]\nmax_parallel_model_calls = 0",
            "[runtime]\nmodel_stream_idle_timeout_seconds = 0",
            "[runtime]\nmodel_request_deadline_seconds = 0",
            "[runtime]\nmax_output_tokens = 0",
            "[agents.general_task]\nmax_output_tokens = 0",
            "[handles]\nforeground_tool_timeout_seconds = 0",
            "[handles]\nwait_timeout_seconds = 0",
            "[handles]\nforeground_tool_timeout_seconds = 30\nwait_timeout_seconds = 30",
        ] {
            let config: AppConfig = toml::from_str(source).unwrap();
            assert!(config.validate().is_err(), "accepted {source}");
        }
    }

    #[test]
    fn handle_defaults_use_a_short_shared_foreground_window() {
        let config = AppConfig::default();

        assert_eq!(config.handles.foreground_tool_timeout_seconds, 30);
        assert_eq!(config.handles.wait_timeout_seconds, 10);
    }

    #[test]
    fn rejects_a_relative_memory_root() {
        let config: AppConfig = toml::from_str("[memory]\nglobal_root = 'relative'").unwrap();
        assert!(config.validate().is_err());
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
