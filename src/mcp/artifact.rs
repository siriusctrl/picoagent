use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::LazyLock,
};

use anyhow::{Context, Result, bail, ensure};
use regex::Regex;
use rmcp::model::Tool as RemoteTool;
use serde::Deserialize;

const SOURCE_MAP_FILE: &str = "MCP.md";
const CATALOG_FILE: &str = "catalog.json";

static MARKDOWN_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\[[^\]]*\]\(([^)\s]+)\)"#).expect("valid link regex"));

#[derive(Debug, Clone)]
pub struct McpArtifact {
    pub name: String,
    pub description: String,
    pub directory: PathBuf,
    pub source_map: PathBuf,
    tools: BTreeMap<String, RemoteTool>,
}

impl McpArtifact {
    pub fn load(workspace: &Path, configured_name: &str, configured_path: &Path) -> Result<Self> {
        let directory = if configured_path.is_absolute() {
            configured_path.to_owned()
        } else {
            workspace.join(configured_path)
        };
        let directory = fs::canonicalize(&directory)
            .with_context(|| format!("resolve MCP artifact {}", directory.display()))?;
        ensure!(
            directory.is_dir(),
            "MCP artifact is not a directory: {}",
            directory.display()
        );

        let source_map = directory.join(SOURCE_MAP_FILE);
        let source = fs::read_to_string(&source_map)
            .with_context(|| format!("read MCP source map {}", source_map.display()))?;
        let (frontmatter, body) = parse_source_map(&source)
            .with_context(|| format!("parse MCP source map {}", source_map.display()))?;
        validate_namespace(&frontmatter.name)?;
        ensure!(
            frontmatter.name == configured_name,
            "MCP artifact name `{}` does not match configured namespace `{configured_name}`",
            frontmatter.name
        );
        validate_source_map_links(&directory, body)?;

        let catalog_path = directory.join(CATALOG_FILE);
        let catalog = fs::read(&catalog_path)
            .with_context(|| format!("read MCP catalog {}", catalog_path.display()))?;
        let remote_tools: Vec<RemoteTool> = serde_json::from_slice(&catalog)
            .with_context(|| format!("parse MCP catalog {}", catalog_path.display()))?;
        let mut tools = BTreeMap::new();
        for tool in remote_tools {
            let name = tool.name.to_string();
            if tools.insert(name.clone(), tool).is_some() {
                bail!("MCP catalog contains duplicate tool `{name}`");
            }
        }

        Ok(Self {
            name: frontmatter.name,
            description: normalize_description(&frontmatter.description),
            directory,
            source_map,
            tools,
        })
    }

    pub fn tool(&self, name: &str) -> Option<&RemoteTool> {
        self.tools.get(name)
    }

    pub fn tools(&self) -> impl Iterator<Item = &RemoteTool> {
        self.tools.values()
    }
}

