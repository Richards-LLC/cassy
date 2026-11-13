### Fixed — Factory workers

- **Park proof follows the delivered tip after a rebase** — An ancestry-preserving rebase merge now uses the existing delivery ownership and patch checks. Park reads proof at the accepted delivery boundary and reports unattributable restored checkpoints before asking for stale proof. First-parent checkpoints, stacked delivery boundaries and generated-output safeguards retain their existing ownership and proof checks.
