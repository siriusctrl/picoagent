use anyhow::{Result, bail};

use super::Replacement;

#[derive(Debug)]
pub(super) struct MatchedReplacement {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) replacement: String,
    pub(super) used_flexible_match: bool,
}

pub(super) fn match_replacement(
    content: &str,
    edit: Replacement,
    index: usize,
) -> Result<MatchedReplacement> {
    let old_text = normalize_line_endings(&edit.old_text);
    let replacement = normalize_line_endings(&edit.new_text);
    let exact = overlapping_matches(content, &old_text);
    match exact.as_slice() {
        [(start, end)] => {
            return Ok(MatchedReplacement {
                start: *start,
                end: *end,
                replacement,
                used_flexible_match: false,
            });
        }
        matches if matches.len() > 1 => {
            bail!(
                "edits[{index}].old_text matches {} regions; include more context",
                matches.len()
            );
        }
        _ => {}
    }

    let flexible = flexible_line_matches(content, &old_text);
    match flexible.as_slice() {
        [(start, end, actual_indent, expected_indent)] => Ok(MatchedReplacement {
            start: *start,
            end: *end,
            replacement: reindent(&replacement, expected_indent, actual_indent),
            used_flexible_match: true,
        }),
        [] => {
            bail!("edits[{index}].old_text was not found; re-read the file and provide exact text")
        }
        matches => bail!(
            "edits[{index}].old_text flexibly matches {} regions; include more context",
            matches.len()
        ),
    }
}

fn flexible_line_matches(content: &str, needle: &str) -> Vec<(usize, usize, String, String)> {
    let needle_has_newline = needle.ends_with('\n');
    let needle_core = needle.strip_suffix('\n').unwrap_or(needle);
    let needle_lines = needle_core.split('\n').collect::<Vec<_>>();
    if needle_lines.is_empty() {
        return Vec::new();
    }
    let ranges = line_ranges(content);
    let expected_indent = common_indent(&needle_lines);
    let mut matches = Vec::new();
    for window in ranges.windows(needle_lines.len()) {
        let actual_lines = window
            .iter()
            .map(|(start, end)| &content[*start..*end])
            .collect::<Vec<_>>();
        if actual_lines
            .iter()
            .zip(&needle_lines)
            .all(|(actual, expected)| actual.trim() == expected.trim())
        {
            let start = window[0].0;
            let mut end = window[window.len() - 1].1;
            if needle_has_newline && content.as_bytes().get(end) == Some(&b'\n') {
                end += 1;
            }
            matches.push((
                start,
                end,
                common_indent(&actual_lines),
                expected_indent.clone(),
            ));
        }
    }
    matches
}

fn line_ranges(content: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (index, byte) in content.bytes().enumerate() {
        if byte == b'\n' {
            ranges.push((start, index));
            start = index + 1;
        }
    }
    if start <= content.len() {
        ranges.push((start, content.len()));
    }
    ranges
}

fn common_indent(lines: &[&str]) -> String {
    lines
        .iter()
        .find(|line| !line.trim().is_empty())
        .map(|line| {
            line.chars()
                .take_while(|character| character.is_whitespace())
                .collect()
        })
        .unwrap_or_default()
}

fn reindent(replacement: &str, expected: &str, actual: &str) -> String {
    if expected == actual {
        return replacement.to_owned();
    }
    replacement
        .split_inclusive('\n')
        .map(|line| {
            let (body, ending) = line
                .strip_suffix('\n')
                .map_or((line, ""), |body| (body, "\n"));
            if body.trim().is_empty() {
                format!("{body}{ending}")
            } else {
                let body = body.strip_prefix(expected).unwrap_or(body.trim_start());
                format!("{actual}{body}{ending}")
            }
        })
        .collect()
}

pub(super) fn normalize_line_endings(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}

pub(super) fn detect_line_ending(value: &str) -> Result<&'static str> {
    let has_crlf = value.contains("\r\n");
    let remainder = value.replace("\r\n", "");
    let has_lf = remainder.contains('\n');
    let has_cr = remainder.contains('\r');
    if (has_crlf as u8 + has_lf as u8 + has_cr as u8) > 1 {
        bail!(
            "write targeted edits refuse mixed line endings; use a full-file content write to normalize intentionally"
        );
    }
    Ok(if has_crlf {
        "\r\n"
    } else if has_cr {
        "\r"
    } else {
        "\n"
    })
}

fn overlapping_matches(content: &str, needle: &str) -> Vec<(usize, usize)> {
    content
        .char_indices()
        .filter_map(|(start, _)| {
            content[start..]
                .starts_with(needle)
                .then_some((start, start + needle.len()))
        })
        .collect()
}
