use std::{fs::OpenOptions, io::Write, process::Command};

use fiasco::{
    model::{Message, Role},
    storage::{RunDirStore, RunRecord, RunState},
};
use tempfile::TempDir;

async fn workspace_with_run() -> (TempDir, RunDirStore) {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let run = RunRecord::new(
        "run-1",
        "root",
        "inspect me",
        "test-provider",
        "test-model",
        workspace.path().to_owned(),
        None,
    );
    store.create_run(&run).await.unwrap();
    store
        .append_message("run-1", &Message::text(Role::User, "hello"))
        .await
        .unwrap();
    store
        .append_message("run-1", &Message::text(Role::Assistant, "goodbye"))
        .await
        .unwrap();
    store
        .update_state("run-1", RunState::Completed)
        .await
        .unwrap();
    (workspace, store)
}

fn fiasco(workspace: &TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_fiasco"));
    command.args(["--workspace", workspace.path().to_str().unwrap()]);
    command
}

#[tokio::test]
async fn redirected_inspect_writes_exact_complete_ndjson_without_loading_config() {
    let (workspace, store) = workspace_with_run().await;
    let paths = store.paths("run-1");
    let complete = std::fs::read(&paths.messages).unwrap();
    let bad_config = workspace.path().join("bad.toml");
    std::fs::write(&bad_config, "this is not valid toml = [").unwrap();
    let mut file = OpenOptions::new()
        .append(true)
        .open(&paths.messages)
        .unwrap();
    file.write_all(b"{\"ref\":\"m3\"").unwrap();
    file.flush().unwrap();

    let output = fiasco(&workspace)
        .args(["--config", bad_config.to_str().unwrap(), "inspect", "run-1"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, complete);
    for line in output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        serde_json::from_slice::<serde_json::Value>(line).unwrap();
    }
}

#[tokio::test]
async fn explicit_ndjson_matches_redirected_default() {
    let (workspace, store) = workspace_with_run().await;
    let expected = std::fs::read(store.paths("run-1").messages).unwrap();
    let output = fiasco(&workspace)
        .args(["inspect", "run-1", "--output", "ndjson"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(output.stdout, expected);
}

#[tokio::test]
async fn summary_preserves_metadata_and_final_view() {
    let (workspace, store) = workspace_with_run().await;
    store.write_final("run-1", "FINAL_OK").await.unwrap();
    let output = fiasco(&workspace)
        .args(["inspect", "run-1", "--summary"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\"id\": \"run-1\""));
    assert!(stdout.contains("\n--- final ---\nFINAL_OK\n"));
}

#[tokio::test]
async fn conflicting_inspect_flags_are_rejected_by_clap() {
    let (workspace, _store) = workspace_with_run().await;
    for arguments in [
        &["inspect", "run-1", "--follow", "--summary"][..],
        &["inspect", "run-1", "--follow", "--output", "ndjson"][..],
        &["inspect", "run-1", "--summary", "--output", "ndjson"][..],
    ] {
        let output = fiasco(&workspace).args(arguments).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
    }
}

#[tokio::test]
async fn redirected_follow_is_rejected_instead_of_degrading_to_snapshot_ndjson() {
    let (workspace, _store) = workspace_with_run().await;
    let output = fiasco(&workspace)
        .args(["inspect", "run-1", "--follow"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("requires an interactive terminal on stdout")
    );
    assert!(output.stdout.is_empty());
}
