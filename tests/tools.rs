use std::sync::Arc;

use picoagent::{
    artifact::ArtifactStore,
    tools::{BashTool, ReadTool, Tool, ToolContext, WebSearchTool, WriteTool, build_app_tools},
};
use serde_json::json;
#[cfg(target_os = "linux")]
use sha2::{Digest, Sha256};
use tempfile::tempdir;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path, query_param},
};

fn manifest(source: &str) -> serde_json::Value {
    serde_yaml_ng::from_str(source).unwrap()
}

fn model_description(definition: &serde_json::Value) -> String {
    format!(
        "{}\n\nReturns: {}",
        definition["description"].as_str().unwrap(),
        definition["returns"].as_str().unwrap()
    )
}

fn context(workspace: &std::path::Path, call_id: &str) -> ToolContext {
    ToolContext {
        run_id: "run-1".to_owned(),
        call_id: call_id.to_owned(),
        workspace: workspace.to_owned(),
    }
}

async fn bash_text(workspace: &std::path::Path, call_id: &str, command: &str) -> (bool, String) {
    let output = BashTool
        .execute(context(workspace, call_id), json!({ "command": command }))
        .await
        .unwrap();
    assert!(output.content.is_empty());
    let source_path = output.source_path.expect("bash output should be spooled");
    let text = tokio::fs::read_to_string(source_path).await.unwrap();
    (output.is_error, text)
}

#[test]
fn app_registry_uses_the_embedded_tool_manifests() {
    let registry = build_app_tools(Arc::new(Default::default()), None).unwrap();

    assert_eq!(
        registry.names().collect::<Vec<_>>(),
        ["bash", "load_skill", "read", "write"]
    );
    let specs = registry
        .specs()
        .into_iter()
        .map(|spec| (spec.name.clone(), spec))
        .collect::<std::collections::BTreeMap<_, _>>();
    for (name, source) in [
        ("bash", include_str!("../src/tools/bash/tool.yaml")),
        (
            "load_skill",
            include_str!("../src/tools/load_skill/tool.yaml"),
        ),
        ("read", include_str!("../src/tools/read/tool.yaml")),
        ("write", include_str!("../src/tools/write/tool.yaml")),
    ] {
        let definition = manifest(source);
        assert_eq!(specs[name].description, model_description(&definition));
        assert_eq!(specs[name].input_schema, definition["input_schema"]);
    }
}

#[tokio::test]
async fn read_returns_a_bounded_line_range() {
    let workspace = tempdir().unwrap();
    tokio::fs::write(
        workspace.path().join("sample.txt"),
        "zero\none\ntwo\nthree\n",
    )
    .await
    .unwrap();
    let output = ReadTool
        .execute(
            context(workspace.path(), "read"),
            json!({ "path": "sample.txt", "offset": 1, "limit": 2 }),
        )
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8(output.content).unwrap(),
        "one\ntwo\n[read truncated: line limit reached; continue with offset=3]"
    );
}

#[tokio::test]
async fn read_does_not_report_truncation_at_exact_line_ending_eof() {
    let workspace = tempdir().unwrap();
    tokio::fs::write(workspace.path().join("exact.txt"), "zero\none\n")
        .await
        .unwrap();
    let output = ReadTool
        .execute(
            context(workspace.path(), "read-exact"),
            json!({ "path": "exact.txt", "limit": 2 }),
        )
        .await
        .unwrap();

    assert_eq!(String::from_utf8(output.content).unwrap(), "zero\none");
}

#[tokio::test]
async fn read_bounds_a_single_long_utf8_line_by_bytes() {
    let workspace = tempdir().unwrap();
    tokio::fs::write(workspace.path().join("long.txt"), "甲".repeat(10_000))
        .await
        .unwrap();
    let output = ReadTool
        .execute(
            context(workspace.path(), "read-long"),
            json!({ "path": "long.txt", "max_bytes": 101 }),
        )
        .await
        .unwrap();
    let text = String::from_utf8(output.content).unwrap();
    assert!(text.len() < 256);
    assert!(text.contains("read truncated: max_bytes reached"));
    assert!(!text.contains('\u{fffd}'));
}

#[tokio::test]
async fn write_creates_a_file_and_applies_multiple_atomic_edits() {
    let workspace = tempdir().unwrap();
    let tool = WriteTool::default();
    let created = tool
        .execute(
            context(workspace.path(), "write"),
            json!({ "path": "nested/sample.txt", "content": "alpha\nbeta\ngamma\n" }),
        )
        .await
        .unwrap();
    let edited = tool
        .execute(
            context(workspace.path(), "edit"),
            json!({
                "path": "nested/sample.txt",
                "edits": [
                    { "old_text": "alpha", "new_text": "one" },
                    { "old_text": "gamma", "new_text": "three" }
                ]
            }),
        )
        .await
        .unwrap();

    assert!(
        String::from_utf8(created.content)
            .unwrap()
            .starts_with("Wrote ")
    );
    assert!(
        String::from_utf8(edited.content)
            .unwrap()
            .starts_with("Applied 2 atomic replacement")
    );

    assert_eq!(
        tokio::fs::read_to_string(workspace.path().join("nested/sample.txt"))
            .await
            .unwrap(),
        "one\nbeta\nthree\n"
    );
}

