use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use walkdir::WalkDir;

use crate::{
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

/// The location from which a skill was discovered. Later sources override
/// earlier ones: user, workspace `.agents`, then workspace-local `skills`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    User,
    WorkspaceAgents,
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub source: SkillSource,
}

/// A progressively-loaded skill index. Discovery reads only the YAML-like
/// frontmatter; the body is read when `load` is called.
#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    skills: BTreeMap<String, SkillMetadata>,
}

impl SkillRegistry {
    pub fn discover(workspace: &Path, home: Option<&Path>) -> Result<Self> {
        let mut registry = Self::default();
        if let Some(home) = home {
            registry.scan_root(&home.join(".agents/skills"), SkillSource::User)?;
        }
        registry.scan_root(
            &workspace.join(".agents/skills"),
            SkillSource::WorkspaceAgents,
        )?;
        registry.scan_root(&workspace.join("skills"), SkillSource::Workspace)?;
        Ok(registry)
    }

    pub fn list(&self) -> impl Iterator<Item = &SkillMetadata> {
        self.skills.values()
    }

    pub fn get(&self, name: &str) -> Option<&SkillMetadata> {
        self.skills.get(name)
    }

    pub fn load(&self, name: &str) -> Result<String> {
        let skill = self
            .skills
            .get(name)
            .with_context(|| format!("unknown skill `{name}`"))?;
        std::fs::read_to_string(&skill.path).with_context(|| {
            format!(
                "failed to load skill `{name}` from {}",
                skill.path.display()
            )
        })
    }

    /// Compact prompt material: metadata only, never the skill body.
    pub fn prompt_index(&self) -> String {
        self.skills
            .values()
            .map(|skill| format!("- {}: {}", skill.name, skill.description))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn scan_root(&mut self, root: &Path, source: SkillSource) -> Result<()> {
        if !root.is_dir() {
            return Ok(());
        }

        let mut files = WalkDir::new(root)
            .min_depth(1)
            .max_depth(2)
            .follow_links(false)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file() && entry.file_name() == "SKILL.md")
            .map(|entry| entry.into_path())
            .collect::<Vec<_>>();
        files.sort();

        for path in files {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let (name, description) = parse_frontmatter(&content)
                .with_context(|| format!("invalid skill metadata in {}", path.display()))?;
            self.skills.insert(
                name.clone(),
                SkillMetadata {
                    name,
                    description,
                    path,
                    source,
                },
            );
        }
        Ok(())
    }
}

fn parse_frontmatter(content: &str) -> Result<(String, String)> {
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        bail!("SKILL.md must begin with `---` frontmatter");
    }

    let mut frontmatter = Vec::new();
    let mut closed = false;
    for line in lines {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        frontmatter.push(line);
    }
    if !closed {
        bail!("frontmatter is missing its closing `---`");
    }

    let mut name = None;
    let mut description = None;
    let mut index = 0;
    while index < frontmatter.len() {
        let line = frontmatter[index];
        let Some((key, raw_value)) = line.split_once(':') else {
            index += 1;
            continue;
        };
        let key = key.trim();
        let raw_value = raw_value.trim();
        if key == "name" {
            name = Some(unquote(raw_value).to_owned());
        } else if key == "description" {
            if matches!(raw_value, ">" | ">-" | "|" | "|-") {
                let mut parts = Vec::new();
                index += 1;
                while index < frontmatter.len() {
                    let continuation = frontmatter[index];
                    if !continuation.chars().next().is_some_and(char::is_whitespace) {
                        index -= 1;
                        break;
                    }
                    parts.push(continuation.trim());
                    index += 1;
                }
                description = Some(parts.join(" "));
            } else {
                description = Some(unquote(raw_value).to_owned());
            }
        }
        index += 1;
    }
    let name = name
        .filter(|value| !value.is_empty())
        .context("frontmatter is missing `name`")?;
    let description = description
        .filter(|value| !value.is_empty())
        .context("frontmatter is missing `description`")?;
    Ok((name, description))
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
}

#[derive(Clone)]
pub struct LoadSkillTool {
    registry: Arc<SkillRegistry>,
}

impl LoadSkillTool {
    pub fn new(registry: Arc<SkillRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for LoadSkillTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "load_skill".to_owned(),
            description: "Load the complete instructions for one skill by name. Skill metadata is available before loading; use this only when the skill applies.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, _context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let name = arguments
            .get("name")
            .and_then(Value::as_str)
            .context("`name` is required")?;
        Ok(RawToolOutput::text(self.registry.load(name)?))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn write_skill(root: &Path, dir: &str, name: &str, description: &str, body: &str) {
        let dir = root.join(dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n{body}"),
        )
        .unwrap();
    }

    #[test]
    fn discovery_is_progressive_and_workspace_overrides_user() {
        let workspace = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        write_skill(
            &home.path().join(".agents/skills"),
            "demo",
            "demo",
            "user",
            "user body",
        );
        write_skill(
            &workspace.path().join("skills"),
            "demo",
            "demo",
            "workspace",
            "workspace body",
        );

        let registry = SkillRegistry::discover(workspace.path(), Some(home.path())).unwrap();
        let metadata = registry.get("demo").unwrap();
        assert_eq!(metadata.description, "workspace");
        assert_eq!(metadata.source, SkillSource::Workspace);
        assert!(!registry.prompt_index().contains("workspace body"));
        assert!(registry.load("demo").unwrap().contains("workspace body"));
    }

    #[test]
    fn malformed_skill_is_rejected() {
        let workspace = TempDir::new().unwrap();
        let dir = workspace.path().join("skills/broken");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "# no frontmatter").unwrap();
        assert!(SkillRegistry::discover(workspace.path(), None).is_err());
    }

    #[test]
    fn folded_frontmatter_description_is_supported() {
        let content = "---\nname: demo\ndescription: >-\n  First line.\n  Second line.\n---\nbody";
        assert_eq!(
            parse_frontmatter(content).unwrap(),
            ("demo".to_owned(), "First line. Second line.".to_owned())
        );
    }
}
