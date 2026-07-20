use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// Direct byte accounting for the model-facing portion of an artifact.
///
/// Head bytes come from the start of the artifact and tail bytes from the end;
/// `omitted_bytes` is everything between or after them. That makes separate
/// strategy and omitted-region fields unnecessary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreviewInfo {
    pub shown_head_bytes: u64,
    pub shown_tail_bytes: u64,
    pub omitted_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limitation: Option<PreviewLimitation>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreviewLimitation {
    BinaryOrNonUtf8,
}

impl PreviewLimitation {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::BinaryOrNonUtf8 => "binary_or_non_utf8",
        }
    }
}

struct PreviewSlice {
    pub text: String,
    pub shown_head_bytes: usize,
    pub shown_tail_bytes: usize,
}

pub(super) fn textual_preview(
    content: &[u8],
    head_limit: usize,
    tail_limit: usize,
) -> (String, PreviewInfo) {
    let desired = preview_bytes(content, head_limit, tail_limit);
    preview_output(desired, content.len() as u64, None)
}

pub(super) async fn file_preview(
    path: &Path,
    bytes: u64,
    head_limit: usize,
    tail_limit: usize,
) -> Result<(String, PreviewInfo)> {
    let Some(desired) = preview_file(path, bytes, head_limit, tail_limit).await? else {
        return Ok(unavailable_preview(bytes));
    };
    Ok(preview_output(desired, bytes, None))
}

pub(super) fn unavailable_preview(total_bytes: u64) -> (String, PreviewInfo) {
    (
        String::new(),
        PreviewInfo {
            shown_head_bytes: 0,
            shown_tail_bytes: 0,
            omitted_bytes: total_bytes,
            limitation: Some(PreviewLimitation::BinaryOrNonUtf8),
        },
    )
}

fn preview_output(
    preview: PreviewSlice,
    total_bytes: u64,
    limitation: Option<PreviewLimitation>,
) -> (String, PreviewInfo) {
    let shown_head_bytes = preview.shown_head_bytes as u64;
    let shown_tail_bytes = preview.shown_tail_bytes as u64;
    let shown_bytes = shown_head_bytes.saturating_add(shown_tail_bytes);
    let omitted_bytes = total_bytes.saturating_sub(shown_bytes);
    (
        preview.text,
        PreviewInfo {
            shown_head_bytes,
            shown_tail_bytes,
            omitted_bytes,
            limitation,
        },
    )
}

fn preview_bytes(content: &[u8], head_bytes: usize, tail_bytes: usize) -> PreviewSlice {
    let head_end = floor_char_boundary(content, head_bytes.min(content.len()));
    let tail_start = ceil_char_boundary(content, content.len().saturating_sub(tail_bytes));

    if head_end >= tail_start {
        return PreviewSlice {
            text: String::from_utf8_lossy(content).into_owned(),
            shown_head_bytes: content.len(),
            shown_tail_bytes: 0,
        };
    }

    PreviewSlice {
        text: format!(
            "{}\n\n... {} bytes omitted ...\n\n{}",
            String::from_utf8_lossy(&content[..head_end]),
            tail_start - head_end,
            String::from_utf8_lossy(&content[tail_start..]),
        ),
        shown_head_bytes: head_end,
        shown_tail_bytes: content.len() - tail_start,
    }
}

async fn preview_file(
    path: &Path,
    bytes: u64,
    head_bytes: usize,
    tail_bytes: usize,
) -> Result<Option<PreviewSlice>> {
    if bytes <= head_bytes.saturating_add(tail_bytes).saturating_add(8) as u64 {
        let content = tokio::fs::read(path).await?;
        if std::str::from_utf8(&content).is_err() {
            return Ok(None);
        }
        return Ok(Some(preview_bytes(&content, head_bytes, tail_bytes)));
    }

    let mut file = tokio::fs::File::open(path).await?;
    let head_read = head_bytes.saturating_add(4).min(bytes as usize);
    let mut head = vec![0_u8; head_read];
    file.read_exact(&mut head).await?;
    let head_end = floor_char_boundary(&head, head_bytes.min(head.len()));
    head.truncate(head_end);

    let tail_read = tail_bytes.saturating_add(4).min(bytes as usize);
    file.seek(std::io::SeekFrom::Start(bytes - tail_read as u64))
        .await?;
    let mut tail = vec![0_u8; tail_read];
    file.read_exact(&mut tail).await?;
    let desired_start = tail_read.saturating_sub(tail_bytes);
    let tail_start = ceil_char_boundary(&tail, desired_start);
    let tail = &tail[tail_start..];
    match (std::str::from_utf8(&head), std::str::from_utf8(tail)) {
        (Ok(head), Ok(tail)) => Ok(Some(PreviewSlice {
            text: format!(
                "{head}\n\n... {} bytes omitted ...\n\n{tail}",
                bytes.saturating_sub(head.len() as u64 + tail.len() as u64)
            ),
            shown_head_bytes: head.len(),
            shown_tail_bytes: tail.len(),
        })),
        _ => Ok(None),
    }
}

fn floor_char_boundary(bytes: &[u8], mut index: usize) -> usize {
    while index > 0 && index < bytes.len() && (bytes[index] & 0b1100_0000) == 0b1000_0000 {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && (bytes[index] & 0b1100_0000) == 0b1000_0000 {
        index += 1;
    }
    index
}
