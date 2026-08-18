# Spike A — Cross-repo sweep topology + Cassy repo discovery

**Task:** cas-7ef1 (spike) · **EPIC:** cas-7c88 · **Date:** 2026-04-21
**Requirements covered:** R3, R4, R8, R9, R15 (portability-critical)
**Unblocks:** cas-a54d (Unit 4), cas-8141 (Unit 3)
**Related:** `docs/brainstorms/2026-04-21-worktree-leak-and-supervisor-discipline-requirements.md`

## Evidence gathered

### Cassy storage topology on this host

- **Per-repo `.cas/`** — each project directory has its own `.cas/cas.db`, e.g. `/home/pippenz/Petrastella/cas-src/.cas/cas.db` (1409 tasks), `/home/pippenz/Petrastella/pantheon/.cas/cas.db` (22 tasks). `find /home/pippenz -maxdepth 4 -name .cas -type d` returns 20+ repos.
- **Host `~/.cas/`** — global scope: `cas.db` (1231 tasks, 15 sessions), `config.toml`, `cloud.json`, `proxy_catalog.json`, `sessions/*.json`, `factory-*.sock`, `logs/factory/<session>/`.
- **`global_cas_dir()`** (cas-cli/src/config/access/global.rs:3) returns `dirs::config_dir().join("cas")` → `~/.config/cas` on Linux, `~/Library/Application Support/cas` on macOS. This is **not** where the active host state lives. Everything referenced below uses `dirs::home_dir().join(".cas")` (e.g. cas-cli/src/ui/factory/session.rs:22-26, :74-77). There is a latent inconsistency between the two, but it is out-of-scope for this spike — treat `~/.cas/` as the de-facto host root.

### Candidate "known repos" sources that already exist

| Source | Path | Contents | Portable? | Gaps |
|---|---|---|---|---|
| Session metadata JSON | `~/.cas/sessions/<name>.json` | `project_dir` absolute path per factory session | Yes (no `$HOME` embedded in schema) | Only captures factory sessions, not bare `cas` CLI usage |
| `sessions.cwd` (host DB) | `~/.cas/cas.db` → `sessions(cwd TEXT)` | Distinct cwd per Claude Code session: 9 distinct, mostly absolute repo paths | Yes | Includes non-repo cwds (e.g. `/home/pippenz`); not all repos have a session row |
| Factory socket filenames | `~/.cas/factory-<leaf>-<adj>-<noun>-<n>.sock` | Leaf name of project only (see session.rs:100-108) | Partially | **Leaf-only → irreversible to absolute path**, collides across repos with same basename |
| `daemon_instances` table | `~/.cas/cas.db` | `id, pid, daemon_type, heartbeat` | Yes | No repo field at all |
| `proxy_catalog.json` | `~/.cas/proxy_catalog.json` | MCP tool proxy catalog | — | Not a repo registry (verified: contains playwright/context7/neon entries only) |

### Code-search negative results

`rg 'known_repos|list_repos|list_projects|registered_repos|host_registry'` → **0 matches** in product code (only hits are in migration shell scripts and an unrelated MCP proxy doc). Confirms: no canonical `list_repos` API exists today.

### Daemon architecture today

- Standalone `cas daemon` process **has been removed** (see cas-cli/src/daemon/mod.rs:11-13). Maintenance runs either (a) embedded in any MCP server on idle, or (b) as a one-shot via `cas daemon run`.
- Factory daemons (`cas-cli/src/ui/factory/daemon/`) are per-session, socket-addressed at `~/.cas/factory-<session>.sock`. Their lifecycle is tied to a supervisor TUI, not the host.
- No persistent host-level process exists today. Any "always-on" new process would be new infrastructure.

---

## Q1 — Canonical repo discovery

**Recommendation:** Add a `known_repos` table to the **host** DB at `~/.cas/cas.db`, written on `cas init` and on factory-session startup. Use a lightweight migration; read back via a single `list_known_repos()` store method.

**Minimal schema:**

```sql
CREATE TABLE known_repos (
    path           TEXT PRIMARY KEY,   -- canonicalized absolute path, no trailing slash
    first_seen_at  TEXT NOT NULL,
    last_touched_at TEXT NOT NULL,
    touch_count    INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_known_repos_last_touched ON known_repos(last_touched_at DESC);
```

**Write points (all idempotent UPSERT on `path`):**
- `cas init` (creates `.cas/` in a repo) → insert + set `first_seen_at`.
- Factory session spawn → already has `project_dir` in SessionSummary; upsert at daemon start.
- MCP server startup with a non-empty `.cas/` in CWD → upsert (catches "cas used without init", handles legacy repos).

**Why a new table, not re-use:**
- `sessions.cwd` is noisy (contains `$HOME` rows, non-repo dirs) and only populated by Claude Code hook sessions.
- `~/.cas/sessions/*.json` is factory-only — misses solo-CLI users.
- Socket filenames discard the absolute path.
- A `known_repos` table is the single portable source that works whether the user runs `cas` from a bare shell, a Claude Code session, or a factory TUI.

