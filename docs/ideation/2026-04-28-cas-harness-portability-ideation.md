---
date: 2026-04-28
topic: cas-harness-portability
focus: extend Cassy to support OpenCode and Claude Code as alternative harnesses (driver — strategic portability)
---

# Ideation: Cassy Harness Portability — OpenCode + Claude Code

## Grounding Summary

### Codebase context

Cassy is a Rust monorepo (~15 crates under `crates/` plus `cas-cli`) providing memory, tasks, rules, skills, and a multi-agent factory. It already has a *partial* harness abstraction that has never been forced to prove itself:

- `crates/cas-mux/src/harness.rs` defines `SupervisorCli` enum (`Claude`, `Codex`) with a capability matrix: `supports_hooks`, `supports_subagents`, `supports_textbox_submit`, `tool_prefix`.
- `crates/cas-mux/src/harness_policy.rs` gates verification + tool-prefix selection from that matrix.
- Factory wiring: `CAS_FACTORY_SUPERVISOR_CLI` / `CAS_FACTORY_WORKER_CLI` env vars; `--supervisor` / `--worker` CLI flags.
- The `Codex` variant is largely unused on this machine — abstraction never battle-tested under real load.

Tightly coupled to Claude Code today (no abstraction):

- **Hook protocol**: JSON stdin/stdout invocation of `cas hook <Event>`. Event names hardcoded (`SessionStart`, `Stop`, `PreToolUse`, `PostToolUse`, `UserPromptSubmit`, `SubagentStart`, `SubagentStop`, `PermissionRequest`, `Notification`, `PreCompact`, `SessionEnd`). Lives in `cas-cli/src/cli/hook.rs` + `crates/cas-core/src/hooks/types.rs`.
- **Hardcoded sync paths**: `.claude/skills/cas-*/SKILL.md` (`cas-cli/src/sync/skills.rs`), `.claude/rules/cas/` (`cas-cli/src/config/settings.rs:610`), `.claude/settings.json`, `.mcp.json`.
- **StatusLine**: Claude-Code-only feature wired through `.claude/settings.json`.
- **MCP tool prefix swap**: `mcp__cas__*` ↔ `mcp__cs__*` per harness — incomplete.
- **Three-belt permission system** for Claude Code 2.1.x deeply tied to `PreToolUse` + `PermissionRequest` hook semantics.
- **Skill format**: `.claude/skills/<name>/SKILL.md` with Claude-specific YAML frontmatter (`agent`, `allowed-tools`, `argument-hint`, context modes).

**Zero OpenCode references** in the codebase. Fresh integration.

### Past learnings (Cassy memory)

