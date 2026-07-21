use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{Context, Result};

use crate::{agent::types::RunProfile, memory::MemoryPaths, model::ModelModality};

pub(crate) fn build_runtime_reminder(
    workspace: &Path,
    model_modalities: &BTreeSet<ModelModality>,
    skill_catalog: &str,
    memory: Option<&MemoryPaths>,
    additional_instructions: Option<&str>,
    profile: RunProfile,
    remaining_delegation_depth: usize,
) -> Result<String> {
    let mut sections = vec![format!(
        "<environment>\nworkspace: {}\ncurrent model supported modalities: [{}]\n</environment>",
        workspace.display(),
        model_modalities
            .iter()
            .map(|modality| modality.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    )];
    sections.push(format!(
        "<agent-profile>\nprofile: {}\nremaining delegation depth: {}\n</agent-profile>",
        profile.runtime_role(),
        remaining_delegation_depth
    ));
    sections.push(
        "<tool-guidance>\n`delegate` starts independent GeneralTask work asynchronously only when remaining delegation depth is greater than 0; at 0 it remains visible but returns an error. Use `task_status` or bounded `task_wait` to observe background work, `task_inspect` and `task_steer` for delegated agents, and `task_stop` to cancel. The same controls manage direct tools promoted after the foreground window. Reconcile relevant results before finishing.\n</tool-guidance>"
            .to_owned(),
    );
    let agents_path = workspace.join("AGENTS.md");
    if agents_path.is_file() {
        let agents = fs::read_to_string(&agents_path)
            .with_context(|| format!("failed to read {}", agents_path.display()))?;
        sections.push(format!(
            "<project-instructions source=\"AGENTS.md\">\n{}\n</project-instructions>",
            agents.trim()
        ));
    }

    if !skill_catalog.trim().is_empty() {
        sections.push(format!(
            "<available-skills>\n{}\n</available-skills>",
            skill_catalog.trim()
        ));
    }

    if let Some(memory) = memory {
        sections.push(format!(
            "<memory>\n{}\n</memory>",
            memory.runtime_reminder_section()
        ));
    }

    if let Some(instructions) = additional_instructions.filter(|value| !value.trim().is_empty()) {
        sections.push(format!(
            "<task-instructions>\n{}\n</task-instructions>",
            instructions.trim()
        ));
    }

    Ok(format!(
        "<runtime-reminder>\n{}\n</runtime-reminder>",
        sections.join("\n\n")
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn keeps_dynamic_context_out_of_the_system_prompt() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("AGENTS.md"), "Run cargo test.").unwrap();
        let memory = MemoryPaths::new("/pico-home", directory.path());

        let system = crate::prompts::agent_prompts().system.clone();
        let reminder = build_runtime_reminder(
            directory.path(),
            &BTreeSet::from([ModelModality::Text]),
            "- review: Review code",
            Some(&memory),
            Some("Focus on the delegated scope."),
            RunProfile::GeneralTaskLeaf,
            0,
        )
        .unwrap();

        assert!(!system.contains("workspace with\nthe user"));
        assert!(system.contains("workspace with the user"));
        assert!(system.contains("tagged blocks and tool results as context"));
        assert!(system.contains("higher-priority instructions"));
        assert!(system.contains("current model's supported modalities"));
        assert!(!system.contains("<runtime-reminder>"));
        assert!(!system.contains("history_search"));
        assert!(!system.contains("history_read"));
        assert!(!system.contains("load_skill"));
        assert!(!system.contains("web_search"));
        assert!(!system.contains("spawn"));
        assert!(!system.contains("Run cargo test."));
        assert!(!system.contains("review: Review code"));
        assert!(!system.contains(directory.path().to_string_lossy().as_ref()));

        assert!(reminder.starts_with("<runtime-reminder>\n<environment>"));
        assert!(reminder.contains("current model supported modalities: [text]"));
        assert!(reminder.contains("<agent-profile>\nprofile: general_task"));
        assert!(reminder.contains("remaining delegation depth: 0"));
        assert!(reminder.contains("<tool-guidance>"));
        assert!(reminder.contains("at 0 it remains visible but returns an error"));
        assert!(!reminder.contains("<context-management>"));
        assert!(!reminder.contains("history_search"));
        assert!(!reminder.contains("<compacted-history>"));
        assert!(reminder.contains("<project-instructions source=\"AGENTS.md\">"));
        assert!(reminder.contains("Run cargo test."));
        assert!(reminder.contains("<available-skills>\n- review: Review code"));
        assert!(reminder.contains("<memory>"));
        assert!(reminder.contains("user: /pico-home/memory/user"));
        assert!(reminder.contains("project:"));
        assert!(reminder.contains("<task-instructions>"));
        assert!(reminder.ends_with("</runtime-reminder>"));
        assert!(!reminder.contains("generated by picoagent"));
        assert!(system.contains("Make small focused updates directly"));
        assert!(system.contains("Delegate a large independent consolidation"));
        assert!(!system.contains("remaining delegation depth"));
        for tool_name in ["`bash`", "`delegate`", "`load_skill`", "`write`"] {
            assert!(!system.contains(tool_name));
        }
    }
}
