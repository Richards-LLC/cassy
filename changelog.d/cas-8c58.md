### Honor the configured Codex worker account

`spawn_workers` now uses the local project's `factory.codex_home` when no explicit batch or per-worker `config_dir` is set, including lane requests. The spawn receipt identifies the configuration key as the account source. Existing paths, `~/` paths and account aliases use the normal account validation; an invalid preference refuses the spawn before queueing. Clear the value to restore account recommendations.
