use std::{
    fs::{File, Metadata},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use fmtview::view::{RecordTimeline, TimelineRefresh, TimelineResetReason};

use super::{TranscriptInstrumentation, TranscriptTimeline, read_run, scan::read_range};
use crate::storage::message_log::decoder::CheckpointDecoder;

const SAMPLE_BYTES: usize = 64;

impl TranscriptTimeline {
    pub(super) fn refresh_timeline(&mut self) -> Result<TimelineRefresh> {
        let run = read_run(&self.metadata_path)?;
        let mut file = File::open(&self.messages_path)
            .with_context(|| format!("reopen transcript {}", self.messages_path.display()))?;
        let metadata = file
            .metadata()
            .with_context(|| format!("stat transcript {}", self.messages_path.display()))?;
        let identity = FileIdentity::from_metadata(&metadata);
        if identity != self.identity {
            return self.reset(TimelineResetReason::IdentityChanged);
        }
        if metadata.len() < self.committed_end {
            return self.reset(TimelineResetReason::Truncated);
        }
        if !self.prefix_sample.matches(
            &mut file,
            self.committed_end,
            &mut self.instrumentation,
            &self.label,
        )? {
            return self.reset(TimelineResetReason::Replaced);
        }

        let old_committed_end = self.committed_end;
        if !self.suffix_tracker.matches(
            &mut file,
            metadata.len(),
            &mut self.instrumentation,
            &self.label,
        )? {
            self.suffix_tracker = SuffixTracker::new(self.committed_end, self.committed_next_seq);
        }
        self.suffix_tracker.scan_to(
            &mut file,
            &self.messages_path,
            metadata.len(),
            &mut self.instrumentation,
            &self.label,
        )?;
        self.file = file;
        self.state = run.state;
        self.observed_end = metadata.len();
        self.committed_end = self.suffix_tracker.committed_end();
        self.committed_next_seq = self.suffix_tracker.next_seq();
        if self.committed_end != old_committed_end {
            self.prefix_sample = PrefixSample::read(
                &mut self.file,
                self.committed_end,
                &mut self.instrumentation,
                &self.label,
            )?;
            return Ok(TimelineRefresh::Appended(self.snapshot()));
        }
        if self.is_terminal() {
            Ok(TimelineRefresh::End(self.snapshot()))
        } else if self.observed_end > self.committed_end {
            Ok(TimelineRefresh::Pending(self.snapshot()))
        } else {
            Ok(TimelineRefresh::NoChange(self.snapshot()))
        }
    }

    fn reset(&mut self, reason: TimelineResetReason) -> Result<TimelineRefresh> {
        let epoch = self
            .epoch
            .checked_add(1)
            .context("transcript identity epoch overflow")?;
        let replacement = Self::open_paths(
            self.label.clone(),
            self.metadata_path.clone(),
            self.messages_path.clone(),
            epoch,
        )?;
        *self = replacement;
        Ok(TimelineRefresh::Reset {
            reason,
            snapshot: self.snapshot(),
        })
    }
}

pub(super) struct SuffixTracker {
    decoder: CheckpointDecoder,
    scan_cursor: u64,
    partial_line: Vec<u8>,
    sample: RangeSample,
}

impl SuffixTracker {
    pub(super) fn new(committed_end: u64, next_seq: u64) -> Self {
        Self {
            decoder: CheckpointDecoder::new(next_seq, committed_end),
            scan_cursor: committed_end,
            partial_line: Vec::new(),
            sample: RangeSample::empty(committed_end),
        }
    }

    pub(super) fn committed_end(&self) -> u64 {
        self.decoder.committed_end()
    }

    pub(super) fn next_seq(&self) -> u64 {
        self.decoder.next_seq()
    }

    fn matches(
        &self,
        file: &mut File,
        observed_end: u64,
        instrumentation: &mut TranscriptInstrumentation,
        label: &str,
    ) -> Result<bool> {
        if observed_end < self.scan_cursor {
            return Ok(false);
        }
        self.sample.matches(file, instrumentation, label)
    }

    pub(super) fn scan_to(
        &mut self,
        file: &mut File,
        path: &Path,
        observed_end: u64,
        instrumentation: &mut TranscriptInstrumentation,
        label: &str,
    ) -> Result<()> {
        ensure!(
            observed_end >= self.scan_cursor,
            "transcript shrank below the pending suffix cursor"
        );
        if observed_end > self.scan_cursor {
            let mut reader = BufReader::new(
                file.try_clone()
                    .with_context(|| format!("clone transcript {label}"))?,
            );
            reader
                .seek(SeekFrom::Start(self.scan_cursor))
                .with_context(|| format!("seek transcript {label}"))?;
            while self.scan_cursor < observed_end {
                let remaining = observed_end - self.scan_cursor;
                let read = reader
                    .by_ref()
                    .take(remaining)
                    .read_until(b'\n', &mut self.partial_line)
                    .with_context(|| format!("refresh transcript {label}"))?;
                instrumentation.read_operations = instrumentation.read_operations.saturating_add(1);
                instrumentation.bytes_read = instrumentation.bytes_read.saturating_add(read as u64);
                if read == 0 {
                    break;
                }
                self.scan_cursor = self
                    .scan_cursor
                    .checked_add(read as u64)
                    .context("transcript byte offset overflow")?;
                if !self.partial_line.ends_with(b"\n") {
                    break;
                }
                let line = std::mem::take(&mut self.partial_line);
                let _ = self
                    .decoder
                    .push_complete_line(path, line, self.scan_cursor)?;
            }
        }
        self.sample = RangeSample::read(
            file,
            self.decoder.committed_end(),
            self.scan_cursor,
            instrumentation,
            label,
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PrefixSample {
    range: RangeSample,
}

impl PrefixSample {
    pub(super) fn read(
        file: &mut File,
        committed_end: u64,
        instrumentation: &mut TranscriptInstrumentation,
        label: &str,
    ) -> Result<Self> {
        Ok(Self {
            range: RangeSample::read(file, 0, committed_end, instrumentation, label)?,
        })
    }

    fn matches(
        &self,
        file: &mut File,
        committed_end: u64,
        instrumentation: &mut TranscriptInstrumentation,
        label: &str,
    ) -> Result<bool> {
        ensure!(
            self.range.end == committed_end,
            "committed prefix sample boundary changed without refresh"
        );
        self.range.matches(file, instrumentation, label)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RangeSample {
    end: u64,
    head: Sample,
    middle: Sample,
    tail: Sample,
}

impl RangeSample {
    fn empty(offset: u64) -> Self {
        let sample = Sample {
            offset,
            bytes: Vec::new(),
        };
        Self {
            end: offset,
            head: sample.clone(),
            middle: sample.clone(),
            tail: sample,
        }
    }

    fn read(
        file: &mut File,
        start: u64,
        end: u64,
        instrumentation: &mut TranscriptInstrumentation,
        label: &str,
    ) -> Result<Self> {
        ensure!(end >= start, "sample range ends before it starts");
        if start == end {
            return Ok(Self::empty(start));
        }
        let len = end - start;
        let sample_len = len.min(SAMPLE_BYTES as u64);
        let middle_offset = start + len.saturating_sub(sample_len) / 2;
        let tail_offset = end - sample_len;
        Ok(Self {
            end,
            head: Sample::read(file, start, sample_len, instrumentation, label)?,
            middle: Sample::read(file, middle_offset, sample_len, instrumentation, label)?,
            tail: Sample::read(file, tail_offset, sample_len, instrumentation, label)?,
        })
    }

    fn matches(
        &self,
        file: &mut File,
        instrumentation: &mut TranscriptInstrumentation,
        label: &str,
    ) -> Result<bool> {
        Ok(self.head.matches(file, instrumentation, label)?
            && self.middle.matches(file, instrumentation, label)?
            && self.tail.matches(file, instrumentation, label)?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Sample {
    offset: u64,
    bytes: Vec<u8>,
}

impl Sample {
    fn read(
        file: &mut File,
        offset: u64,
        len: u64,
        instrumentation: &mut TranscriptInstrumentation,
        label: &str,
    ) -> Result<Self> {
        Ok(Self {
            offset,
            bytes: read_range(file, offset, offset + len, instrumentation, label)?,
        })
    }

    fn matches(
        &self,
        file: &mut File,
        instrumentation: &mut TranscriptInstrumentation,
        label: &str,
    ) -> Result<bool> {
        Ok(self.bytes
            == read_range(
                file,
                self.offset,
                self.offset + self.bytes.len() as u64,
                instrumentation,
                label,
            )?)
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(unix)]
impl FileIdentity {
    pub(super) fn from_metadata(metadata: &Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

#[cfg(not(unix))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FileIdentity {
    created: Option<std::time::SystemTime>,
}

#[cfg(not(unix))]
impl FileIdentity {
    pub(super) fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            created: metadata.created().ok(),
        }
    }
}
