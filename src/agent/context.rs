use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{Context, Result};

use crate::{
    agent::{task::BackgroundTaskRecord, types::RunProfile},
    memory::MemoryPaths,
    model::{Message, MessageContent, ModelModality, Role, active_background_tasks_section},
    prompts::agent_prompts,
};

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

pub(crate) fn append_active_task_reminder(
    messages: &mut Vec<Message>,
    active_tasks: &[BackgroundTaskRecord],
) {
    let Some(section) = active_background_tasks_section(
        active_tasks
            .iter()
            .map(|task| (task.id.as_str(), task.name.as_str(), task.status())),
    ) else {
        return;
    };

    let compaction_resume = agent_prompts().compaction_resume.trim();
    let continuation = messages
        .iter_mut()
        .flat_map(|message| message.content.iter_mut())
        .find_map(|content| match content {
            MessageContent::RuntimeReminder { text } if text.contains(compaction_resume) => {
                Some(text)
            }
            _ => None,
        });
    if let Some(continuation) = continuation
        && let Some(prefix) = continuation.strip_suffix("\n</runtime-reminder>")
    {
        *continuation = format!("{prefix}\n\n{section}\n</runtime-reminder>");
        return;
    }

    messages.push(Message {
        role: Role::User,
        content: vec![MessageContent::RuntimeReminder {
            text: format!("<runtime-reminder>\n{section}\n</runtime-reminder>"),
        }],
    });
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
        let memory = MemoryPaths::new("/fiasco-home", directory.path());

        let system = crate::prompts::agent_prompts().system.clone();
        let reminder = build_runtime_reminder(
            directory.path(),
            &BTreeSet::from([ModelModality::Text]),
            "- review: Review code",
            Some(&memory),
            Some(crate::prompts::agent_prompts().general_task.as_str()),
            RunProfile::GeneralTaskLeaf,
            0,
        )
        .unwrap();

        assert!(!system.contains("workspace with\nthe user"));
        assert!(system.contains("workspace with the user"));
        assert!(system.contains("tagged blocks and tool results as context"));
        assert!(system.contains("higher-priority instructions"));
        assert!(system.contains("starts one reusable GeneralTask agent asynchronously"));
        assert!(
            crate::prompts::agent_prompts()
                .general_task
                .contains("complete assignment")
        );
        assert!(system.contains("current model's supported modalities"));
        assert!(!system.contains("<runtime-reminder>"));
        assert!(system.contains("`history_search` and `history_read`"));
        assert!(system.contains("exact omitted fact"));
        assert!(system.contains("Task ids are local to the run"));
        assert!(system.contains("planning graph records work topology"));
        assert!(system.contains("required work has been verified"));
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
        assert!(!reminder.contains("<tool-guidance>"));
        assert!(!reminder.contains("Task ids are local"));
        assert!(!reminder.contains("<context-management>"));
        assert!(!reminder.contains("history_search"));
        assert!(!reminder.contains("<compacted-history>"));
        assert!(reminder.contains("<project-instructions source=\"AGENTS.md\">"));
        assert!(reminder.contains("Run cargo test."));
        assert!(reminder.contains("<available-skills>\n- review: Review code"));
        assert!(reminder.contains("<memory>"));
        assert!(reminder.contains("user: /fiasco-home/memory/user"));
        assert!(reminder.contains("project:"));
        assert!(reminder.contains("<task-instructions>"));
        assert!(reminder.contains(
            "task text paired with this reminder is your complete assignment and defines your immediate scope"
        ));
        assert!(reminder.contains("including prohibitions on edits or delegation"));
        assert!(reminder.ends_with("</runtime-reminder>"));
        assert!(!reminder.contains("generated by fiasco"));
        assert!(system.contains("Make small focused updates directly"));
        assert!(system.contains("Delegate a large independent consolidation"));
        assert!(!system.contains("remaining delegation depth:"));
        for tool_name in [
            "`delegate`",
            "`task_wait`",
            "`task_list`",
            "`task_inspect`",
            "`task_send`",
            "`task_stop`",
            "`task_close`",
            "`history_search`",
            "`history_read`",
            "`write`",
            "`graph_list`",
        ] {
            assert!(system.contains(tool_name));
        }
    }
}