**Code pointers for implementers (Unit 3 / Unit 4):**
- Migration: new file `cas-cli/src/migration/migrations/m199_known_repos.rs` (follow m195-m198 pattern).
- Store: add `KnownRepoStore` alongside `crates/cas-store/src/worktree_store.rs` — same `shared_db.rs` pool, same `cas_dir.join("cas.db")` convention. But: scope this store to the **host** `~/.cas/`, not the repo-local `.cas/`. Callers pass `dirs::home_dir().join(".cas")` explicitly; do **not** add a `~/Petrastella` assumption (R15).
- Init hook: `cas-cli/src/commands/` init command (currently implicit — search `\.cas.*create_dir_all` in bridge/factory paths).

**Fallback for already-deployed hosts:** on first read, if `known_repos` is empty, seed from `UNION DISTINCT` of (a) `sessions.cwd` filtered to paths where `<cwd>/.cas/` exists on disk, (b) `project_dir` parsed from every `~/.cas/sessions/*.json`, (c) a one-time `find $HOME -maxdepth 5 -name .cas -type d` behind an opt-in flag (slow, user-initiated). This gives Unit 3/4 a working registry immediately without waiting for re-init.

---

## Q2 — Sweep topology

**Recommendation: (b) — a single global sweeper, implemented as an *opportunistic* sweep triggered by any CAS MCP server or factory daemon startup, debounced via a host-level `~/.cas/last_global_sweep` timestamp (default: 1h).** Use `known_repos` as the input set.

**Two-sentence justification:** This piggybacks on existing always-created lifecycles (MCP server startup per session, factory daemon startup per factory) instead of introducing a new persistent process the removed-daemon architecture no longer supports. Debouncing on a host file keeps the cost O(1) per extra invocation, isolates failure (one repo's sweep error doesn't block others), and needs zero user-facing installation — works identically on Linux and macOS.

**Rejected alternative: (c) host cron/launchd.** Reason: non-portable surface area — Linux needs `systemd --user` or crontab, macOS needs launchd plist, Windows (out of scope today but a future tripwire) needs Task Scheduler. Each requires install-time user consent, and silent failures on any of them are invisible to Cassy. Installation friction alone disqualifies it per R15; (a) is rejected secondarily because a daemon bound to repo X has no legitimate reason to write into repo Y's `.cas/`, violating isolation.

**Why opportunistic-(b) over dedicated-daemon-(b):** the standalone daemon pattern was deliberately removed (cas-cli/src/daemon/mod.rs:11-13). Reintroducing a persistent host-level process would re-open that design question; the opportunistic variant satisfies the functional requirement (cross-repo sweep runs regularly) without the architectural reversal.

**Implementation sketch for Unit 4 (cas-a54d):**
- Add `cas sweep-all` one-shot CLI command (callable from tests and from `cas daemon run` scripts).
- In MCP server startup path and factory daemon startup path, after `known_repos` upsert, check `~/.cas/last_global_sweep` mtime; if older than threshold, spawn sweep on a detached task (Tokio `spawn` + `tracing::error!` on panic; do **not** block startup).
- On success, touch `~/.cas/last_global_sweep`.
- Failure-per-repo must not abort the loop: wrap each repo sweep in `catch_unwind`-equivalent and log.

---

## Next steps for Units 3 & 4

### Unit 3 (cas-8141) — repo discovery & one-shot CLI
1. Add migration `m199_known_repos` (schema above, new file).
2. Add `KnownRepoStore` with `upsert(path)`, `list()`, `touch(path)` — routes to host `~/.cas/cas.db`.
3. Wire upsert into: `cas init`, factory daemon startup (project_dir is already available), MCP server startup.
4. Implement one-time seed fallback (union of sessions.cwd + session JSON files) behind `cas known-repos seed`.
5. Add `cas known-repos list` CLI for diagnostics.

### Unit 4 (cas-a54d) — global sweeper
1. Implement `cas sweep-all` consuming `KnownRepoStore::list()`.
2. Per-repo: iterate worktrees under `<repo>/.cas/worktrees/`, apply existing cleanup logic from factory-worktree-leak playbook; continue-on-error.
3. Add opportunistic trigger hook (debounced via `~/.cas/last_global_sweep`) at MCP server and factory daemon startup.
4. No cron, no launchd, no systemd unit — keep it pure-Rust.
5. Surface sweep results via `cas sweep-all --dry-run` + log to `~/.cas/logs/global-sweep.log`.

### Portability guardrails (R15)
- No code path may embed `Petrastella`, `cas-src`, or any user's `$HOME` layout.
- All host-level paths must go through `dirs::home_dir()` (current convention) or the yet-to-be-reconciled `global_cas_dir()`.
- Sweep thresholds and debounce intervals must be configurable via `config.toml`, defaults chosen to be safe on both solo-laptop and multi-project setups.
