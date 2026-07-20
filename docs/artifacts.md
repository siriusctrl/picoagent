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

Small UTF-8 foreground results remain inline. When a result exceeds `inline_bytes`, the
complete bytes are written to the artifact store and the model receives a
compact `[Tool output]` envelope with:

- whether the tool result is an error
- whether any source bytes were actually truncated from the preview
- total, preview-head, preview-tail, and omitted byte counts
- an optional preview limitation when content cannot be previewed safely
- media type
- SHA-256 digest
- stable relative path
- instructions to use `read` with its returned `line_offset` or `byte_offset`,
  or `bash` with `rg`

An artifact may have `truncated: false` when its configured preview contains
all source bytes; artifact-backed and truncated are separate properties.

The normal textual preview retains both the beginning and ending because
compiler and command failures commonly appear at the end of command output.
The byte counts directly describe what the model received: head bytes come from
the start, tail bytes from the end, and `omitted` is everything not shown. This
avoids storing separate strategy and omitted-region fields that can be derived
from the counts.

Binary and non-UTF-8 data is never decoded into a lossy inline string. Its
envelope reports zero preview bytes and
`preview_limitation: binary_or_non_utf8`, with metadata and a path only.

An image returned by `read` follows the same artifact contract and is also sent
to the model as a native image attachment. The immediate tool result retains
the artifact path and digest; after every result from that assistant tool-call
batch has been emitted in original call order, picoagent appends one user
message containing the images and a runtime reminder that lists their source
call ids in attachment order. This preserves provider tool-call/result pairing
even when several tools ran concurrently. OpenAI Chat uses `image_url` data
URLs, OpenAI Responses uses `input_image`, and Anthropic uses base64 image
sources. JPG, PNG, and WebP pass through; GIF first frames and BMP files are
normalized to PNG for a consistent provider surface.

Artifact-reference overhead and ordinary conversation text still count toward
the provider context, so deployments should set provider token limits
appropriate to the model. Successful and failed foreground results share this
per-result policy; a large error is an artifact-backed bounded result rather
than an unbounded exception string in the next model request.

### Background delivery

Every terminal background result is persisted as an artifact, including small
completed values and failure, cancellation, or interruption details. The parent
receives one batched runtime message per ready set. Each `<background_task>`
body is only the workspace-relative artifact path; it does not contain a
preview or the original output. This keeps the runtime message bounded and
makes reading the artifact an explicit model choice.

If an image read exceeds the foreground window and becomes a background task,
its terminal notice follows this artifact-only rule. Reading that artifact
again attaches the image on demand.

A status-less background notice is only a running acknowledgement and has no
result artifact. A terminal notice includes `status` and pairs its path with the
same `ArtifactRef` in `message_metadata.jsonl`.

### History-query boundaries

History tools use this same envelope when their returned JSON or JSONL is too
large. For `history_search`, that artifact contains the complete result after
applying `history_search_max_matches`; it does not contain older matches omitted
by the query cap. A `truncated: true` search result therefore means “refine the
regex,” while an artifact preview means “use bounded `read` on the referenced
complete bounded result.” Neither history tool uses a cursor.

Full-text history search reads the exact `ArtifactRef` stored with each
foreground or background result in `message_metadata.jsonl`. The Chat-shaped
`messages.jsonl` line remains unchanged, and history retrieval does not parse
the model-facing preview envelope. A foreground `ToolResult` is correlated by
its provider `tool_call_id`. After promotion, the running acknowledgement still
occupies that provider tool-result slot, while the later terminal background
message is correlated by `task_id`. The local reader follows the exact
`ArtifactRef` paired with that message, streams the referenced candidate
through SHA-256 verification, then performs its bounded `rg` search. Artifact
matches return the exact path as well as the owning message ref, so a batched
message with several task results is not ambiguous. Reused call ids do not
create ambiguity, and same-length content mutation is rejected.

## Lifecycle

Artifacts belong to a run, not to long-term memory. Memory extraction may cite
or summarize selected artifacts, but raw tool output is not automatically
promoted into user or project memory.

Picoagent does not delete run artifacts automatically in the launch release.
Cloud deployments may apply an external retention policy to completed run
directories.
