use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use rmcp::model::Tool as RemoteTool;
use serde::Serialize;
use serde_json::{Map, Number, Value};

use super::artifact::McpArtifact;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CompiledMcpCall {
    pub source: String,
    pub tool: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Default)]
pub struct McpArtifactRegistry {
    artifacts: BTreeMap<String, McpArtifact>,
}

impl McpArtifactRegistry {
    pub fn register(&mut self, artifact: McpArtifact) -> Result<()> {
        let name = artifact.name.clone();
        if self.artifacts.insert(name.clone(), artifact).is_some() {
            bail!("MCP artifact `{name}` is already registered");
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&McpArtifact> {
        self.artifacts.get(name)
    }

    pub fn prompt_index(&self) -> String {
        self.artifacts
            .values()
            .map(|artifact| {
                format!(
                    "- {}: {}\n  source map: {}",
                    artifact.name,
                    artifact.description,
                    artifact.source_map.display()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn compile(&self, command: &str) -> Result<CompiledMcpCall> {
        let tokens = shell_words::split(command).context("parse MCP command")?;
        ensure!(
            tokens.len() >= 2,
            "MCP command must begin with `<source> <tool>`"
        );
        let source = &tokens[0];
        let tool_name = &tokens[1];
        let artifact = self
            .artifacts
            .get(source)
            .with_context(|| format!("unknown MCP source `{source}`"))?;
        let tool = artifact
            .tool(tool_name)
            .with_context(|| format!("unknown tool `{tool_name}` in MCP source `{source}`"))?;
        let arguments = compile_arguments(tool, &tokens[2..])?;
        Ok(CompiledMcpCall {
            source: source.clone(),
            tool: tool_name.clone(),
            arguments: Value::Object(arguments),
        })
    }
}

fn compile_arguments(tool: &RemoteTool, tokens: &[String]) -> Result<Map<String, Value>> {
    let schema = tool.input_schema.as_ref();
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    let additional_properties = schema
        .get("additionalProperties")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let mut arguments = Map::new();
    if tokens.len() == 1 && !tokens[0].contains('=') && properties.len() == 1 {
        let (name, property_schema) = properties.iter().next().expect("one property");
        arguments.insert(
            name.clone(),
            compile_value(&tokens[0], Some(property_schema))
                .with_context(|| format!("compile argument `{name}`"))?,
        );
    } else {
        for token in tokens {
            let (name, raw) = token.split_once('=').with_context(|| {
                format!(
                    "argument `{token}` must use `name=value`; positional shorthand is available only for one-parameter tools"
                )
            })?;
            ensure!(!name.is_empty(), "MCP argument name must not be empty");
            ensure!(
                !arguments.contains_key(name),
                "duplicate MCP argument `{name}`"
            );
            let property_schema = properties.get(name);
            ensure!(
                property_schema.is_some() || additional_properties,
                "unknown MCP argument `{name}`"
            );
            arguments.insert(
                name.to_owned(),
                compile_value(raw, property_schema)
                    .with_context(|| format!("compile argument `{name}`"))?,
            );
        }
    }

    let missing = required
        .into_iter()
        .filter(|name| !arguments.contains_key(*name))
        .collect::<Vec<_>>();
    ensure!(
        missing.is_empty(),
        "missing required MCP argument(s): {}",
        missing.join(", ")
    );
    Ok(arguments)
}

fn compile_value(raw: &str, schema: Option<&Value>) -> Result<Value> {
    let expected = schema.and_then(simple_schema_type);
    match expected {
        Some("string") => Ok(Value::String(raw.to_owned())),
        Some("boolean") => match raw {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => bail!("expected `true` or `false`"),
        },
        Some("integer") => parse_integer(raw),
        Some("number") => {
            let value: f64 = raw.parse().context("expected a JSON number")?;
            let number = Number::from_f64(value).context("number must be finite")?;
            Ok(Value::Number(number))
        }
        Some("array") => parse_compound(raw, "array", Value::is_array),
        Some("object") => parse_compound(raw, "object", Value::is_object),
        Some("null") => {
            ensure!(raw == "null", "expected `null`");
            Ok(Value::Null)
        }
        _ if raw.starts_with('[') || raw.starts_with('{') => {
            serde_json::from_str(raw).context("parse JSON argument")
        }
        _ => Ok(Value::String(raw.to_owned())),
    }
}

fn simple_schema_type(schema: &Value) -> Option<&str> {
    match schema.get("type")? {
        Value::String(kind) => Some(kind),
        Value::Array(kinds) => {
            let mut non_null = kinds
                .iter()
                .filter_map(Value::as_str)
                .filter(|kind| *kind != "null");
            let kind = non_null.next()?;
            non_null.next().is_none().then_some(kind)
        }
        _ => None,
    }
}

fn parse_integer(raw: &str) -> Result<Value> {
    if let Ok(value) = raw.parse::<i64>() {
        return Ok(Value::Number(value.into()));
    }
    let value = raw.parse::<u64>().context("expected a JSON integer")?;
    Ok(Value::Number(value.into()))
}

fn parse_compound(
    raw: &str,
    expected: &str,
    predicate: impl FnOnce(&Value) -> bool,
) -> Result<Value> {
    let value: Value =
        serde_json::from_str(raw).with_context(|| format!("parse JSON {expected}"))?;
    ensure!(predicate(&value), "expected a JSON {expected}");
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    fn registry_with_schema(schema: Value) -> McpArtifactRegistry {
        let workspace = TempDir::new().unwrap();
        let root = workspace.path().join("github");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("MCP.md"),
            "---\nname: github\ndescription: GitHub tools\n---\n",
        )
        .unwrap();
        fs::write(
            root.join("catalog.json"),
            serde_json::to_vec(&json!([{
                "name": "search_code",
                "inputSchema": schema
            }]))
            .unwrap(),
        )
        .unwrap();
        let artifact = McpArtifact::load(workspace.path(), "github", Path::new("github")).unwrap();
        let mut registry = McpArtifactRegistry::default();
        registry.register(artifact).unwrap();
        registry
    }

    #[test]
    fn compiles_one_parameter_positional_shorthand() {
        let registry = registry_with_schema(json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"],
            "additionalProperties": false
        }));
        assert_eq!(
            registry.compile("github search_code 'some code'").unwrap(),
            CompiledMcpCall {
                source: "github".into(),
                tool: "search_code".into(),
                arguments: json!({"query": "some code"})
            }
        );
    }

    #[test]
    fn compiles_named_schema_typed_values() {
        let registry = registry_with_schema(json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "limit": {"type": "integer"},
                "archived": {"type": "boolean"},
                "labels": {"type": "array"}
            },
            "required": ["query"],
            "additionalProperties": false
        }));
        assert_eq!(
            registry
                .compile(
                    r#"github search_code query='some code' limit=10 archived=false labels='["bug","help wanted"]'"#
                )
                .unwrap()
                .arguments,
            json!({
                "query": "some code",
                "limit": 10,
                "archived": false,
                "labels": ["bug", "help wanted"]
            })
        );
    }

    #[test]
    fn rejects_missing_unknown_and_ambiguous_positional_arguments() {
        let registry = registry_with_schema(json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "limit": {"type": "integer"}
            },
            "required": ["query"],
            "additionalProperties": false
        }));
        assert!(registry.compile("github search_code").is_err());
        assert!(
            registry
                .compile("github search_code unknown=value")
                .is_err()
        );
        assert!(registry.compile("github search_code positional").is_err());
    }
}
