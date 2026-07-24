use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use fmtview::view::{RecordId, RecordLoadLimit, TimelineRecord};

use super::TranscriptInstrumentation;
use crate::storage::message_log::decoder::{DecodedRecord, LineDecoder};

const REVERSE_SCAN_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy)]
pub(super) struct VisibleTail {
    pub(super) visible_end: u64,
    pub(super) next_seq: u64,
}

pub(super) fn find_visible_tail(
    file: &mut File,
    path: &Path,
    observed_end: u64,
    instrumentation: &mut TranscriptInstrumentation,
) -> Result<VisibleTail> {
    let visible_end =
        find_complete_physical_end(file, observed_end, instrumentation, "transcript")?;
    if visible_end == 0 {
        return Ok(VisibleTail {
            visible_end: 0,
            next_seq: 1,
        });
    }
    let record =
        read_record_ending_at(file, path, visible_end, None, instrumentation, "transcript")?;
    if record.source_offset == 0 {
        ensure!(
            record.trajectory.seq == 1,
            "transcript starts at m{} instead of m1",
            record.trajectory.seq
        );
    }
    Ok(VisibleTail {
        visible_end,
        next_seq: record
            .trajectory
            .seq
            .checked_add(1)
            .context("message sequence overflow after transcript tail")?,
    })
}

