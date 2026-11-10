### Fixed — Violet
- **Reachable post recovery** — Posting permission now includes caller-scoped outcome reconciliation with the same role and delegation restrictions. Request IDs require usable recovery; definitive no-post and pre-dispatch failures keep their original result. Recovery guidance retains the original ID for idempotent retries. Closes #1778.
