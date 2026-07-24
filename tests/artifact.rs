use fiasco::{
    artifact::{ArtifactPolicy, ArtifactStore},
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
    assert!(!workspace.path().join(".fiasco").exists());
}

#[tokio::test]
async fn each_small_result_stays_inline_independently() {
    let workspace = tempdir().unwrap();
    let store = ArtifactStore::new(ArtifactPolicy {
        inline_limit_bytes: 32,
        preview_head_bytes: 8,
        preview_tail_bytes: 4,
    });
    let first = store
        .persist_output(&context(workspace.path()), RawToolOutput::text("12345678"))
        .await
        .unwrap();
    assert!(first.artifact.is_none());

    let mut second_raw = RawToolOutput::text("abcdefgh");
    second_raw.is_error = true;
    let second = store
        .persist_output(&context(workspace.path()), second_raw)
        .await
        .unwrap();
    assert_eq!(second.preview, "abcdefgh");
    assert!(second.artifact.is_none());
    assert!(second.is_error);
}

#[tokio::test]
async fn artifact_backed_full_preview_is_not_labeled_truncated() {
    let workspace = tempdir().unwrap();
    let store = ArtifactStore::new(ArtifactPolicy {
        inline_limit_bytes: 4,
        preview_head_bytes: 8,
        preview_tail_bytes: 8,
    });
    let output = store
        .persist_output(&context(workspace.path()), RawToolOutput::text("abcdef"))
        .await
        .unwrap();

    assert!(output.artifact.is_some());
    assert!(!output.truncated);
    assert_eq!(output.preview, "abcdef");
    assert!(output.model_content().contains("truncated: false"));
    assert!(
        output
            .model_content()
            .contains("bytes: total=6; preview_head=6; preview_tail=0; omitted=0")
    );
    assert!(output.model_content().contains("preview_limitation: none"));
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
                attach_to_model: false,
            },
        )
        .await
        .unwrap();

    assert!(output.truncated);
    assert!(output.preview.is_empty());
    let model_content = output.model_content();
    assert!(model_content.contains("preview_head=0; preview_tail=0; omitted=4"));
    assert!(model_content.contains("preview_limitation: binary_or_non_utf8"));
    assert!(!model_content.contains("[Preview]"));
    assert!(output.artifact.unwrap().path.contains("call-binary-"));
}

#[tokio::test]
async fn image_results_are_artifacts_with_model_attachments() {
    let workspace = tempdir().unwrap();
    let bytes = b"\x89PNG\r\n\x1a\nimage";
    let output = ArtifactStore::default()
        .persist_output(
            &context(workspace.path()),
            RawToolOutput::image(bytes.to_vec(), "image/png"),
        )
        .await
        .unwrap();

    let artifact = output.artifact.as_ref().unwrap();
    assert_eq!(artifact.media_type, "image/png");
    assert!(artifact.path.ends_with(".png"));
    let attachment = output.attachment.as_ref().unwrap();
    assert_eq!(attachment.media_type, "image/png");
    assert_eq!(attachment.data, "iVBORw0KGgppbWFnZQ==");
    assert!(output.model_content().contains("media_type: image/png"));
}

#[tokio::test]
async fn spills_large_results_with_a_run_local_attachment_and_head_tail_preview() {
    let workspace = tempdir().unwrap();
    let store = ArtifactStore::new(ArtifactPolicy {
        inline_limit_bytes: 20,
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
    assert!(model_content.contains("preview_head=8; preview_tail=8"));
    assert!(model_content.contains("preview_limitation: none"));
    assert!(model_content.contains("mutable run-local attachment"));
    assert!(model_content.contains("searches observe its current contents"));
    assert!(model_content.contains("generation-time preview"));
    assert!(model_content.contains("returned `line_offset` or `byte_offset`"));
    assert!(model_content.contains("`bash`/`rg`"));
    assert!(!model_content.contains("sha256:"));

    let artifact = output.artifact.unwrap();
    assert!(
        artifact
            .path
            .starts_with(".fiasco/runs/run-1/artifacts/call_1-")
    );
    assert!(artifact.path.ends_with(".txt"));

    let stored = tokio::fs::read_to_string(workspace.path().join(&artifact.path))
        .await
        .unwrap();
    assert_eq!(stored, content);
    let artifact_directory = workspace.path().join(".fiasco/runs/run-1/artifacts");
    let entries = std::fs::read_dir(artifact_directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0], workspace.path().join(&artifact.path));

    let preview_info = serde_json::to_value(output.preview_info.unwrap()).unwrap();
    assert!(preview_info.get("strategy").is_none());
    assert!(preview_info.get("omitted_region").is_none());
    assert!(preview_info.get("reason").is_none());
}

#[tokio::test]
async fn artifact_reference_remains_valid_after_the_attachment_changes() {
    let workspace = tempdir().unwrap();
    let store = ArtifactStore::new(ArtifactPolicy {
        inline_limit_bytes: 1,
        preview_head_bytes: 4,
        preview_tail_bytes: 4,
    });
    let output = store
        .persist_output(&context(workspace.path()), RawToolOutput::text("original"))
        .await
        .unwrap();
    let artifact = output.artifact.unwrap();

    tokio::fs::write(workspace.path().join(&artifact.path), "updated attachment")
        .await
        .unwrap();

    assert_eq!(
        tokio::fs::read_to_string(workspace.path().join(&artifact.path))
            .await
            .unwrap(),
        "updated attachment"
    );
    assert_eq!(artifact.media_type, "text/plain; charset=utf-8");
}

#[tokio::test]
async fn repeated_call_ids_do_not_overwrite_prior_artifacts() {
    let workspace = tempdir().unwrap();
    let store = ArtifactStore::new(ArtifactPolicy {
        inline_limit_bytes: 1,
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
