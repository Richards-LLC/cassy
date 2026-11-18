### Fixed — Factory verification

- **Keep automatic review input bounded** — Large task and epic histories move to an immutable local context file instead of exceeding the verifier harness input limit; all criteria, evidence and dispatch identity remain available to the reviewer.
- **Replay reviews without namespace tools** — Historical reviewer evaluation probes Linux namespace support before spending a model run and offers a source-input-only replay with pinned source/diff, shell tools disabled and no verification authority.
- **Explain approval reuse accurately** — Merge retries reuse an unchanged candidate's approval, including a supervisor approval after dispatch expiry; changed delivery code or review context requires another review.
