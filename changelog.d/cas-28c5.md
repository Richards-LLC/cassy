### Fixed — Factory workers

- **Park proof follows the delivered tip after a rebase** — An ancestry-preserving rebase merge now uses the existing delivery ownership and patch checks. Fresh park on an owned task branch reads proof at the measured pushed tip, and reports attribution failures before asking for tests at a stale checkpoint. Shared lanes retain their stacked delivery boundaries and subject ownership checks.
