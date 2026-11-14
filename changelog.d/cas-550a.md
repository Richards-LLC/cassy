### Fixed — Factory

- **Syntax checks after owned edits** — A preceding heredoc edit no longer makes a plain `bash -n` check look like unresolved shell execution. Existing command-scope and write-ownership checks still apply.
