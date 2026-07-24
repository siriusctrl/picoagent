# ADR 0046: Treat Artifacts As Run-Local Mutable Attachments

- Status: Accepted
- Date: 2026-07-24
- Refines: ADR 0003 (local artifact history search)
- Refines: ADR 0018 (artifact-backed result representation)
- Refines: ADR 0022 (image artifact storage)

## Context

The previous artifact reference duplicated facts already available from the
message and filesystem: format version, artifact id, run id, call id, byte
length, and content digest. Every spill also wrote a JSON sidecar. History
search then revalidated those fields and streamed the whole file once to hash
it before streaming it again through `rg`.

That machinery treated a tool result as content-addressed immutable evidence,
but the surrounding runtime exposes ordinary workspace files. It made a useful
operation such as updating a generated attachment look like corruption, and it
added a second artifact identity system beside the durable message that already
stores the exact result-to-path association.

The real external boundaries are narrower: history search must not escape the
current run's artifact directory or accept a non-file target, and each new
spill must not overwrite an existing attachment.

## Decision

- An `ArtifactRef` contains only `path` and `media_type`.
- Each spill creates
  `<sanitized-tool-call-id>-<ulid>.<extension>` under the current run's
  artifact directory with no sidecar and no content-derived filename.
- Creation uses no-overwrite filesystem operations. A repeated tool call id
  therefore produces a distinct attachment.
- A referenced attachment is mutable. Later reads and history search use its
  current bytes and current filesystem metadata without digest or recorded
  length validation.
- The model-facing envelope retains generation-time preview and byte counts.
  Mutating the attachment does not rewrite the originating message.
- Before local history search reads an attachment, it canonicalizes both paths,
  requires containment under the current run's artifact directory, and requires
  a regular file.
- Structured result metadata remains the authority for which path belongs to a
  message. History search never parses preview prose or infers an attachment
  from a reused call id.

## Consequences

- Sidecar I/O, digest computation, duplicated identity fields, and mutation
  rejection disappear from the normal artifact path.
- Updating an attachment is intentional and immediately visible to later
  inspection and history search.
- The originating preview remains useful as generation-time evidence but is not
  proof of the attachment's current contents.
- Run directories remain the containment and lifetime boundary. Moving an
  individual attachment without its run/message context is not a supported
  identity-preserving operation.
- Consumers that require immutable historical bytes must copy them elsewhere;
  Fiasco does not provide content-addressed artifact versioning.

## Alternatives Considered

- **Keep hashes but stop rejecting mutation.** Rejected because the digest
  would become stale metadata with no authority.
- **Rewrite the sidecar and message after every edit.** Rejected because it
  creates a second mutation protocol and rewrites durable history for ordinary
  filesystem changes.
- **Keep content-derived filenames without verification.** Rejected because
  the name would misleadingly describe old content after an update.
- **Allow arbitrary workspace paths.** Rejected because current-run containment
  and regular-file checks are inexpensive real boundaries for history search.

## Related Documents

- [Artifact contract](../artifacts.md)
- [Architecture](../architecture.md)
- [Runtime model](../runtime-model.md)
- [ADR 0018](0018-limit-tool-output-per-result.md)
- [ADR 0022](0022-native-image-attachments-after-tool-results.md)
