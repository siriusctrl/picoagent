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

Small UTF-8 results remain inline. When a result exceeds `inline_bytes`, the
complete bytes are written to the artifact store and the model receives:

- `truncated: true`
- media type and byte length
- SHA-256 digest
- stable relative path
- a beginning preview
- an ending preview
- instructions to use bounded `read` or `bash` with `rg`

The ending preview is required because compiler and command failures commonly
appear at the end of stdout or stderr.

Binary data is never decoded into a lossy inline string. Its envelope contains
metadata and a path only.

The run-level `max_inline_bytes_per_run` budget also forces later small results
to artifacts. Preview bytes consume that budget; after it is exhausted, later
results carry reference metadata without a content preview. Artifact-reference
overhead and ordinary conversation text still count toward the provider context,
so deployments should also set provider token limits appropriate to the model.

## Lifecycle

Artifacts belong to a run, not to long-term memory. Memory extraction may cite
or summarize selected artifacts, but raw tool output is not automatically
promoted into user or project memory.

Picoagent does not delete run artifacts automatically in the launch release.
Cloud deployments may apply an external retention policy to completed run
directories.
