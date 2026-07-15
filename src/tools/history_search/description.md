Regex-search completed messages omitted from active context, including full
text in linked foreground and background result artifacts. `pattern` uses Rust
regex; use inline flags such as `(?i)`. Matches are newest-first and include
refs for `history_read`. At the configured limit, omitted older matches are not
in any output artifact; refine the regex. A spill artifact, if reported,
contains only the returned matches.
