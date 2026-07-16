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
- total, preview-head, preview-tail, and omitted byte counts
- an optional preview limitation when the run budget forces artifact spill,
  shortens the normal head/tail preview, or content cannot be previewed safely
- media type
- SHA-256 digest
- stable relative path
- instructions to use bounded `read` or `bash` with `rg`

An artifact may have `truncated: false` when its configured preview contains
all source bytes; artifact-backed and truncated are separate properties.

The normal textual preview retains both the beginning and ending because
compiler and command failures commonly appear at the end of stdout or stderr.
The byte counts directly describe what the model received: head bytes come from
the start, tail bytes from the end, and `omitted` is everything not shown. This
avoids storing separate strategy and omitted-region fields that can be derived
from the counts.

If the remaining run-level budget forces an otherwise inline result to spill,
or cannot hold the normal preview, picoagent reports
`preview_limitation: run_preview_budget_limited`. Once the budget is empty it
reports `run_preview_budget_exhausted`, returns zero preview bytes, and directs
the model to the complete artifact.

Binary and non-UTF-8 data is never decoded into a lossy inline string. Its
envelope reports zero preview bytes and
`preview_limitation: binary_or_non_utf8`, with metadata and a path only.

The run-level `max_inline_bytes_per_run` budget also forces later small results
to artifacts. Preview bytes consume that budget; after it is exhausted, later
results carry reference metadata without a content preview. Artifact-reference
overhead and ordinary conversation text still count toward the provider context,
so deployments should also set provider token limits appropriate to the model.
Successful and failed foreground/background results share this policy; a large
error is an artifact-backed bounded result rather than an unbounded exception
string in the next model request.

### History-query boundaries

History tools use this same envelope when their returned JSON or JSONL is too
large. For `history_search`, that artifact contains the complete result after
applying `history_search_max_matches`; it does not contain older matches omitted
by the query cap. A `truncated: true` search result therefore means “refine the
regex,” while an artifact preview means “use bounded `read` or `bash`/`rg` on
the referenced complete bounded result.” Neither history tool uses a cursor.

Full-text history search reads the exact `ArtifactRef` stored with each
foreground or background result in `message_metadata.jsonl`. The Chat-shaped
`messages.jsonl` line remains unchanged, and history retrieval does not parse
the model-facing preview envelope. The local reader checks that the ref belongs
to the current run and result call, streams the referenced candidate through
SHA-256 verification, then performs its bounded `rg` search. Reused call ids do
not create ambiguity, and same-length content mutation is rejected.

## Lifecycle

Artifacts belong to a run, not to long-term memory. Memory extraction may cite
or summarize selected artifacts, but raw tool output is not automatically
promoted into user or project memory.

Picoagent does not delete run artifacts automatically in the launch release.
Cloud deployments may apply an external retention policy to completed run
directories.
