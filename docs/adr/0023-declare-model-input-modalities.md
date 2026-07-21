# ADR 0023: Declare Model Input Modalities

- Status: Accepted
- Date: 2026-07-21
- Refines: ADR 0004 (stable prompt and dynamic runtime context)
- Refines: ADR 0006 (resume capability identity)
- Refines: ADR 0022 (image-read availability)

## Context

Fiasco can attach images, but an OpenAI-compatible endpoint does not imply
that its selected model supports vision. Sending a native image message to the
configured text-only model produced a provider 400 after the image tool result
had already been committed. Inferring support from model names would require a
stale registry, and endpoint probing is neither portable nor reliable.

The harness has one configured provider and a very small deployment surface. It
does not need dynamic routing between text and vision models, but the capability
must not be hard-coded to one known model name.

## Decision

- Each provider configuration accepts `modalities`, a sorted set that defaults
  to `["text"]`. `text` is required. The initial implementation also accepts
  `image`; other values are rejected until the harness actually supports them.
- The declaration applies to the primary model and any GeneralTask model
  override in that process. Fiasco does not select another model based on a
  task or attachment.
- Add one stable system rule: the current model's supported modalities are
  authoritative and the agent must not request or claim an absent modality.
- The initial runtime reminder records only the concrete fact as `current model
  supported modalities: [text]` or `[text, image]`. This keeps configuration out
  of the stable system prefix.
- App-tool assembly gives `read` one image-enabled boolean. Under a text-only
  configuration, an image path returns a normal tool error before file loading,
  artifact creation, attachment persistence, or another provider request. The
  static `read` schema and description remain unchanged across runs.
- Run record version 6 stores the modality set. Resume requires the current set
  to equal the stored set before advancing the run.

## Consequences

- Text models fail locally and intelligibly instead of receiving unsupported
  multimodal wire content.
- Vision support is explicit without a model-name allowlist, capability probe,
  model registry, or dynamic routing layer.
- Models receive one short capability fact in the existing runtime reminder;
  stable prompt caching remains intact.
- A GeneralTask model override cannot declare a different modality set. This is
  an intentional simplification for the current single-model deployment style.
- Existing pre-release run records are not resumable after the version change.

## Alternatives Considered

- **Infer modalities from the model name.** Rejected because aliases and vendor
  releases make the mapping incomplete and quickly stale.
- **Probe the provider.** Rejected because compatible APIs expose no uniform,
  trustworthy capability endpoint.
- **Dynamically choose a vision model.** Rejected because current deployments
  do not need routing and it would complicate credentials, resume, and costs.
- **Accept future modalities such as video now.** Rejected because configuration
  should not promise a message/tool/provider path that is not implemented.
- **Change the `read` schema for text-only runs.** Rejected because text reading
  remains available and frozen static tool schemas are easier to cache and
  inspect.

## Related Documents

- [ADR 0004: Stable agent prefix and core history tools](0004-stable-agent-prefix-and-core-history-tools.md)
- [ADR 0006: Complete-message resume and child coordination](0006-complete-message-resume-and-durable-child-coordination.md)
- [ADR 0022: Native image attachments after tool results](0022-native-image-attachments-after-tool-results.md)
- [Configuration](../configuration.md)
- [Architecture](../architecture.md)
