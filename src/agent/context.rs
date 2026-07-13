use std::{fs, path::Path};

use anyhow::{Context, Result};

use crate::memory::MemoryPaths;

pub fn build_system_prompt(
    workspace: &Path,
    skill_catalog: &str,
    memory: Option<&MemoryPaths>,
) -> Result<String> {
    let mut sections = vec![BASE_INSTRUCTIONS.trim().to_owned()];

    let agents_path = workspace.join("AGENTS.md");
    if agents_path.is_file() {
        let agents = fs::read_to_string(&agents_path)
            .with_context(|| format!("failed to read {}", agents_path.display()))?;
        sections.push(format!("# Project instructions\n\n{}", agents.trim()));
    }

    if !skill_catalog.trim().is_empty() {
        sections.push(format!("# Available skills\n\n{}", skill_catalog.trim()));
    }

    if let Some(memory) = memory {
        sections.push(memory.prompt_section());
    }

    Ok(sections.join("\n\n"))
}

const BASE_INSTRUCTIONS: &str = r#"
# Picoagent

Work autonomously toward the requested outcome. Use tools when evidence or file
changes are needed. Tools run with the same host permissions as picoagent; this
runtime does not provide a security sandbox.

Use `read` for known files and `bash` with `rg` for local discovery. Use
`web_search` only for internet research. `write` can replace a complete file or
apply several exact, non-overlapping edits atomically.

Direct tool calls are synchronous. Use `spawn` only for independent work that
can safely continue in the background, and use `wait` before consuming a
background result or depending on a background mutation. Background results
arrive as new runtime messages when they complete.

Tool results can be truncated. A truncated result includes a stable artifact
path under `.pico/runs/<run-id>/artifacts/`. Use `read` with offset/limit or
`bash` with `rg` to inspect the complete result instead of rerunning the tool.

Keep the final answer concise and include paths to important artifacts.
"#;

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn appends_project_instructions_after_stable_base_prompt() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("AGENTS.md"), "Run cargo test.").unwrap();
        let prompt = build_system_prompt(directory.path(), "- review: Review code", None).unwrap();
        assert!(prompt.starts_with("# Picoagent"));
        assert!(prompt.contains("Run cargo test."));
        assert!(prompt.contains("review: Review code"));
    }
}
