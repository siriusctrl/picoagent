# Artifact Contract

Tool output is part of the execution record, even when it is too large for the
model context. Picoagent therefore separates the complete artifact from the
bounded model-facing envelope.

## Location

```text
.pico/runs/<run-id>/artifacts/
  <tool-call-id>-<sha256-prefix>.<extension>
  <tool-call-id>-<sha256-prefix>.artifact.json
```

Persisted references are relative to the workspace. Run directories can be
uploaded as a unit by a cloud worker.

## Identity And Metadata

Artifact ids are immutable within a run. Metadata includes:

- format version
- artifact id
- run id and tool call id
- relative content path
- media type
- byte length
- SHA-256 digest

Changing the content while retaining the same id and hash metadata is invalid.

## Model-Facing Envelope

Small UTF-8 results remain inline. When a result exceeds `inline_bytes` or the
remaining run preview budget, the complete bytes are written to the artifact
store and the model receives a compact `[Tool output]` envelope with:

- whether any source bytes were actually truncated from the preview
- preview strategy (`full`, `head_tail`, `head_only`, `tail_only`, or `none`)
- total, shown, shown-head, shown-tail, and omitted byte counts
- the omitted region (`head`, `middle`, `tail`, `all`, or `none`)
- the reason the artifact or reduced preview was required
- media type
- SHA-256 digest
- stable relative path
- instructions to use bounded `read` or `bash` with `rg`

An artifact may have `truncated: false` when its configured preview contains
all source bytes; artifact-backed and truncated are separate properties.

The normal textual strategy is `head_tail`: it retains both a beginning and an
ending preview because compiler and command failures commonly appear at the end
of stdout or stderr. If the remaining run-level preview budget cannot hold that
envelope, picoagent switches to `head_only` and reports
`run_preview_budget_limited`. Once the budget is empty it uses `none`, reports
`run_preview_budget_exhausted`, and directs the model to the complete artifact.
The model never has to infer whether the omitted bytes came from the middle,
tail, or entire result.

Binary and non-UTF-8 data is never decoded into a lossy inline string. Its
envelope uses `preview: none`, `omitted_region: all`, and
`reason: binary_or_non_utf8`, with metadata and a path only.

The run-level `max_inline_bytes_per_run` budget also forces later small results
to artifacts. Preview bytes consume that budget; after it is exhausted, later
results carry reference metadata without a content preview. Artifact-reference
overhead and ordinary conversation text still count toward the provider context,
so deployments should also set provider token limits appropriate to the model.

### History-query boundaries

History tools use this same envelope when their returned JSONL is too large.
For `history_search`, that artifact contains the complete result after applying
`history_search_max_matches`; it does not contain older matches omitted by the
query cap. A `truncation_reason: max_matches` search summary therefore means
“refine the regex,” while an artifact preview means “use bounded `read` or
`bash`/`rg` on the referenced complete bounded result.” Neither history tool
uses a cursor.

Full-text history search links foreground and background result messages to
artifacts by call id and the SHA-256 carried in picoagent's current or legacy
model-facing envelope. This remains correct if a provider reuses a call id. The
local reader streams each candidate through SHA-256 verification before its
bounded `rg` search; same-length content mutation is therefore rejected. A
plain result without a recognized envelope is not linked by call id alone, and
the artifact source does not guess when an identity-free lookup has multiple
sidecars.

## Lifecycle

Artifacts belong to a run, not to long-term memory. Memory extraction may cite
or summarize selected artifacts, but raw tool output is not automatically
promoted into user or project memory.

Picoagent does not delete run artifacts automatically in the launch release.
Cloud deployments may apply an external retention policy to completed run
directories.
