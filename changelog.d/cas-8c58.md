### Fixed — Worker accounts

- **Honor the configured Codex worker account** — `spawn_workers` uses the local project's `factory.codex_home` when no explicit batch or per-worker `config_dir` is set, including lane requests. The receipt names the configuration source. Existing paths, `~/` paths and account aliases use normal account validation; invalid preferences refuse queueing. Clear the value to restore recommendations.
