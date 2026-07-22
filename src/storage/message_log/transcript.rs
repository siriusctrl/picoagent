use std::{
    fs::File,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use fmtview::view::{
    RecordLoadLimit, RecordTimeline, TimelineRead, TimelineReadNext, TimelineRefresh,
    TimelineSnapshot,
};
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader as AsyncBufReader};

use super::{CommittedCheckpointReader, DecodeResult};
use crate::storage::{RunDirStore, RunRecord, RunState, ensure_run_exists, validate_loaded_run};

mod refresh;
mod scan;

use refresh::{FileIdentity, PrefixSample, SuffixTracker};
use scan::{
    checkpoint_first_seq, checkpoint_next_seq, checkpoint_raw_bytes, decode_group_with_limit,
    find_committed_tail, normalized_limit, read_forward_batch, timeline_records,
};

/// A checkpoint-safe, bidirectional view over one Fiasco transcript.
///
/// Construction reads only the physical EOF suffix and the last logical
/// checkpoint. Both directional cursors start at that committed tail.
pub struct TranscriptTimeline {
    label: String,
    metadata_path: PathBuf,
    messages_path: PathBuf,
    file: File,
    identity: FileIdentity,
    epoch: u64,
    state: RunState,
    committed_end: u64,
    committed_next_seq: u64,
    observed_end: u64,
    older_cursor: u64,
    older_next_seq: u64,
    newer_cursor: u64,
    newer_next_seq: u64,
    prefix_sample: PrefixSample,
    suffix_tracker: SuffixTracker,
    instrumentation: TranscriptInstrumentation,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TranscriptInstrumentation {
    bytes_read: u64,
    read_operations: u64,
    records_yielded: u64,
}

impl TranscriptTimeline {
    pub fn open(store: &RunDirStore, run_id: &str) -> Result<Self> {
        let paths = store.paths(run_id);
        Self::open_paths(
            format!("fiasco run {run_id}"),
            paths.metadata,
            paths.messages,
            1,
        )
    }

    fn open_paths(
        label: String,
        metadata_path: PathBuf,
        messages_path: PathBuf,
        epoch: u64,
    ) -> Result<Self> {
        let run = read_run(&metadata_path)?;
        let mut file = File::open(&messages_path)
            .with_context(|| format!("open transcript {}", messages_path.display()))?;
        let metadata = file
            .metadata()
            .with_context(|| format!("stat transcript {}", messages_path.display()))?;
        ensure!(
            metadata.is_file(),
            "transcript is not a file: {}",
            messages_path.display()
        );
        let observed_end = metadata.len();
        let mut instrumentation = TranscriptInstrumentation::default();
        let tail = find_committed_tail(
            &mut file,
            &messages_path,
            observed_end,
            &mut instrumentation,
        )?;
        let prefix_sample =
            PrefixSample::read(&mut file, tail.committed_end, &mut instrumentation, &label)?;
        let mut suffix_tracker = SuffixTracker::new(tail.committed_end, tail.next_seq);
        suffix_tracker.scan_to(
            &mut file,
            &messages_path,
            observed_end,
            &mut instrumentation,
            &label,
        )?;
        ensure!(
            suffix_tracker.committed_end() == tail.committed_end,
            "tail discovery and forward suffix validation disagree"
        );

        Ok(Self {
            label,
            metadata_path,
            messages_path,
            file,
            identity: FileIdentity::from_metadata(&metadata),
            epoch,
            state: run.state,
            committed_end: tail.committed_end,
            committed_next_seq: tail.next_seq,
            observed_end,
            older_cursor: tail.committed_end,
            older_next_seq: tail.next_seq,
            newer_cursor: tail.committed_end,
            newer_next_seq: tail.next_seq,
            prefix_sample,
            suffix_tracker,
            instrumentation,
        })
    }

    fn is_terminal(&self) -> bool {
        is_terminal(self.state)
    }

    fn boundary_next(&self) -> TimelineReadNext {
        if self.is_terminal() {
            TimelineReadNext::End
        } else {
            TimelineReadNext::Pending
        }
    }

    fn empty_boundary(&self) -> TimelineRead {
        if self.is_terminal() {
            TimelineRead::End
        } else {
            TimelineRead::Pending
        }
    }

    #[cfg(test)]
    fn instrumentation(&self) -> TranscriptInstrumentation {
        self.instrumentation
    }
}

impl RecordTimeline for TranscriptTimeline {
    fn label(&self) -> &str {
        &self.label
    }

    fn snapshot(&self) -> TimelineSnapshot {
        TimelineSnapshot {
            epoch: self.epoch,
            committed_end: self.committed_end,
            observed_end: self.observed_end,
            pending_bytes: self.observed_end.saturating_sub(self.committed_end),
        }
    }

    fn probe_prefix(&mut self, limit: RecordLoadLimit) -> Result<TimelineRead> {
        if self.committed_end == 0 {
            return Ok(self.empty_boundary());
        }
        let batch = read_forward_batch(
            &mut self.file,
            &self.messages_path,
            self.epoch,
            0,
            1,
            self.committed_end,
            normalized_limit(limit),
            &mut self.instrumentation,
            &self.label,
        )?;
        Ok(TimelineRead::Records {
            records: batch.records,
            next: if batch.cursor < self.committed_end {
                TimelineReadNext::More
            } else {
                self.boundary_next()
            },
        })
    }

