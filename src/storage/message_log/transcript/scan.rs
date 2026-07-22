use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use fmtview::view::{RecordId, RecordLoadLimit, TimelineRecord};

use crate::trajectory::message_ref_seq;

use super::TranscriptInstrumentation;
use crate::storage::message_log::{
    DecodeResult,
    decoder::{CheckpointDecoder, CommittedCheckpoint},
    parse_stored_line,
};

const REVERSE_SCAN_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy)]
pub(super) struct CommittedTail {
    pub(super) committed_end: u64,
    pub(super) next_seq: u64,
}

pub(super) fn find_committed_tail(
    file: &mut File,
    path: &Path,
    observed_end: u64,
    instrumentation: &mut TranscriptInstrumentation,
) -> Result<CommittedTail> {
    let mut cursor = find_complete_physical_end(file, observed_end, instrumentation, "transcript")?;
    let mut incomplete_first_seq = None;
    if cursor == 0 {
        return Ok(CommittedTail {
            committed_end: 0,
            next_seq: 1,
        });
    }
    loop {
        let group = decode_group_ending_at(file, path, cursor, instrumentation, "transcript")?;
        match group.decode {
            DecodeResult::Checkpoint(checkpoint) => {
                let first_seq = checkpoint_first_seq(&checkpoint)?;
                let next_seq = checkpoint_next_seq(&checkpoint)?;
                if let Some(incomplete_first_seq) = incomplete_first_seq {
                    ensure!(
                        next_seq == incomplete_first_seq,
                        "incomplete tail starts at m{incomplete_first_seq} after m{}",
                        next_seq.saturating_sub(1)
                    );
                }
                if group.start == 0 {
                    ensure!(
                        first_seq == 1,
                        "transcript starts at m{first_seq} instead of m1"
                    );
                }
                return Ok(CommittedTail {
                    committed_end: cursor,
                    next_seq,
                });
            }
            DecodeResult::NeedMore => {
                ensure!(
                    incomplete_first_seq.is_none(),
                    "more than one incomplete checkpoint appears at the transcript tail"
                );
                incomplete_first_seq = Some(group.first_seq);
                cursor = group.start;
                if cursor == 0 {
                    ensure!(
                        group.first_seq == 1,
                        "transcript starts at m{} instead of m1",
                        group.first_seq
                    );
                    return Ok(CommittedTail {
                        committed_end: 0,
                        next_seq: 1,
                    });
                }
            }
        }
    }
}

pub(super) struct DecodedGroup {
    pub(super) start: u64,
    pub(super) first_seq: u64,
    pub(super) decode: DecodeResult,
}

pub(super) fn decode_group_ending_at(
    file: &mut File,
    path: &Path,
    end: u64,
    instrumentation: &mut TranscriptInstrumentation,
    label: &str,
) -> Result<DecodedGroup> {
    decode_group_with_limit(file, path, end, None, instrumentation, label)?
        .context("unlimited checkpoint decode was skipped")
}

pub(super) fn decode_group_with_limit(
    file: &mut File,
    path: &Path,
    end: u64,
    remaining: Option<RecordLoadLimit>,
    instrumentation: &mut TranscriptInstrumentation,
    label: &str,
) -> Result<Option<DecodedGroup>> {
    ensure!(end > 0, "cannot decode a checkpoint ending at byte zero");
    let last_start = find_start_of_lines(file, end, 1, instrumentation, label)?;
    let last_line = read_range(file, last_start, end, instrumentation, label)?;
    ensure!(
        last_line.ends_with(b"\n"),
        "checkpoint candidate at byte {end} lacks a terminating newline"
    );
    let stored = parse_stored_line(path, &last_line)?;
    let (group_line_count, declared_records) = match stored.local.checkpoint.as_ref() {
        Some(checkpoint) => (
            checkpoint
                .index
                .checked_add(1)
                .context("checkpoint index overflow")?,
            checkpoint.count,
        ),
        None => (1, 1),
    };
    if remaining.is_some_and(|limit| {
        declared_records > u64::try_from(limit.max_records).unwrap_or(u64::MAX)
    }) {
        return Ok(None);
    }
    let start = find_start_of_lines(file, end, group_line_count, instrumentation, label)?;
    if remaining
        .is_some_and(|limit| end - start > u64::try_from(limit.max_bytes).unwrap_or(u64::MAX))
    {
        return Ok(None);
    }
    let first_line_end = find_next_newline(file, start, end, instrumentation, label)?;
    let first_line = read_range(file, start, first_line_end, instrumentation, label)?;
    let first = parse_stored_line(path, &first_line)?;
    let first_seq = message_ref_seq(&first.message_ref)
        .with_context(|| format!("stored message has invalid ref `{}`", first.message_ref))?;
    let mut decoder = CheckpointDecoder::new(first_seq, start);
    let mut reader = BufReader::new(
        file.try_clone()
            .with_context(|| format!("clone transcript {label}"))?,
    );
    reader
        .seek(SeekFrom::Start(start))
        .with_context(|| format!("seek transcript {label}"))?;
    let mut cursor = start;
    let mut final_decode = DecodeResult::NeedMore;
    while cursor < end {
        let remaining = end - cursor;
        let mut line = Vec::new();
        let read = reader
            .by_ref()
            .take(remaining)
            .read_until(b'\n', &mut line)
            .with_context(|| format!("read checkpoint from {label}"))?;
        instrumentation.read_operations = instrumentation.read_operations.saturating_add(1);
        instrumentation.bytes_read = instrumentation.bytes_read.saturating_add(read as u64);
        ensure!(
            read > 0,
            "checkpoint at byte {cursor} made no read progress"
        );
        cursor = cursor
            .checked_add(read as u64)
            .context("checkpoint byte offset overflow")?;
        ensure!(
            line.ends_with(b"\n"),
            "checkpoint line ending at byte {cursor} is torn"
        );
        final_decode = decoder.push_complete_line(path, line, cursor)?;
        if matches!(final_decode, DecodeResult::Checkpoint(_)) {
            ensure!(
                cursor == end,
                "checkpoint ending at byte {cursor} overlaps a later physical record"
            );
        }
    }
    Ok(Some(DecodedGroup {
        start,
        first_seq,
        decode: final_decode,
    }))
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
    let mut decoder = CheckpointDecoder::new(next_seq, start);
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
        cursor = cursor
            .checked_add(read as u64)
            .context("transcript byte offset overflow")?;
        ensure!(line.ends_with(b"\n"), "committed transcript line is torn");
        if let DecodeResult::Checkpoint(checkpoint) =
            decoder.push_complete_line(path, line, cursor)?
        {
            bytes = bytes.saturating_add(checkpoint_raw_bytes(&checkpoint));
            records.extend(timeline_records(epoch, checkpoint));
            if records.len() >= limit.max_records || bytes >= limit.max_bytes {
                break;
            }
        }
    }
    ensure!(
        decoder.committed_end() == cursor,
        "committed transcript boundary splits a checkpoint at byte {cursor}"
    );
    instrumentation.records_yielded = instrumentation
        .records_yielded
        .saturating_add(records.len() as u64);
    Ok(ForwardBatch {
        records,
        cursor,
        next_seq: decoder.next_seq(),
    })
}

