use std::{mem, path::PathBuf};

use anyhow::{Context, Result, ensure};
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

use crate::trajectory::TrajectoryMessage;

use super::{parse_stored_line, trajectory_record};

/// One complete physical message record and its exact source representation.
///
/// `raw` includes the terminating LF and remains byte-for-byte identical to the
/// durable log, so viewers and NDJSON sinks never need to reserialize it.
#[derive(Debug)]
pub(crate) struct DecodedRecord {
    pub(crate) trajectory: TrajectoryMessage,
    pub(crate) raw: Vec<u8>,
    pub(crate) source_offset: u64,
    pub(crate) end_offset: u64,
}

/// Validates consecutive newline-terminated message records.
#[derive(Debug)]
pub(crate) struct LineDecoder {
    next_seq: u64,
    next_line_offset: u64,
}

impl Default for LineDecoder {
    fn default() -> Self {
        Self::new(1, 0)
    }
}

impl LineDecoder {
    pub(crate) fn new(next_seq: u64, source_offset: u64) -> Self {
        Self {
            next_seq,
            next_line_offset: source_offset,
        }
    }

    pub(crate) fn visible_end(&self) -> u64 {
        self.next_line_offset
    }

    pub(crate) fn push_complete_line(
        &mut self,
        path: &std::path::Path,
        line_with_newline: Vec<u8>,
        line_end: u64,
    ) -> Result<DecodedRecord> {
        ensure!(
            line_with_newline.ends_with(b"\n"),
            "message decoder requires a newline-terminated record"
        );
        let line_len = u64::try_from(line_with_newline.len())
            .context("message record length does not fit in u64")?;
        let source_offset = line_end
            .checked_sub(line_len)
            .context("message record offset precedes the start of the file")?;
        ensure!(
            source_offset == self.next_line_offset,
            "message record starts at byte {source_offset}, expected {}",
            self.next_line_offset
        );
        let stored = parse_stored_line(path, &line_with_newline)?;
        let trajectory = trajectory_record(stored, self.next_seq)?;
        self.next_seq = self
            .next_seq
            .checked_add(1)
            .context("message sequence overflow")?;
        self.next_line_offset = line_end;
        Ok(DecodedRecord {
            trajectory,
            raw: line_with_newline,
            source_offset,
            end_offset: line_end,
        })
    }
}

/// Reads complete newline-delimited messages lazily. A partial final line stays
/// buffered so the same reader can continue after a live writer appends it.
pub(crate) struct CompleteLineReader<R> {
    reader: R,
    path: PathBuf,
    decoder: LineDecoder,
    partial_line: Vec<u8>,
    bytes_read: u64,
}

impl<R> CompleteLineReader<R> {
    pub(crate) fn new(reader: R, path: PathBuf) -> Self {
        Self::with_position(reader, path, 1, 0)
    }

    pub(crate) fn with_position(
        reader: R,
        path: PathBuf,
        next_seq: u64,
        source_offset: u64,
    ) -> Self {
        Self {
            reader,
            path,
            decoder: LineDecoder::new(next_seq, source_offset),
            partial_line: Vec::new(),
            bytes_read: source_offset,
        }
    }

    pub(crate) fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    pub(crate) fn visible_end(&self) -> u64 {
        self.decoder.visible_end()
    }
}

impl<R: AsyncBufRead + Unpin> CompleteLineReader<R> {
    pub(crate) async fn next_record(&mut self) -> Result<Option<DecodedRecord>> {
        let read = self
            .reader
            .read_until(b'\n', &mut self.partial_line)
            .await
            .with_context(|| format!("read initialized message log {}", self.path.display()))?;
        self.bytes_read = self
            .bytes_read
            .checked_add(read as u64)
            .context("message log byte offset overflow")?;
        if !self.partial_line.ends_with(b"\n") {
            return Ok(None);
        }
        let line = mem::take(&mut self.partial_line);
        self.decoder
            .push_complete_line(&self.path, line, self.bytes_read)
            .map(Some)
    }
}

#[cfg(test)]
mod tests;