    fn load_older(&mut self, limit: RecordLoadLimit) -> Result<TimelineRead> {
        if self.older_cursor == 0 {
            return Ok(TimelineRead::End);
        }
        let limit = normalized_limit(limit);
        let mut cursor = self.older_cursor;
        let mut expected_next_seq = self.older_next_seq;
        let mut groups = Vec::new();
        let mut record_count = 0_usize;
        let mut byte_count = 0_usize;

        while cursor > 0 {
            let remaining = (!groups.is_empty()).then(|| {
                RecordLoadLimit::new(
                    limit.max_records.saturating_sub(record_count),
                    limit.max_bytes.saturating_sub(byte_count),
                )
            });
            let Some(group) = decode_group_with_limit(
                &mut self.file,
                &self.messages_path,
                cursor,
                remaining,
                &mut self.instrumentation,
                &self.label,
            )?
            else {
                break;
            };
            let checkpoint = match group.decode {
                DecodeResult::Checkpoint(checkpoint) => checkpoint,
                DecodeResult::NeedMore => {
                    bail!("incomplete checkpoint ends inside committed transcript at byte {cursor}")
                }
            };
            let first_seq = checkpoint_first_seq(&checkpoint)?;
            let next_seq = checkpoint_next_seq(&checkpoint)?;
            ensure!(
                next_seq == expected_next_seq,
                "checkpoint before byte {cursor} ends at m{}, expected m{}",
                next_seq.saturating_sub(1),
                expected_next_seq.saturating_sub(1)
            );
            if group.start == 0 {
                ensure!(
                    first_seq == 1,
                    "transcript starts at m{first_seq} instead of m1"
                );
            }
            let group_records = checkpoint.records.len();
            let group_bytes = checkpoint_raw_bytes(&checkpoint);
            record_count = record_count.saturating_add(group_records);
            byte_count = byte_count.saturating_add(group_bytes);
            cursor = group.start;
            expected_next_seq = first_seq;
            groups.push(checkpoint);
            if record_count >= limit.max_records || byte_count >= limit.max_bytes {
                break;
            }
        }

        let mut records = Vec::with_capacity(record_count);
        for checkpoint in groups.into_iter().rev() {
            records.extend(timeline_records(self.epoch, checkpoint));
        }
        self.older_cursor = cursor;
        self.older_next_seq = expected_next_seq;
        self.instrumentation.records_yielded = self
            .instrumentation
            .records_yielded
            .saturating_add(records.len() as u64);
        Ok(TimelineRead::Records {
            records,
            next: if cursor == 0 {
                TimelineReadNext::End
            } else {
                TimelineReadNext::More
            },
        })
    }

    fn load_newer(&mut self, limit: RecordLoadLimit) -> Result<TimelineRead> {
        if self.newer_cursor >= self.committed_end {
            return Ok(self.empty_boundary());
        }
        let batch = read_forward_batch(
            &mut self.file,
            &self.messages_path,
            self.epoch,
            self.newer_cursor,
            self.newer_next_seq,
            self.committed_end,
            normalized_limit(limit),
            &mut self.instrumentation,
            &self.label,
        )?;
        self.newer_cursor = batch.cursor;
        self.newer_next_seq = batch.next_seq;
        Ok(TimelineRead::Records {
            records: batch.records,
            next: if self.newer_cursor < self.committed_end {
                TimelineReadNext::More
            } else {
                self.boundary_next()
            },
        })
    }

    fn refresh(&mut self) -> Result<TimelineRefresh> {
        self.refresh_timeline()
    }
}

impl RunDirStore {
    /// Copy only complete committed checkpoints to an NDJSON sink.
    ///
    /// Record bytes, including their LF delimiters, are not reserialized.
    pub async fn write_committed_ndjson<W>(&self, run_id: &str, output: &mut W) -> Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        self.load_run(run_id).await?;
        let paths = self.paths(run_id);
        ensure_run_exists(&paths).await?;
        let file = tokio::fs::File::open(&paths.messages)
            .await
            .with_context(|| format!("open transcript {}", paths.messages.display()))?;
        let mut reader =
            CommittedCheckpointReader::new(AsyncBufReader::new(file), paths.messages.clone());
        while let DecodeResult::Checkpoint(checkpoint) = reader.next_checkpoint().await? {
            for record in checkpoint.records {
                output
                    .write_all(&record.raw)
                    .await
                    .context("write committed transcript NDJSON")?;
            }
        }
        output.flush().await.context("flush transcript NDJSON")
    }
}

fn read_run(path: &Path) -> Result<RunRecord> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let run: RunRecord =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    validate_loaded_run(&run)?;
    Ok(run)
}

fn is_terminal(state: RunState) -> bool {
    matches!(
        state,
        RunState::Completed | RunState::Failed | RunState::Cancelled | RunState::Closed
    )
}

#[cfg(test)]
mod tests;
