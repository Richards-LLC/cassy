# 2026-07-22 — Harness diary sweep — #cas-internal thread

## Parent post

🧭 The harness watch is current again: we reviewed the latest Grok, Claude Code, and Codex releases against Cassy's launch, isolation, hooks, MCP, skills, and long-session touchpoints.

The overall posture is healthy but not hands-off. Claude delivered several direct host-side safety and reliability wins; Grok and Codex introduced changes worth validating at their integration boundaries. Where upstream evidence is missing, the diary now says so instead of guessing. This is a harness compatibility and watch-ledger update—not a Cassy runtime release.

## Grok reply

**Grok 0.2.102–0.2.106:** the strongest signals are in 0.2.104–0.2.105: persistent background-work status and idle-auth recovery should help long sessions, while Grok 4.5 defaults, reasoning effort, login-shell environment loading, global-rule discovery, and compaction all touch Cassy launch assumptions. Version 0.2.106's scheduled-task and clipboard changes are separate from Cassy lifecycle and need no action.

**Verdict:** 👀 validate explicit model/effort selection, Cassy identity variables after login-shell setup, injected-rule precedence, compaction survival, and transcript/liveness evidence. The installed changelog has no 0.2.102 or 0.2.103 sections, so those releases remain explicit source gaps with no invented verdicts.

## Claude reply

**Claude Code 2.1.210–2.1.217:** this band is mostly good news for Cassy. It closes cross-worktree git escape routes, restores hook blocking and resumed-agent restrictions, adds long-tool heartbeats and bounded subagent fan-out, preserves subagent model identity, reports real completion, and improves transcript/MCP memory safety.

**Verdict:** 🟢 direct host-side isolation, hook-authority, lifecycle, and observability wins; no Cassy change required. Keep 👀 on 2.1.212's automatic backgrounding of MCP calls longer than two minutes because callers may expect a foreground result. Anthropic's official changelog has no 2.1.213 section, so that version remains a documented source gap.

## Codex reply

**Codex stable 0.145.0:** multi-agent V2 is now stable and opt-in, `/import` reaches more settings and MCP/plugin state, MCP startup/auth and tool-catalog handling were hardened, skill discovery became more concurrent, and approval/sandbox behavior tightened. The local install and latest stable now match at 0.145.0; 0.146 prereleases remain untracked.

**Verdict:** 👀 watch the integration seams, not the release number: confirm model/effort and developer instructions still govern the root session, `cs` MCP tools start with a fresh catalog, mirrored skills and agents still load, and non-interactive approval behavior remains intact. These are upgrade checks; no Cassy runtime change was needed for this diary refresh.
