### Fixed — Factory merges

- **Automatic CI merge retry survives a GitHub lookup timeout** — when a parked merge is waiting for lane CI, a timed-out or failed `gh api` lookup no longer stops the armed retry or reports the worker as blocked. The retry keeps waiting for the next CI watch pass and still ends on a red or green result, a moved lane, or the two-hour limit.
