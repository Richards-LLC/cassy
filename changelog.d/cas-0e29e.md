### Fixed — Releases

- **Release preview no longer stalls on large rewritten hunks** — Was: the
  ownership walk re-tokenized every pair of owned and added lines in a
  changed hunk for each delivered line. Cost grew with the square of the
  hunk size, and the v4.29 cut preview ran for up to 16 minutes. Now: each
  hunk line is tokenized once per walked commit and only lines sharing the
  owned line's tokens are compared. Close decisions are unchanged.
