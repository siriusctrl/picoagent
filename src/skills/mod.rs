use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

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
        let content = std::fs::read_to_string(&skill.path).with_context(|| {
            format!(
                "failed to load skill `{name}` from {}",
                skill.path.display()
            )
        })?;
        let (_, _, body) = parse_skill_document(&content)
            .with_context(|| format!("invalid skill metadata in {}", skill.path.display()))?;
        let body = body.trim_matches(|character| matches!(character, '\r' | '\n'));
        let path = std::fs::canonicalize(&skill.path)
            .with_context(|| format!("failed to resolve skill path {}", skill.path.display()))?;
        let directory = path
            .parent()
            .context("skill path has no parent directory")?;
        Ok(format!(
            "Skill directory: {}\n\n{}",
            directory.display(),
            body
        ))
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
            let (name, description, _) = parse_skill_document(&content)
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

#[derive(Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
}

fn parse_skill_document(content: &str) -> Result<(String, String, &str)> {
    let mut lines = content.split_inclusive('\n');
    let first = lines.next().context("SKILL.md is empty")?;
    if first.trim() != "---" {
        bail!("SKILL.md must begin with `---` frontmatter");
    }

    let frontmatter_start = first.len();
    let mut frontmatter_end = None;
    let mut body_start = None;
    let mut offset = first.len();
    for line in lines {
        if line.trim() == "---" {
            frontmatter_end = Some(offset);
            body_start = Some(offset + line.len());
            break;
        }
        offset += line.len();
    }
    let frontmatter_end = frontmatter_end.context("frontmatter is missing its closing `---`")?;
    let body_start = body_start.context("frontmatter is missing its closing `---`")?;
    let metadata: SkillFrontmatter =
        serde_yaml_ng::from_str(&content[frontmatter_start..frontmatter_end])
            .context("parse SKILL.md frontmatter")?;
    let name = metadata.name.trim().to_owned();
    if name.is_empty() {
        bail!("frontmatter `name` must not be empty");
    }
    let description = metadata
        .description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if description.is_empty() {
        bail!("frontmatter `description` must not be empty");
    }
    Ok((name, description, &content[body_start..]))
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
        let loaded = registry.load("demo").unwrap();
        let directory = fs::canonicalize(workspace.path().join("skills/demo")).unwrap();
        assert_eq!(
            loaded,
            format!("Skill directory: {}\n\nworkspace body", directory.display())
        );
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
        let content = "---\nname: demo\ndescription: >-\n  First line.\n  Second line.\nlicense: MIT\nmetadata:\n  version: 1\n---\nbody";
        assert_eq!(
            parse_skill_document(content).unwrap(),
            (
                "demo".to_owned(),
                "First line. Second line.".to_owned(),
                "body"
            )
        );
    }

    #[test]
    fn loading_omits_catalog_metadata_and_exposes_resource_root() {
        let workspace = TempDir::new().unwrap();
        let skill_root = workspace.path().join("skills/composite");
        fs::create_dir_all(skill_root.join("references")).unwrap();
        fs::write(skill_root.join("references/checklist.md"), "check details").unwrap();
        write_skill(
            &workspace.path().join("skills"),
            "composite",
            "composite",
            "Already catalogued description.",
            "Read references/checklist.md before proceeding.",
        );

        let registry = SkillRegistry::discover(workspace.path(), None).unwrap();
        let loaded = registry.load("composite").unwrap();
        let skill_root = fs::canonicalize(skill_root).unwrap();

        assert_eq!(
            loaded,
            format!(
                "Skill directory: {}\n\nRead references/checklist.md before proceeding.",
                skill_root.display()
            )
        );
    }

    #[test]
    fn loading_trims_boundary_line_breaks_but_preserves_internal_whitespace() {
        let workspace = TempDir::new().unwrap();
        let body = "\r\n    indented instruction\n\nsecond instruction\n\r\n";
        write_skill(
            &workspace.path().join("skills"),
            "verbatim",
            "verbatim",
            "Catalog metadata.",
            body,
        );

        let registry = SkillRegistry::discover(workspace.path(), None).unwrap();
        let loaded = registry.load("verbatim").unwrap();
        let (_, loaded_body) = loaded.split_once("\n\n").unwrap();

        assert_eq!(
            loaded_body,
            "    indented instruction\n\nsecond instruction"
        );
    }
}