#[tokio::test]
async fn write_rejects_an_ambiguous_edit_without_changing_the_file() {
    let workspace = tempdir().unwrap();
    let path = workspace.path().join("sample.txt");
    tokio::fs::write(&path, "same same").await.unwrap();
    let error = WriteTool::default()
        .execute(
            context(workspace.path(), "edit"),
            json!({
                "path": "sample.txt",
                "edits": [{ "old_text": "same", "new_text": "new" }]
            }),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("matches 2 regions"));
    assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "same same");
}

#[tokio::test]
async fn write_counts_overlapping_matches_as_ambiguous() {
    let workspace = tempdir().unwrap();
    let path = workspace.path().join("sample.txt");
    tokio::fs::write(&path, "aaa").await.unwrap();
    let error = WriteTool::default()
        .execute(
            context(workspace.path(), "edit"),
            json!({
                "path": "sample.txt",
                "edits": [{ "old_text": "aa", "new_text": "b" }]
            }),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("matches 2 regions"));
    assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "aaa");
}

#[tokio::test]
async fn write_refuses_to_silently_normalize_mixed_line_endings() {
    let workspace = tempdir().unwrap();
    let path = workspace.path().join("mixed.txt");
    tokio::fs::write(&path, "alpha\r\nbeta\n").await.unwrap();
    let error = WriteTool::default()
        .execute(
            context(workspace.path(), "edit"),
            json!({
                "path": "mixed.txt",
                "edits": [{ "old_text": "beta", "new_text": "gamma" }]
            }),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("mixed line endings"));
    assert_eq!(
        tokio::fs::read_to_string(path).await.unwrap(),
        "alpha\r\nbeta\n"
    );
}

