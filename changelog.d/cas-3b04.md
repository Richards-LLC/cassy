### Fixed — Factory

- **Accurate pre-push scope** — Fast rows compare the branch with its merge-base against the selected target, so later target-only changes do not run unrelated checks. Script changes made by the branch still run their matching tests.
