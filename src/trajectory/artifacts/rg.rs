use std::{path::Path, process::Stdio};

use anyhow::{Context, Result, bail};
use regex::Regex;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt},
    process::{Child, Command},
};

const MAX_RG_LINE_BYTES: usize = 128;
const MAX_RG_STDERR_BYTES: usize = 4 * 1024;
const SNIPPET_BEFORE_BYTES: u64 = 512;
const SNIPPET_AFTER_BYTES: u64 = 1024;
const SNIPPET_BEFORE_CHARS: usize = 120;
const SNIPPET_AFTER_CHARS: usize = 240;

pub(super) async fn search_file(
    path: &Path,
    file_bytes: u64,
    pattern: &Regex,
) -> Result<Option<String>> {
    let Some(offset) = first_match_offset(path, pattern).await? else {
        return Ok(None);
    };
    bounded_snippet(path, file_bytes, offset).await.map(Some)
}

async fn first_match_offset(path: &Path, pattern: &Regex) -> Result<Option<u64>> {
    let mut child = Command::new("rg")
        .arg("--no-config")
        .arg("--text")
        .arg("--multiline")
        .arg("--color=never")
        .arg("--no-filename")
        .arg("--byte-offset")
        .arg("--only-matching")
        .arg("--max-count=1")
        .arg("--replace=")
        .arg("--")
        .arg(pattern.as_str())
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| {
            "start `rg` for compacted-history artifact search; install ripgrep and ensure it is on PATH"
        })?;

    read_offset(&mut child, path).await
}

async fn read_offset(child: &mut Child, path: &Path) -> Result<Option<u64>> {
    let mut stdout = child.stdout.take().context("capture rg stdout")?;
    let stderr = child.stderr.take().context("capture rg stderr")?;
    let stderr_task = tokio::spawn(read_capped_and_drain(stderr, MAX_RG_STDERR_BYTES));
    let mut line = Vec::new();
    let mut buffer = [0_u8; 64];
    let found_line = loop {
        let read = stdout.read(&mut buffer).await.context("read rg stdout")?;
        if read == 0 {
            break false;
        }
        let chunk = &buffer[..read];
        if let Some(newline) = chunk.iter().position(|byte| *byte == b'\n') {
            if line.len() + newline > MAX_RG_LINE_BYTES {
                terminate(child).await;
                bail!("rg returned an unexpectedly long match offset");
            }
            line.extend_from_slice(&chunk[..newline]);
            break true;
        }
        if line.len() + chunk.len() > MAX_RG_LINE_BYTES {
            terminate(child).await;
            bail!("rg returned an unexpectedly long match offset");
        }
        line.extend_from_slice(chunk);
    };

    if found_line {
        terminate(child).await;
        let _ = stderr_task.await;
        return parse_offset(&line).map(Some);
    }

    let status = child.wait().await.context("wait for rg")?;
    let stderr = stderr_task.await.context("join rg stderr reader")??;
    if status.code() == Some(1) {
        return Ok(None);
    }
    if status.success() {
        return Ok(None);
    }
    let stderr = String::from_utf8_lossy(&stderr);
    bail!(
        "rg failed while searching artifact {} (status {}): {}",
        path.display(),
        status,
        stderr.trim()
    )
}

fn parse_offset(line: &[u8]) -> Result<u64> {
    let offset = line
        .strip_suffix(b":")
        .context("rg match offset did not end with `:`")?;
    let offset = std::str::from_utf8(offset).context("rg match offset was not UTF-8")?;
    offset
        .parse::<u64>()
        .with_context(|| format!("parse rg match offset `{offset}`"))
}

async fn terminate(child: &mut Child) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

async fn read_capped_and_drain(
    mut reader: impl AsyncRead + Unpin,
    cap: usize,
) -> std::io::Result<Vec<u8>> {
    let mut captured = Vec::with_capacity(cap);
    let mut buffer = [0_u8; 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(captured);
        }
        let remaining = cap.saturating_sub(captured.len());
        captured.extend_from_slice(&buffer[..read.min(remaining)]);
    }
}

async fn bounded_snippet(path: &Path, file_bytes: u64, offset: u64) -> Result<String> {
    if offset > file_bytes {
        bail!(
            "rg returned byte offset {offset} beyond artifact size {file_bytes}: {}",
            path.display()
        );
    }
    let start = offset.saturating_sub(SNIPPET_BEFORE_BYTES);
    let end = offset.saturating_add(SNIPPET_AFTER_BYTES).min(file_bytes);
    let length = usize::try_from(end - start).context("artifact snippet length exceeds usize")?;
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("open artifact snippet {}", path.display()))?;
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .with_context(|| format!("seek artifact snippet {}", path.display()))?;
    let mut bytes = vec![0_u8; length];
    file.read_exact(&mut bytes)
        .await
        .with_context(|| format!("read artifact snippet {}", path.display()))?;

    let split = usize::try_from(offset - start).context("artifact snippet offset exceeds usize")?;
    let (trimmed_start, trimmed_end) = valid_utf8_window(&bytes)
        .with_context(|| format!("artifact snippet is not UTF-8: {}", path.display()))?;
    if split < trimmed_start || split > trimmed_end {
        bail!(
            "rg match offset falls outside the valid UTF-8 snippet window: {}",
            path.display()
        );
    }
    let text = std::str::from_utf8(&bytes[trimmed_start..trimmed_end])
        .expect("valid_utf8_window returned invalid UTF-8");
    let text_split = split - trimmed_start;
    if !text.is_char_boundary(text_split) {
        bail!(
            "rg match offset is not a UTF-8 character boundary: {}",
            path.display()
        );
    }
    let (full_prefix, full_suffix) = text.split_at(text_split);
    let prefix_chars = full_prefix.chars().count();
    let suffix_chars = full_suffix.chars().count();
    let prefix = last_chars(full_prefix, SNIPPET_BEFORE_CHARS);
    let suffix = first_chars(full_suffix, SNIPPET_AFTER_CHARS);
    let leading = if start > 0 || trimmed_start > 0 || prefix_chars > SNIPPET_BEFORE_CHARS {
        "…"
    } else {
        ""
    };
    let trailing =
        if end < file_bytes || trimmed_end < bytes.len() || suffix_chars > SNIPPET_AFTER_CHARS {
            "…"
        } else {
            ""
        };
    Ok(format!("{leading}{prefix}{suffix}{trailing}"))
}

/// Removes only partial scalar values caused by bounded reads at the two file
/// edges. Any invalid UTF-8 inside the selected window remains an error.
fn valid_utf8_window(bytes: &[u8]) -> Result<(usize, usize)> {
    let leading = bytes
        .iter()
        .take_while(|byte| (**byte & 0b1100_0000) == 0b1000_0000)
        .count();
    if leading > 3 {
        bail!("more than three leading UTF-8 continuation bytes");
    }
    let candidate = &bytes[leading..];
    match std::str::from_utf8(candidate) {
        Ok(_) => Ok((leading, bytes.len())),
        Err(error) if error.error_len().is_none() => Ok((leading, leading + error.valid_up_to())),
        Err(error) => bail!("invalid UTF-8 at byte {}", leading + error.valid_up_to()),
    }
}

fn first_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn last_chars(value: &str, limit: usize) -> String {
    let count = value.chars().count();
    value.chars().skip(count.saturating_sub(limit)).collect()
}
