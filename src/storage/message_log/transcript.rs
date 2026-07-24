use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fmtview::view::{
    RecordLoadLimit, RecordTimeline, TimelineRead, TimelineReadNext, TimelineRefresh,
    TimelineSnapshot,
};
use fmtview_core::FileRecordTimeline;
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader as AsyncBufReader};

use super::CompleteLineReader;
use crate::storage::{RunDirStore, RunRecord, RunState, ensure_run_exists, validate_loaded_run};

/// A thin Fiasco adapter over fmtview's generic growing-file timeline.
///
/// fmtview owns physical newline discovery, tail-first paging, and follow
/// refresh. Fiasco owns only run lookup and mapping a terminal run's live
/// boundary to `End`.
pub struct TranscriptTimeline {
    metadata_path: PathBuf,
    inner: FileRecordTimeline,
    terminal: bool,
}

impl TranscriptTimeline {
    pub fn open(store: &RunDirStore, run_id: &str) -> Result<Self> {
        let paths = store.paths(run_id);
        let run = read_run(&paths.metadata)?;
        let inner = FileRecordTimeline::open(paths.messages, format!("fiasco run {run_id}"))?;
        Ok(Self {
            metadata_path: paths.metadata,
            inner,
            terminal: is_terminal(run.state),
        })
    }

    fn map_read_boundary(&self, read: TimelineRead) -> TimelineRead {
        if !self.terminal {
            return read;
        }
        match read {
            TimelineRead::Pending => TimelineRead::End,
            TimelineRead::Records { records, next } => TimelineRead::Records {
                records,
                next: if next == TimelineReadNext::Pending {
                    TimelineReadNext::End
                } else {
                    next
                },
            },
            TimelineRead::End => TimelineRead::End,
        }
    }
}

impl RecordTimeline for TranscriptTimeline {
    fn label(&self) -> &str {
        self.inner.label()
    }

    fn snapshot(&self) -> TimelineSnapshot {
        self.inner.snapshot()
    }

    fn probe_prefix(&mut self, limit: RecordLoadLimit) -> Result<TimelineRead> {
        let read = self.inner.probe_prefix(limit)?;
        Ok(self.map_read_boundary(read))
    }

    fn load_older(&mut self, limit: RecordLoadLimit) -> Result<TimelineRead> {
        let read = self.inner.load_older(limit)?;
        Ok(self.map_read_boundary(read))
    }

    fn load_newer(&mut self, limit: RecordLoadLimit) -> Result<TimelineRead> {
        let read = self.inner.load_newer(limit)?;
        Ok(self.map_read_boundary(read))
    }

    fn refresh(&mut self) -> Result<TimelineRefresh> {
        let run = read_run(&self.metadata_path)?;
        let refresh = self.inner.refresh()?;
        self.terminal = is_terminal(run.state);
        Ok(refresh)
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
    matches!(state, RunState::Completed | RunState::Closed)
}

#[cfg(test)]
mod tests;