pub(super) struct ForwardBatch {
    pub(super) records: Vec<TimelineRecord>,
    pub(super) cursor: u64,
    pub(super) next_seq: u64,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn read_forward_batch(
    file: &mut File,
    path: &Path,
    epoch: u64,
    start: u64,
    next_seq: u64,
    end: u64,
    limit: RecordLoadLimit,
    instrumentation: &mut TranscriptInstrumentation,
    label: &str,
) -> Result<ForwardBatch> {
    let mut reader = BufReader::new(
        file.try_clone()
            .with_context(|| format!("clone transcript {label}"))?,
    );
    reader
        .seek(SeekFrom::Start(start))
        .with_context(|| format!("seek transcript {label}"))?;
    let mut decoder = LineDecoder::new(next_seq, start);
    let mut cursor = start;
    let mut records = Vec::new();
    let mut bytes = 0_usize;
    while cursor < end {
        let remaining = end - cursor;
        let mut line = Vec::new();
        let read = reader
            .by_ref()
            .take(remaining)
            .read_until(b'\n', &mut line)
            .with_context(|| format!("read transcript {label}"))?;
        instrumentation.read_operations = instrumentation.read_operations.saturating_add(1);
        instrumentation.bytes_read = instrumentation.bytes_read.saturating_add(read as u64);
        ensure!(
            read > 0,
            "transcript at byte {cursor} made no read progress"
        );
        ensure!(line.ends_with(b"\n"), "visible transcript line is torn");
        if !records.is_empty()
            && (records.len() >= limit.max_records
                || bytes.saturating_add(line.len()) > limit.max_bytes)
        {
            break;
        }
        let line_end = cursor
            .checked_add(read as u64)
            .context("transcript byte offset overflow")?;
        let record = decoder.push_complete_line(path, line, line_end)?;
        bytes = bytes.saturating_add(record.raw.len());
        records.push(timeline_record(epoch, record));
        cursor = line_end;
    }
    instrumentation.records_yielded = instrumentation
        .records_yielded
        .saturating_add(records.len() as u64);
    Ok(ForwardBatch {
        records,
        cursor,
        next_seq: decoder.next_seq(),
    })
}

pub(super) struct ReverseBatch {
    pub(super) records: Vec<TimelineRecord>,
    pub(super) cursor: u64,
    pub(super) next_seq: u64,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn read_reverse_batch(
    file: &mut File,
    path: &Path,
    epoch: u64,
    start: u64,
    next_seq: u64,
    limit: RecordLoadLimit,
    instrumentation: &mut TranscriptInstrumentation,
    label: &str,
) -> Result<ReverseBatch> {
    let mut cursor = start;
    let mut expected_next_seq = next_seq;
    let mut records = Vec::new();
    let mut bytes = 0_usize;
    while cursor > 0 {
        if !records.is_empty() && records.len() >= limit.max_records {
            break;
        }
        let record = read_record_ending_at(
            file,
            path,
            cursor,
            Some(expected_next_seq),
            instrumentation,
            label,
        )?;
        if !records.is_empty() && bytes.saturating_add(record.raw.len()) > limit.max_bytes {
            break;
        }
        bytes = bytes.saturating_add(record.raw.len());
        cursor = record.source_offset;
        expected_next_seq = record.trajectory.seq;
        records.push(timeline_record(epoch, record));
    }
    records.reverse();
    instrumentation.records_yielded = instrumentation
        .records_yielded
        .saturating_add(records.len() as u64);
    Ok(ReverseBatch {
        records,
        cursor,
        next_seq: expected_next_seq,
    })
}

pub(super) fn normalized_limit(limit: RecordLoadLimit) -> RecordLoadLimit {
    RecordLoadLimit::new(limit.max_records.max(1), limit.max_bytes.max(1))
}

fn read_record_ending_at(
    file: &mut File,
    path: &Path,
    end: u64,
    expected_next_seq: Option<u64>,
    instrumentation: &mut TranscriptInstrumentation,
    label: &str,
) -> Result<DecodedRecord> {
    ensure!(end > 0, "cannot decode a message ending at byte zero");
    let start = find_previous_line_start(file, end, instrumentation, label)?;
    let raw = read_range(file, start, end, instrumentation, label)?;
    ensure!(
        raw.ends_with(b"\n"),
        "message candidate at byte {end} lacks a terminating newline"
    );
    let stored = super::super::parse_stored_line(path, &raw)?;
    let seq = crate::trajectory::message_ref_seq(&stored.message_ref)
        .with_context(|| format!("stored message has invalid ref `{}`", stored.message_ref))?;
    if let Some(expected_next_seq) = expected_next_seq {
        ensure!(
            seq.checked_add(1) == Some(expected_next_seq),
            "message before byte {end} is m{seq}, expected m{}",
            expected_next_seq.saturating_sub(1)
        );
    }
    let mut decoder = LineDecoder::new(seq, start);
    decoder.push_complete_line(path, raw, end)
}

fn timeline_record(epoch: u64, record: DecodedRecord) -> TimelineRecord {
    TimelineRecord {
        id: RecordId {
            epoch,
            start_offset: record.source_offset,
            end_offset: record.end_offset,
        },
        raw: record.raw,
    }
}

fn find_complete_physical_end(
    file: &mut File,
    observed_end: u64,
    instrumentation: &mut TranscriptInstrumentation,
    label: &str,
) -> Result<u64> {
    let mut cursor = observed_end;
    let mut buffer = vec![0_u8; REVERSE_SCAN_CHUNK_BYTES];
    while cursor > 0 {
        let start = cursor.saturating_sub(buffer.len() as u64);
        let count = usize::try_from(cursor - start).unwrap_or(buffer.len());
        read_exact_at(file, start, &mut buffer[..count], instrumentation, label)?;
        if let Some(index) = buffer[..count].iter().rposition(|byte| *byte == b'\n') {
            return Ok(start + index as u64 + 1);
        }
        cursor = start;
    }
    Ok(0)
}

fn find_previous_line_start(
    file: &mut File,
    end: u64,
    instrumentation: &mut TranscriptInstrumentation,
    label: &str,
) -> Result<u64> {
    let mut cursor = end
        .checked_sub(1)
        .context("message end precedes its terminating newline")?;
    let mut buffer = vec![0_u8; REVERSE_SCAN_CHUNK_BYTES];
    while cursor > 0 {
        let start = cursor.saturating_sub(buffer.len() as u64);
        let count = usize::try_from(cursor - start).unwrap_or(buffer.len());
        read_exact_at(file, start, &mut buffer[..count], instrumentation, label)?;
        if let Some(index) = buffer[..count].iter().rposition(|byte| *byte == b'\n') {
            return Ok(start + index as u64 + 1);
        }
        cursor = start;
    }
    Ok(0)
}

pub(super) fn read_range(
    file: &mut File,
    start: u64,
    end: u64,
    instrumentation: &mut TranscriptInstrumentation,
    label: &str,
) -> Result<Vec<u8>> {
    let len = usize::try_from(end.saturating_sub(start))
        .context("transcript range does not fit in memory")?;
    let mut bytes = vec![0_u8; len];
    read_exact_at(file, start, &mut bytes, instrumentation, label)?;
    Ok(bytes)
}

fn read_exact_at(
    file: &mut File,
    offset: u64,
    buffer: &mut [u8],
    instrumentation: &mut TranscriptInstrumentation,
    label: &str,
) -> Result<()> {
    file.seek(SeekFrom::Start(offset))
        .with_context(|| format!("seek transcript {label}"))?;
    file.read_exact(buffer)
        .with_context(|| format!("read transcript {label}"))?;
    instrumentation.read_operations = instrumentation.read_operations.saturating_add(1);
    instrumentation.bytes_read = instrumentation
        .bytes_read
        .saturating_add(buffer.len() as u64);
    Ok(())
}
