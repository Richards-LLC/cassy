## Core Concepts

Host retention automatically sweeps registered projects during daemon idle maintenance
and when disk usage exceeds 80%. Artifacts stay for at least 14 days, and live task
citations, unmerged deliveries and the latest three releases preserve their evidence.
Retired worker checkouts require clean, fully pushed Git state and no live task;
their target directories are disposable. Scratch and review scratch expire after
24 hours unless referenced by live work. Unknown ownership, native process use,
symlinks and unavailable reference evidence preserve files.

`cas factory gc report` previews the host policy, `cas factory gc cleanup` applies
it, and `--dry-run` previews retention. Each applied sweep records paths and
per-project/category bytes in `~/.cas/retention/sweeps.jsonl`. Preflight reports
`disk.pressure` above 80% with the last sweep and retained paths.
