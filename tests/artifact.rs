use picoagent::{
    artifact::{ArtifactPolicy, ArtifactRef, ArtifactStore},
    tools::{RawToolOutput, ToolContext},
};
use tempfile::tempdir;

fn context(workspace: &std::path::Path) -> ToolContext {
    ToolContext {
        run_id: "run-1".to_owned(),
        call_id: "call/1".to_owned(),
        workspace: workspace.to_owned(),
    }
}

#[tokio::test]
async fn keeps_small_results_inline() {
    let workspace = tempdir().unwrap();
    let output = ArtifactStore::default()
        .persist_output(
            &context(workspace.path()),
            RawToolOutput::text("small result"),
        )
        .await
        .unwrap();

    assert_eq!(output.preview, "small result");
    assert!(!output.truncated);
    assert!(output.artifact.is_none());
    assert_eq!(output.model_content(), "small result");
    assert!(!workspace.path().join(".pico").exists());
}

#[tokio::test]
async fn cumulative_budget_forces_later_small_results_to_artifacts() {
    let workspace = tempdir().unwrap();
    let store = ArtifactStore::new(ArtifactPolicy {
        inline_limit_bytes: 32,
        max_inline_bytes_per_run: 10,
        preview_head_bytes: 8,
        preview_tail_bytes: 4,
    });
    let first = store
        .persist_output_with_budget(
            &context(workspace.path()),
            RawToolOutput::text("12345678"),
            10,
        )
        .await
        .unwrap();
    assert!(first.artifact.is_none());

    let second = store
        .persist_output_with_budget(
            &context(workspace.path()),
            RawToolOutput::text("abcdefgh"),
            2,
        )
        .await
        .unwrap();
    assert!(second.artifact.is_some());
    assert!(second.preview.len() <= 2);
}

#[tokio::test]
async fn spills_small_binary_results_without_lossy_inline_decoding() {
    let workspace = tempfile::tempdir().unwrap();
    let context = ToolContext {
        run_id: "run-binary".into(),
        call_id: "call-binary".into(),
        workspace: workspace.path().to_path_buf(),
    };
    let output = ArtifactStore::default()
        .persist_output(
            &context,
            RawToolOutput {
                content: vec![0, 159, 146, 150],
                source_path: None,
                media_type: "application/octet-stream".into(),
                is_error: false,
            },
        )
        .await
        .unwrap();

    assert!(output.truncated);
    assert!(output.preview.contains("non-UTF-8"));
    assert!(output.artifact.unwrap().path.contains("call-binary-"));
}

#[tokio::test]
async fn spills_large_results_with_versioned_sidecar_and_head_tail_preview() {
    let workspace = tempdir().unwrap();
    let store = ArtifactStore::new(ArtifactPolicy {
        inline_limit_bytes: 20,
        max_inline_bytes_per_run: 100,
        preview_head_bytes: 8,
        preview_tail_bytes: 8,
    });
    let content = "BEGIN-abcdefghijklmnopqrstuvwxyz-END";
    let output = store
        .persist_output(&context(workspace.path()), RawToolOutput::text(content))
        .await
        .unwrap();

    assert!(output.truncated);
    assert!(output.preview.starts_with("BEGIN-ab"));
    assert!(output.preview.ends_with("wxyz-END"));
    assert!(output.preview.contains("bytes omitted"));
    let model_content = output.model_content();
    assert!(model_content.contains("truncated: true"));
    assert!(model_content.contains("media_type: text/plain"));

    let artifact = output.artifact.unwrap();
    assert_eq!(artifact.version, 1);
    assert_eq!(artifact.run_id, "run-1");
    assert_eq!(artifact.call_id, "call/1");
    assert_eq!(artifact.bytes, content.len() as u64);
    assert_eq!(artifact.artifact_id, format!("sha256:{}", artifact.sha256));
    assert!(
        artifact
            .path
            .starts_with(".pico/runs/run-1/artifacts/call_1-")
    );
    assert!(artifact.path.ends_with(".txt"));

    let stored = tokio::fs::read_to_string(workspace.path().join(&artifact.path))
        .await
        .unwrap();
    assert_eq!(stored, content);
    let sidecar = workspace
        .path()
        .join(artifact.path.strip_suffix(".txt").unwrap().to_owned() + ".artifact.json");
    let reference: ArtifactRef =
        serde_json::from_slice(&tokio::fs::read(sidecar).await.unwrap()).unwrap();
    assert_eq!(reference, artifact);
}

#[tokio::test]
async fn repeated_call_ids_do_not_overwrite_prior_artifacts() {
    let workspace = tempdir().unwrap();
    let store = ArtifactStore::new(ArtifactPolicy {
        inline_limit_bytes: 1,
        max_inline_bytes_per_run: 100,
        preview_head_bytes: 1,
        preview_tail_bytes: 1,
    });
    let context = context(workspace.path());
    let first = store
        .persist_output(&context, RawToolOutput::text("first"))
        .await
        .unwrap()
        .artifact
        .unwrap();
    let second = store
        .persist_output(&context, RawToolOutput::text("second"))
        .await
        .unwrap()
        .artifact
        .unwrap();

    assert_ne!(first.path, second.path);
    assert_eq!(
        tokio::fs::read_to_string(workspace.path().join(first.path))
            .await
            .unwrap(),
        "first"
    );
}

#[tokio::test]
async fn preview_does_not_split_utf8_code_points() {
    let workspace = tempdir().unwrap();
    let store = ArtifactStore::new(ArtifactPolicy {
        inline_limit_bytes: 4,
        max_inline_bytes_per_run: 100,
        preview_head_bytes: 5,
        preview_tail_bytes: 5,
    });
    let output = store
        .persist_output(
            &context(workspace.path()),
            RawToolOutput::text("甲乙丙丁戊己庚辛"),
        )
        .await
        .unwrap();

    assert!(!output.preview.contains('\u{fffd}'));
}

#[tokio::test]
async fn spooled_file_preview_does_not_split_utf8_code_points() {
    let workspace = tempdir().unwrap();
    let source = workspace.path().join("spooled.tmp");
    let content = "甲乙丙丁戊己庚辛";
    tokio::fs::write(&source, content).await.unwrap();
    let store = ArtifactStore::new(ArtifactPolicy {
        inline_limit_bytes: 4,
        max_inline_bytes_per_run: 100,
        preview_head_bytes: 5,
        preview_tail_bytes: 5,
    });
    let output = store
        .persist_output(
            &context(workspace.path()),
            RawToolOutput::file(source.clone(), "text/plain", false),
        )
        .await
        .unwrap();

    assert!(!source.exists());
    assert!(!output.preview.contains("Non-UTF-8"));
    assert!(!output.preview.contains('\u{fffd}'));
    let artifact = output.artifact.unwrap();
    assert_eq!(
        tokio::fs::read(workspace.path().join(artifact.path))
            .await
            .unwrap(),
        content.as_bytes()
    );
}
