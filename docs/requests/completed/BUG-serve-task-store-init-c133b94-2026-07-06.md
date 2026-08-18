---
date: 2026-07-06
reporter: supervisor session (ozer)
severity: high
component: cas serve / task_store
binary: c133b94 (2026-07-06)
status: open
---

# `cas serve` fails eager store init at `task_store` on c133b94

## Symptom

Freshly built binary at HEAD `c133b94` exits immediately on startup:

```
[Cassy] Serve panic log: .../.cas/logs/cas-serve-2026-07-06.log
[Cassy] Daemon socket listening at ".../.cas/daemon.sock"
[Cassy] Running initial cloud sync (push stale + pull)...
[ERROR] eager store init failed at 'task_store'
```

The panic log file is never written (empty/missing). No further diagnostics emitted.

## Repro

1. `cargo build --release` at `c133b94`
2. Install over `~/.local/bin/cas`
3. `cas serve` in the ozer project (`/home/pippenz/Petrastella/ozer`)
4. Exits with the error above

## Rollback confirms regression window

Prior binary `325edcf` (also built 2026-07-06) starts fine against the same
`.cas` store. Suspect range `325edcf..c133b94` (23 commits, includes cas-604d
session-scoped spawn queue / agent visibility / message delivery merge and
cas-6d83 model-steering merge).

## Impact

Preflight-mandated rebuild (per cas-supervisor preflight.md) bricked the
daemon mid-session; supervisor session lost its CAS MCP connection and factory
capability. Rolled back to `325edcf` (kept at `~/.local/bin/cas.c133b94-broken`
for the broken build).

## Triage

| Question | Answer |
|---|---|
| Reproducible | Yes, deterministic on ozer store |
| Data loss | None observed (rollback reads store fine) |
| Workaround | Roll back to 325edcf |
