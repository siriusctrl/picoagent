use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const AGENT_PROMPTS_YAML: &str = include_str!("../prompts/agents.yaml");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPrompts {
    pub system: String,
    pub compaction_request: String,
    pub compaction_resume: String,
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
        ("compaction_resume", &prompts.compaction_resume),
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
        assert!(
            prompts
                .system
                .contains("starts one reusable GeneralTask agent asynchronously")
        );
        assert!(prompts.general_task.contains("complete assignment"));
        assert!(prompts.compaction_request.contains("# Compacted state"));
        assert!(!prompts.compaction_request.ends_with('\n'));
        assert!(!prompts.compaction_request.contains("history_search"));
        assert!(!prompts.compaction_request.contains("history_read"));
        assert!(prompts.compaction_resume.contains("not a final answer"));
        assert!(!prompts.compaction_resume.contains("history_search"));
        assert!(!prompts.compaction_resume.contains("history_read"));
        assert!(
            prompts
                .general_task
                .contains("task text paired with this reminder is your complete assignment and defines your immediate scope")
        );
    }

    #[test]
    fn prompt_schema_rejects_unknown_or_empty_fields() {
        let unknown = format!("{AGENT_PROMPTS_YAML}\nunknown: value\n");
        assert!(parse_agent_prompts(&unknown).is_err());

        let empty = "system: value\ncompaction_request: value\ncompaction_resume: value\ngeneral_task: ''\n";
        assert!(parse_agent_prompts(empty).is_err());
    }
}
