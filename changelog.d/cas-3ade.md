### Fixed — Cloud sync

- **The conflict log stays small** — Was: every sync conflict saved another
  full copy of the local row, forever, and one project database grew to
  22 GB. Now: each task or memory keeps its ten most recent conflict copies,
  plus the latest copy of each conflict kind that Cassy still reads.
