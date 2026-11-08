### Fixed — Release latency receipt

- **The report and receipt state one main-to-published budget** — Was: when publication was timed from main landing, the receipt recorded the ten-minute default budget while stating a twenty-minute main-to-published budget, so the release report printed 10m. Now: a main-landed publication uses the twenty-minute main-to-published budget in both fields; an explicit `--budget-seconds` still wins.
