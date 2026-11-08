### Fixed — Verification

- **A supervisor verdict no longer closes a dispatch under a running verifier** — Was: a supervisor answer on a dispatch resolved it while an independent task-verifier was still running, so that verifier's later rejection could not be recorded and an incomplete delivery merged. Now: a supervisor-direct verdict is refused while a live verifier is bound to the dispatch and owes its verdict; a question the verifier itself raised is still answered normally.
