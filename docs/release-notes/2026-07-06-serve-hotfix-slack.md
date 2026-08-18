# Slack draft — serve startup hotfix (2026-07-06)

Channel: #cas-internal (`C0B44GUKDK2`) — two top-level posts.

---

## Post 1 — User

Today's session-isolation release had a serious bug: upgrading the binary made Cassy refuse to start on any **existing** project — it died instantly at startup with a one-line error and no explanation, taking the whole factory down with it. Fresh projects were fine, which is exactly why our tests missed it. Fixed within the hour.

- Upgrading no longer breaks existing projects: Cassy starts normally, and if your project's database needs a schema update it now **tells you exactly what to run** (`cas update --schema-only`) instead of silently failing.
- Startup errors now show the full cause instead of a vague one-liner — the report that caught this said "no further diagnostics," and that's fixed too.
- If you hit this today: update, run `cas update --schema-only` once in each project, and you're back.

## Post 2 — Dev

The isolation release added `CREATE INDEX … ON agents(factory_session)` to the baseline `AGENT_SCHEMA` as well as migration m204. Baselines execute at every store open, before migrations — so on any pre-m204 database the `CREATE TABLE IF NOT EXISTS` no-oped and the index creation aborted eager store init (`no such column: factory_session`), killing `cas serve`. Fresh databases get the column via the baseline table, so CI and both release gates were green.

- Fix: index moved out of the baseline into detect-guarded migration m205 (`sqlite_master` index-existence detect, so it also covers fresh stores where m204's column-detect skips). Rule now documented in the schema file: never index a migration-added column from a baseline schema.
- Regression test guards the class: baseline `AGENT_SCHEMA` must apply cleanly over a pre-m204 table shape.
- Observability: the top-level error printer now renders the anyhow chain (`{e:#}`) — the original failure printed only "eager store init failed at 'task_store'" and discarded the actual SQLite error; and serve now emits a startup warning with the exact remediation command when schema migrations are pending (serve intentionally never runs DDL; degraded-until-update is the design, and now it's loud).
- Recovery for anyone who upgraded today: rebuild at main, `cas update --schema-only` per project, restart.