#[tokio::test]
async fn write_preserves_bom_and_crlf() {
    let workspace = tempdir().unwrap();
    let path = workspace.path().join("windows.txt");
    tokio::fs::write(&path, "\u{feff}alpha\r\nbeta\r\n")
        .await
        .unwrap();
    WriteTool::default()
        .execute(
            context(workspace.path(), "edit"),
            json!({
                "path": "windows.txt",
                "edits": [{ "old_text": "beta", "new_text": "gamma" }]
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        tokio::fs::read_to_string(path).await.unwrap(),
        "\u{feff}alpha\r\ngamma\r\n"
    );
}

#[tokio::test]
async fn write_uses_conservative_indentation_matching() {
    let workspace = tempdir().unwrap();
    let path = workspace.path().join("sample.rs");
    tokio::fs::write(&path, "fn main() {\n    old();\n}\n")
        .await
        .unwrap();
    let output = WriteTool::default()
        .execute(
            context(workspace.path(), "edit"),
            json!({
                "path": "sample.rs",
                "edits": [{ "old_text": "      old();", "new_text": "      new();" }]
            }),
        )
        .await
        .unwrap();
    assert!(
        String::from_utf8(output.content)
            .unwrap()
            .contains("normalization")
    );
    assert_eq!(
        tokio::fs::read_to_string(path).await.unwrap(),
        "fn main() {\n    new();\n}\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn write_follows_an_existing_symlink_without_replacing_it() {
    let workspace = tempdir().unwrap();
    let target = workspace.path().join("target.txt");
    let alias = workspace.path().join("alias.txt");
    tokio::fs::write(&target, "old\n").await.unwrap();
    std::os::unix::fs::symlink(&target, &alias).unwrap();
    WriteTool::default()
        .execute(
            context(workspace.path(), "edit-link"),
            json!({
                "path": "alias.txt",
                "edits": [{ "old_text": "old", "new_text": "new" }]
            }),
        )
        .await
        .unwrap();
    assert!(
        tokio::fs::symlink_metadata(&alias)
            .await
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(tokio::fs::read_to_string(target).await.unwrap(), "new\n");
}

#[tokio::test]
async fn bash_combines_streams_in_capture_order_and_marks_nonzero_exit() {
    let workspace = tempdir().unwrap();
    let (is_error, text) = bash_text(
        workspace.path(),
        "bash-order",
        "printf 'stdout-1\\n'; printf 'stderr-1\\n' >&2; printf 'stdout-2'; exit 7",
    )
    .await;

    assert!(is_error);
    assert!(
        text.ends_with("stdout-1\nstderr-1\nstdout-2\n\nCommand exited with code 7"),
        "unexpected bash output: {text}"
    );
}

#[tokio::test]
async fn bash_returns_success_output_without_a_status_line() {
    let workspace = tempdir().unwrap();
    let (is_error, text) = bash_text(workspace.path(), "bash-success", "printf done").await;

    assert!(!is_error);
    assert!(text.ends_with("done"), "unexpected bash output: {text}");
    assert!(!text.contains("Command exited with code"));
}

#[tokio::test]
async fn bash_is_non_login_and_inherits_the_process_path() {
    let workspace = tempdir().unwrap();
    let command =
        "if shopt -q login_shell; then printf login; else printf 'non-login\\n%s' \"$PATH\"; fi";
    let (is_error, text) = bash_text(workspace.path(), "bash-environment", command).await;

    assert!(!is_error);
    let (kind, path) = text.split_once('\n').unwrap();
    assert_eq!(kind, "non-login");
    assert_eq!(path, std::env::var("PATH").unwrap());
}

#[tokio::test]
async fn bash_large_combined_output_uses_the_artifact_contract() {
    let workspace = tempdir().unwrap();
    let context = context(workspace.path(), "bash-large");
    let raw = BashTool
        .execute(context.clone(), json!({ "command": "printf '%40000s' x" }))
        .await
        .unwrap();
    let output = ArtifactStore::default()
        .persist_output(&context, raw)
        .await
        .unwrap();

    assert!(!output.is_error);
    assert!(output.truncated);
    assert!(output.preview.ends_with('x'));
    assert!(output.preview.contains("bytes omitted"));
    let artifact = output.artifact.unwrap();
    assert!(artifact.bytes >= 40_000);
    let stored = tokio::fs::read(workspace.path().join(artifact.path))
        .await
        .unwrap();
    assert!(stored.len() >= 40_000);
    assert_eq!(stored.last(), Some(&b'x'));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn bash_artifact_is_immutable_after_an_escaped_descendant_writes() {
    let workspace = tempdir().unwrap();
    let context = context(workspace.path(), "bash-escaped-writer");
    let raw = BashTool
        .execute(
            context.clone(),
            json!({
                "command": "printf '%40000s' x; setsid bash -c 'sleep 0.5; printf late-output' & exit 7"
            }),
        )
        .await
        .unwrap();
    let output = ArtifactStore::default()
        .persist_output(&context, raw)
        .await
        .unwrap();

    assert!(output.is_error);
    let artifact = output.artifact.unwrap();
    let path = workspace.path().join(&artifact.path);
    let initial = tokio::fs::read(&path).await.unwrap();
    assert!(initial.ends_with(b"Command exited with code 7"));
    assert_eq!(initial.len() as u64, artifact.bytes);
    assert_eq!(format!("{:x}", Sha256::digest(&initial)), artifact.sha256);

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let after_descendant = tokio::fs::read(path).await.unwrap();
    assert_eq!(after_descendant, initial);
    assert_eq!(
        format!("{:x}", Sha256::digest(&after_descendant)),
        artifact.sha256
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cancelling_bash_terminates_its_process_group() {
    let workspace = tempdir().unwrap();
    let tool_context = context(workspace.path(), "bash-timeout");
    let execution = tokio::spawn(async move {
        BashTool
            .execute(
                tool_context,
                json!({
                    "command": "sleep 30 & child=$!; echo $child > child.pid; wait"
                }),
            )
            .await
    });
    let pid_path = workspace.path().join("child.pid");
    let pid = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match tokio::fs::read_to_string(&pid_path).await {
                Ok(pid) => break pid,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                Err(error) => panic!("failed to read child pid: {error}"),
            }
        }
    })
    .await
    .expect("bash command did not start its background child");

    execution.abort();
    assert!(
        execution.await.unwrap_err().is_cancelled(),
        "bash execution was not cancelled"
    );
    let stopped = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let alive = std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success();
            if !alive {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await;
    assert!(
        stopped.is_ok(),
        "background child survived Bash cancellation"
    );
}

#[tokio::test]
async fn web_search_uses_brave_request_shape_and_returns_compact_results() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("q", "rust agents"))
        .and(query_param("count", "2"))
        .and(header("x-subscription-token", "secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "web": { "results": [{
                "title": "Result one",
                "url": "https://example.com/one",
                "description": "A useful result",
                "age": "1 day ago",
                "extra_snippets": ["More context"]
            }] }
        })))
        .mount(&server)
        .await;
    let tool = WebSearchTool::with_endpoint(format!("{}/search", server.uri()), "secret", 8);
    let definition = manifest(include_str!("../src/tools/web_search/tool.yaml"));
    let spec = tool.spec();
    assert_eq!(spec.description, model_description(&definition));
    assert_eq!(spec.input_schema, definition["input_schema"]);
    let output = tool
        .execute(
            context(tempdir().unwrap().path(), "web"),
            json!({ "query": "rust agents", "count": 2 }),
        )
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output.content).unwrap();
    assert_eq!(value["query"], "rust agents");
    assert_eq!(value["results"][0]["title"], "Result one");
    assert_eq!(value["results"][0]["extra_snippets"][0], "More context");
}
