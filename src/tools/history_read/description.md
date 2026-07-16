Read exact completed messages around a ref from `history_search`. `before` and
`after` count nearby compacted messages. Each JSONL record contains only a ref
and the same Chat-compatible message shape stored in `messages.jsonl`. Tool
call/result pairs may expand together. This tool cannot modify history.
