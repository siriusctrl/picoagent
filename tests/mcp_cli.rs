use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::{Command, Output},
};

use fiasco::config::{AppConfig, McpServerConfig};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResult, ContentBlock, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool as RemoteTool,
    },
    service::RequestContext,
};
use serde_json::{Value, json};
use tempfile::TempDir;

const HELPER_ENV: &str = "FIASCO_MCP_TEST_HELPER";

struct FixtureServer;

impl ServerHandler for FixtureServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: fixture_tools(),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let arguments = request.arguments.unwrap_or_default();
        match request.name.as_ref() {
            "echo" => {
                let text = arguments
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ErrorData::invalid_params("text is required", None))?;
                Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                    "echo: {text}"
                ))]))
            }
            "add" => {
                let left = arguments
                    .get("left")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| ErrorData::invalid_params("left is required", None))?;
                let right = arguments
                    .get("right")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| ErrorData::invalid_params("right is required", None))?;
                Ok(CallToolResult::structured(json!({"sum": left + right})))
            }
            name => Err(ErrorData::invalid_params(
                format!("unknown tool {name}"),
                None,
            )),
        }
    }
}

fn fixture_tools() -> Vec<RemoteTool> {
    serde_json::from_value(json!([
        {
            "name": "echo",
            "description": "Echo one string",
            "inputSchema": {
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"],
                "additionalProperties": false
            }
        },
        {
            "name": "add",
            "description": "Add two integers",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "left": {"type": "integer"},
                    "right": {"type": "integer"}
                },
                "required": ["left", "right"],
                "additionalProperties": false
            }
        }
    ]))
    .unwrap()
}

#[tokio::test]
async fn mcp_stdio_helper() -> anyhow::Result<()> {
    if std::env::var(HELPER_ENV).as_deref() != Ok("1") {
        return Ok(());
    }
    let server = FixtureServer.serve(rmcp::transport::stdio()).await?;
    server.waiting().await?;
    Ok(())
}

#[test]
fn capture_check_compile_and_call_share_one_artifact_contract() {
    let workspace = TempDir::new().unwrap();
    write_config(workspace.path());

    let capture = fiasco(workspace.path(), &["mcp", "capture", "fixture"]);
    assert_success(&capture);
    let catalog: Vec<RemoteTool> = serde_json::from_slice(
        &fs::read(workspace.path().join(".agents/mcp/fixture/catalog.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(catalog, fixture_tools());

    write_source_map(workspace.path());
    assert_success(&fiasco(workspace.path(), &["mcp", "check", "fixture"]));
    assert_success(&fiasco(
        workspace.path(),
        &["mcp", "check", "fixture", "--live"],
    ));

    let compile = fiasco(
        workspace.path(),
        &["mcp", "compile", "fixture add left=7 right=5"],
    );
    assert_success(&compile);
    let compiled: Value = serde_json::from_slice(&compile.stdout).unwrap();
    assert_eq!(compiled["source"], "fixture");
    assert_eq!(compiled["tool"], "add");
    assert_eq!(compiled["arguments"], json!({"left": 7, "right": 5}));

    let verification = Command::new(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("skills/register-mcp/scripts/verify-artifact.sh"),
    )
    .env("FIASCO_BIN", env!("CARGO_BIN_EXE_fiasco"))
    .args(["--offline"])
    .arg(workspace.path())
    .args(["fixture", "fixture add left=7 right=5"])
    .output()
    .unwrap();
    assert_success(&verification);

    let call = fiasco(
        workspace.path(),
        &["mcp", "call", "fixture echo 'hello world'"],
    );
    assert_success(&call);
    assert_eq!(
        String::from_utf8(call.stdout).unwrap(),
        "echo: hello world\n"
    );

    let structured = fiasco(
        workspace.path(),
        &["mcp", "call", "fixture add left=7 right=5"],
    );
    assert_success(&structured);
    assert_eq!(
        serde_json::from_slice::<Value>(&structured.stdout).unwrap(),
        json!({"sum": 12})
    );

    let run = fiasco(
        workspace.path(),
        &["run", "Confirm that the fixture capability is available."],
    );
    assert_success(&run);
    let run_directories = fs::read_dir(workspace.path().join(".fiasco/runs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    assert_eq!(run_directories.len(), 1);
    let messages = fs::read_to_string(run_directories[0].join("messages.jsonl")).unwrap();
    assert!(messages.contains("<available-mcp-sources>"));
    assert!(messages.contains("- fixture: Echo text and add integers."));
    assert!(messages.contains("source map:"));
    assert!(!messages.contains("Echo one string"));
    assert!(!messages.contains("\"inputSchema\""));
}

fn write_config(workspace: &Path) {
    let mut config = AppConfig::default();
    config.mcp.insert(
        "fixture".into(),
        McpServerConfig {
            artifact: ".agents/mcp/fixture".into(),
            command: std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            args: vec![
                "--exact".into(),
                "mcp_stdio_helper".into(),
                "--quiet".into(),
                "--nocapture".into(),
                "--test-threads".into(),
                "1".into(),
            ],
            env: BTreeMap::from([(HELPER_ENV.into(), "1".into())]),
        },
    );
    let directory = workspace.join(".fiasco");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("config.toml"),
        toml::to_string_pretty(&config).unwrap(),
    )
    .unwrap();
}

fn write_source_map(workspace: &Path) {
    let root = workspace.join(".agents/mcp/fixture");
    fs::create_dir_all(root.join("references")).unwrap();
    fs::write(
        root.join("MCP.md"),
        "---\nname: fixture\ndescription: Echo text and add integers.\n---\n\n# Source map\n\n- [Text and arithmetic](references/basics.md)\n",
    )
    .unwrap();
    fs::write(
        root.join("references/basics.md"),
        "# Text and arithmetic\n\nUse `echo` for text and `add` for integers.\n",
    )
    .unwrap();
}

fn fiasco(workspace: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fiasco"))
        .arg("--workspace")
        .arg(workspace)
        .args(arguments)
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
