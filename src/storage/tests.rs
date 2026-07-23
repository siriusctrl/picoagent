use tempfile::TempDir;

use super::RunDirStore;

#[tokio::test]
async fn list_child_runs_rejects_an_invalid_record_matching_the_parent_projection() {
    let workspace = TempDir::new().unwrap();
    let store = RunDirStore::new(workspace.path());
    let paths = store.paths("matching-invalid");
    tokio::fs::create_dir_all(&paths.directory).await.unwrap();
    tokio::fs::write(
        &paths.metadata,
        r#"{"id":"matching-invalid","parent_run_id":"parent"}"#,
    )
    .await
    .unwrap();

    let error = store.list_child_runs("parent").await.unwrap_err();
    assert!(
        format!("{error:#}").contains("parse child run `matching-invalid`"),
        "unexpected error: {error:#}"
    );
}
