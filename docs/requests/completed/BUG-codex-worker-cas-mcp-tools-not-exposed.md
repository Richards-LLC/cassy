# BUG: Codex factory worker not given CAS MCP tools; wrong tool prefix; stops after startup

## Resolution

Resolved by `cas-bbc2` (`32a3796`, `fix(cas-bbc2): spawn-inject CAS MCP server into Codex agents + single-task worker prompt`).
`PtyConfig::codex` now calls `push_codex_mcp_server_args` for every Codex agent before adding role-specific instructions, injecting `mcp_servers.cs.command="cas"`, `mcp_servers.cs.args=["serve"]`, and the required Cassy session/factory identity environment. The injected server uses the `cs` alias, so Codex workers receive `mcp__cs__*` tools even in projects with Claude-only `.mcp.json` configuration and no `.codex/config.toml`.

The same change corrected the reported Codex prompt surface: Codex worker/supervisor instructions use the `mcp__cs__` alias and enforce a single-task worker loop. Existing regression coverage in `crates/cas-pty/src/pty.rs` checks worker and supervisor MCP injection, the `mcp__cs__` prefix, and one-task-at-a-time wording. No additional runtime fix was needed for this request.

**Date:** 2026-06-24
**Reporter:** supervisor (Petra Stella Cloud factory session quick-salmon-65, session ae28fe25-14ab-49bc-a770-3cd42259c88a)
**Severity:** P1 — `cli=codex` factory workers cannot reach Cassy tooling natively and produce zero code
**cas:** 2.21.0 (e5c1a78-dirty 2026-06-23) · **codex:** codex-cli 0.128.0, model gpt-5.5 medium
**Evidence:** `./BUG-codex-worker-cas-mcp-tools-not-exposed.codex-rollout-2026-06-24.jsonl` (full Codex rollout, 125 lines, secrets redacted) + `./artifacts/codex-cx-detail-2026-06-24/` (gitignored: unredacted rollout, daemon log, worker settings)
**Related:** complements `BUG-codex-worker-stalls-in-task-lifecycle-gates.md` (2026-06-23, gpt-5.4) — same symptom (Codex worker, zero code), but a **different and more upstream** root cause. That report's worker reached the task-lifecycle gate; this one never even saw the Cassy tools.

## Summary

A factory worker spawned with `cli=codex` (gpt-5.5) was assigned one small, isolated task (`cas-ca12`). It produced **zero implementation** — worktree ended clean at the base commit. Unlike the 2026-06-23 report (worker stalled on the unverified-in-progress gate), this worker **was never given the `mcp__cas__*` coordination/task tools at all**. It noticed, improvised a workaround (hand-started its own `cas serve` over stdio), completed the *startup* lifecycle, then treated "startup complete" as turn-complete and ended without doing the work.

The Cassy server is healthy — the worker's self-started `cas serve` exposed every expected tool. The gap is purely how Codex workers are wired to it at spawn, plus a wrong tool-name prefix in the kickoff prompt.

## Timeline (UTC, from the rollout + cas daemon log)

- `12:44:34` Worker spawned; worktree `factory/cx-detail` from `main` @ `39e1cb3`.
- `12:45:02` `list_mcp_resources` → `{"resources":[]}`.
- `12:45:06` `tool_search "Cassy coordination task mcp__cs__coordination mcp__cs__task"` → returns only a `google_calendar` namespace. **No Cassy tools.**
- `12:45:13` Worker: *"The expected `mcp__cs__...` tools aren't exposed in this tool list…"*
- `12:45:14` `command -v cas` → `/home/pippenz/.local/bin/cas`; `.cas` not present in the worktree.
- `12:45:18` `cas --help` — CLI has no `task`/`coordination` subcommands (MCP-only).
- `12:45:28` `cas mcp list --json` (⚠ leaked secrets — see below); finds project `.mcp.json`.
- `12:45:36` Reads `.mcp.json` → CAS MCP server defined as `cas serve`.
- `~12:45:40` Worker **manually launches `cas serve` over stdio**, queries it, runs the startup calls.
- `12:45:41` Daemon: `Agent registered agent_name=cx-detail role=worker` — i.e. registration happened only via the worker's *self-started* serve, ~67s after spawn.
- `12:46:10` `task_complete`: *"Worker startup complete… found one assigned task, showed and started `cas-ca12`, and added a progress note."* Duration 83.6s. **Turn ended; task never implemented.**

## Root causes

### A (P1) — Cassy coordination/task MCP tools absent from the Codex worker tool surface
`list_mcp_resources` empty; `tool_search` for Cassy coordination found zero Cassy tools; worker explicitly states they aren't exposed. The factory wrote `cx-detail` a **Claude-Code settings file** (`artifacts/.../cx-detail-settings.json`: hooks `cas hook PreToolUse`, `permissions.allow:[Read,Write,Edit,Glob,Grep,Bash,NotebookEdit]`) with **no `mcpServers` block**. The CAS MCP server is only declared in the project's `.mcp.json` (a Claude-Code convention), which Codex does not consume the same way → the Codex worker launches without `mcp__cas__*`.

### B (P2) — Kickoff/developer instructions use the wrong prefix `mcp__cs__`
Both the injected `developer_instructions` and the startup `user` message reference `mcp__cs__coordination` / `mcp__cs__task`. The real namespace is `mcp__cas__*`. Even if (A) is fixed, the worker is told to call tools that don't exist. (It searched for the literal `mcp__cs__` names and found nothing.)

### C (P2) — Worker ends its turn after "startup", never reaching implementation
This matches the 2026-06-23 report's root causes #1/#4. The kickoff prompt enumerates startup steps ("register, whoami, mine, start the assigned task with a progress note"); the model did exactly those and stopped. Codex has no Stop-hook to re-prompt. The kickoff should chain past startup, e.g. *"After startup, immediately implement the assigned task end-to-end; do not end your turn until it is implemented, verified (`pnpm build && pnpm test`), and closed."*

### Security (P1) — `cas mcp list --json` leaked live credentials into logs
The worker's workaround ran `cas mcp list --json`, which printed the **GitHub PAT, Neon API key, and Vercel token** in plaintext into `~/.codex/sessions/.../rollout-*.jsonl`. This is a downstream effect of (A) — a tool-less worker improvising. Recommend `cas mcp list` redact secrets by default (opt-in `--show-secrets`); rotate the exposed tokens. (The committed rollout sibling here is redacted; the unredacted copy is in the gitignored `artifacts/` dir.)

## Recommended fixes
1. Inject the Cassy `serve` MCP server into the **Codex** worker launch (codex config / equivalent), not only `.mcp.json`; assert `mcp__cas__*` appears in the Codex worker's tool list at spawn.
2. Correct the kickoff/developer-instruction prefix `mcp__cs__` → `mcp__cas__`.
3. Drive the kickoff prompt through implementation, not just startup (Codex single-turn semantics; no Stop hook). Coordinate with the 2026-06-23 report's fix.
4. Redact secrets in `cas mcp list` output.

## Impact
- `cli=codex` workers remain non-functional for real work across two consecutive days (different root causes). Supervisors should spawn `cli=claude` until these land.
- Supervisor intervention required: killed the Codex worker, reset `cas-ca12`, reassigned to a Claude worker.
