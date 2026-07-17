use std::{fs, path::Path};

use anyhow::{Context, Result};

use crate::memory::MemoryPaths;

pub fn build_system_prompt() -> String {
    normalize_prompt_markdown(BASE_INSTRUCTIONS)
}

pub fn build_runtime_reminder(
    workspace: &Path,
    skill_catalog: &str,
    memory: Option<&MemoryPaths>,
    memory_update_available: bool,
    additional_instructions: Option<&str>,
) -> Result<String> {
    let mut sections = vec![format!(
        "<environment>\nworkspace: {}\n</environment>",
        workspace.display()
    )];
    let agents_path = workspace.join("AGENTS.md");
    if agents_path.is_file() {
        let agents = fs::read_to_string(&agents_path)
            .with_context(|| format!("failed to read {}", agents_path.display()))?;
        sections.push(format!(
            "<project-instructions source=\"AGENTS.md\">\n{}\n</project-instructions>",
            normalize_prompt_markdown(&agents)
        ));
    }

    if !skill_catalog.trim().is_empty() {
        sections.push(format!(
            "<available-skills>\n{}\n</available-skills>",
            normalize_prompt_markdown(skill_catalog)
        ));
    }

    if let Some(memory) = memory {
        sections.push(format!(
            "<memory>\n{}\n</memory>",
            memory.runtime_reminder_section(memory_update_available)
        ));
    }

    if let Some(instructions) = additional_instructions.filter(|value| !value.trim().is_empty()) {
        sections.push(format!(
            "<task-instructions>\n{}\n</task-instructions>",
            normalize_prompt_markdown(instructions)
        ));
    }

    Ok(format!(
        "<runtime-reminder>\n{}\n</runtime-reminder>",
        sections.join("\n\n")
    ))
}

const BASE_INSTRUCTIONS: &str = include_str!("../../prompts/agents/system.md");

pub(crate) fn normalize_prompt_markdown(source: &str) -> String {
    let mut output = Vec::new();
    let mut paragraph: Option<String> = None;
    let mut in_fence = false;

    let source = source.trim_matches(|character| matches!(character, '\r' | '\n'));
    for line in source.lines() {
        let trimmed = line.trim();
        let fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");

        if in_fence {
            output.push(line.to_owned());
            if fence {
                in_fence = false;
            }
            continue;
        }

        if fence {
            flush_paragraph(&mut output, &mut paragraph);
            output.push(line.to_owned());
            in_fence = true;
        } else if trimmed.is_empty() {
            flush_paragraph(&mut output, &mut paragraph);
            if output.last().is_some_and(|line| !line.is_empty()) {
                output.push(String::new());
            }
        } else if is_semantic_line(line, trimmed) {
            flush_paragraph(&mut output, &mut paragraph);
            output.push(line.to_owned());
        } else if is_list_item(trimmed) {
            flush_paragraph(&mut output, &mut paragraph);
            paragraph = Some(line.to_owned());
        } else {
            let paragraph = paragraph.get_or_insert_with(String::new);
            if !paragraph.is_empty() {
                paragraph.push(' ');
            }
            paragraph.push_str(trimmed);
        }
    }
    flush_paragraph(&mut output, &mut paragraph);
    while output.last().is_some_and(String::is_empty) {
        output.pop();
    }
    output.join("\n")
}

fn flush_paragraph(output: &mut Vec<String>, paragraph: &mut Option<String>) {
    if let Some(paragraph) = paragraph.take() {
        output.push(paragraph);
    }
}

fn is_list_item(line: &str) -> bool {
    if ["- ", "* ", "+ "]
        .iter()
        .any(|prefix| line.starts_with(prefix))
    {
        return true;
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0
        && line
            .get(digits..)
            .is_some_and(|suffix| suffix.starts_with(". ") || suffix.starts_with(") "))
}

fn is_semantic_line(original: &str, trimmed: &str) -> bool {
    let indented_code = original.starts_with('\t') || original.starts_with("    ");
    let heading = trimmed.starts_with("# ") || trimmed.starts_with("##");
    let quote = trimmed.starts_with('>');
    let tag = trimmed.starts_with('<') && trimmed.ends_with('>');
    let table = trimmed.contains('|');
    let link_definition = trimmed.starts_with('[') && trimmed.contains("]: ");
    let directive = trimmed.starts_with(":::");
    let thematic_break = trimmed.len() >= 3
        && trimmed
            .chars()
            .all(|character| matches!(character, '-' | '*' | '_' | ' '));
    let hard_break = original.ends_with("  ") || original.ends_with('\\');

    indented_code
        || heading
        || quote
        || tag
        || table
        || link_definition
        || directive
        || thematic_break
        || hard_break
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

        let system = build_system_prompt();
        let reminder = build_runtime_reminder(
            directory.path(),
            "- review: Review code",
            Some(&memory),
            true,
            Some("Focus on the delegated scope."),
        )
        .unwrap();
        let read_only_reminder =
            build_runtime_reminder(directory.path(), "", Some(&memory), false, None).unwrap();

        assert!(!system.contains("workspace with\nthe user"));
        assert!(system.contains("workspace with the user"));
        assert!(system.contains("tagged blocks and tool results as context"));
        assert!(system.contains("higher-priority instructions"));
        assert!(!system.contains("<runtime-reminder>"));
        assert!(!system.contains("history_search"));
        assert!(!system.contains("Run cargo test."));
        assert!(!system.contains("review: Review code"));
        assert!(!system.contains(directory.path().to_string_lossy().as_ref()));

        assert!(reminder.starts_with("<runtime-reminder>\n<environment>"));
        assert!(!reminder.contains("<context-management>"));
        assert!(!reminder.contains("history_search"));
        assert!(!reminder.contains("<compacted-history>"));
        assert!(reminder.contains("<project-instructions source=\"AGENTS.md\">"));
        assert!(reminder.contains("Run cargo test."));
        assert!(reminder.contains("<available-skills>\n- review: Review code"));
        assert!(reminder.contains("<memory>"));
        assert!(reminder.contains("Use `memory_update`"));
        assert!(reminder.contains("<task-instructions>"));
        assert!(reminder.ends_with("</runtime-reminder>"));
        assert!(!reminder.contains("generated by picoagent"));
        assert!(read_only_reminder.contains("Treat these memory files as read-only"));
        assert!(!read_only_reminder.contains("Use `memory_update`"));
    }

    #[test]
    fn reflows_soft_wrapping_but_preserves_markdown_boundaries() {
        let source = "A soft-wrapped\nparagraph.\n\n## Heading\n\n- one item that\n  continues here\n- another item\n\n```bash\necho one\necho two\n```";

        assert_eq!(
            normalize_prompt_markdown(source),
            "A soft-wrapped paragraph.\n\n## Heading\n\n- one item that continues here\n- another item\n\n```bash\necho one\necho two\n```"
        );
        assert_eq!(
            normalize_prompt_markdown("Line one  \nline two"),
            "Line one  \nline two"
        );
    }
}
