### Fixed — Release latency receipt

- **The report and receipt state one main-to-published budget** — Was: a release published from a main push recorded the ten-minute default as `BUDGET_SECONDS` while stating a twenty-minute main-to-published budget, so the release report printed 10m. Now: every main-push release (merge-queue green or main landing) takes its budget from the publish entry of `scripts/release-stage-budgets.json`; an explicit `--budget-seconds` still wins.
