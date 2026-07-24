use std::{
    fs::File,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use fmtview::view::{
    RecordLoadLimit, RecordTimeline, TimelineRead, TimelineReadNext, TimelineRefresh,
    TimelineSnapshot,
};
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader as AsyncBufReader};

use super::CompleteLineReader;
use crate::storage::{RunDirStore, RunRecord, RunState, ensure_run_exists, validate_loaded_run};

mod refresh;
mod scan;

use refresh::{FileIdentity, PrefixSample, SuffixTracker};
use scan::{find_visible_tail, normalized_limit, read_forward_batch, read_reverse_batch};

/// A newline-aware, bidirectional view over one Fiasco transcript.
///
/// Every complete physical line is visible. A partial final line remains
/// pending until it receives its terminating newline.
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
        let tail = find_visible_tail(
            &mut file,
            &messages_path,
            observed_end,
            &mut instrumentation,
        )?;
        let prefix_sample =
            PrefixSample::read(&mut file, tail.visible_end, &mut instrumentation, &label)?;
        let mut suffix_tracker = SuffixTracker::new(tail.visible_end, tail.next_seq);
        suffix_tracker.scan_to(
            &mut file,
            &messages_path,
            observed_end,
            &mut instrumentation,
            &label,
        )?;
        ensure!(
            suffix_tracker.visible_end() == tail.visible_end,
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
            committed_end: tail.visible_end,
            committed_next_seq: tail.next_seq,
            observed_end,
            older_cursor: tail.visible_end,
            older_next_seq: tail.next_seq,
            newer_cursor: tail.visible_end,
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
        let batch = read_reverse_batch(
            &mut self.file,
            &self.messages_path,
            self.epoch,
            self.older_cursor,
            self.older_next_seq,
            limit,
            &mut self.instrumentation,
            &self.label,
        )?;
        self.older_cursor = batch.cursor;
        self.older_next_seq = batch.next_seq;
        Ok(TimelineRead::Records {
            records: batch.records,
            next: if self.older_cursor == 0 {
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
    /// Copy only complete newline-terminated messages to an NDJSON sink.
    ///
    /// Record bytes, including their LF delimiters, are not reserialized.
    pub async fn write_complete_ndjson<W>(&self, run_id: &str, output: &mut W) -> Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        self.load_run(run_id).await?;
        let paths = self.paths(run_id);
        ensure_run_exists(&paths).await?;
        let file = tokio::fs::File::open(&paths.messages)
            .await
            .with_context(|| format!("open transcript {}", paths.messages.display()))?;
        let mut reader = CompleteLineReader::new(AsyncBufReader::new(file), paths.messages.clone());
        while let Some(record) = reader.next_record().await? {
            output
                .write_all(&record.raw)
                .await
                .context("write visible transcript NDJSON")?;
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
