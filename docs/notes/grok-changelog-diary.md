# Grok Build Changelog Diary — CAS Response Ledger

A living, **newest-first** ledger of xAI Grok Build CLI releases and how CAS
responded to each. Sibling to `claude-code-changelog-diary.md` and
`codex-changelog-diary.md` — CAS supports three harnesses (`cli=claude` /
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
3. Verdict each user-facing item against the **CAS ↔ Grok touchpoints** below.
   Most TUI polish is `⏭ n/a`. Prefer items that touch permissions, session IDs,
   rules/system prompt, MCP discovery, env inheritance, transcripts, or hooks.
4. Add a newest-first entry + index row. File a CAS task only when work is required.
5. **Version gap matters:** keep **validated pin** (pty.rs comment) vs **locally
   installed** vs **latest in changelog** honest. Do not invent older releases —
   if the local changelog only has N versions, seed those N and mark the **seed floor**.
6. After the diary update merges, publish the mandatory shared **#cas-internal**
   harness thread: one parent plus exactly three replies ordered **Grok, Claude,
   Codex**. Follow [the release Slack rubric](../RELEASE_SLACK_RUBRIC.md), including
   its version-range, verdict/action, source-gap, and no-internal-narration rules.

**Verdict legend:** ✅ no action · 🟢 already covered · 👀 watch (touches a CAS
dependency, verify on upgrade) · 🔧 fix shipped · 🏗 EPIC · ⏭ n/a

## Version status

- **CAS validated against:** Grok Build **0.2.93** (comment on
  `crates/cas-pty/src/pty.rs` `PtyConfig::grok`, verified live 2026-07-09 via
  `grok --help` / `grok inspect` / `grok mcp doctor`).
- **Locally installed:** **0.2.114** (`grok 0.2.114 (0c78503879) [stable]`, checked
  2026-07-30).
- **Latest versioned local changelog evidence:** **0.2.112** (2026-07-24). The
  current `~/.grok/CHANGELOG.md` contains only a **0.2.112** section. The current
  `~/.grok/CHANGELOG.json` is a flat item list with no version or date fields; its
  items are therefore only **current-install evidence**, not grounds to assign
  history to 0.2.113–0.2.114 or another guessed release. Earlier host snapshots
  captured the evidence-backed **0.2.100–0.2.101** and **0.2.104–0.2.106** entries
  retained below. Versions **0.2.102–0.2.103**, **0.2.107–0.2.111**, and
  **0.2.113–0.2.114** have no attributable notes on the available local surfaces
  and remain explicit source gaps. **0.2.100** remains the diary's evidence-backed
  **seed floor**.
- **Gap:** ~21 patch versions between the validated pin (0.2.93) and locally
  installed 0.2.114; the newest versioned changelog evidence trails the install by
  two patches. Entries below are a *changelog triage pass*, not a
  re-run of the 0.2.93 live verification. Upgrade-time re-verification of the full
  touchpoint checklist remains the trigger for promoting any 👀 to a task.

## CAS ↔ Grok touchpoints (what a release can break)

The load-bearing surface is `crates/cas-pty/src/pty.rs::PtyConfig::grok` (approx.
lines 423–580; instructions constants near top of file). Ground truth is that
code + its "Verified against … 0.2.93" block — re-read it on upgrade rather than
trusting this diary alone.

### CLI flags (spawn args)

- **`--permission-mode bypassPermissions`** — factory workers skip interactive
  approval (Grok's analogue of Claude's bypass / Codex's `--yolo`). Any rename,
  removal, or semantic narrowing of `bypassPermissions` breaks unattended workers.
- **`--session-id <uuid>`** — fresh UUID per *new* conversation (anti-overwrite
  model, same family as Claude; not Codex's `codex-<name>-<uuid>` prefix). Phase 4
  transcript resolution keys on this exact value. Doc comment also notes short form
  `-s/--session-id` on the 0.2.93 binary.
- **`-m` / `--model <MODEL>`** — optional model pin when the factory requests one.
- **`--reasoning-effort <EFFORT>`** (alias `--effort` on the verified binary) —
  vocabulary minimal/low/medium/high/xhigh via `Effort::as_claude_arg()` (no
  separate `as_grok_arg`). Any "reasoning effort" changelog line is a 👀.
- **`--cwd <path>`** — worktree/working directory for the worker process.
- **`--rules <text>`** — **"Extra rules to append to the system prompt."** This is
  the load-bearing context path for factory role priming. CAS injects
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

| Grok version | Headline | CAS verdict | Pointer |
|--------------|----------|-------------|---------|
| 0.2.114 | Missing from available versioned local changelog evidence | — (no attributable evidence) | this doc |
| 0.2.113 | Missing from available versioned local changelog evidence | — (no attributable evidence) | this doc |
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
| *(seed floor)* | No evidence-backed versions before 0.2.100; current host changelog contains only 0.2.112 | — | — |

---

## Entries

### 0.2.114 — missing from the available versioned local changelog

Reviewed 2026-07-30. The host binary is **0.2.114**, but
`~/.grok/CHANGELOG.md` contains only a 0.2.112 section. The flat
`~/.grok/CHANGELOG.json` has no version or date fields, so its current-install
items cannot be attributed to 0.2.114. No release items, date, or CAS verdict are
reconstructed from the installed version or neighboring release.

### 0.2.113 — missing from the available versioned local changelog

Reviewed 2026-07-30. Neither `~/.grok/CHANGELOG.md` nor the unversioned,
flat `~/.grok/CHANGELOG.json` attributes notes to 0.2.113. The 0.2.112 section
below does not fill this gap, and no history is inferred from its date.

### 0.2.112 — version policy · env/MCP/hooks · session and workflow lifecycle

Reviewed 2026-07-30. Source: the versioned **0.2.112 — 2026-07-24** section in
`~/.grok/CHANGELOG.md`. The current `~/.grok/CHANGELOG.json` contains the same
items as a flat list but supplies no independent version attribution.

- **CLI version policy now separates soft update floors/ceilings from hard startup
  requirements.** → 👀 **watch — unattended launch availability.** CAS launches
  Grok workers directly; a hard startup requirement could prevent a factory pane
  from reaching its injected `--rules`, env, or MCP contract. No flag change is
  documented here, but upgrade/startup failures should distinguish Grok's version
  gate from CAS worker lifecycle state.
- **Custom model providers can take query parameters, environment-backed headers,
  and an allowlist controlling which variables reach shell tools.** → 👀 **watch —
  process env boundary.** CAS sets identity and factory variables on the Grok child.
  Provider-header lookup and shell-variable filtering are separate config surfaces;
  smoke that `CAS_AGENT_NAME`, `CAS_SESSION_ID`, and
  `CAS_FACTORY_WORKER_CLI=grok` still reach the required child/tool paths when
  operators enable these options.
- **`tool_overrides` / `toolOverrides` adds date cutoffs and domain allowlists for
  built-in search.** → ✅ no CAS launch change. These settings affect Grok-owned
  search tools, not persistent CAS MCP discovery or the `cas__*` namespace.
- **`/resume` defaults to native Grok sessions, `grok --resume` accepts a title,
  resumed/replayed conversations restore file attachments, and rewound-session
  forks copy live-branch history correctly.** → 👀 **watch — session/transcript
  behavior.** Factory workers still launch with a fresh `--session-id` and CAS
  resolves liveness under `~/.grok/sessions/*`; these resume/fork changes do not
  authorize title-based lookup or foreign-session fallback in CAS. Confirm that
  native transcript resolution remains keyed by the injected UUID.
- **Remote-client terminal output is recorded so read-file hints and monitors work.**
  → 👀 **watch — transcript/liveness evidence.** More complete recording is
  favorable, but CAS still depends on the Grok session tree and correct
  `CAS_FACTORY_WORKER_CLI=grok`, not on the interactive monitor alone.
- **MCP tools appear without restart after managed-service enrollment/update, and
  plugin subagents inherit the parent's MCP tools.** → 👀 **watch — MCP discovery
  and subagents.** This should improve tool availability, but CAS still supplies no
  per-spawn MCP override: discovery must expose the persistent server with Grok's
  `cas__*` tool names to both the parent and any Grok-owned subagent.
- **Hooks can now be defined in `config.toml` as well as JSON.** → 👀 **watch —
  hook/config layering.** This expands Grok's hook configuration surface but does
  not change CAS's posture: SessionStart stdout is not the role-context path;
  explicit `--rules` plus inherited env remain load-bearing.
- **Workflow overlays show live per-agent progress, failed workflow runs can resume,
  and clicking “still running” opens the tasks pane.** → 👀 **watch — factory
  messaging and lifecycle diagnosis.** These are Grok-owned workflow/task views,
  not CAS coordination messages, leases, or factory membership. Do not treat their
  roster or resumed-run state as CAS authority.
- **Background shell commands now report real exit codes; the task tray clears
  killed work and preserves descriptions after reconnect; startup hangs after
  concurrent launches were fixed.** → ✅ **no CAS code action; operational
  reliability win.** These fixes make shell proof and worker diagnosis less
  misleading, while CAS remains responsible for process launch and task state.
- **Queued prompts add an edit action, repeated identical tool calls stop silently,
  and parked turns no longer duplicate transcript timing markers.** → 👀 **watch —
  queued factory-message visibility and transcript evidence.** These are harness
  behavior changes around the same surfaces operators inspect during injected
  turns; CAS delivery truth remains its coordination state, not TUI copy alone.
- **`/doctor`, tmux clipboard repair, auth/account, voice, image-edit, marketplace,
  slash-command labels, colors, and other TUI fixes.** → ⏭ n/a unless a concrete
  launch or tool-discovery regression is reproduced.

### 0.2.111 — missing from the available versioned local changelog

Reviewed 2026-07-30. `~/.grok/CHANGELOG.md` contains no 0.2.111 section, and
the flat `~/.grok/CHANGELOG.json` does not attribute items to it. No release
history or CAS verdict is inferred.

### 0.2.110 — missing from the available versioned local changelog

Reviewed 2026-07-30. `~/.grok/CHANGELOG.md` contains no 0.2.110 section, and
the flat `~/.grok/CHANGELOG.json` does not attribute items to it. No release
history or CAS verdict is inferred.

### 0.2.109 — missing from the available versioned local changelog

Reviewed 2026-07-30. `~/.grok/CHANGELOG.md` contains no 0.2.109 section, and
the flat `~/.grok/CHANGELOG.json` does not attribute items to it. No release
history or CAS verdict is inferred.

### 0.2.108 — missing from the available versioned local changelog

Reviewed 2026-07-30. `~/.grok/CHANGELOG.md` contains no 0.2.108 section, and
the flat `~/.grok/CHANGELOG.json` does not attribute items to it. No release
history or CAS verdict is inferred.

### 0.2.107 — missing from the available versioned local changelog

Reviewed 2026-07-30. `~/.grok/CHANGELOG.md` contains no 0.2.107 section, and
the flat `~/.grok/CHANGELOG.json` does not attribute items to it. No release
history or CAS verdict is inferred.

### 0.2.106 — scheduled-task lifecycle · clipboard fallback

Reviewed 2026-07-22 (diary-grok / cas-4aef). Host install is **0.2.106**.
Source: `~/.grok/CHANGELOG.md` (2026-07-18).

- **“Added `GROK_CLIPBOARD_NO_OSC52` env var”** and **copies always write a backup
  file when the terminal clipboard fails.** → ⏭ n/a. These improve interactive
  clipboard recovery and terminal compatibility; CAS does not configure Grok's
  clipboard transport in `PtyConfig::grok`.
- **“Scheduled tasks can now be updated in place; one-time tasks are retired in
  favor of background commands.”** → ✅ no action. This changes Grok-native task
  scheduling/background commands, not CAS task leases or the CAS-managed factory
  worker process lifecycle. Keep the distinction explicit when diagnosing a Grok
  “background task”: it is not necessarily a CAS task or worker.
- **Minimal-mode syntax highlighting is visible on light terminals.** → ⏭ n/a
  (rendering only; factory workers are not launched through minimal-mode UI flows).

### 0.2.105 — model defaults · login-shell env · rules/MCP · compaction · fleet UX

Reviewed 2026-07-22 (diary-grok / cas-4aef). Source:
`~/.grok/CHANGELOG.md` (2026-07-18).

- **“Default model is now Grok 4.5 with high/medium/low reasoning effort and
  improved compaction settings.”** → 👀 **watch — model/effort defaults.** CAS only
  passes `--model` and `--reasoning-effort` when the factory requests them, so an
  unpinned worker now inherits Grok 4.5 and its new defaults. The listed effort
  levels remain within CAS's verified vocabulary, but this changelog review does
  not replace a live flag/behavior smoke.
- **“Local shell tools now see the same environment variables, aliases, and
  functions as your login shell.”** → 👀 **watch — environment boundary.** CAS
  supplies identity and factory metadata on the top-level Grok child. This fix is
  favorable for commands Grok launches, but login-shell initialization can also
  add or override environment state; smoke that `CAS_AGENT_NAME`, `CAS_SESSION_ID`,
  and `CAS_FACTORY_WORKER_CLI=grok` remain visible after the upgrade.
- **“Global rules from `~/.grok/rules` and compatible vendor homes are now
  discovered correctly.”** → 👀 **watch — system-prompt layering.** CAS's
  load-bearing role contract is appended explicitly with `--rules`; newly restored
  global/vendor rules are another prompt source and must not displace or contradict
  that injected contract. No spawn change is indicated.
- **MCP OAuth logins now accept RFC 9207 issuer callbacks.** → ✅ no action for the
  current CAS stdio server. It improves discovery/login compatibility for remote
  OAuth MCP servers but does not alter CAS's persistent project/user MCP discovery
  or Grok's `cas__*` tool namespace.
- **Background tasks finishing after Ctrl+C no longer resume the model; Ctrl+\\ from
  the dashboard returns to the originating agent; fleet roster entries render even
  with an empty local agent list.** → 👀 **watch — lifecycle/roster UX.** These are
  Grok-owned background-agent and dashboard behaviors, not CAS lease/roster state.
  The fixes reduce confusing post-cancel resumes and missing rows, but CAS remains
  authoritative for factory membership and worker lifecycle.
- **Long-session compaction no longer fails when servers reject `tool_choice: none`
  with tools attached.** → 👀 **watch — long factory sessions.** This is a direct
  reliability improvement for tool-using workers; verify that CAS rules and identity
  survive a real compaction before treating the 0.2.106 install as validated.
- **`/btw` in minimal mode, snap-prompt appearance, `/summarize`, syntax colors, and
  scrolling smoothness.** → ⏭ n/a (interactive commands/rendering; no CAS launch,
  MCP, rules, transcript, or process contract change).

### 0.2.104 — background status · idle authentication recovery

Reviewed 2026-07-22 (diary-grok / cas-4aef). Source:
`~/.grok/CHANGELOG.md` (2026-07-17).

- **“Background work counts now appear in a persistent status line instead of
  repeated transcript messages.”** → 👀 **watch — transcript/liveness evidence.** A
  presentation change should not alter session transcript activity, but CAS liveness
  resolves Grok sessions under `~/.grok/sessions/*`. Confirm long background work
  still produces enough transcript/file activity for diagnostics rather than
  assuming status-line updates are persisted messages.
- **Authentication recovery for idle sessions after token timeouts.** → 👀 **watch —
  worker longevity.** This should reduce dead idle workers after auth expiry; it does
  not change CAS leases, restarts, or its source of worker truth.
- **Retry errors hide raw HTML, rate-limit messages show server detail, and in-place
  prompt editing is temporarily disabled.** → ⏭ n/a (error copy and interactive
  editor behavior only).

### 0.2.103 — missing from the installed local changelog

Reviewed 2026-07-22 (diary-grok / cas-4aef). The installed
`~/.grok/CHANGELOG.md` jumps from **0.2.104** to the end of the file; neither it nor
the current flat `~/.grok/CHANGELOG.json` provides a 0.2.103 section. No release
items, date, or CAS verdict are fabricated from the version number alone.

### 0.2.102 — missing from the installed local changelog

Reviewed 2026-07-22 (diary-grok / cas-4aef). The installed
`~/.grok/CHANGELOG.md` has no 0.2.102 section, and the current flat
`~/.grok/CHANGELOG.json` does not attribute any item to it. No release items, date,
or CAS verdict are fabricated. The already-recorded 0.2.101 entry below comes from
the earlier 2026-07-14 host snapshot; it does not fill this evidence gap.

### 0.2.101 — inspect multi-harness settings · queue/status polish · refresh rate

Reviewed 2026-07-14 (w-grok-diary / cas-5828). Host install is **0.2.101**.
Source: `~/.grok/CHANGELOG.md` (2026-07-13).

- **"grok inspect now shows effective compatibility settings for Cursor, Claude, and
  Codex sessions."** → 👀 **opportunity / ops win, no CAS code required.** Multi-harness
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
  glitches can look like "message lost" during ops; this is a harness fix, not a CAS
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
  multi-harness hosts running CAS, but factory panes use fresh `--session-id` UUIDs and
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
  CAS-managed panes; a crash on post-close resume-hint printing could have looked like
  a worker death. Fix is pure harness; confirm no residual panic on worker shutdown
  after upgrade. No CAS change expected.
- **"Long-running turns with multiple waits show updated status markers instead of
  appearing stuck."** → ✅ no action — direct win for long factory tasks (stall
  false-positives from "stuck" UI). Complements CAS is-wedged logic; does not replace
  transcript-path correctness (`~/.grok/sessions/*` + `CAS_FACTORY_WORKER_CLI=grok`).
- **"Claude and Cursor hooks are now correctly disabled at session start when disabled
  in config."** → 👀 **touchpoint: hooks/config posture.** Grok already ignores
  SessionStart *stdout* for CAS context injection (we use `--rules` + env). This line is
  about honoring "disabled in config" for Claude/Cursor-compat hooks — verify that
  disabling hooks in config does not also strip something CAS still relies on (unlikely
  for factory spawn, since we do not depend on SessionStart stdout). On upgrade, re-check
  that `CAS_SESSION_ID` registration and `--rules` role text still land with hooks
  disabled.

---

## Backlog of opportunities (not required, tracked)

- **0.2.93 → 0.2.114 upgrade validation:** re-run the live checklist that pinned 0.2.93
  (`grok --help` flags, `grok inspect`, `grok mcp doctor`, factory spawn smoke):
  `--permission-mode bypassPermissions`, `--session-id`, `-m` / `--reasoning-effort`,
  `--cwd`, `--rules`, MCP discovery of `cas` without per-spawn `-c`, env inheritance
  into `cas serve`, tools as `cas__*`, transcripts under `~/.grok/sessions/*`.
- **Validated-pin comment bump:** after a successful factory smoke on 0.2.114, update
  the `PtyConfig::grok` "Verified against … 0.2.93" comment (and this diary's Version
  status) so the pin tracks reality.
- **Changelog history depth:** find an authoritative release surface for the missing
  0.2.102–0.2.103, 0.2.107–0.2.111, and 0.2.113–0.2.114 notes, plus any
  pre-0.2.100 history, before backfilling them. The current local Markdown contains
  only 0.2.112, and its companion JSON is unversioned; keep every gap and the
  0.2.100 evidence-backed seed floor explicit until attributable sources exist.
- **SessionStart stdout:** if a future Grok release starts delivering SessionStart
  stdout like Claude, re-evaluate whether `--rules` remains the sole context path or
  becomes defense-in-depth (would be a deliberate EPIC, not a silent drop of `--rules`).
- **Queue/input reliability:** 0.2.100–0.2.101 cluster of queue/Enter fixes — if factory
  operators still report lost mid-turn messages on Grok workers, capture repro before
  assuming CAS delivery is at fault.
