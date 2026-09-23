# Grok Build Changelog Diary — Cassy Response Ledger

A living, **newest-first** ledger of xAI Grok Build CLI releases and how Cassy
responded to each. Sibling to `claude-code-changelog-diary.md` and
`codex-changelog-diary.md` — Cassy supports three harnesses (`cli=claude` /
`cli=codex` / `cli=grok`, EPIC cas-8888), so we track Grok drift too.

Grok ships a local changelog at `~/.grok/CHANGELOG.md` (and a flat item list at
`~/.grok/CHANGELOG.json` for the current install). Unlike Claude Code (upstream
GitHub CHANGELOG) or Codex (GitHub releases), the installable history on a host
may only cover the versions present on that binary's changelog surface.

## How to update

When a new Grok Build version ships (or after `grok` upgrades on the host):

1. Confirm the binary: `grok --version` (example: `grok 0.2.101 (… ) [stable]`).
2. Read the local changelog: `~/.grok/CHANGELOG.md` (version sections) and, if
   useful, `~/.grok/CHANGELOG.json` (flat feature/fix list for the current install).
3. Verdict each user-facing item against the **Cassy ↔ Grok touchpoints** below.
   Most TUI polish is `⏭ n/a`. Prefer items that touch permissions, session IDs,
   rules/system prompt, MCP discovery, env inheritance, transcripts, or hooks.
4. Add a newest-first entry + index row. File a Cassy task only when work is required.
5. **Version gap matters:** keep **validated pin** (pty.rs comment) vs **locally
   installed** vs **latest in changelog** honest. Do not invent older releases —
   if the local changelog only has N versions, seed those N and mark the **seed floor**.
6. After the diary update merges, publish the mandatory shared **#cas-internal**
   harness thread: one parent plus exactly three replies ordered **Grok, Claude,
   Codex**. Follow [the release Slack rubric](../RELEASE_SLACK_RUBRIC.md), including
   its version-range, verdict/action, source-gap, and no-internal-narration rules.

**Verdict legend:** ✅ no action · 🟢 already covered · 👀 watch (touches a Cassy
dependency, verify on upgrade) · 🔧 fix shipped · 🏗 EPIC · ⏭ n/a

## Version status

- **Cassy validated against:** Grok Build **1.0.40**
  (`grok 1.0.40 (eb1a2256660d) [stable]`), verified live 2026-09-23 through
  the complete isolated `PtyConfig::grok` worker matrix, including the urgent
  interrupt redirect, and recorded in the typed
  `grok-build-1.0.40-2026-09-23` conformance receipt. The prior
  `grok-build-1.0.5-2026-08-25` and `grok-build-0.2.114-2026-07-30` receipts
  remain historical evidence.
