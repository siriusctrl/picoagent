Read completed messages around a `ref` returned by `history_search`. `ref` is
`m<N>`, where smaller `N` is older. `before` and `after` count nearby messages
omitted from active context, not compaction cycles.

Returns chronological JSONL with one object per message:
`{"ref":"m<N>","message":<OpenAI Chat-compatible message>}`. Tool call/result
pairs may expand together, so the result can contain more records than the
requested window. Harness-only context-management records are excluded. For a
search match with `source: "artifact"`, the matching text came from the full
spilled result and may be outside the returned message's bounded preview; use
the artifact path in that tool-result message with `read`. This tool cannot
modify history.
