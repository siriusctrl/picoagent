use std::path::Path;

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

pub(super) fn cap_utf8(mut value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }
    let mut boundary = limit;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}

pub(super) fn preview_bytes(content: &[u8], head_bytes: usize, tail_bytes: usize) -> String {
    let head_end = floor_char_boundary(content, head_bytes.min(content.len()));
    let tail_start = ceil_char_boundary(content, content.len().saturating_sub(tail_bytes));

    if head_end >= tail_start {
        return String::from_utf8_lossy(content).into_owned();
    }

    format!(
        "{}\n\n... {} bytes omitted ...\n\n{}",
        String::from_utf8_lossy(&content[..head_end]),
        tail_start - head_end,
        String::from_utf8_lossy(&content[tail_start..]),
    )
}

pub(super) async fn preview_file(
    path: &Path,
    bytes: u64,
    head_bytes: usize,
    tail_bytes: usize,
) -> Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let head_read = head_bytes.saturating_add(4).min(bytes as usize);
    let mut head = vec![0_u8; head_read];
    file.read_exact(&mut head).await?;
    let head_end = floor_char_boundary(&head, head_bytes.min(head.len()));
    head.truncate(head_end);

    let tail_read = (tail_bytes + 4).min(bytes as usize);
    file.seek(std::io::SeekFrom::Start(bytes - tail_read as u64))
        .await?;
    let mut tail = vec![0_u8; tail_read];
    file.read_exact(&mut tail).await?;
    let desired_start = tail_read.saturating_sub(tail_bytes);
    let tail_start = ceil_char_boundary(&tail, desired_start);
    let tail = &tail[tail_start..];
    match (std::str::from_utf8(&head), std::str::from_utf8(tail)) {
        (Ok(head), Ok(tail)) => Ok(format!(
            "{head}\n\n... {} bytes omitted ...\n\n{tail}",
            bytes.saturating_sub(head.len() as u64 + tail.len() as u64)
        )),
        _ => Ok(format!(
            "[Non-UTF-8 output omitted from context: {bytes} bytes]"
        )),
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