pub(super) fn timeline_records(epoch: u64, checkpoint: CommittedCheckpoint) -> Vec<TimelineRecord> {
    checkpoint
        .records
        .into_iter()
        .map(|record| {
            let end_offset = record.source_offset.saturating_add(record.raw.len() as u64);
            TimelineRecord {
                id: RecordId {
                    epoch,
                    start_offset: record.source_offset,
                    end_offset,
                },
                raw: record.raw,
            }
        })
        .collect()
}

pub(super) fn checkpoint_first_seq(checkpoint: &CommittedCheckpoint) -> Result<u64> {
    let first = checkpoint
        .records
        .first()
        .context("committed checkpoint contains no records")?;
    Ok(first.trajectory.seq)
}

pub(super) fn checkpoint_next_seq(checkpoint: &CommittedCheckpoint) -> Result<u64> {
    let last = checkpoint
        .records
        .last()
        .context("committed checkpoint contains no records")?;
    last.trajectory
        .seq
        .checked_add(1)
        .context("message sequence overflow after committed checkpoint")
}

pub(super) fn checkpoint_raw_bytes(checkpoint: &CommittedCheckpoint) -> usize {
    checkpoint
        .records
        .iter()
        .fold(0_usize, |sum, record| sum.saturating_add(record.raw.len()))
}

pub(super) fn normalized_limit(limit: RecordLoadLimit) -> RecordLoadLimit {
    RecordLoadLimit::new(limit.max_records.max(1), limit.max_bytes.max(1))
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

fn find_start_of_lines(
    file: &mut File,
    end: u64,
    line_count: u64,
    instrumentation: &mut TranscriptInstrumentation,
    label: &str,
) -> Result<u64> {
    ensure!(line_count > 0, "checkpoint line count must be positive");
    let mut remaining = line_count;
    let mut cursor = end
        .checked_sub(1)
        .context("checkpoint end precedes its terminating newline")?;
    let mut buffer = vec![0_u8; REVERSE_SCAN_CHUNK_BYTES];
    while cursor > 0 {
        let start = cursor.saturating_sub(buffer.len() as u64);
        let count = usize::try_from(cursor - start).unwrap_or(buffer.len());
        read_exact_at(file, start, &mut buffer[..count], instrumentation, label)?;
        for index in (0..count).rev() {
            if buffer[index] == b'\n' {
                remaining -= 1;
                if remaining == 0 {
                    return Ok(start + index as u64 + 1);
                }
            }
        }
        cursor = start;
    }
    Ok(0)
}

fn find_next_newline(
    file: &mut File,
    start: u64,
    end: u64,
    instrumentation: &mut TranscriptInstrumentation,
    label: &str,
) -> Result<u64> {
    let mut cursor = start;
    let mut buffer = vec![0_u8; REVERSE_SCAN_CHUNK_BYTES];
    while cursor < end {
        let count =
            usize::try_from((end - cursor).min(buffer.len() as u64)).unwrap_or(buffer.len());
        read_exact_at(file, cursor, &mut buffer[..count], instrumentation, label)?;
        if let Some(index) = buffer[..count].iter().position(|byte| *byte == b'\n') {
            return Ok(cursor + index as u64 + 1);
        }
        cursor = cursor
            .checked_add(count as u64)
            .context("transcript byte offset overflow")?;
    }
    bail!("checkpoint beginning at byte {start} has no complete first line")
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
