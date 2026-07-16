Regex-search completed messages omitted from active context, including full
text in linked foreground and background result artifacts. `pattern` uses Rust
regex; use inline flags such as `(?i)`. Matches are newest-first and return only
the message `ref`, match `source`, and a short `snippet`; pass a ref to
`history_read` for exact context. If `truncated` is true, refine the regex to
inspect omitted older matches.
