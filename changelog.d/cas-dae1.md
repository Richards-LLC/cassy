### Fixed — Task close

- **Cited proof must be portable before parking** — Was: an ordinary delivery
  could park after a cited evidence upload failed, and missing cited files were
  silently omitted. Now: the park refuses with all unpublished paths, including
  missing files under the configured evidence directory or worktree test logs.
  Successful uploads are retained for the next attempt; optional uncited copies
  keep their existing best-effort publication within the shared time budget.
