use std::{mem, path::PathBuf};

use anyhow::{Context, Result, ensure};
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

use crate::trajectory::TrajectoryMessage;

use super::{MessageCheckpoint, parse_stored_line, trajectory_record};

/// One committed physical message record and its exact source representation.
///
/// `raw` includes the terminating LF and remains byte-for-byte identical to the
/// durable log, so an embedded viewer or NDJSON sink never needs to serialize a
/// parsed [`TrajectoryMessage`] back into JSON or recreate record delimiters.
#[derive(Debug)]
pub(crate) struct CommittedRecord {
    pub(crate) trajectory: TrajectoryMessage,
    pub(crate) raw: Vec<u8>,
    pub(crate) source_offset: u64,
}

/// A complete logical commit group. No record is returned before the declared
/// checkpoint count, indexes, refs, and newline boundaries all validate.
#[derive(Debug)]
pub(crate) struct CommittedCheckpoint {
    pub(crate) records: Vec<CommittedRecord>,
    pub(crate) committed_end: u64,
}

#[derive(Debug)]
pub(crate) enum DecodeResult {
    Checkpoint(CommittedCheckpoint),
    NeedMore,
}

#[derive(Debug)]
struct PendingCheckpoint {
    metadata: MessageCheckpoint,
    records: Vec<CommittedRecord>,
}

/// Incrementally validates the shared physical-record and logical-checkpoint
/// contract. At most one not-yet-committed checkpoint is retained.
#[derive(Debug)]
pub(crate) struct CheckpointDecoder {
    next_seq: u64,
    committed_end: u64,
    next_line_offset: u64,
    pending: Option<PendingCheckpoint>,
}

impl Default for CheckpointDecoder {
    fn default() -> Self {
        Self::new(1, 0)
    }
}

impl CheckpointDecoder {
    pub(crate) fn new(next_seq: u64, committed_end: u64) -> Self {
        Self {
            next_seq,
            committed_end,
            next_line_offset: committed_end,
            pending: None,
        }
    }

    pub(crate) fn committed_end(&self) -> u64 {
        self.committed_end
    }

    pub(crate) fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Validate one complete line as the first record of the next checkpoint
    /// without advancing decoder state. Directional readers use this to stop
    /// before a checkpoint that cannot fit the remainder of a non-empty batch.
    pub(crate) fn preflight_checkpoint_start(
        &self,
        path: &std::path::Path,
        line_with_newline: &[u8],
    ) -> Result<u64> {
        ensure!(
            self.pending.is_none(),
            "checkpoint preflight requires a checkpoint boundary"
        );
        ensure!(
            line_with_newline.ends_with(b"\n"),
            "checkpoint preflight requires a newline-terminated record"
        );
        let stored = parse_stored_line(path, line_with_newline)?;
        let checkpoint = stored.local.checkpoint.clone();
        self.validate_checkpoint_start(&stored, checkpoint.as_ref())?;
        let _ = trajectory_record(stored, self.next_seq)?;
        Ok(checkpoint.map_or(1, |checkpoint| checkpoint.count))
    }

