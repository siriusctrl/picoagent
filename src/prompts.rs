use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const AGENT_PROMPTS_YAML: &str = include_str!("../prompts/agents.yaml");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPrompts {
    pub system: String,
    pub compaction_request: String,
    pub general_task: String,
}

pub fn agent_prompts() -> &'static AgentPrompts {
    static PROMPTS: OnceLock<AgentPrompts> = OnceLock::new();
    PROMPTS.get_or_init(|| {
        parse_agent_prompts(AGENT_PROMPTS_YAML).expect("embedded prompts/agents.yaml must be valid")
    })
}

fn parse_agent_prompts(source: &str) -> Result<AgentPrompts> {
    let prompts: AgentPrompts =
        serde_yaml_ng::from_str(source).context("parse agent prompt YAML")?;
    for (name, value) in [
        ("system", &prompts.system),
        ("compaction_request", &prompts.compaction_request),
        ("general_task", &prompts.general_task),
    ] {
        if value.trim().is_empty() {
            bail!("agent prompt `{name}` must not be empty")
        }
    }
    Ok(prompts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_agent_prompts_are_complete_and_fold_source_wrapping() {
        let prompts = agent_prompts();

        assert!(prompts.system.contains("workspace with the user"));
        assert!(!prompts.system.contains("workspace with\nthe user"));
        assert!(prompts.compaction_request.contains("# Compacted state"));
        assert!(!prompts.compaction_request.ends_with('\n'));
    }

    #[test]
    fn prompt_schema_rejects_unknown_or_empty_fields() {
        let unknown = format!("{AGENT_PROMPTS_YAML}\nunknown: value\n");
        assert!(parse_agent_prompts(&unknown).is_err());

        let empty = "system: value\ncompaction_request: value\ngeneral_task: ''\n";
        assert!(parse_agent_prompts(empty).is_err());
    }
}
