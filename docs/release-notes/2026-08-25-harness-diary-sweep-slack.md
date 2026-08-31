# 2026-08-25 — Harness diary sweep — #cas-internal thread

## Parent post

🧭 Cassy’s harness compatibility picture is current through Grok Build 1.0.5,
Claude Code 2.1.245, and Codex 0.149.1. Across the sweep, upstream reduced
MCP, session, worktree, hook, and tool-evidence failure modes; Cassy’s runtime
is unchanged, while the newer harnesses remain upgrade-validation targets before
their validated pins move.

Cassy users should see a healthier upstream baseline on supported launch
surfaces. The diaries separate direct reliability gains from compatibility checks
that still need a fresh matrix.

## Grok reply

**Grok Build 1.0.4–1.0.5:** session and environment identity, permission grants,
headless MCP readiness, subagent/session recovery, safe worktree reclaim, hook
diagnostics, and configuration overrides changed across this range.

**Verdict/action:** 🟢 take the lifecycle, recovery, and observability gains; 👀
keep Cassy’s validated 0.2.114 pin until a fresh complete `PtyConfig::grok`
matrix proves `CAS_SESSION_ID`/`GROK_SESSION_ID` alignment, `cas__*` discovery,
`--rules` priming, bypass-permission behavior, Grok transcript identity, and
worktree containment. **Source gaps:** none in official attribution; the local
changelog stops at 1.0.3, while xAI’s official page covers 1.0.4–1.0.5.

## Claude reply

**Claude Code 2.1.232–2.1.245:** MCP reconnect and startup, cross-session
delivery, background/subagent lifecycle, hook matching, skill/plugin reload,
worktree and path hardening, session liveness, and diagnostics improved across
the reviewed range.

**Verdict/action:** 🟢 take the host-side reliability, isolation, and evidence
gains; 👀 continue upgrade checks for MCP startup, hook routing, and Cassy
skill/agent mirror precedence. No Cassy runtime change is required. **Source
gaps:** the official `CHANGELOG.md` has no sections for 2.1.242 or 2.1.244;
no behavior is inferred for either version.

## Codex reply

**Codex stable 0.148.0–0.149.1:** async/MCP hooks and recovery, skills-loader
and `AGENTS.md` behavior, fail-closed sandboxing, the agents dashboard, SDK
config and effort controls, MCP hooks, and the rmcp 3.1.2 update changed across
the range; 0.149.1 is a patch release with no release-note body.

**Verdict/action:** 👀 keep the validated 0.146.0 pin until a fresh
`PtyConfig::codex` matrix proves `mcp__cs__*` discovery/readiness, skill/agent
and `AGENTS.md` loading, `--yolo`, model/effort config, and resume/approval
continuity. No Cassy code change is required from the notes alone. **Source
gaps:** none for 0.148.0 and 0.149.0; 0.149.1 has no release-note body, so no
item-level change is inferred.

## POSTED

Channel: `#cas-internal` (`C0B44GUKDK2`) · Posted 2026-08-31 via the approved Claude profile route (embargo lifted by the operator on 2026-08-31).

| Message | Slack ts | Permalink |
| --- | --- | --- |
| Parent | 1788180395.467059 | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788180395467059 |
| Grok reply | 1788180401.859449 | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788180401859449?thread_ts=1788180395.467059&cid=C0B44GUKDK2 |
| Claude reply | 1788180408.002849 | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788180408002849?thread_ts=1788180395.467059&cid=C0B44GUKDK2 |
| Codex reply | 1788180413.884179 | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788180413884179?thread_ts=1788180395.467059&cid=C0B44GUKDK2 |