    pub(crate) fn push_complete_line(
        &mut self,
        path: &std::path::Path,
        line_with_newline: Vec<u8>,
        line_end: u64,
    ) -> Result<DecodeResult> {
        ensure!(
            line_with_newline.ends_with(b"\n"),
            "checkpoint decoder requires a newline-terminated record"
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
        let checkpoint = stored.local.checkpoint.clone();
        let expected_index = self
            .pending
            .as_ref()
            .map_or(0, |pending| pending.records.len() as u64);
        if let Some(pending) = self.pending.as_ref() {
            let actual = checkpoint.as_ref().with_context(|| {
                format!(
                    "message `{}` is missing checkpoint metadata inside group `{}`",
                    stored.message_ref, pending.metadata.first_message_ref
                )
            })?;
            ensure!(
                actual.first_message_ref == pending.metadata.first_message_ref
                    && actual.count == pending.metadata.count
                    && actual.index == expected_index,
                "message `{}` has inconsistent checkpoint metadata",
                stored.message_ref
            );
        } else {
            self.validate_checkpoint_start(&stored, checkpoint.as_ref())?;
        }
        let expected_seq = self
            .next_seq
            .checked_add(expected_index)
            .context("message sequence overflow inside checkpoint")?;
        let trajectory = trajectory_record(stored, expected_seq)?;
        let record = CommittedRecord {
            trajectory,
            raw: line_with_newline,
            source_offset,
        };

        if let Some(pending) = self.pending.as_mut() {
            self.next_line_offset = line_end;
            pending.records.push(record);
            if expected_index + 1 < pending.metadata.count {
                return Ok(DecodeResult::NeedMore);
            }
            return self.commit_pending(line_end);
        }

        let Some(checkpoint) = checkpoint else {
            // Pre-checkpoint logs are singleton records. New appends always
            // persist explicit count=1 checkpoint metadata.
            self.next_seq = self
                .next_seq
                .checked_add(1)
                .context("message sequence overflow after singleton checkpoint")?;
            self.committed_end = line_end;
            self.next_line_offset = line_end;
            return Ok(DecodeResult::Checkpoint(CommittedCheckpoint {
                records: vec![record],
                committed_end: line_end,
            }));
        };
        let count = checkpoint.count;
        self.next_line_offset = line_end;
        self.pending = Some(PendingCheckpoint {
            metadata: checkpoint,
            records: vec![record],
        });
        if count == 1 {
            self.commit_pending(line_end)
        } else {
            Ok(DecodeResult::NeedMore)
        }
    }

    fn validate_checkpoint_start(
        &self,
        stored: &super::StoredMessage,
        checkpoint: Option<&MessageCheckpoint>,
    ) -> Result<()> {
        if let Some(checkpoint) = checkpoint {
            ensure!(
                checkpoint.count > 0,
                "message checkpoint count must be positive"
            );
            ensure!(
                checkpoint.index == 0,
                "message checkpoint `{}` starts at index {} instead of 0",
                checkpoint.first_message_ref,
                checkpoint.index
            );
            ensure!(
                checkpoint.first_message_ref == stored.message_ref,
                "message checkpoint `{}` starts with message `{}`",
                checkpoint.first_message_ref,
                stored.message_ref
            );
            usize::try_from(checkpoint.count)
                .context("message checkpoint count does not fit in memory")?;
            self.next_seq
                .checked_add(checkpoint.count)
                .context("message sequence overflow after checkpoint")?;
        }
        Ok(())
    }

    fn commit_pending(&mut self, committed_end: u64) -> Result<DecodeResult> {
        let pending = self
            .pending
            .as_ref()
            .context("checkpoint decoder has no pending checkpoint to commit")?;
        ensure!(
            pending.records.len() as u64 == pending.metadata.count,
            "checkpoint decoder committed an incomplete checkpoint"
        );
        let next_seq = self
            .next_seq
            .checked_add(pending.metadata.count)
            .context("message sequence overflow after checkpoint")?;
        let pending = self
            .pending
            .take()
            .context("checkpoint decoder lost its pending checkpoint")?;
        self.next_seq = next_seq;
        self.committed_end = committed_end;
        Ok(DecodeResult::Checkpoint(CommittedCheckpoint {
            records: pending.records,
            committed_end,
        }))
    }
}

/// Reads newline-delimited physical records lazily and feeds them to the
/// checkpoint decoder. A partial final line remains buffered so the same reader
/// can continue after a live writer appends its remainder.
pub(crate) struct CommittedCheckpointReader<R> {
    reader: R,
    path: PathBuf,
    decoder: CheckpointDecoder,
    partial_line: Vec<u8>,
    bytes_read: u64,
}

impl<R> CommittedCheckpointReader<R> {
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
            decoder: CheckpointDecoder::new(next_seq, source_offset),
            partial_line: Vec::new(),
            bytes_read: source_offset,
        }
    }

    pub(crate) fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    pub(crate) fn committed_end(&self) -> u64 {
        self.decoder.committed_end()
    }
}

impl<R: AsyncBufRead + Unpin> CommittedCheckpointReader<R> {
    pub(crate) async fn next_checkpoint(&mut self) -> Result<DecodeResult> {
        loop {
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
                return Ok(DecodeResult::NeedMore);
            }
            let line = mem::take(&mut self.partial_line);
            match self
                .decoder
                .push_complete_line(&self.path, line, self.bytes_read)?
            {
                DecodeResult::Checkpoint(checkpoint) => {
                    return Ok(DecodeResult::Checkpoint(checkpoint));
                }
                DecodeResult::NeedMore => {}
            }
        }
    }
}

#[cfg(test)]
mod tests;
