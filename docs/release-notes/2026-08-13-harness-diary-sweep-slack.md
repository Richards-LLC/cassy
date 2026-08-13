# 2026-08-13 — Harness diary sweep — #cas-internal thread

## Parent post

🧭 The compatibility picture is current through Grok Build 1.0.3, Claude Code
2.1.231, and Codex 0.147.0: upstream is improving worktree safety, MCP startup,
tool evidence, and subagent cleanup, while the newest Codex and Grok versions
remain upgrade-validation targets before their CAS pins move.

CAS users should see a healthier harness baseline without a runtime behavior
change. The diaries now distinguish direct reliability gains from integration
surfaces that still need a fresh launch matrix.

## Grok reply

**Grok Build 0.2.115–1.0.3:** tool-result history, background-task cleanup,
headless streaming, MCP image handling, bounded subagent fan-out, read-only tool
metadata, headless MCP readiness, worktree fetch safety, and hook-result display
all changed across this range.

**Verdict/action:** 🟢 take the evidence, cleanup, isolation, and observability
improvements; 👀 re-run the Grok compatibility matrix before advancing CAS’s validated
0.2.114 pin, with emphasis on `cas__*` discovery, rules/environment priming,
session/transcript identity, and permission bypass. **Source gaps:** none.

## Claude reply

**Claude Code 2.1.221–2.1.231:** print-mode and managed MCP startup, worktree and
background-hook isolation, skill precedence, cross-session delivery, workflow
fan-out, and OAuth recovery received safety and reliability fixes.

**Verdict/action:** 🟢 take the worktree, hook, lifecycle, and evidence gains; 👀
continue checking MCP startup and CAS-synced skill precedence on upgrade. **Source
gaps:** 2.1.230 has no section in Anthropic’s official changelog; no behavior is
attributed to it.

## Codex reply

**Codex stable 0.146.1–0.147.0:** the new stable adds MCP 2026-07-28 discovery
and non-blocking startup, portable plugins and imported skills, explicit project
trust, approval/network hardening, and safer automatic-review defaults.

**Verdict/action:** 👀 run a fresh Codex compatibility matrix before moving the validated
0.146.0 pin: prove `mcp__cs__*` readiness/catalog freshness, mirrored
skill/agent precedence, and non-interactive approval continuity. **Source gaps:**
none.

## POSTED

Channel: #cas-internal (C0B44GUKDK2)
Diary sweep: merged to main at 33fb46c9 (PR #283), posted 2026-08-13T13:16:08Z

| Post | UTC | Permalink |
| --- | --- | --- |
| Parent (cross-harness) | 2026-08-13T13:15:59Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786626959964829 |
| Grok reply | 2026-08-13T13:16:07Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786626967629759?thread_ts=1786626959.964829&cid=C0B44GUKDK2 |
| Claude reply | 2026-08-13T13:16:08Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786626968094259?thread_ts=1786626959.964829&cid=C0B44GUKDK2 |
| Codex reply | 2026-08-13T13:16:08Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786626968544459?thread_ts=1786626959.964829&cid=C0B44GUKDK2 |
