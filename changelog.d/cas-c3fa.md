### Fixed — Factory pushes

- **Unchanged version fixtures allow pushes** — The next-release literal check
  scans only lines added by a lane, so existing dependency fixtures no longer
  block an unrelated change in the same file.
