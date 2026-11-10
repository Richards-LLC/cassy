### Fixed — QA ledger

- **Ledger init resumes an existing ledger** — Rerunning `cas qa ledger init --task <id>` on a resumed task keeps the existing ledger byte for byte and prints its path, instead of failing with "File exists". An existing ledger that does not validate for the task is refused with its diagnostics, and is never overwritten.
