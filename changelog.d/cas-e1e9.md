### Changed — Install

- **Factory sessions update as soon as their own project is refreshed** — Was: `cas update` re-execed every running factory session only after all projects refreshed, so a machine with many projects (31 on one Mac) left each session on the old daemon for four to five minutes. Now: each session re-execs right after its own project's refresh; sessions whose project failed, or that match no refreshed project, still re-exec at the end, and each session appears once in the receipt.
