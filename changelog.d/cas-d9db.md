### Changed — Agent instructions

- **Agent instructions build the simplest complete thing** — Supervisor, worker and reviewer instructions are about a third shorter and open with one rule: hit every mark, delete before you add, and treat speed as part of done. They no longer restate gates the tools already refuse and explain, and they now agree that workers close their own tasks and that the send result reports whether a message woke its recipient.
- **The task verifier sends overbuilt work back** — It now asks whether work is overbuilt, whether a smaller change fully solves it, and whether it slows the flow without catching a real failure. A named smaller sufficient change, or an unearned wait, is rejected like broken work.
- **`/finish` has workers push and park first** — It never forces a shutdown past unsaved work without the operator, and it names any kept worker's unsaved work before the operator exits.

### Added — Friction ledger

- **Release speed and complexity in `cas ledger report`** — Each release shows commit-to-main time (median and p90), main-to-publish time, and the complexity it added: lines added and removed, new config keys and new checks. Missing data reads as unknown with a reason.

### Fixed — Supervisor wake warning

- **The "no wake source" warning only appears with open work** — A supervisor turn with no parked delivery and no live worker no longer gets the reminder nag.
