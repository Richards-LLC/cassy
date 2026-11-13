### Fixed — Factory guard

- **Commit guard reads heredocs and substitutions like the shell** — Was: when
  any heredoc in a worker command expanded, the commit guard read every
  heredoc body, including quoted source and Markdown, as shell and refused it,
  while a commit inside `$(…)` was never checked. Now: heredoc bodies are data,
  every command substitution is checked in the directory it runs in, and a
  substitution the guard cannot bound is refused.
