# ADR 0022: Send Native Image Attachments After Ordered Tool Results

- Status: Accepted
- Date: 2026-07-20
- Refines: ADR 0005 (multimodal user content)
- Refines: ADR 0017 (direct tool-batch result ordering)

ADR 0023 later makes image availability explicit in provider configuration and
causes text-only image reads to fail before attachment creation. The native
attachment contract here remains accepted for image-capable runs.

ADR 0032 later stores the provider-neutral image attachment directly in the
self-contained message record instead of projecting it to Chat plus layout
metadata. The attachment ordering and provider projection decisions here remain
accepted.

ADR 0046 later replaces immutable image artifacts with mutable run-local
attachments referenced by path and media type. Native image attachment ordering
and provider projection remain accepted.

## Context

The `read` tool could inspect bounded UTF-8 text but not show an image to the
model. Returning base64 as tool text would waste context and leave each provider
adapter to guess that the result was visual. Inserting a user image immediately
after its individual tool result would also split the tool-result sequence when
one assistant message requested several concurrent calls. Some providers
require every tool call to have its matching result before conversation resumes
with another user message.

Fiasco also persists a directly inspectable Chat-compatible trajectory.
Images therefore need both a provider-neutral runtime representation and an
ordinary Chat representation that survives resume without adding private fields
to `messages.jsonl`.

## Decision

- Add one canonical image-attachment content block containing media type and
  standard base64 bytes. Provider adapters alone project it to OpenAI Chat
  `image_url`, OpenAI Responses `input_image`, or Anthropic base64 image source
  blocks.
- `read` accepts jpg/jpeg, png, gif, webp, and bmp paths. JPG, PNG, and WebP are
  passed through. GIF uses its first frame and GIF/BMP are normalized to PNG so
  the adapters receive a conservative common format.
- Every image is preserved as a normal immutable run artifact. Its tool result
  contains the artifact envelope, not inline binary data.
- A direct batch first returns every tool result in the assistant's original
  call order. It then appends at most one user message containing a short
  runtime reminder with source call ids followed by all image attachments in
  that order.
- Text-only persisted user messages retain string `content`. Image-bearing user
  messages use the native Chat content-part array and data URLs. The paired
  metadata records the internal content layout and commits exact reconstruction.
- A promoted/background image result remains artifact-only under the existing
  background-delivery contract. A later `read` of that image artifact attaches
  it on demand.

ADR 0030 later generalizes background delivery to the ordinary per-result
policy; image bytes remain artifact-backed because they are binary.

## Consequences

- The model receives actual image inputs rather than large textual base64, and
  the same runner works across all current provider adapters.
- Concurrent tool pairing stays deterministic. An extra user message appears
  only for batches that produced immediate image results.
- Image bytes exist both in the immutable artifact and as base64 in the durable
  Chat message. This is deliberate: the artifact preserves job output while the
  message makes resume reproduce the exact model input.
- GIF animation is not preserved and BMP is not sent directly. The first-frame
  PNG conversion favors predictable provider acceptance over format fidelity.

## Alternatives Considered

- **Return base64 inside the tool result.** Rejected because it consumes text
  context and does not create a native vision input.
- **Insert one user attachment after each result.** Rejected because it can
  interrupt the complete result sequence for a concurrent assistant tool batch.
- **Store only an artifact path and require a second read.** Rejected for normal
  foreground reads because the requested image should reach the model in the
  same tool turn.
- **Put provider-specific image blocks in the agent loop.** Rejected because
  wire formats belong to provider adapters.

## Related Documents

- [ADR 0005: Chat-compatible messages with a metadata sidecar](0005-openai-chat-compatible-message-log.md)
- [ADR 0017: Concurrent tool batches and explicit task controls](0017-concurrent-tool-batches-and-explicit-task-controls.md)
- [Artifact contract](../artifacts.md)
- [Architecture](../architecture.md)
- [Runtime model](../runtime-model.md)