pub fn write_catalog(directory: &Path, tools: &[RemoteTool]) -> Result<PathBuf> {
    fs::create_dir_all(directory)
        .with_context(|| format!("create MCP artifact directory {}", directory.display()))?;
    let path = directory.join(CATALOG_FILE);
    let bytes = serde_json::to_vec_pretty(tools).context("serialize MCP tool catalog")?;
    fs::write(&path, bytes).with_context(|| format!("write MCP catalog {}", path.display()))?;
    Ok(path)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct McpFrontmatter {
    name: String,
    description: String,
}

fn parse_source_map(content: &str) -> Result<(McpFrontmatter, &str)> {
    let mut lines = content.split_inclusive('\n');
    let first = lines.next().context("MCP.md is empty")?;
    if first.trim() != "---" {
        bail!("MCP.md must begin with `---` frontmatter");
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
    let metadata: McpFrontmatter =
        serde_yaml_ng::from_str(&content[frontmatter_start..frontmatter_end])
            .context("parse MCP.md frontmatter")?;
    ensure!(
        !metadata.description.trim().is_empty(),
        "frontmatter `description` must not be empty"
    );
    Ok((metadata, &content[body_start..]))
}

fn validate_namespace(name: &str) -> Result<()> {
    ensure!(!name.is_empty(), "MCP namespace must not be empty");
    ensure!(
        name.len() <= 64,
        "MCP namespace must be at most 64 characters"
    );
    ensure!(
        name.chars().all(|character| character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '-'),
        "MCP namespace `{name}` must use lowercase letters, digits, and hyphens"
    );
    ensure!(
        name.starts_with(|character: char| character.is_ascii_lowercase()),
        "MCP namespace `{name}` must start with a lowercase letter"
    );
    ensure!(
        !name.ends_with('-') && !name.contains("--"),
        "MCP namespace `{name}` must not end with or repeat hyphens"
    );
    Ok(())
}

fn normalize_description(description: &str) -> String {
    description.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn validate_source_map_links(directory: &Path, body: &str) -> Result<()> {
    let mut checked = BTreeSet::new();
    for captures in MARKDOWN_LINK.captures_iter(body) {
        let target = captures.get(1).expect("capture exists").as_str();
        let target = target.split('#').next().unwrap_or_default();
        if target.is_empty()
            || target.starts_with("http://")
            || target.starts_with("https://")
            || target.starts_with("mailto:")
        {
            continue;
        }
        let relative = Path::new(target);
        ensure!(
            !relative.is_absolute()
                && !relative
                    .components()
                    .any(|component| component == Component::ParentDir),
            "MCP source-map link must stay within the artifact: `{target}`"
        );
        if checked.insert(relative.to_owned()) {
            let path = directory.join(relative);
            ensure!(
                path.is_file(),
                "MCP source-map link does not exist: {}",
                path.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    fn write_artifact(root: &Path, name: &str, body: &str) {
        fs::create_dir_all(root.join("references")).unwrap();
        fs::write(root.join("references/search.md"), "Search details").unwrap();
        fs::write(
            root.join(SOURCE_MAP_FILE),
            format!(
                "---\nname: {name}\ndescription: >-\n  Search and inspect repositories.\n---\n{body}"
            ),
        )
        .unwrap();
        fs::write(
            root.join(CATALOG_FILE),
            serde_json::to_vec_pretty(&json!([{
                "name": "search_code",
                "description": "Search code",
                "inputSchema": {
                    "type": "object",
                    "properties": {"query": {"type": "string"}},
                    "required": ["query"]
                }
            }]))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn loads_source_map_and_exact_catalog() {
        let workspace = TempDir::new().unwrap();
        let root = workspace.path().join(".agents/mcp/github");
        write_artifact(&root, "github", "[Repository search](references/search.md)");

        let artifact =
            McpArtifact::load(workspace.path(), "github", Path::new(".agents/mcp/github")).unwrap();
        assert_eq!(artifact.name, "github");
        assert_eq!(artifact.description, "Search and inspect repositories.");
        assert!(artifact.tool("search_code").is_some());
        assert_eq!(artifact.tools().count(), 1);
    }

    #[test]
    fn rejects_namespace_mismatch_and_missing_reference() {
        let workspace = TempDir::new().unwrap();
        let root = workspace.path().join("artifact");
        write_artifact(&root, "github", "[Missing](references/missing.md)");

        assert!(McpArtifact::load(workspace.path(), "gitlab", &root).is_err());
        assert!(McpArtifact::load(workspace.path(), "github", &root).is_err());
    }

    #[test]
    fn catalog_capture_is_pretty_and_loadable() {
        let root = TempDir::new().unwrap();
        let tools: Vec<RemoteTool> = serde_json::from_value(json!([{
            "name": "ping",
            "inputSchema": {"type": "object"}
        }]))
        .unwrap();

        let path = write_catalog(root.path(), &tools).unwrap();
        let captured: Vec<RemoteTool> = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(captured, tools);
    }
}
