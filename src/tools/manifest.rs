use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::Value;

use crate::model::ToolSpec;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolDefinition {
    name: String,
    description: String,
    returns: String,
    input_schema: Value,
}

fn parse_tool_definition(source: &str) -> Result<ToolSpec> {
    let definition: ToolDefinition =
        serde_yaml_ng::from_str(source).context("parse tool definition YAML")?;
    ensure!(
        !definition.name.is_empty() && definition.name.trim() == definition.name,
        "tool name must be non-empty and have no boundary whitespace"
    );
    ensure!(
        !definition.description.is_empty()
            && definition.description.trim() == definition.description,
        "tool description must be non-empty and have no boundary whitespace"
    );
    ensure!(
        !definition.returns.is_empty() && definition.returns.trim() == definition.returns,
        "tool returns must be non-empty and have no boundary whitespace"
    );
    ensure!(
        definition.input_schema.get("type").and_then(Value::as_str) == Some("object"),
        "tool input_schema must describe an object"
    );
    Ok(ToolSpec {
        name: definition.name,
        description: format!(
            "{}\n\nReturns: {}",
            definition.description, definition.returns
        ),
        input_schema: definition.input_schema,
    })
}

pub(crate) fn embedded_tool_spec(source: &str, owner: &str) -> ToolSpec {
    parse_tool_definition(source)
        .unwrap_or_else(|error| panic!("invalid embedded tool definition for {owner}: {error:#}"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn definition_is_typed_folded_and_requires_an_object_schema() {
        let spec = parse_tool_definition(
            "name: sample\ndescription: >-\n  Sample tool with\n  source wrapping.\nreturns: >-\n  A sample\n  result.\ninput_schema:\n  type: object\n",
        )
        .unwrap();
        assert_eq!(spec.name, "sample");
        assert_eq!(
            spec.description,
            "Sample tool with source wrapping.\n\nReturns: A sample result."
        );

        assert!(
            parse_tool_definition(
                "name: sample\ndescription: Sample tool\nreturns: Sample result\ninput_schema:\n  type: array\n"
            )
            .is_err()
        );
        assert!(
            parse_tool_definition(
                "name: sample\ndescription: Sample tool\nreturns: Sample result\ninput_schema:\n  type: object\nunknown: value\n"
            )
            .is_err()
        );
        assert!(
            parse_tool_definition(
                "name: sample\ndescription: ''\nreturns: Sample result\ninput_schema:\n  type: object\n"
            )
            .is_err()
        );
        assert!(
            parse_tool_definition(
                "name: sample\ndescription: Sample tool\nreturns: ''\ninput_schema:\n  type: object\n"
            )
            .is_err()
        );
        assert!(
            parse_tool_definition(
                "name: sample\ndescription: Sample tool\ninput_schema:\n  type: object\n"
            )
            .is_err()
        );
    }

    #[test]
    fn definition_rejects_boundary_whitespace() {
        for source in [
            "name: ' sample'\ndescription: Sample tool\nreturns: Sample result\ninput_schema:\n  type: object\n",
            "name: sample\ndescription: 'Sample tool '\nreturns: Sample result\ninput_schema:\n  type: object\n",
            "name: sample\ndescription: Sample tool\nreturns: 'Sample result '\ninput_schema:\n  type: object\n",
        ] {
            assert!(parse_tool_definition(source).is_err());
        }
    }

    #[test]
    fn every_local_manifest_parses_and_has_a_unique_name() {
        let definitions = [
            include_str!("bash/tool.yaml"),
            include_str!("delegate/tool.yaml"),
            include_str!("graph/init/tool.yaml"),
            include_str!("graph/list/tool.yaml"),
            include_str!("history/read/tool.yaml"),
            include_str!("history/search/tool.yaml"),
            include_str!("load_skill/tool.yaml"),
            include_str!("read/tool.yaml"),
            include_str!("handle/close/tool.yaml"),
            include_str!("handle/inspect/tool.yaml"),
            include_str!("handle/list/tool.yaml"),
            include_str!("handle/send/tool.yaml"),
            include_str!("handle/status/tool.yaml"),
            include_str!("handle/stop/tool.yaml"),
            include_str!("handle/wait/tool.yaml"),
            include_str!("web_search/tool.yaml"),
            include_str!("write/tool.yaml"),
        ];
        let specs = definitions
            .into_iter()
            .map(parse_tool_definition)
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let names = specs
            .iter()
            .map(|spec| spec.name.as_str())
            .collect::<BTreeSet<_>>();

        for spec in &specs {
            assert_eq!(spec.description.matches("\n\nReturns: ").count(), 1);
        }

        assert_eq!(
            names,
            BTreeSet::from([
                "bash",
                "delegate",
                "graph_init",
                "graph_list",
                "history_read",
                "history_search",
                "load_skill",
                "read",
                "close",
                "inspect",
                "list_handles",
                "send_message",
                "status",
                "stop",
                "wait",
                "web_search",
                "write",
            ])
        );
    }
}
