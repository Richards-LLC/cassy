### Fixed — Factory

- **Cassy spends far fewer GitHub API calls** — The factory's CI watcher no longer re-reads a delivery's pull request every 30 seconds after GitHub has reported it merged; in batch QA those deliveries wait for the release verdict, and polling them was the factory's largest GraphQL cost. A process now resolves a repository's canonical name once an hour instead of on every poll, merge and close, and one CI poll reads completed workflow runs once instead of twice.
- **One rate-limit response pauses every caller** — When GitHub refuses a call for rate limiting, every Cassy `gh` call on that machine fails fast for two minutes instead of each caller retrying into the limit.
- **See who spends the budget** — Every Cassy `gh` call is logged with its caller and endpoint to `~/.cas/logs/gh-calls-<date>.tsv` (kept 7 days), and `cas doctor` shows the last hour's total, GraphQL share and top five callers.