- Memory `2026-04-23-13` — Claude Code 2.1.117/2.1.118 shipped a Bun/React-Ink regression that crashed factory workers on Edit/Write to `.claude/` paths (upstream issue #52337). Three workers lost in one day. Mitigation: hand-pinned downgrade to 2.1.116. Direct evidence that single-harness coupling has real cost.
- Memory `2026-04-15-2` — Claude Code v2.1.x does NOT auto-discover per-team `settings.json`. The factory needed a three-belt permission stack (per-role `--settings`, `PreToolUse` hook returning `permissionDecision:allow`, `PermissionRequest` notification hook auto-approve). Belts 2 & 3 require Claude-specific hook surface — won't naively port.
- Memory `2026-04-15-1` — Claude Code's team permission routing intercepts BEFORE the local bypass-permissions check, so allowlists alone are insufficient.
- Memory `2026-04-13-4` — CODEMAP staleness gating fires through `PreToolUse`. Such gating is policy that lives inside the harness's hook contract today.
- Memory `2026-04-27-5` — User explicitly framed: "we are worried about claude not cursor." `.codex/` is deprioritized; OpenCode is being raised as something different from Codex/Cursor.

### Driver

User selected **strategic portability** — Cassy as a harness-agnostic "agent operating system" where the user picks the harness while the Cassy brain stays the same. Bias toward clean abstraction layers over tactical fixes.

## Ranked Ideas

### 1. `.cas/` as source of truth; `.claude/` and `.opencode/` as generated projections
**Description:** Single canonical Cassy state lives under `.cas/` (skills, rules, settings, MCP wiring, hook definitions). A `cas project` step (run inside `cas update --sync`) emits per-harness files: `.claude/skills/cas-*/SKILL.md`, `.claude/settings.json`, `.mcp.json`, OpenCode equivalents. Hand-editing emitted files is forbidden and detected via mtime-vs-source checks and a pre-commit `cas project --check` hook.
**Rationale:** Hardcoded sync paths are scattered across the codebase. Adding a second harness without consolidating doubles every edit site. With projection, "add a harness" reduces to "add an emitter."
**Downsides:**
- Authoring/debugging workflow changes — the user can't `cat .claude/settings.json` to introspect state without mentally projecting from `.cas/config.toml`.
- Drift detection is the load-bearing piece, not the projector. If detection misses, user changes vanish on next emit.
- `.mcp.json` is consumed by Claude Code at startup *before* Cassy hooks run — projector probably must run unconditionally pre-launch (in `cas update --sync`), not as a hook.
- Live deltas in current git status (`.claude/CODEMAP.md`, `.claude/settings.json`) mean migration must coexist with in-flight work.
**Confidence:** 80%
**Complexity:** Medium-High
**Grounding:** `cas-cli/src/sync/skills.rs`, `cas-cli/src/config/settings.rs:610`, `.mcp.json` writer, `cas update --sync` path
**Status:** Unexplored

### 2. Versioned Capability Manifest with graceful degradation, hosted in `cas-mux` as the abstraction home
**Description:** Promote the ad-hoc `Capabilities` struct in `crates/cas-mux/src/harness.rs` into a versioned, declarative manifest each adapter ships (TOML/JSON-schema). Fields cover hook events supported, tool-prefix convention, skill format, permission model, statusline support, MCP variant, blocklisted upstream versions, and a degradation registry per feature (e.g., `statusline: none → notify_via: log`). Refactor `cas-mux` from "PTY/process manager" into the lifecycle owner that mounts adapters, drives manifest negotiation at boot, and dispatches to it. Cassy subsystems query the manifest at runtime, never `match` on a hardcoded enum.
**Rationale:** Without a versioned public contract, the abstraction is whatever the most recent code change happens to make it — exactly how the `Codex` variant ended up unverified. The manifest is the contract; `cas-mux` becomes where that contract lives.
**Downsides:**
- Manifest schema evolution is its own commitment — once external adapters consume the schema, breaking changes mean coordinated migration. v1 must be deliberate.
- Graceful degradation is easy to declare and hard to deliver — each `none → fallback` rule is a real product decision in disguise.
- Refactoring `cas-mux` while it's actively used by the factory is high-risk — recall the 715891c → 959e69b dance which left the per-role hooks block missing for days (memory `2026-04-15-2`). This refactor is bigger.
- Promoting capability flags from compile-time gates to runtime queries means auditing every site that reads the matrix and deciding which should degrade vs refuse. That audit is the hidden bulk of the work.
**Confidence:** 75%
**Complexity:** Medium-High
**Grounding:** `crates/cas-mux/src/harness.rs:48-54`, `crates/cas-mux/src/harness_policy.rs`, factory env wiring, memory `2026-04-15-2`
**Status:** Unexplored

### 3. Conformance suite + in-process fake harness binary; harden Codex variant *before* writing the OpenCode adapter
**Description:** Two pieces, one sequencing rule. (a) `cas-conformance` crate: canonical scenarios — factory boot, supervisor → worker handoff, EPIC completion, hook-event ordering, permission round-trip, manifest claim vs actual behavior. (b) `cas-fake-harness` binary that speaks the hook protocol on a scripted timeline, drop-in for `claude` in `CAS_FACTORY_SUPERVISOR_CLI`. Then: run the suite against `SupervisorCli::Codex` until green. Treat any test passing only on Claude as a bug. Only after Codex is green begin the OpenCode adapter.
**Rationale:** Without this, ideas #1, #2, #4, #5 ship over an abstraction that has *never been forced to prove itself* — the `Codex` variant being largely unused is the proof. You'd discover every latent Claude assumption simultaneously while writing OpenCode. CI exercising the full adapter without real Claude/OpenCode binaries also decouples your release cadence from upstream availability and would have caught the 2.1.117/118 Bun/Ink regression before workers got bricked.
**Downsides:**
- The "harden Codex first" gate adds calendar weeks before OpenCode work begins. Political pressure to skip will be real.
- The fake harness binary is a real product, not a stub — must reproduce edge cases (Bun/Ink-style "tool call fires but UI silently dies", team-permission escalation race, hook event ordering). A toy fake is worse than no fake because of false confidence.
- Scenario discovery is open-ended — important scenarios appear only after a real bug exposes them. v1 must accept the suite grows for years.
- Codex hardening might surface "Codex doesn't actually work; the variant was vaporware" — pivot to using the fake harness as the second adapter. Be ready.
**Confidence:** 90%
**Complexity:** Medium-High
**Grounding:** `crates/cas-mux/src/harness.rs:5-8` (Codex variant), `crates/cas-factory/`, memory `2026-04-23-13`, memory `2026-04-15-2`
**Status:** Unexplored

### 4. Cassy-canonical hook event taxonomy with adapter translation (CHP Phase 1)
**Description:** Replace stringly-typed `cas hook <Event>` dispatch (where event names are verbatim Claude Code's: `SessionStart`, `PreToolUse`, etc.) with a typed Cassy-defined enum (`HarnessEvent`) inside `crates/cas-core/src/hooks/`. Each adapter translates its native protocol into the enum. Unmapped events are hard errors. Closed set, exhaustive match, compiler-enforced completeness. This is Phase 1 of the larger "Cassy Harness Protocol" idea — Phases 2-3 (one-MCP-server, native CASP) cut from strict survivor list as not-essential-for-v1.
**Rationale:** Today the event names match Claude Code by accident-not-design. Without this, every Cassy handler in `handlers_events/` is reading Claude's wire payload (`tool_input`, `tool_response`, `permission_mode` field names match Claude literally), so adding OpenCode means a parallel fork of every handler — and bug fixes diverge.
**Downsides:**
- Choosing the canonical taxonomy is a design decision that locks in semantics. Should `SubagentStart` and `SessionStart` be unified? Should `Notification` collapse into a generic `UserMessage`? Each choice has implications for skills, rules, and the permission engine (#5).
- `crates/cas-core/src/hooks/types.rs` `HookInput` mirrors Claude's wire format directly. Translating means every handler that touches `HookInput` gets touched. That's the entire `handlers_events/` tree. Bounded but not small.
- The hook protocol is the primary integration surface. Refactoring it during active feature work risks the kind of silent breakage commit `715891c` introduced. Sequence behind #2 so the manifest can declare which event semantics each adapter promises.
- Phase 1 alone delivers most of the portability value, but the bigger CHP vision (one MCP server, native Cassy protocol, MCP-as-adapter) may pull you back later. Choosing the smaller scope is a deliberate hedge.
**Confidence:** 80%
**Complexity:** Medium
**Grounding:** `cas-cli/src/cli/hook.rs:33-78`, `crates/cas-core/src/hooks/types.rs:9-78`, `cas-cli/src/hooks/handlers/handlers_events/`
**Status:** Unexplored

### 5. Cassy-owned permission policy engine; harnesses execute verdicts only
**Description:** All allow/deny/ask logic moves into a Cassy policy engine (consumes rules + memory + manifest capabilities). Adapter's only job: ask Cassy, get verdict, plumb it into the harness's permission gate. The Claude-Code-specific three-belt system collapses into one in-process state machine on the Cassy side. The adapter becomes a translator, not a co-author of policy.
**Rationale:** Without this, OpenCode either (a) re-implements the three-belt mess in a different shape, or (b) ships with divergent permission semantics — meaning "single source of policy" is a lie and the abstraction has a permission-shaped hole.
**Downsides:**
- The current belts work and are recently battle-tested. Memory `2026-04-15-2` describes a fix that just landed (commit `959e69b`, 2026-04-27). Touching this code while it's freshly stabilized is risky. Phased rollout: new engine emits verdicts that existing belts consume *first*, then later replace belts with a thin shim.
- The policy is bigger than it looks. `FACTORY_AUTO_APPROVE_TOOLS = [Read, Write, Edit, Glob, Grep, Bash, NotebookEdit]` lives in `pre_tool.rs:847`. CODEMAP staleness gating in `pre_tool.rs:95-116`. Team permission routing semantics. All policy. Centralizing means inventorying every site that decides anything permission-shaped — that audit is the bulk of the work.
- OpenCode's permission model may not honor verdicts. If OpenCode lacks a "consult external policy before tool call" hook, the adapter shim has nothing to plumb the verdict into. The manifest from #2 must declare `permission_decisions: yes | advisory | none` and the engine must operate in advisory mode where needed (probably via PreToolUse-equivalent), not just verdict emission.
- Most defer-able of the survivors. Skipping ships OpenCode portability faster but commits to a divergent permission code path that's painful to retroactively unify.
**Confidence:** 70%
**Complexity:** Medium-High
**Grounding:** memory `2026-04-15-2`, memory `2026-04-15-1`, memory `2026-04-13-4`, commit `959e69b`, `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs:95-116, :847`, `crates/cas-mux/src/harness_policy.rs`
**Status:** Unexplored

## Sequencing

These five aren't independent. Forced ordering by dependency:

```
#3 conformance suite + fake harness   →  prerequisite for everything else proving honest
#2 manifest in cas-mux                →  the contract; #1 #4 #5 all query it
#4 hook event taxonomy                →  inside #2's home, before #5 needs the events
#1 .cas/ projections                  →  parallel-safe with #4 once #2 lands
#5 Cassy-side permission engine         →  last; depends on manifest declaring permission model
```

Fastest honest path: ship #3's conformance scaffolding first (against existing `Codex` variant), then #2 + #4 in parallel, then #1, then #5.

## Rejection Summary

### Rejected in adversarial pass (11)

| # | Idea | Reason Rejected |
|---|------|-----------------|
| 1 | Inverted long-lived event broker (kill per-event spawn cost) | Depends on CHP wire protocol; better as follow-on inside a future CHP Phase 2 |
| 2 | Test-derived capability matrix (capabilities measured not declared) | Cute but mechanism is fragile; better as a property of #3's conformance suite — manifest claims validated by tests |
| 3 | CHP-driven factory daemon multiplexes sessions | Strictly downstream of CHP wire protocol; not realizable without it |
| 4 | Capability-gated `requires_capability:` blocks in skills | Falls out of #1 + #2 once they exist; feature, not foundation |
| 5 | Cassy-owned subagent runtime (harness-agnostic spawning) | High value but orthogonal to portability driver; revisit only if OpenCode subagent gap proves blocking |
| 6 | Multi-harness role/task routing (supervisor on Claude, workers on OpenCode) | User chose "one or the other" — closes mesh option for v1; premature optimization |
| 7 | Cassy-native TUI, harnesses as headless backends | Too expensive for chosen driver; solves a problem user hasn't expressed; competes with CC's existing TUI |
| 8 | Delete statusLine entirely | Cosmetic-but-load-bearing (user glances at it daily); falls under #2's degradation registry |
| 9 | StatusLine as harness-agnostic daemon stream | Same — single feature, falls under #2's degradation registry |
| 10 | `cas harness migrate / use` swap tool | Falls out of #1 naturally as two projector passes; double-billing |
| 11 | Harness-leakage UX cleanup in user-facing strings | Polish, not strategic; checklist item once #1 + #4 land |

### Rejected in strict pass (3 cuts from initial survivor list)

| # | Idea | Reason Rejected |
|---|------|-----------------|
| 12 | `cas doctor --harness` + crash-signature version blocklist | Operationally valuable but tactical, not essential. Cleanly defers as Phase-2 hygiene; manifest in #2 already provides the version field it needs. Memory `2026-04-23-13` follow-up still standing. |
| 13 | Adapter SDK extraction (cargo-generate template, public traits, derive macros) | Premature platform play — risks freezing wrong abstraction. Defers cleanly until #3 has run conformance against two real adapters. The `cas-mux` refactor part survived as the home for #2's manifest. |
| 14 | CHP wire-protocol Phases 2-3 (one MCP server, namespace overhaul, native Cassy protocol) | Boldest vision but not necessary for portability v1. Phase 1 (now #4) captures most of the leverage. Reconsider after #1-#5 land. |

## Session Log

- 2026-04-28: Initial ideation. Driver chosen: strategic portability. 41 raw candidates from 4 parallel ideation agents (frames: pain/friction, inversion/removal, assumption-breaking, leverage/compounding) → 18 distinct after dedupe → 7 survivors after adversarial pass → 5 strict survivors after second-pass bar-raise. Cuts in second pass: `cas doctor --harness`, Adapter SDK extraction, CHP Phases 2-3. No idea selected for brainstorming yet.
