use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use ulid::Ulid;

use crate::{
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

use super::paths::{display_path, resolve_path};

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

#[derive(Debug)]
struct MatchedReplacement {
    start: usize,
    end: usize,
    replacement: String,
    used_flexible_match: bool,
}

#[async_trait]
impl Tool for WriteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write".to_owned(),
            description: "Create, overwrite, or precisely patch one UTF-8 file. Provide content for an intentional full-file write, or edits for one atomic set of targeted replacements. Every old_text is matched against the original file, must resolve to one non-overlapping region, and may use conservative line/indentation normalization only when exact matching fails. Prefer edits for existing files and include enough unchanged context to make each target unique.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative or absolute file path" },
                    "content": { "type": "string", "description": "Complete new file content; mutually exclusive with edits" },
                    "edits": {
                        "type": "array",
                        "minItems": 1,
                        "description": "Atomic replacements matched against the original file, not sequentially",
                        "items": {
                            "type": "object",
                            "properties": {
                                "old_text": { "type": "string", "description": "Unique text to replace; keep it small but include enough context" },
                                "new_text": { "type": "string", "description": "Replacement text" }
                            },
                            "required": ["old_text", "new_text"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["path"],
                "oneOf": [
                    { "required": ["content"] },
                    { "required": ["edits"] }
                ],
                "additionalProperties": false
            }),
        }
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

fn match_replacement(content: &str, edit: Replacement, index: usize) -> Result<MatchedReplacement> {
    let old_text = normalize_line_endings(&edit.old_text);
    let replacement = normalize_line_endings(&edit.new_text);
    let exact = overlapping_matches(content, &old_text);
    match exact.as_slice() {
        [(start, end)] => {
            return Ok(MatchedReplacement {
                start: *start,
                end: *end,
                replacement,
                used_flexible_match: false,
            });
        }
        matches if matches.len() > 1 => {
            bail!(
                "edits[{index}].old_text matches {} regions; include more context",
                matches.len()
            );
        }
        _ => {}
    }

    let flexible = flexible_line_matches(content, &old_text);
    match flexible.as_slice() {
        [(start, end, actual_indent, expected_indent)] => Ok(MatchedReplacement {
            start: *start,
            end: *end,
            replacement: reindent(&replacement, expected_indent, actual_indent),
            used_flexible_match: true,
        }),
        [] => {
            bail!("edits[{index}].old_text was not found; re-read the file and provide exact text")
        }
        matches => bail!(
            "edits[{index}].old_text flexibly matches {} regions; include more context",
            matches.len()
        ),
    }
}

fn flexible_line_matches(content: &str, needle: &str) -> Vec<(usize, usize, String, String)> {
    let needle_has_newline = needle.ends_with('\n');
    let needle_core = needle.strip_suffix('\n').unwrap_or(needle);
    let needle_lines = needle_core.split('\n').collect::<Vec<_>>();
    if needle_lines.is_empty() {
        return Vec::new();
    }
    let ranges = line_ranges(content);
    let expected_indent = common_indent(&needle_lines);
    let mut matches = Vec::new();
    for window in ranges.windows(needle_lines.len()) {
        let actual_lines = window
            .iter()
            .map(|(start, end)| &content[*start..*end])
            .collect::<Vec<_>>();
        if actual_lines
            .iter()
            .zip(&needle_lines)
            .all(|(actual, expected)| actual.trim() == expected.trim())
        {
            let start = window[0].0;
            let mut end = window[window.len() - 1].1;
            if needle_has_newline && content.as_bytes().get(end) == Some(&b'\n') {
                end += 1;
            }
            matches.push((
                start,
                end,
                common_indent(&actual_lines),
                expected_indent.clone(),
            ));
        }
    }
    matches
}

fn line_ranges(content: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (index, byte) in content.bytes().enumerate() {
        if byte == b'\n' {
            ranges.push((start, index));
            start = index + 1;
        }
    }
    if start <= content.len() {
        ranges.push((start, content.len()));
    }
    ranges
}

fn common_indent(lines: &[&str]) -> String {
    lines
        .iter()
        .find(|line| !line.trim().is_empty())
        .map(|line| {
            line.chars()
                .take_while(|character| character.is_whitespace())
                .collect()
        })
        .unwrap_or_default()
}

fn reindent(replacement: &str, expected: &str, actual: &str) -> String {
    if expected == actual {
        return replacement.to_owned();
    }
    replacement
        .split_inclusive('\n')
        .map(|line| {
            let (body, ending) = line
                .strip_suffix('\n')
                .map_or((line, ""), |body| (body, "\n"));
            if body.trim().is_empty() {
                format!("{body}{ending}")
            } else {
                let body = body.strip_prefix(expected).unwrap_or(body.trim_start());
                format!("{actual}{body}{ending}")
            }
        })
        .collect()
}

fn normalize_line_endings(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}

fn detect_line_ending(value: &str) -> Result<&'static str> {
    let has_crlf = value.contains("\r\n");
    let remainder = value.replace("\r\n", "");
    let has_lf = remainder.contains('\n');
    let has_cr = remainder.contains('\r');
    if (has_crlf as u8 + has_lf as u8 + has_cr as u8) > 1 {
        bail!(
            "write targeted edits refuse mixed line endings; use a full-file content write to normalize intentionally"
        );
    }
    Ok(if has_crlf {
        "\r\n"
    } else if has_cr {
        "\r"
    } else {
        "\n"
    })
}

fn overlapping_matches(content: &str, needle: &str) -> Vec<(usize, usize)> {
    content
        .char_indices()
        .filter_map(|(start, _)| {
            content[start..]
                .starts_with(needle)
                .then_some((start, start + needle.len()))
        })
        .collect()
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
    let temporary = parent.join(format!(".{name}.pico-{}.tmp", Ulid::new()));
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
