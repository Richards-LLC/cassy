### Fixed — Factory checks

- **Resumed deliveries retain their complete scope** — Nonempty delivery retargets keep their original start audit while checking ownership against the current target. References to another task as port context no longer drop intervening implementation or changelog commits. Reverted journey edits do not require stability identities; surviving ambiguous edits still refuse.