- **2026-09-23 operator decision:** Defer the audit gaps for MCP discovery health, busy-worker message delivery, compaction survival, consent popups, background liveness, and memory; these are not tracked.
- **Locally installed and latest stable:** **1.0.41** (`grok 1.0.41
  (4220f3b224a6) [stable]`, checked 2026-09-23). The exact 1.0.40 binary
  remains retained and is the binary named by the validation receipt. Wingetly
  independently lists the 1.0.40 package among shipped versions:
  [Grok Build versions](https://wingetly.io/apps/x-ai/grok-build).
- **Latest release-note evidence:** the local `~/.grok/CHANGELOG.md` snapshot
  provides versioned sections for **1.0.6–1.0.13**. Its companion
  `~/.grok/CHANGELOG.json` is a flat item list with no version/date attribution.
  Releasebot's [Grok Build feed](https://releasebot.io/updates/xai/grok-build),
  curated from xAI and updated 2026-09-18, provides per-version notes for
  **1.0.17–1.0.25** and **1.0.30–1.0.34**. xAI's official [Grok Build
  changelog](https://x.ai/build/changelog) still shows **0.2.117** as its latest
  release-note page; direct curl is Cloudflare-protected (HTTP 403 on
  2026-09-23), so that page remains the 0.2.117 source rather than evidence for
  newer 1.0.x releases.
- **Validated-version source gap:** local notes cover 1.0.6–1.0.13 and
  Releasebot covers 1.0.17–1.0.25 plus 1.0.30–1.0.34;
  1.0.14–1.0.16, 1.0.26–1.0.29, and 1.0.35–1.0.41 are consolidated below as
  a per-version source gap. The 1.0.40 receipt explicitly records the
  delegated 1.0.17 MCP-input, 1.0.8 consent-popup, 1.0.34 memory, and
  0.2.105 compaction checks as not covered by this bounded non-interactive
  matrix. The earlier 0.2.102–0.2.103 and 0.2.107–0.2.111 gaps remain
  documented below.

## Cassy ↔ Grok touchpoints (what a release can break)

The load-bearing surface is `crates/cas-pty/src/pty.rs::PtyConfig::grok` (approx.
lines 423–580; instructions constants near top of file). Ground truth is that
code + its "Verified against … 0.2.114" block — re-read it on upgrade rather than
trusting this diary alone.

### CLI flags (spawn args)

- **`--permission-mode bypassPermissions`** — factory workers skip interactive
  approval (Grok's analogue of Claude's bypass / Codex's `--yolo`). Any rename,
  removal, or semantic narrowing of `bypassPermissions` breaks unattended workers.
- **`--session-id <uuid>`** — fresh UUID per *new* conversation (anti-overwrite
  model, same family as Claude; not Codex's `codex-<name>-<uuid>` prefix). Phase 4
  transcript resolution keys on this exact value. Doc comment also notes short form
  `-s/--session-id` on the validated 0.2.114 binary.
- **`-m` / `--model <MODEL>`** — optional model pin when the factory requests one.
- **`--reasoning-effort <EFFORT>`** (alias `--effort` on the verified binary) —
  vocabulary minimal/low/medium/high/xhigh via `Effort::as_claude_arg()` (no
  separate `as_grok_arg`). Any "reasoning effort" changelog line is a 👀.
- **`--cwd <path>`** — worktree/working directory for the worker process.
- **`--rules <text>`** — **"Extra rules to append to the system prompt."** This is
  the load-bearing context path for factory role priming. Cassy injects
  `GROK_WORKER_INSTRUCTIONS` or `GROK_SUPERVISOR_INSTRUCTIONS` (same file). Grok's
  **SessionStart hook fires but its stdout is ignored** (delta #2) — do not assume
  Claude-style SessionStart `additionalContext` delivery on Grok.

### MCP discovery (no per-spawn `-c` override)

- Grok has **no** ephemeral per-launch MCP config flag analogous to Codex
  `-c mcp_servers.*`. Servers come from persistent discovery:
  project `.mcp.json`, `~/.claude.json`, and/or `~/.grok/config.toml`
  (`grok mcp add` writes the latter).
- Tools are namespaced **`cas__*`** on Grok (e.g. `cas__task`, `cas__coordination`)
  — **not** `mcp__cas__*` and **not** Codex's `mcp__cs__*` / `cs` prefix. Worker and
  supervisor `--rules` text must keep that prefix honest.
- Identity for `cas serve` rides ordinary **child-process env inheritance** from the
  grok process (same pattern as Claude; no `mcp_servers.*.env` TOML block).

### Process env (set on the grok child)

At minimum, `PtyConfig::grok` sets:

- `CAS_AGENT_NAME`, `CAS_AGENT_ROLE`
- `CAS_FACTORY_MODE=1` (verification-jail / factory exemptions)
- `CAS_SESSION_ID` — same UUID as `--session-id`; load-bearing identity when hooks
  cannot deliver SessionStart context
- `CAS_CLONE_PATH`, optional `CAS_ROOT`, optional `CAS_SUPERVISOR_NAME`
- `CAS_FACTORY_WORKER_CLI=grok` — **unconditional** on a grok process (cas-921f);
  required so harness-aware liveness looks under Grok paths, not Claude's
- Plus shared factory metadata / cargo / zig env helpers used by other CLIs

### Transcripts / liveness

- Grok session transcripts live under **`~/.grok/sessions/*`**, not
  `~/.claude/projects/*`. If `CAS_FACTORY_WORKER_CLI` is wrong, is-wedged/liveness
  globs the Claude tree and always resolves `None` for a real grok worker.

### Hooks posture (contrast Claude)

- Claude path: SessionStart / PreToolUse are load-bearing.
- Grok path: SessionStart stdout ignored → **`--rules` + env** carry factory
  identity and role text. Changelog lines about "hooks disabled at session start"
  or hook config are still 👀 (config surface), but do not restore Claude-style
  stdout injection unless Grok documents a behavior change.

## Index

| Grok version | Headline | Cassy verdict | Pointer |
| --- | --- | --- | --- |
| 1.0.14–1.0.16, 1.0.26–1.0.29, 1.0.35–1.0.41 | No per-version notes in checked feeds | — (source gap) | this doc |
| 1.0.34 | Memory generally available · Markdown heading colors | 👀 / ⏭ | this doc |
| 1.0.33 | Structured MCP results · cancellation/session/subagent recovery · clone/skill fixes | 👀 / 🟢 | this doc |
| 1.0.32 | Pre-session config listing · first-session crash/TLS fixes | 👀 / ⏭ | this doc |
| 1.0.31 | MCP prefix retention · worktree path/scrollback/dashboard fixes | 👀 / 🟢 | this doc |
| 1.0.30 | Session timing · workflow status · tmux lag | 👀 / ⏭ | this doc |
| 1.0.25 | Hook silence · headless timeout · MCP/session/workflow fixes | 👀 / 🟢 | this doc |
| 1.0.24 | Esc no longer cancels a running turn | 🟢 already covered | this doc |
| 1.0.23 | Wrapped URL/email links | ⏭ | this doc |
| 1.0.22 | MCP precedence · permission diffs · session/subagent/workflow reliability | 👀 / 🟢 | this doc |
| 1.0.21 | Permission mode/session welcome-screen fixes | 👀 / ⏭ | this doc |
| 1.0.20 | Scroll-history viewport fix | ⏭ | this doc |
| 1.0.19 | MCP policy/session resume · headless worktree · background lifecycle | 👀 / 🟢 | this doc |
| 1.0.18 | Background hooks/startup · auth/retry · subagent/session reliability | 👀 / 🟢 | this doc |
| 1.0.17 | Mid-call MCP input and resume | 👀 | this doc |
| 1.0.13 | Response continuation · hook decisions/context · retry and MCP startup reliability | 👀 / 🟢 | this doc |
| 1.0.12 | MCP retries · interjection/subagent waits · context and recap correctness | 👀 / ✅ | this doc |
| 1.0.11 | Headless resume/permissions · session history · background wait correctness | 👀 / 🟢 | this doc |
| 1.0.10 | Linked-worktree reuse for `grok clone` | 👀 | this doc |
| 1.0.9 | MCP identity · linked worktrees · rules/workflow/subagent reliability | 👀 / 🟢 | this doc |
| 1.0.8 | MCP consent forms · workflow/subagent follow-up correctness | 👀 / ✅ | this doc |
| 1.0.7 | Startup timeout · MCP auth · permission persistence · workflow catalog | 👀 / 🟢 | this doc |
| 1.0.6 | Capability-mode removal · session startup · projected clone trees | 👀 | this doc |
| 1.0.5 | Config overrides · safe worktree reclaim · hook/session/tool robustness | ✅ | this doc |
| 1.0.4 | `GROK_SESSION_ID` · permission grants · hook/subagent/worktree/session recovery | 👀 / 🟢 | this doc |
| 1.0.3 | Faster subagent spawning · session-info and high-refresh TUI polish | 🟢 / ⏭ | this doc |
| 1.0.2 | Startup diagnostics · worktree fetch safety · hook/tool presentation | 🟢 / 👀 | this doc |
| 1.0.1 | Bounded subagents · read-only tool metadata · MCP/headless lifecycle | 👀 / 🟢 | this doc |
| 1.0.0 | MCP image reliability · permission visibility · session/task lifecycle | 👀 / 🟢 | this doc |
| 0.2.119 | Bash allow-list, plan/task, auth, and startup reliability | 👀 / ✅ | this doc |
| 0.2.118 | Session controls · doctor/compact · background-task correctness | ✅ / ⏭ | this doc |
| 0.2.117 | TLS roots · background-subagent stop · ACP task reliability | 👀 / 🟢 | this doc |
| 0.2.116 | Headless streaming JSON · undo · token-refresh reliability | 👀 / ✅ | this doc |
| 0.2.115 | Tool-result history correctness · prompt-cache reliability | 🟢 direct win | this doc |
| 0.2.114 | Session deletion · no-free-thread startup crash fix · full Cassy factory matrix | ✅ | this doc |
| 0.2.113 | MCP enable/disable · SessionEnd · auth/process/session reliability · instant cold start | 👀 / ✅ / ⏭ | this doc |
| 0.2.112 | Version policy · env/provider config · session/resume/transcripts · MCP/subagents · hooks/workflows/background lifecycle | 👀 / ✅ / ⏭ | this doc |
| 0.2.111 | Missing from available versioned local changelog evidence | — (no attributable evidence) | this doc |
| 0.2.110 | Missing from available versioned local changelog evidence | — (no attributable evidence) | this doc |
| 0.2.109 | Missing from available versioned local changelog evidence | — (no attributable evidence) | this doc |
| 0.2.108 | Missing from available versioned local changelog evidence | — (no attributable evidence) | this doc |
| 0.2.107 | Missing from available versioned local changelog evidence | — (no attributable evidence) | this doc |
| 0.2.106 | Clipboard fallback/env opt-out · scheduled tasks become background commands · minimal-mode highlighting | ✅ / ⏭ | this doc |
| 0.2.105 | Grok 4.5 defaults/effort + compaction · login-shell env · global rules discovery · MCP OAuth · background lifecycle/fleet roster | 👀 / ✅ / ⏭ | this doc |
| 0.2.104 | Persistent background-work status · idle auth recovery · error/rate-limit copy · prompt editing | 👀 / ⏭ | this doc |
| 0.2.103 | Missing from installed local changelog | — (no evidence) | this doc |
| 0.2.102 | Missing from installed local changelog | — (no evidence) | this doc |
| 0.2.101 | **grok inspect** multi-harness compatibility settings · TUI refresh cadence · queue/status/subagent polish · rate-limit copy | 👀 / ✅ / ⏭ | this doc |
| 0.2.100 | **Session picker + welcome resume** across Claude/Codex/Cursor · web-fetch artifacts · queue/multiline Enter · pane-closed resume crash · hooks honor disabled-at-start · long-turn status markers | 👀 / ✅ / ⏭ | this doc |
| *(seed floor)* | No evidence-backed versions before 0.2.100 in retained host snapshots | — | — |

---

## Entries

### 1.0.14–1.0.16, 1.0.26–1.0.29, 1.0.35–1.0.41 — consolidated release-note source gap

Reviewed 2026-09-23. The checked sources are the local versioned
`~/.grok/CHANGELOG.md` (through 1.0.13), its flat/unversioned
`~/.grok/CHANGELOG.json`, the [Releasebot Grok Build feed](https://releasebot.io/updates/xai/grok-build)
(per-version entries through 1.0.34, updated 2026-09-18), the [official xAI
changelog](https://x.ai/build/changelog) (currently showing 0.2.117), and
[Wingetly's package history](https://wingetly.io/apps/x-ai/grok-build) (which
lists 1.0.40 and 1.0.34 among shipped versions). No per-version release notes
were attributable to these ranges. Direct curl to x.ai returned HTTP 403 from
Cloudflare on this review date. → — **source gap; no release behavior or Cassy
verdict is inferred.** The exact 1.0.40 binary is validated by the separate
`grok-build-1.0.40-2026-09-23` receipt; 1.0.41 is installed but has no
corresponding release-note evidence or validation receipt.

### 1.0.34 — memory generally available · Markdown heading colors

Reviewed 2026-09-23. Source: [Releasebot's per-version Grok Build feed](https://releasebot.io/updates/xai/grok-build),
which attributes the **1.0.34 — 2026-09-16** notes to xAI.

- **Memory is generally available.** → 👀 **watch — process/session context.**
  Cassy memory and Grok's model-context memory are separate authorities; verify
  that `--rules`, inherited `CAS_*` identity, and the `cas__*` MCP discovery
  contract remain present when memory is enabled.
- **Markdown headings receive theme colors correctly.** → ⏭ **n/a** (display
  only; no Cassy launch or transcript contract change).

### 1.0.33 — structured MCP results · cancellation/session/subagent recovery

Reviewed 2026-09-23. Source: [Releasebot's per-version Grok Build feed](https://releasebot.io/updates/xai/grok-build),
which attributes the **1.0.33 — 2026-09-15** notes to xAI.

- **MCP results can include structured JSON; direct MCP rows show names,
  arguments, and errors; cancelled or timed-out MCP calls stop.** → 👀 / 🟢
  **MCP observability and lifecycle wins.** Cassy still requires persistent
  `cas__*` discovery and remains authoritative for tool/task state; the next
  matrix should verify that structured results do not alter receipt parsing.
- **Background subagents report cancellation correctly; old compacted history is
  retained; rewind/session recovery and scheduled-task recap behavior improve.**
  → 🟢 / 👀 **lifecycle and transcript wins; retain session-path watch.** Grok
  transcripts remain under `~/.grok/sessions/*` keyed by Cassy's injected UUID.
- **Skill-size truncation notes and Windows clone fixes** → ⏭ **n/a** for this
  Linux launch contract, apart from the favorable diagnostic behavior.

### 1.0.32 — pre-session config listing · first-session crash/TLS fixes

Reviewed 2026-09-23. Source: [Releasebot's per-version Grok Build feed](https://releasebot.io/updates/xai/grok-build),
which attributes the **1.0.32 — 2026-09-14** notes to xAI.

- **Plugin and skill listings reflect current `config.toml` before the first
  session.** → 👀 **watch — prompt/config layering.** Cassy's explicit
  `--rules`, inherited env, and persistent `cas__*` discovery must not be
  displaced by the earlier config read.
- **Windows ARM64 TLS first-handshake crashes and `/feedback` during plan
  approval are fixed.** → ⏭ **n/a** for this Linux factory path and Cassy
  unattended launch.

### 1.0.31 — MCP prefix retention · worktree path/scrollback/dashboard fixes

Reviewed 2026-09-23. Source: [Releasebot's per-version Grok Build feed](https://releasebot.io/updates/xai/grok-build),
which attributes the **1.0.31 — 2026-09-13** notes to xAI.

- **MCP tools with long server prefixes are no longer silently dropped.** → 🟢
  **discovery reliability win.** This supports Cassy's persistent `cas__*`
  namespace; still verify the complete tool set in the 1.0.40 matrix.
- **Worktree headers no longer add a suffix; folded subagent scrollback labels
  running/completed counts correctly.** → 👀 / 🟢 **worktree/lifecycle watch and
  observability win.** UI labels do not replace Cassy's branch, lease, or
  `CAS_CLONE_PATH` authority.
- Dashboard search, cancellation copy, and long-path shortening → ⏭ **n/a** for
  the unattended launch contract.

### 1.0.30 — session timing · workflow status · tmux lag

Reviewed 2026-09-23. Source: [Releasebot's per-version Grok Build feed](https://releasebot.io/updates/xai/grok-build),
which attributes the **1.0.30 — 2026-09-11** notes to xAI.

- **Session headers/timing, watcher/loop due times, workflow status, and tmux
  pane lag improve.** → 👀 / ⏭ **diagnostic watch; otherwise n/a.** Cassy
  liveness uses Grok transcript files and `CAS_FACTORY_WORKER_CLI=grok`, not
  TUI timing or workflow rows; no launch flag change is documented.

### 1.0.25 — hook silence · headless timeout · MCP/session/workflow fixes

Reviewed 2026-09-23. Source: [Releasebot's per-version Grok Build feed](https://releasebot.io/updates/xai/grok-build),
which attributes the **1.0.25 — 2026-09-09** notes to xAI.

- **Successful hook runs are silent; only blocking/failing hooks show status.**
  → 👀 **watch — hook posture.** Cassy does not depend on Grok SessionStart
  stdout; `--rules` plus inherited env remain the role/identity path. Verify
  that silent output does not suppress a blocking hook signal needed for cleanup.
- **Headless prompts time out cleanly, `grok -c` avoids attaching the wrong
  empty session, and startup settings no longer clobber across concurrent boots.**
  → 👀 / 🟢 **session/config wins with launch watch.** Cassy has no ephemeral
  `-c` MCP override, so persistent `cas__*` discovery and injected
  `--session-id` remain the contract to recheck.
- Workflow stop/pause, scheduled-task reminders, complete shell output, upload,
  feedback, and dashboard fixes → 🟢 / ⏭ **operational wins or interactive UX;
  no Cassy code action indicated.**

### 1.0.24 — Esc no longer cancels a running turn

Reviewed 2026-09-23. Source: [Releasebot's per-version Grok Build feed](https://releasebot.io/updates/xai/grok-build),
which attributes the **1.0.24 — 2026-09-07** note to xAI.

- **Esc no longer cancels a running turn and instead reminds users to use
  Ctrl+C.** → 🟢 **already covered.** Cassy's urgent interrupt-and-redirect
  calls `Pane::break_turn` (`crates/cas-mux/src/pane/mod.rs:1462`), which
  writes the harness's own cancel bytes; the Grok backend returns Ctrl+C
  (`0x03`), not Esc (`crates/cas-mux/src/backend/grok.rs:70-72`, pinned by
  `crates/cas-mux/src/harness.rs:293-296`, since cas-7f6f). Live confirmation
  against 1.0.40 passed in **cas-ef93** and is recorded in the typed
  `grok-build-1.0.40-2026-09-23` receipt.

### 1.0.23 — wrapped URL/email links

Reviewed 2026-09-23. Source: [Releasebot's per-version Grok Build feed](https://releasebot.io/updates/xai/grok-build),
which attributes the **1.0.23 — 2026-09-07** note to xAI. → ⏭ **n/a**
(pager rendering only; no launch, MCP, env, hook, or transcript change).

### 1.0.22 — MCP precedence · permission diffs · session/subagent/workflow reliability

Reviewed 2026-09-23. Source: [Releasebot's per-version Grok Build feed](https://releasebot.io/updates/xai/grok-build),
which attributes the **1.0.22 — 2026-09-07** notes to xAI.

- **Built-in agent tools take precedence over user MCP servers when names
  collide; MCP connectors show re-authentication state; opening/resuming sessions
  no longer pauses while reading config.** → 👀 **watch — MCP discovery/config
  layering.** Cassy's persistent server must remain discoverable as `cas__*`, and
  its names must not be shadowed by a Grok built-in.
- **Subagent continuation, background completion output, resumed mid-turn text,
  and monitor wakeups are more reliable.** → 🟢 / 👀 **lifecycle wins; retain
  transcript and subagent watch.** Grok-owned subagents do not replace Cassy
  workers or leases.
- **Permission diffs expand automatically, auto mode refuses destructive
  checkout commands, parent bash/background settings propagate, and foreground
  commands report backgrounding.** → 👀 / 🟢 **permission and process wins.**
  Cassy's explicit `bypassPermissions` and factory scope remain authoritative.
- Skills refresh, workflow/queue/session UI, rewind recovery, and copy changes →
  ⏭ / 🟢 **interactive or operational improvements; no code action indicated.**

### 1.0.21 — permission mode/session welcome-screen fixes

Reviewed 2026-09-23. Source: [Releasebot's per-version Grok Build feed](https://releasebot.io/updates/xai/grok-build),
which attributes the **1.0.21 — 2026-09-04** notes to xAI.

- **Permission mode stays visible in plan mode and is restored on exit.** → 👀
  **watch — permission semantics.** Cassy still passes
  `--permission-mode bypassPermissions`; the 1.0.40 matrix must confirm mode
  persistence does not reintroduce an interactive gate.
- Welcome-screen and crowded-terminal layout fixes → ⏭ **n/a** for unattended
  worker launch.

### 1.0.20 — scroll-history viewport fix

Reviewed 2026-09-23. Source: [Releasebot's per-version Grok Build feed](https://releasebot.io/updates/xai/grok-build),
which attributes the **1.0.20 — 2026-09-04** note to xAI. → ⏭ **n/a**
(interactive scrollback only).

### 1.0.19 — MCP policy/session resume · headless worktree · background lifecycle

Reviewed 2026-09-23. Source: [Releasebot's per-version Grok Build feed](https://releasebot.io/updates/xai/grok-build),
which attributes the **1.0.19 — 2026-09-04** notes to xAI.

- **`/loop` always runs in the background; organization-blocked MCP servers are
  refused before config changes; MCP connects in the background after login.**
  → 👀 / 🟢 **MCP/policy and startup wins.** Cassy still needs persistent
  `cas__*` discovery, and a policy-blocked server must surface as a real
  preflight failure rather than look like a missing tool.
- **Session resume reports active loops/subagents/workflows; headless sessions
  support `--worktree`; dashboard dispatch and terminal-close behavior improve.**
  → 👀 **session/worktree watch.** Factory worktrees remain Cassy-managed, and
  liveness remains keyed to the injected UUID under `~/.grok/sessions/*`.
- Transparent themes, feedback, URL wrapping, and dashboard copy → ⏭ **n/a**
  outside the launch contract.

### 1.0.18 — background hooks/startup · auth/retry · subagent/session reliability

Reviewed 2026-09-23. Source: [Releasebot's per-version Grok Build feed](https://releasebot.io/updates/xai/grok-build),
which attributes the **1.0.18 — 2026-09-02** notes to xAI.

- **New sessions prepare in the background; start hooks run in the background;
  startup no longer hangs on slow networks; responses/session bookkeeping run
  asynchronously.** → 👀 **watch — hooks/startup/transcript timing.** Grok's
  SessionStart stdout remains non-authoritative for Cassy; `--rules`, env, and
  transcript registration must still complete before a worker is considered
  live.
- **Authentication recovery and configurable rate-limit retries improve;
  subagent sessions inherit selected-model retry settings.** → 🟢 / 👀
  **reliability win; retain subagent/process watch.** Cassy leases and model
  selection remain separate from Grok-native subagent behavior.
- Feedback, chart/image, composer, picker, and shortcut fixes → ⏭ **n/a** for
  Cassy's unattended launch contract.

### 1.0.17 — mid-call MCP input and resume

Reviewed 2026-09-23. Source: [Releasebot's per-version Grok Build feed](https://releasebot.io/updates/xai/grok-build),
which attributes the **1.0.17 — 2026-09-01** notes to xAI.

- **MCP tools can request additional input mid-call and resume when answered.**
  → 👀 **watch — non-interactive MCP behavior.** Cassy's factory server and
  worker rules must not require an unanswered interactive prompt; verify the
  headless path in the separate **cas-ef93** matrix.
- Ghost-suggestion and `/btw` presentation fixes → ⏭ **n/a** (interactive UI).

### 1.0.13 — response continuation · hook decisions/context · retry and MCP startup reliability

Reviewed 2026-09-23. Source: the versioned **1.0.13 — 2026-08-28** section in
`~/.grok/CHANGELOG.md`; the flat `~/.grok/CHANGELOG.json` supplies no
independent version attribution.

- **Length-truncated responses and complete truncated tool calls continue or
  execute; transient inference failures retry; session data saves more reliably.**
  → 🟢 / 👀 **reliability wins; retain transcript/liveness watch.** These are
  favorable for unattended workers, but do not change Cassy's fresh session UUID,
  Grok transcript path, or coordination authority.
- **Hooks can request confirmation, deferral, or post-tool model context; session
  close records timing data.** → 👀 **hook and transcript watch.** This expands
  Grok's hook result surface, while Cassy still carries worker identity and role
  through `--rules` plus inherited env; SessionStart stdout remains ignored.
- **MCP startup is faster with configured auth and no longer stalls behind a
  fixed batch size; subagent spawning recovers faster after connection drops.**
  → 🟢 **operational wins** for persistent `cas__*` discovery and Grok-owned
  subagents; Cassy's own leases and concurrency remain authoritative.
- Scheduled-task reminders/UUIDs, image previews, Windows behavior, and pager
  links → ⏭ **n/a** for the Cassy launch contract.

### 1.0.12 — MCP retries · interjection/subagent waits · context and recap correctness

Reviewed 2026-09-23. Source: the versioned **1.0.12 — 2026-08-27** section in
`~/.grok/CHANGELOG.md`; the flat `~/.grok/CHANGELOG.json` supplies no
independent version attribution.

- **Transient MCP connection failures retry; waiting on subagents after an
  interjection no longer blocks on unrelated background work.** → 👀 / 🟢
  **discovery and lifecycle watch/win.** This should make unattended turns more
  reliable, but the next matrix must still confirm persistent `cas__*` discovery
  and that Cassy remains the source of worker state.
- **Hook descriptions are friendlier; context/token estimates, compaction
  progress, and auto-recap timing are corrected.** → ✅ / 👀 **diagnostic wins;
  retain transcript watch.** No `PtyConfig::grok` flag or env contract changes
  are attributed.
- Wrapped table copying, filesystem watcher load, and faster worktree creation
  → ⏭ **n/a** (interactive or performance-only behavior; no new worktree
  authority is documented).

### 1.0.11 — headless resume/permissions · session history · background wait correctness

Reviewed 2026-09-23. Source: the versioned **1.0.11 — 2026-08-26** section in
`~/.grok/CHANGELOG.md`; the flat `~/.grok/CHANGELOG.json` supplies no
independent version attribution.

- **Headless sessions appear in the resume picker, can auto-allow permission
  prompts with a startup hint, and new-session default permission mode is
  configurable.** → 👀 **permission/session watch.** Cassy explicitly passes
  `--permission-mode bypassPermissions`; the 1.0.40 matrix must confirm this
  still bypasses prompts and does not alter `CAS_SESSION_ID` transcript lookup.
- Session history footers, resume duration, blocked-prompt scrollback, image
  previews, and terminal input fixes → ⏭ **n/a** for the launch contract.
- **Background command waits finish when the process exits; common command-chain
  permission prompts are more reliable.** → 🟢 / 👀 **operational win; retain
  approval watch.** Cassy task leases and tool scope remain independent.

### 1.0.10 — linked-worktree reuse for `grok clone`

Reviewed 2026-09-23. Source: the versioned **1.0.10 — 2026-08-24** section in
`~/.grok/CHANGELOG.md`; the flat `~/.grok/CHANGELOG.json` supplies no
independent version attribution.

- **`grok clone` reuses matching local checkouts as linked worktrees.** → 👀
  **watch — worktree containment.** This is a Grok-owned optimization and does
  not authorize reclaiming Cassy-managed checkouts, branches, or active factory
  leases; validate path and branch isolation in the separate **cas-ef93** matrix.

### 1.0.9 — MCP identity · linked worktrees · rules/workflow/subagent reliability

Reviewed 2026-09-23. Source: the versioned **1.0.9 — 2026-08-24** section in
`~/.grok/CHANGELOG.md`; the flat `~/.grok/CHANGELOG.json` supplies no
independent version attribution.

- **MCP servers receive a Grok CLI User-Agent; MCP startup and concurrent
  subagent bursts are more reliable.** → 🟢 / 👀 **operational win; discovery
  watch.** The User-Agent does not replace the persistent `cas__*` namespace or
  Cassy's inherited identity; verify both on upgrade.
- **`grok clone` can fetch a branch tip and reuse a matching checkout as a
  linked worktree.** → 👀 **watch — worktree containment.** Cassy still owns its
  factory branch and worktree lifecycle.
- Markdown headings in rules render correctly; workflow prompts, plugin agents,
  and subagent tool visibility are corrected. → 👀 / ✅ **prompt-layer watch;
  lifecycle win.** Explicit `--rules` remains the load-bearing factory context
  path, and Grok-owned workflow agents are not Cassy workers.
- Interactive mode, slash menus, editor behavior, and display changes → ⏭
  **n/a** for the unattended Cassy launch path.

### 1.0.8 — MCP consent forms · workflow/subagent follow-up correctness

Reviewed 2026-09-23. Source: the versioned **1.0.8 — 2026-08-20** section in
`~/.grok/CHANGELOG.md`; the flat `~/.grok/CHANGELOG.json` supplies no
independent version attribution.

- **MCP servers can request form input or URL consent through the question
  popup.** → 👀 **watch — MCP/permission surface.** Cassy's factory server is
  discovered persistently as `cas__*` and runs non-interactively; the matrix
  must ensure this new interactive consent path cannot block a worker.
- **Follow-up messages send immediately while waiting; subagents and workflow
  agents no longer see the top-level workflow tool.** → 🟢 / ✅ **lifecycle and
  isolation wins.** Cassy coordination remains the authority for worker messages
  and leases.
- Prompt draft stashing, archive/download behavior, and status-line rendering
  → ⏭ **n/a** for the launch contract.

### 1.0.7 — startup timeout · MCP auth · permission persistence · workflow catalog

Reviewed 2026-09-23. Source: the versioned **1.0.7 — 2026-08-19** section in
`~/.grok/CHANGELOG.md`; the flat `~/.grok/CHANGELOG.json` supplies no
independent version attribution.

- **`GROK_CONNECT_UI_TIMEOUT_SECS` can raise the startup connect budget; tokenless
  MCP servers no longer require authentication in non-interactive sessions.**
  → 👀 / 🟢 **launch/discovery watch and direct reliability win.** Cassy does
  not currently set the timeout variable; confirm headless startup and
  persistent `cas__*` discovery in the 1.0.40 matrix.
- **Permission prompts add persistent Always/Never choices for MCP tools and
  web-fetch domains.** → 👀 **permission watch.** Cassy's explicit
  `bypassPermissions` mode must remain authoritative for unattended workers.
- Auth-refresh startup races, repeated tool-call loops, subagent access to the
  question tool, workflow catalog, and mail links → 🟢 / ⏭ **reliability wins or
  interactive UX outside Cassy's launch contract.**

### 1.0.6 — capability-mode removal · session startup · projected clone trees

Reviewed 2026-09-23. Source: the versioned **1.0.6 — 2026-08-18** section in
`~/.grok/CHANGELOG.md`; the flat `~/.grok/CHANGELOG.json` supplies no
independent version attribution.

- **Subagent spawning no longer accepts `capability_mode`; tool access is
  controlled by agent type.** → 👀 **watch — subagent launch semantics.** The
  current diary and `PtyConfig::grok` contract do not document a
  `capability_mode` argument, but the separate 1.0.40 matrix must confirm that
  Grok worker launch and any Grok-owned subagents still receive only intended
  tools.
- **`grok clone` can mount a projected working tree from a content store, and
  large/unhealthy repositories no longer hang session startup.** → 👀 / 🟢
  **worktree watch and startup win.** Cassy-managed factory paths and branches
  remain outside Grok's cleanup authority.
- Queued messages during goals, prompt editing, consent links, and command
  result presentation → 👀 / ⏭ **queue watch or interactive UX; no Cassy code
  action indicated.** Windows project-hook expansion and video-storage errors
  → ⏭ **n/a** for this Linux factory path.

### 1.0.5 — config overrides · safe worktree reclaim · hook/session/tool robustness

Reviewed 2026-08-25. Host on **1.0.5**. Source: xAI's official [Grok Build
changelog](https://x.ai/build/changelog) (2026-08-15). The local
`~/.grok/CHANGELOG.md` snapshot has later 1.0.6–1.0.13 sections but no 1.0.5
section, and the flat `~/.grok/CHANGELOG.json` supplies no release attribution
for this entry.

- **`GROK_CONFIG` and `GROK_CONFIG_PATH` can override selected config settings.**
  → 👀 **watch — process configuration boundary.** Cassy still depends on
  persistent `cas__*` discovery plus explicit `--rules` and inherited identity;
  a launcher override must not suppress the configured server, replace the
  worker rules, or hide `CAS_SESSION_ID` / `CAS_FACTORY_WORKER_CLI=grok`. The
  complete production matrix passed and is recorded in
  `grok-build-1.0.5-2026-08-25`.
- **Worktrees under `~/.grok/worktrees` are reclaimed automatically when safe,
  with protection for the last remaining copy.** → 👀 **watch — worktree
  containment.** This is a Grok-owned cleanup surface and does not authorize
  deleting a Cassy-managed checkout or branch; verify active factory worktrees
  remain outside Grok's reclaim set.
- **Hook policy blocks now identify a hook block instead of a user cancellation.**
  → 👀 **watch — hook posture.** Grok's SessionStart stdout remains ignored by
  Cassy; `--rules` plus inherited env remain the load-bearing role and identity
  path. Better diagnostics do not change that contract.
- **Session titles refresh earlier and `/resume` shows a recap/last-turn summary.**
  → 👀 **watch — session/transcript evidence.** Factory workers still use a
  fresh injected `--session-id`, and Cassy resolves liveness under
  `~/.grok/sessions/*` rather than by title or recap text.
- **Tool calls recover after `/dev/null` removal; MCP calls show clearer spinner
  text; Windows skill-home resolution and minimal-mode streaming improve.** →
  ✅ / ⏭ **operational win / n/a.** These do not change Cassy's MCP namespace,
  launch flags, or transcript contract.
- **Source gap:** 1.0.5 is absent from the retained local changelog and the
  companion JSON is unversioned; the official page is the attributable source.
  No release-attribution gap remains for this version.

### 1.0.4 — session env · permission grants · hook/subagent/worktree recovery

Reviewed 2026-08-25. Host on **1.0.5**. Source: xAI's official [Grok Build
changelog](https://x.ai/build/changelog) (2026-08-13). The local
`~/.grok/CHANGELOG.md` snapshot has later 1.0.6–1.0.13 sections but no 1.0.4
section, and the flat `~/.grok/CHANGELOG.json` supplies no release attribution
for this entry.

- **Tool commands and MCP servers now receive `GROK_SESSION_ID`.** → 👀 **watch
  — session identity and env inheritance.** Cassy's load-bearing identity is
  still `CAS_SESSION_ID` matching the fresh `--session-id`; the new Grok-owned
  variable must not replace or diverge from that value when resolving
  `~/.grok/sessions/*` or child-process identity.
- **Auto permission mode honors explicit always-allow grants and narrow allow
  rules.** → 👀 **watch — permission semantics.** Cassy explicitly passes
  `--permission-mode bypassPermissions`; verify that mode still bypasses the
  interactive gate and that Grok's narrower grants do not weaken Cassy's task,
  edit, or tool scope.
- **Headless sessions wait for MCP, non-interactive sessions handle prompts,
  mid-turn steering is reported correctly, and subagent lifecycle events remain
  correct out of order.** → 🟢 / 👀 **lifecycle wins / watch.** These improve
  unattended operation, but Cassy remains authoritative for `cas__*` discovery,
  worker leases, and coordination state; Grok-owned subagents are not Cassy
  workers.
- **Session search can be disabled, finished subagent transcripts are rebuilt
  from disk, session/image recovery improves, and queued prompts no longer
  auto-submit while being edited.** → 👀 **watch — transcript and delivery
  surfaces.** Cassy uses the Grok session tree plus its injected UUID and
  supervisor coordination truth; verify those remain available with the
  default search/config posture.
- **Hook failures show their first stderr line; permission-mode changes on the
  welcome screen apply to the newly created session; worktree commands work on
  Windows when only `USERPROFILE` is set.** → 👀 / ⏭ **watch hook/permission
  posture; Windows-only worktree support is n/a here.** Re-check hook-disabled
  posture, bypass semantics, and child-process cleanup during the next full
  matrix; no Cassy code change is indicated by the notes alone.
- **The remaining `/loop`, web-search domain, UI, keyboard, and media changes**
  → ⏭ **n/a** (Grok-owned UX or tools outside Cassy's launch contract).
- **Source gap:** 1.0.4 is absent from the retained local changelog and the
  companion JSON is unversioned; the official page is the attributable source.
  No release-attribution gap remains for this version.

### 1.0.3–0.2.115 — current release sweep: factory lifecycle and integration boundaries

Reviewed 2026-08-13. Host on **1.0.3**. Sources: the local versioned
`~/.grok/CHANGELOG.md` for 0.2.115–0.2.117 and xAI's official [Grok Build
changelog](https://x.ai/build/changelog) for 0.2.118–1.0.3.

- **0.2.115 fixes duplicate/corrupt tool results; 0.2.116 adds headless streaming JSON; 0.2.117
  stops all prior-turn background subagents and adds a custom TLS-root variable.** → 🟢 / 👀
  **evidence and lifecycle wins; retain launch-environment watch.** More truthful tool history and
  stopped background work improve Cassy proof and cleanup, while `GROK_EXTRA_CA_BUNDLE` is a separate
  transport input that must not hide the inherited Cassy identity or persistent MCP configuration.
- **0.2.118–0.2.119 fix background-task completion state, compaction cancellation, startup/doctor
  behavior, auth recovery, and broad bash allow-list editing.** → ✅ / 👀 **operational wins; watch
  approval semantics.** Cassy continues to set explicit bypass mode and remains the authority for task
  lifecycle; permissive allow-list changes do not replace the factory’s scope and tool guards.
- **1.0.0 improves MCP image results and permission-prompt visibility; 1.0.1 bounds wide subagent
  fan-out, exposes read-only tool metadata, waits for MCP in headless sessions, and stops subagents
  before deleting sessions.** → 👀 / 🟢 **high-value compatibility gains.** Verify `cas__*`
  discovery and `--rules`/environment priming on a fresh headless worker, and keep Cassy’s own
  concurrency and state authority; the host bounds are complementary.
- **1.0.2 makes startup delays diagnosable, preserves worktree fetch safety, and presents hook
  results with grouped tool calls; 1.0.3 speeds subagent spawning.** → 🟢 / 👀 **isolation and
  observability wins.** Re-run the full PTY matrix before advancing the validated pin, particularly
  for fresh session UUIDs, inherited identity, persistent MCP discovery, rules precedence,
  transcript/liveness, and worktree containment.
- **Source gaps:** none for 0.2.115–1.0.3; the official page and retained local changelog provide
  attributable notes for every release in this reviewed range.

### 0.2.114 — session deletion and startup thread-exhaustion fix

Reviewed and validated 2026-07-30. Source: the versioned
**0.2.114 — 2026-07-29** section in `~/.grok/CHANGELOG.md`, plus the complete
live Cassy factory matrix against the retained authenticated 0.2.114 executable.

- **`/delete` removes the current session after confirmation.** → ⏭ **n/a.**
  Cassy launches fresh UUID sessions and does not invoke Grok's destructive
  session command.
- **Startup no longer crashes when the host has no free threads.** → ✅
  **operational reliability win.** The real isolated worker launched and
  completed its Cassy lifecycle under the production PTY configuration.
- **Cassy factory contract:** → ✅ permission bypass, session UUID, model/effort,
  cwd, `--rules`, persistent `cas__*` MCP discovery, inherited identity, Grok
  transcript/liveness, task/edit/commit lifecycle, and hooks-disabled posture
  all passed. Typed evidence:
  `crates/cas-pty/conformance/grok-build-0.2.114-2026-07-30.json`.

### 0.2.113 — MCP controls and lifecycle reliability

Reviewed 2026-07-30. Source: the versioned **0.2.113 — 2026-07-28** section in
`~/.grok/CHANGELOG.md`.

- **MCP servers can be enabled/disabled from the CLI; invalid entries no longer
  block startup.** → 👀 / ✅. Persistent Cassy discovery remains load-bearing and
  passed with 11 tools; operators can still disable `cas`, so preflight must
  report discovery health rather than assume configuration presence.
- **SessionEnd runs in TUI/headless sessions; session registry, auth sharing,
  subprocess cleanup, shell output, and cold-start behavior were hardened.**
  → ✅ operational wins. Cassy continues to use `--rules` plus inherited env, not
  SessionStart stdout, for worker identity and role context.
- Remaining plan/clipboard/background-task/TUI changes are ⏭ Grok-owned UX and
  do not alter the Cassy launch contract.

### 0.2.112 — version policy · env/MCP/hooks · session and workflow lifecycle

Reviewed 2026-07-30. Source: the versioned **0.2.112 — 2026-07-24** section in
`~/.grok/CHANGELOG.md`. The current `~/.grok/CHANGELOG.json` contains the same
items as a flat list but supplies no independent version attribution.

- **CLI version policy now separates soft update floors/ceilings from hard startup
  requirements.** → 👀 **watch — unattended launch availability.** Cassy launches
  Grok workers directly; a hard startup requirement could prevent a factory pane
  from reaching its injected `--rules`, env, or MCP contract. No flag change is
  documented here, but upgrade/startup failures should distinguish Grok's version
  gate from Cassy worker lifecycle state.
- **Custom model providers can take query parameters, environment-backed headers,
  and an allowlist controlling which variables reach shell tools.** → 👀 **watch —
  process env boundary.** Cassy sets identity and factory variables on the Grok child.
  Provider-header lookup and shell-variable filtering are separate config surfaces;
  smoke that `CAS_AGENT_NAME`, `CAS_SESSION_ID`, and
  `CAS_FACTORY_WORKER_CLI=grok` still reach the required child/tool paths when
  operators enable these options.
- **`tool_overrides` / `toolOverrides` adds date cutoffs and domain allowlists for
  built-in search.** → ✅ no Cassy launch change. These settings affect Grok-owned
  search tools, not persistent CAS MCP discovery or the `cas__*` namespace.
- **`/resume` defaults to native Grok sessions, `grok --resume` accepts a title,
  resumed/replayed conversations restore file attachments, and rewound-session
  forks copy live-branch history correctly.** → 👀 **watch — session/transcript
  behavior.** Factory workers still launch with a fresh `--session-id` and Cassy
  resolves liveness under `~/.grok/sessions/*`; these resume/fork changes do not
  authorize title-based lookup or foreign-session fallback in Cassy. Confirm that
  native transcript resolution remains keyed by the injected UUID.
- **Remote-client terminal output is recorded so read-file hints and monitors work.**
  → 👀 **watch — transcript/liveness evidence.** More complete recording is
  favorable, but Cassy still depends on the Grok session tree and correct
  `CAS_FACTORY_WORKER_CLI=grok`, not on the interactive monitor alone.
- **MCP tools appear without restart after managed-service enrollment/update, and
  plugin subagents inherit the parent's MCP tools.** → 👀 **watch — MCP discovery
  and subagents.** This should improve tool availability, but Cassy still supplies no
  per-spawn MCP override: discovery must expose the persistent server with Grok's
  `cas__*` tool names to both the parent and any Grok-owned subagent.
- **Hooks can now be defined in `config.toml` as well as JSON.** → 👀 **watch —
  hook/config layering.** This expands Grok's hook configuration surface but does
  not change Cassy's posture: SessionStart stdout is not the role-context path;
  explicit `--rules` plus inherited env remain load-bearing.
- **Workflow overlays show live per-agent progress, failed workflow runs can resume,
  and clicking “still running” opens the tasks pane.** → 👀 **watch — factory
  messaging and lifecycle diagnosis.** These are Grok-owned workflow/task views,
  not Cassy coordination messages, leases, or factory membership. Do not treat their
  roster or resumed-run state as Cassy authority.
- **Background shell commands now report real exit codes; the task tray clears
  killed work and preserves descriptions after reconnect; startup hangs after
  concurrent launches were fixed.** → ✅ **no Cassy code action; operational
  reliability win.** These fixes make shell proof and worker diagnosis less
  misleading, while Cassy remains responsible for process launch and task state.
- **Queued prompts add an edit action, repeated identical tool calls stop silently,
  and parked turns no longer duplicate transcript timing markers.** → 👀 **watch —
  queued factory-message visibility and transcript evidence.** These are harness
  behavior changes around the same surfaces operators inspect during injected
  turns; Cassy delivery truth remains its coordination state, not TUI copy alone.
- **`/doctor`, tmux clipboard repair, auth/account, voice, image-edit, marketplace,
  slash-command labels, colors, and other TUI fixes.** → ⏭ n/a unless a concrete
  launch or tool-discovery regression is reproduced.

### 0.2.111 — missing from the available versioned local changelog

Reviewed 2026-07-30. `~/.grok/CHANGELOG.md` contains no 0.2.111 section, and
the flat `~/.grok/CHANGELOG.json` does not attribute items to it. No release
history or Cassy verdict is inferred.

### 0.2.110 — missing from the available versioned local changelog

Reviewed 2026-07-30. `~/.grok/CHANGELOG.md` contains no 0.2.110 section, and
the flat `~/.grok/CHANGELOG.json` does not attribute items to it. No release
history or Cassy verdict is inferred.

### 0.2.109 — missing from the available versioned local changelog

Reviewed 2026-07-30. `~/.grok/CHANGELOG.md` contains no 0.2.109 section, and
the flat `~/.grok/CHANGELOG.json` does not attribute items to it. No release
history or Cassy verdict is inferred.

### 0.2.108 — missing from the available versioned local changelog

Reviewed 2026-07-30. `~/.grok/CHANGELOG.md` contains no 0.2.108 section, and
the flat `~/.grok/CHANGELOG.json` does not attribute items to it. No release
history or Cassy verdict is inferred.

### 0.2.107 — missing from the available versioned local changelog

Reviewed 2026-07-30. `~/.grok/CHANGELOG.md` contains no 0.2.107 section, and
the flat `~/.grok/CHANGELOG.json` does not attribute items to it. No release
history or Cassy verdict is inferred.

### 0.2.106 — scheduled-task lifecycle · clipboard fallback

Reviewed 2026-07-22 (diary-grok / cas-4aef). Host install is **0.2.106**.
Source: `~/.grok/CHANGELOG.md` (2026-07-18).

- **“Added `GROK_CLIPBOARD_NO_OSC52` env var”** and **copies always write a backup
  file when the terminal clipboard fails.** → ⏭ n/a. These improve interactive
  clipboard recovery and terminal compatibility; Cassy does not configure Grok's
  clipboard transport in `PtyConfig::grok`.
- **“Scheduled tasks can now be updated in place; one-time tasks are retired in
  favor of background commands.”** → ✅ no action. This changes Grok-native task
  scheduling/background commands, not Cassy task leases or the Cassy-managed factory
  worker process lifecycle. Keep the distinction explicit when diagnosing a Grok
  “background task”: it is not necessarily a Cassy task or worker.
- **Minimal-mode syntax highlighting is visible on light terminals.** → ⏭ n/a
  (rendering only; factory workers are not launched through minimal-mode UI flows).

### 0.2.105 — model defaults · login-shell env · rules/MCP · compaction · fleet UX

Reviewed 2026-07-22 (diary-grok / cas-4aef). Source:
`~/.grok/CHANGELOG.md` (2026-07-18).

- **“Default model is now Grok 4.5 with high/medium/low reasoning effort and
  improved compaction settings.”** → 👀 **watch — model/effort defaults.** Cassy only
  passes `--model` and `--reasoning-effort` when the factory requests them, so an
  unpinned worker now inherits Grok 4.5 and its new defaults. The listed effort
  levels remain within Cassy's verified vocabulary, but this changelog review does
  not replace a live flag/behavior smoke.
- **“Local shell tools now see the same environment variables, aliases, and
  functions as your login shell.”** → 👀 **watch — environment boundary.** Cassy
  supplies identity and factory metadata on the top-level Grok child. This fix is
  favorable for commands Grok launches, but login-shell initialization can also
  add or override environment state; smoke that `CAS_AGENT_NAME`, `CAS_SESSION_ID`,
  and `CAS_FACTORY_WORKER_CLI=grok` remain visible after the upgrade.
- **“Global rules from `~/.grok/rules` and compatible vendor homes are now
  discovered correctly.”** → 👀 **watch — system-prompt layering.** Cassy's
  load-bearing role contract is appended explicitly with `--rules`; newly restored
  global/vendor rules are another prompt source and must not displace or contradict
  that injected contract. No spawn change is indicated.
- **MCP OAuth logins now accept RFC 9207 issuer callbacks.** → ✅ no action for the
  current Cassy stdio server. It improves discovery/login compatibility for remote
  OAuth MCP servers but does not alter Cassy's persistent project/user MCP discovery
  or Grok's `cas__*` tool namespace.
- **Background tasks finishing after Ctrl+C no longer resume the model; Ctrl+\\ from
  the dashboard returns to the originating agent; fleet roster entries render even
  with an empty local agent list.** → 👀 **watch — lifecycle/roster UX.** These are
  Grok-owned background-agent and dashboard behaviors, not Cassy lease/roster state.
  The fixes reduce confusing post-cancel resumes and missing rows, but Cassy remains
  authoritative for factory membership and worker lifecycle.
- **Long-session compaction no longer fails when servers reject `tool_choice: none`
  with tools attached.** → 👀 **watch — long factory sessions.** This is a direct
  reliability improvement for tool-using workers; verify that Cassy rules and identity
  survive a real compaction before treating the 0.2.106 install as validated.
- **`/btw` in minimal mode, snap-prompt appearance, `/summarize`, syntax colors, and
  scrolling smoothness.** → ⏭ n/a (interactive commands/rendering; no Cassy launch,
  MCP, rules, transcript, or process contract change).

### 0.2.104 — background status · idle authentication recovery

Reviewed 2026-07-22 (diary-grok / cas-4aef). Source:
`~/.grok/CHANGELOG.md` (2026-07-17).

- **“Background work counts now appear in a persistent status line instead of
  repeated transcript messages.”** → 👀 **watch — transcript/liveness evidence.** A
  presentation change should not alter session transcript activity, but Cassy liveness
  resolves Grok sessions under `~/.grok/sessions/*`. Confirm long background work
  still produces enough transcript/file activity for diagnostics rather than
  assuming status-line updates are persisted messages.
- **Authentication recovery for idle sessions after token timeouts.** → 👀 **watch —
  worker longevity.** This should reduce dead idle workers after auth expiry; it does
  not change Cassy leases, restarts, or its source of worker truth.
- **Retry errors hide raw HTML, rate-limit messages show server detail, and in-place
  prompt editing is temporarily disabled.** → ⏭ n/a (error copy and interactive
  editor behavior only).

### 0.2.103 — missing from the installed local changelog

Reviewed 2026-07-22 (diary-grok / cas-4aef). The installed
`~/.grok/CHANGELOG.md` jumps from **0.2.104** to the end of the file; neither it nor
the current flat `~/.grok/CHANGELOG.json` provides a 0.2.103 section. No release
items, date, or Cassy verdict are fabricated from the version number alone.

### 0.2.102 — missing from the installed local changelog

Reviewed 2026-07-22 (diary-grok / cas-4aef). The installed
`~/.grok/CHANGELOG.md` has no 0.2.102 section, and the current flat
`~/.grok/CHANGELOG.json` does not attribute any item to it. No release items, date,
or Cassy verdict are fabricated. The already-recorded 0.2.101 entry below comes from
the earlier 2026-07-14 host snapshot; it does not fill this evidence gap.

### 0.2.101 — inspect multi-harness settings · queue/status polish · refresh rate

Reviewed 2026-07-14 (w-grok-diary / cas-5828). Host install is **0.2.101**.
Source: `~/.grok/CHANGELOG.md` (2026-07-13).

- **"grok inspect now shows effective compatibility settings for Cursor, Claude, and
  Codex sessions."** → 👀 **opportunity / ops win, no Cassy code required.** Multi-harness
  inspect is exactly the debugging surface factory hosts need when mixing CLIs. Does not
  change spawn flags; useful when validating MCP discovery and compat layers after
  upgrades. No task.
- **"New setting: Match display refresh rate" (native high-refresh TUI cadence).** →
  ⏭ n/a (host TUI preference; orthogonal to `PtyConfig::grok`).
- **"Parked subagent status no longer duplicates or interleaves incorrectly in
  scrollback."** → ✅ no action — render fix. Factory may spawn Grok-side subagents;
  cleaner scrollback only. Not a spawn/MCP/rules break.
- **"Status line during waits shows elapsed time before the queued-message hint."** →
  ⏭ n/a (TUI chrome).
- **"Queued messages sent with Enter now appear immediately instead of vanishing
  briefly."** + related queue reliability in 0.2.100 → 👀 **watch (factory messaging
  UX).** Supervisor→worker delivery often lands as injected/queued turns. Appearance
  glitches can look like "message lost" during ops; this is a harness fix, not a Cassy
  change. Verify subjectively on upgrade if operators still report vanished queue items.
- **"Resume hint after quitting minimal mode prints the correct `grok --minimal
  --resume` command."** → ⏭ n/a (minimal-mode UX; factory workers are not launched in
  that interactive path).
- **"Rate-limit messages correctly direct API-key users to team plans."** → ⏭ n/a
  (billing/copy).

### 0.2.100 — cross-harness session picker · queue Enter · hooks disabled-at-start · pane-closed crash

Reviewed 2026-07-14 (w-grok-diary / cas-5828). Source: `~/.grok/CHANGELOG.md`
(2026-07-13). **Seed-floor version** — oldest section currently present in the local
changelog; no pre-0.2.100 entries inventable from this host.

- **"Session picker discovers and resumes recent Claude Code, Codex, and Cursor
  sessions"** + **"Welcome screen one-click resume nudge for recent Claude, Codex, or
  Cursor sessions."** → 👀 **strategic / host UX, not factory spawn.** Interesting for
  multi-harness hosts running Cassy, but factory panes use fresh `--session-id` UUIDs and
  do not resume foreign harness sessions via this picker. No code action; note for
  onboarding docs only.
- **"Web fetch tool preserves full truncated page content as readable artifacts."** →
  ✅ no action (agent tool quality; not a launch touchpoint).
- **"Multiline mode correctly sends the top queued message on empty Enter when a turn
  is running"** + **"Queued commands no longer disappear or delay when pressing Enter
  twice quickly during a running turn."** → 👀 **watch — input/queue path.** Same class
  as 0.2.101 queue-visibility fixes: factory coordination depends on messages actually
  enqueueing during long turns. Harness-side reliability win; smoke "message during
  running turn" after big Grok bumps.
- **"Minimal mode text readable on dark terminals."** → ⏭ n/a.
- **"Grok no longer crashes when printing resume hints after the terminal pane has
  closed."** → 👀 **watch — factory mux / pane lifecycle.** Factory workers run inside
  Cassy-managed panes; a crash on post-close resume-hint printing could have looked like
  a worker death. Fix is pure harness; confirm no residual panic on worker shutdown
  after upgrade. No Cassy change expected.
- **"Long-running turns with multiple waits show updated status markers instead of
  appearing stuck."** → ✅ no action — direct win for long factory tasks (stall
  false-positives from "stuck" UI). Complements Cassy is-wedged logic; does not replace
  transcript-path correctness (`~/.grok/sessions/*` + `CAS_FACTORY_WORKER_CLI=grok`).
- **"Claude and Cursor hooks are now correctly disabled at session start when disabled
  in config."** → 👀 **touchpoint: hooks/config posture.** Grok already ignores
  SessionStart *stdout* for Cassy context injection (we use `--rules` + env). This line is
  about honoring "disabled in config" for Claude/Cursor-compat hooks — verify that
  disabling hooks in config does not also strip something Cassy still relies on (unlikely
  for factory spawn, since we do not depend on SessionStart stdout). On upgrade, re-check
  that `CAS_SESSION_ID` registration and `--rules` role text still land with hooks
  disabled.

---

## Backlog of opportunities (not required, tracked)

- **1.0.5 validation:** ✅ complete. The installed/latest release is validated
  through the complete live checklist and typed
  `grok-build-1.0.5-2026-08-25` receipt; the prior 0.2.114 receipt remains the
  historical baseline.
- **Changelog history depth:** find an authoritative release surface for the missing
  0.2.102–0.2.103 and 0.2.107–0.2.111 notes, plus any pre-0.2.100 history,
  before backfilling them. The companion JSON remains unversioned; keep every gap
  and the 0.2.100 evidence-backed seed floor explicit until attributable sources
  exist.
- **SessionStart stdout:** if a future Grok release starts delivering SessionStart
  stdout like Claude, re-evaluate whether `--rules` remains the sole context path or
  becomes defense-in-depth (would be a deliberate EPIC, not a silent drop of `--rules`).
- **Queue/input reliability:** 0.2.100–0.2.101 cluster of queue/Enter fixes — if factory
  operators still report lost mid-turn messages on Grok workers, capture repro before
  assuming Cassy delivery is at fault.
