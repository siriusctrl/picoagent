use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;
use ulid::Ulid;

use crate::{
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

use super::paths::{display_path, resolve_path};
use matching::{detect_line_ending, match_replacement, normalize_line_endings};

mod matching;

#[derive(Debug, Clone, Deserialize)]
struct Replacement {
    old_text: String,
    new_text: String,
}

#[derive(Debug, Deserialize)]
struct WriteArgs {
    path: String,
    content: Option<String>,
    edits: Option<Vec<Replacement>>,
}

#[derive(Clone, Default)]
pub struct WriteTool {
    locks: Arc<Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>>>,
}

#[async_trait]
impl Tool for WriteTool {
    fn spec(&self) -> ToolSpec {
        crate::tools::embedded_tool_spec(include_str!("tool.yaml"), module_path!())
    }

    async fn execute(&self, context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: WriteArgs =
            serde_json::from_value(arguments).context("invalid write arguments")?;
        let path = canonical_write_path(resolve_path(&context.workspace, &args.path)).await?;
        let lock = self.lock_for(path.clone()).await;
        let _guard = lock.lock().await;
        match (args.content, args.edits) {
            (Some(content), None) => write_complete(&context, &path, content).await,
            (None, Some(edits)) if !edits.is_empty() => patch_file(&context, &path, edits).await,
            _ => bail!("write requires exactly one of `content` or non-empty `edits`"),
        }
    }
}

impl WriteTool {
    async fn lock_for(&self, path: PathBuf) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().await;
        locks
            .entry(path)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

async fn write_complete(
    context: &ToolContext,
    path: &std::path::Path,
    content: String,
) -> Result<RawToolOutput> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create directory {}", parent.display()))?;
    }
    replace_file(path, content.as_bytes()).await?;
    Ok(RawToolOutput::text(format!(
        "Wrote {} bytes to {}",
        content.len(),
        display_path(&context.workspace, path)
    )))
}

async fn patch_file(
    context: &ToolContext,
    path: &std::path::Path,
    edits: Vec<Replacement>,
) -> Result<RawToolOutput> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("read UTF-8 file {}", path.display()))?;
    let (bom, without_bom) = raw
        .strip_prefix('\u{feff}')
        .map_or(("", raw.as_str()), |text| ("\u{feff}", text));
    let line_ending = detect_line_ending(without_bom)?;
    let original = normalize_line_endings(without_bom);
    let mut matched = Vec::with_capacity(edits.len());
    for (index, edit) in edits.into_iter().enumerate() {
        if edit.old_text.is_empty() {
            bail!("edits[{index}].old_text must not be empty");
        }
        if edit.old_text == edit.new_text {
            bail!("edits[{index}] does not change the file");
        }
        matched.push(match_replacement(&original, edit, index)?);
    }
    matched.sort_by_key(|edit| edit.start);
    for pair in matched.windows(2) {
        if pair[0].end > pair[1].start {
            bail!("write edits overlap; merge nearby changes into one replacement");
        }
    }
    let flexible_count = matched
        .iter()
        .filter(|edit| edit.used_flexible_match)
        .count();
    let count = matched.len();
    let mut updated = original.clone();
    for edit in matched.into_iter().rev() {
        updated.replace_range(edit.start..edit.end, &edit.replacement);
    }
    if updated == original {
        bail!("write replacements produced no changes");
    }
    let restored = match line_ending {
        "\r\n" => updated.replace('\n', "\r\n"),
        "\r" => updated.replace('\n', "\r"),
        _ => updated,
    };
    replace_file(path, format!("{bom}{restored}").as_bytes()).await?;
    Ok(RawToolOutput::text(format!(
        "Applied {count} atomic replacement(s) to {}{}",
        display_path(&context.workspace, path),
        if flexible_count == 0 {
            String::new()
        } else {
            format!(" ({flexible_count} matched with line/indentation normalization)")
        }
    )))
}

async fn replace_file(path: &std::path::Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("write path has no parent directory")?;
    tokio::fs::create_dir_all(parent).await?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let temporary = parent.join(format!(".{name}.fiasco-{}.tmp", Ulid::new()));
    let result = async {
        tokio::fs::write(&temporary, content).await?;
        if let Ok(metadata) = tokio::fs::metadata(path).await {
            tokio::fs::set_permissions(&temporary, metadata.permissions()).await?;
        }
        tokio::fs::rename(&temporary, path).await?;
        Result::<(), std::io::Error>::Ok(())
    }
    .await;
    if let Err(error) = result {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error).with_context(|| format!("replace file {}", path.display()));
    }
    Ok(())
}

async fn canonical_write_path(path: PathBuf) -> Result<PathBuf> {
    let normalized = normalize_lexically(&path);
    if tokio::fs::try_exists(&normalized).await? {
        return tokio::fs::canonicalize(&normalized)
            .await
            .with_context(|| format!("resolve write target {}", normalized.display()));
    }
    if let Some(parent) = normalized.parent()
        && tokio::fs::try_exists(parent).await?
    {
        let parent = tokio::fs::canonicalize(parent).await?;
        return Ok(parent.join(
            normalized
                .file_name()
                .context("write path has no file name")?,
        ));
    }
    Ok(normalized)
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}
