# Claude Code Changelog Diary — Cassy Response Ledger

A living, **newest-first** ledger of Claude Code releases and how Cassy responded to
each. The question this doc answers in one place: *"Does this Claude Code change
require a Cassy change, and if so, where's the proof?"*

This is the index layer. Deep per-EPIC working notes live in their own dated files
(e.g. `2026-06-02-cc160-hook-surface.md`); this diary links out to them.

Sibling ledgers for the other supported harnesses: `codex-changelog-diary.md` and
`grok-changelog-diary.md`.

## How to update

When a new Claude Code version ships:

1. Pull the changelog: `curl -s https://raw.githubusercontent.com/anthropics/claude-code/main/CHANGELOG.md`
2. For each user-facing item, decide a verdict against Cassy (table legend below).
   Most items are `✅ no action` — Cassy rides the harness, it doesn't fork it.
3. Add a new `## <version>` section at the TOP of the Entries list with the relevant
   items + verdict + reasoning. Only record items with a real Cassy angle; skip pure
   harness UI/perf fixes unless they touch a surface Cassy depends on.
4. Update the Index table.
5. If an item triggers actual work, file a Cassy task/EPIC and link it here. If it's a
   "reviewed, no change needed" conclusion worth not re-litigating, also drop a
   `reference`-type Cassy memory.
6. After the diary update merges, publish the mandatory shared **#cas-internal**
   harness thread: one parent plus exactly three replies ordered **Grok, Claude,
   Codex**. Follow [the release Slack rubric](../RELEASE_SLACK_RUBRIC.md), including
   its version-range, verdict/action, source-gap, and no-internal-narration rules.

**Verdict legend**

| Verdict | Meaning |
|---------|---------|
| ✅ no action | Reviewed; Cassy unaffected or already insulated |
| 🟢 already covered | Cassy already does the equivalent on its own surface |
| 👀 opportunity | Not required, but a cleaner path Cassy could adopt — tracked, not urgent |
| 🔧 fix shipped | Required a Cassy change; landed (link commit/version) |
| 🏗 EPIC | Large enough to warrant a factory EPIC (link epic note) |
| ⏭ n/a | Internal / no user-facing surface |

## Index

| CC version | Headline | Cassy verdict | Pointer |
|------------|----------|-------------|---------|
| 2.1.231 | MCP OAuth redirect-URI repair | ✅ no action | this doc |
| 2.1.230 | No section in Anthropic's official changelog | ⏭ source gap | this doc |
| 2.1.229 | Self-hosted hook/MCP startup fixes · workflow fan-out · sandbox hardening | 👀 / 🟢 | this doc |
| 2.1.228 | Worktree/session cleanup · skill shadowing hardening · deferred-tool correctness | 🟢 direct wins | this doc |
| 2.1.227 | Generic reliability and UI rollup | ✅ no action | this doc |
| 2.1.226 | Generic bug-fix and reliability rollup | ⏭ source-limited | this doc |
| 2.1.225 | Headless message delivery · agent workspace trust · MCP OAuth recovery | 👀 / 🟢 | this doc |
| 2.1.224 | Cross-session message controls · worktree/session isolation · MCP discovery | 👀 / 🟢 | this doc |
| 2.1.223 | Permission/workflow sandbox hardening · bounded context and review behavior | 🟢 direct wins | this doc |
| 2.1.222 | Worktree and background-hook isolation · MCP error visibility | 🟢 direct wins | this doc |
| 2.1.221 | Print-mode MCP startup · background commits/worktrees · permission hardening | 👀 / 🟢 | this doc |
| 2.1.220 | Generic bug-fix and reliability rollup; no component detail in the official note | ⏭ source-limited | this doc |
| 2.1.219 | **Opus 5 default** · bounded dynamic/nested agent changes · MCP startup/connection diagnostics | 👀 config watch / 🟢 / ✅ | this doc |
| 2.1.218 | Built-in `/code-review` backgrounds · **tool/transcript evidence-loss fixes** | ✅ no action / 🟢 win | this doc |
| 2.1.217 | **subagent concurrency/depth caps** · transcript-loss warnings · MCP-output leak + symlink-isolation fixes | 🟢 wins / ✅ | this doc |
| 2.1.216 | **worktree escape routes closed** · resumed-agent restrictions restored · long-session stalls fixed | 🟢 direct wins | this doc |
| 2.1.215 | `/verify` and `/code-review` no longer auto-invoked | ✅ no action | this doc |
| 2.1.214 | **hook exit-2 blocking restored** · long-tool heartbeat · permission + SessionStart fork semantics | 🟢 wins / ✅ | this doc |
| 2.1.213 | No section in Anthropic's official changelog | ⏭ source gap | this doc |
| 2.1.212 | **subagent/session caps** · long MCP calls auto-background · hook-halt + worktree-symlink fixes | 👀 watch / 🟢 wins | this doc |
| 2.1.211 | **subagent model override survives resume** · truthful background results · worktree-wide approvals | 🟢 wins / ✅ | this doc |
| 2.1.210 | **worktree-isolated git mutation fixed** · hook-timeout semantics · dead-worker lock cleanup | 🟢 direct wins | this doc |
| 2.1.209 | `/model` + dialogs unblocked in `claude agents` background sessions | ⏭ n/a | — |
| 2.1.208 | **catastrophic `rm` still prompts under `--dangerously-skip-permissions` when command has `$(…)`** · long-session hook/MCP memory leaks fixed · MCP tool-pool cache | 👀 watch / ✅ | this doc |
| 2.1.207 | **agent-teams mailbox crash-loop fixed** · skill/worktree bracket-glob parse fixes · plugin shell-form `${user_config.*}` rejected | 🟢 win / ✅ | this doc |
| 2.1.206 | **MCP per-server `request_timeout_ms` honored** · OAuth MCP refresh recovery · `EnterWorktree` confirm outside `.claude/worktrees/` | ✅ / 👀 | this doc |
| 2.1.205 | **background notifications deny fabricated human-approval** · `--json-schema` invalid-schema no longer silent · verify-skills rewrite thrash fixed | 🟢 / ✅ | this doc |
| 2.1.204 | **SessionStart hook streaming in headless — no more mid-hook idle-reap** | 🟢 direct win | this doc |
| 2.1.203 | **Bash "arg list too long" with many git worktrees fixed** · worktree-isolated subagent cwd fix · subagents less re-delegate | 🟢 wins | this doc |
| 2.1.202 | Workflow script parse fixes · **re-invoked skill no longer duplicates instructions** · resume-picker slow with many worktrees fixed · `/review` back to single-pass | ✅ no action (wins ride free) | this doc |
| 2.1.201 | Sonnet 5 sessions drop mid-conversation system role for harness reminders | ✅ no action | this doc |
| 2.1.200 | **`AskUserQuestion` no longer auto-continues by default** · "default" mode renamed "Manual" · tmux 3.4+ flicker fix | 👀 watch | this doc |
| 2.1.199 | **SessionStart/SubagentStart stderr no longer hidden on exit 2** · SendMessage respawn-name misroute fix · subagent API-error reporting | 🟢 already covered / ✅ | this doc |
| 2.1.198 | **Subagents background by default** · **agent-teams: dead teammate reports "failed" to lead** · launcher messages = task direction | 👀 watch / 🟢 win | this doc |
| 2.1.197 | **Claude Sonnet 5 GA — new CC default model, 1M context, $2/$10 promo to Aug 31** | 👀 opportunity | this doc |
| 2.1.196 | MCP self-approval tightening · streaming idle watchdog **default-on** · `/code-review` −25% tokens | ✅ no action / 👀 watch | this doc |
| 2.1.195 | **Hook matchers with hyphenated identifiers now exact-match** (Cassy uses `matcher:"task-verifier"`) | 🟢 already aligned | this doc |
| 2.1.193 | `autoMode.classifyAllShell` · OTEL `assistant_response` logging default | ✅ no action | — |
| 2.1.191 | **Comma-separated hook matchers silently never firing — fixed**; permanent stop-agent | ✅ no action | this doc |
| 2.1.187 | **Schema structured-output reliability** · subagent depth · agent-worktree leak cleanup | 👀 watch | this doc |
| 2.1.186 | **`Agent(type)`/`Agent(x,y)` enforced for named spawns** · `claude mcp login/logout` | ✅ no action | this doc |
| 2.1.183 | **tmux teammate pane launch + spawn keystroke-leak fix** · destructive-git auto-block | 👀 touchpoint | this doc |
| 2.1.181 | Bun 1.4 · foreground-subagent depth cap · `mcp get/list` tools-fetch status | ✅ no action | — |
| 2.1.178 | **Agent Teams: `TeamCreate`/`TeamDelete` removed → implicit team** · nested skills · `disallowedTools` MCP specs | 🟢 already covered | this doc |
| 2.1.176 | skill hot-reload · hook `if`-conditions for Read/Edit/Write paths · Fable 5 auto-mode fallback | ✅ no action | — |
| 2.1.174 | **skill hot-reload only re-announces changed skills** · Workflow `agent()` attribution | 🟢 already covered | this doc |
| 2.1.170 | **Claude Fable 5** (Mythos-class model) GA + VS Code / inherited-env transcript-save fix | 👀 evaluate | this doc |
| 2.1.169 | **`--safe-mode`**, `/cd`, **`disableBundledSkills`**, `agents --json` state/id, managed-MCP enforcement fixes | ✅ no action / 👀 noted | this doc |
| 2.1.168 | Bug-fix rollup | ⏭ n/a | — |
| 2.1.167 | Bug-fix rollup | ⏭ n/a | — |
| 2.1.166 | `fallbackModel`, deny-rule globs, **SendMessage authority hardening** | ✅ no action | this doc |
| 2.1.165 | Bug-fix rollup | ⏭ n/a | — |
| 2.1.163 | Managed version pinning, `/plugin list`, **Stop/SubagentStop `additionalContext`** | 👀 opportunity | this doc |
| 2.1.162 | `claude agents` polish, perm-rule correctness, **Ctrl+V image paste** | 🟢 already covered | this doc |
| 2.1.161 | Telemetry labels, MCP secret redaction, parallel-tool isolation | ✅ no action | this doc |
| 2.1.160 | Sensitive-file write prompts, **"workflow"→"ultracode" rename** | ✅ no action | this doc |
| 2.1.159 | Internal infra | ⏭ n/a | — |
| 2.1.152–160 | Hook surface (reloadSkills, sessionTitle, disallowed-tools, MessageDisplay) | 🏗 EPIC → shipped v2.18.0 | [cc160 note](2026-06-02-cc160-hook-surface.md) |

---

## Entries

### 2.1.231–2.1.221 — MCP, worktree, hook, and lifecycle reliability sweep

Reviewed 2026-08-13. Host on **2.1.231**. Source: [Anthropic's official
changelog](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md),
covering 2.1.221–2.1.229 and 2.1.231; the official changelog has no 2.1.230
section.

- **2.1.231 repairs an MCP OAuth redirect-URI mismatch; 2.1.229 fixes strict OAuth redirects,
  remote-session startup with managed MCP, and adds server-supplied hooks for self-hosted runners.**
  → ✅ / 👀 **no direct Cassy change; retain MCP startup watch.** Cassy's local `cs` stdio server has no
  OAuth, but the new behavior remains relevant when a workspace also configures remote MCP servers.
  Cassy's SessionStart/PreToolUse contracts remain configuration-owned; do not treat runner-provided
  hooks as a substitute for them.
- **2.1.229 fixes CPU-limited dynamic-workflow fan-out; 2.1.225–2.1.224 improve headless and
  cross-session message delivery; 2.1.221 makes background sessions commit/push and gives forked
  sessions their own worktree.** → 🟢 **lifecycle and isolation wins.** Cassy keeps task ownership,
  its explicit factory concurrency limit, and worktree records as the authority. The host changes
  reduce stalled or misplaced work without changing that contract.
- **2.1.228 hardens cloud-synced skills against shadowing local commands/MCP prompts and fixes
  deferred-tool duplication; 2.1.229 refreshes command-source plugins per session.** → 👀 **watch
  skill precedence.** On a Claude upgrade, verify the Cassy-synced skill/agent mirror still wins
  predictably in a worktree and that required worker instructions are not silently displaced.
- **2.1.224–2.1.221 close destructive-git/worktree, permission-check, background-hook, print-mode
  MCP-startup, and MCP-error-visibility gaps.** → 🟢 **direct safety and evidence wins.** Cassy's
  own clone-path, hook, and verifier boundaries remain defense in depth; no runtime change follows.
- **2.1.227 is UI/reliability-only and 2.1.226 is a generic rollup.** → ✅ / ⏭ **no action.**
  **Source gap:** 2.1.230 has no official section, so no behavior or verdict is inferred for it.

### 2.1.220 — generic bug-fix and reliability rollup

Reviewed 2026-07-30. Host on **2.1.220**. Source: [Anthropic's official
changelog](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md#21220).

- Anthropic publishes only **"Bug fixes and reliability improvements"** for this version, with no
  affected component, command, hook, agent, or MCP surface named. → ⏭ **source-limited.** There is
  not enough official evidence to attribute a specific fix or regression to Cassy. The installed-version
  status is current; the Cassy verdict remains deliberately non-specific.

### 2.1.219 — Opus 5 default · bounded workflow delegation · MCP diagnostics

Reviewed 2026-07-30. Source: [Anthropic's official
changelog](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md#21219).

- **Claude Opus 5 (`claude-opus-5`) is the default Opus model.** → 👀 **configuration watch, no code
  change.** Cassy's Claude worker fallback deliberately resolves to the stable `opus` alias
  (`cas-cli/src/mcp/tools/service/factory_ops.rs`), and the PTY launcher passes that resolved value
  through `--model` (`crates/cas-pty/src/pty.rs`). The alias therefore follows Anthropic's new Opus
  default by design; deployments that require a fixed model can continue to set an explicit worker
  model.
- **Dynamic workflows now default to a medium size guideline (fewer than 15 agents), while nested
  subagents may spawn to depth 3 by default.** → 🟢 **Cassy remains explicitly bounded.** The
  `cas-code-review` Workflow selects a finite persona set and enforces
  `CODEX_MAX_CONCURRENCY = 4` in `.claude/workflows/cas-code-review.js`; its reviewer agents do not
  depend on recursive delegation. The broader host default does not replace those workflow-owned
  bounds, so no Cassy change is required.
- **Headless init can report invalid `--mcp-config` entries, and `claude mcp list`/`/mcp` now expose
  connection status and error text.** → ✅ **diagnostics win.** Cassy factory startup requires a
  committed `.mcp.json` so isolated sessions receive Cassy tools (`cas-cli/src/cli/factory/mod.rs`), and
  `cas doctor` already validates its `mcpServers.cas` entry. The new host diagnostics make a skipped or
  failed server easier to distinguish from a missing Cassy tool; the `mcp_server_errors` init field is
  additive and requires no Cassy protocol change.

### 2.1.218 — background built-in review · tool/transcript evidence correctness

Reviewed 2026-07-30. Source: [Anthropic's official
changelog](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md#21218).

- **The built-in `/code-review` now runs as a background subagent.** → ✅ **no action — separate
  authority path.** Cassy uses its own `cas-code-review` Workflow and keeps review ownership with the
  supervisor (`cas-cli/src/builtins/skills/cas-code-review/SKILL.md`). Backgrounding Claude's bundled
  command neither invokes nor bypasses Cassy's close and review gates.
- **Tool-executor errors are no longer silently dropped, aborted responses no longer leave an
  unpaired `tool_use` block, and headless/SDK forks retain lineage after compaction.** → 🟢 **evidence
  integrity win.** Cassy records successful tool activity in its PostToolUse attribution path
  (`cas-cli/src/hooks/handlers/handlers_events/attribution.rs`) and task verification relies on
  coherent tool/transcript evidence (`docs/verifier-dispatch-trace.md`). These host fixes reduce false
  evidence gaps without changing Cassy's hook schema or verifier contract.

### 2.1.217 — bounded subagent fan-out · transcript/MCP memory safety · background isolation

Reviewed 2026-07-22 (diary-claude / cas-9642). Host on **2.1.217**.

- **Concurrent subagents are capped at 20 by default, nested spawning is off by default, and
  `--max-budget-usd` now halts background subagents.** → 🟢 **direct safety win.** Cassy verification and
  `cas-code-review` use bounded subagent dispatch (`.claude/agents/task-verifier.md` and the
  `cas-code-review` skill); ordinary fan-out remains well below 20. The host now also bounds accidental
  recursive or runaway delegation. No Cassy change; `CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS` and
  `CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH` are escape hatches if an intentional workflow grows.
- **Transcript-write failures and inherited session-saving disablement now warn instead of silently
  losing history.** → 🟢 **debuggability win.** Cassy incident work relies on session JSONL/transcripts as
  proof, so silent loss made hook and worker failures materially harder to diagnose. No Cassy change.
- **Truncated MCP outputs no longer retain the full result, and background-session isolation now
  canonicalizes symlinked working directories.** → ✅ / 🟢 **no action — direct wins.** Factory turns
  use a large MCP surface for long sessions, while Cassy separately stamps isolated worker paths with
  `CAS_CLONE_PATH` (`crates/cas-pty/src/pty.rs`). These fixes reduce memory growth and close a host-side
  workspace escape; Cassy's own path/branch guards remain defense in depth.
- **Auto-compact now works for Opus 4.8 on Bedrock and `/compact` recovers after the limit.** → ✅ no
  action; long-running supervisors benefit without changing Cassy compaction guidance.

### 2.1.216 — worktree escape closure · resumed-agent identity · long-session responsiveness

Reviewed 2026-07-22.

- **Worktree-isolated subagents can no longer redirect git into the shared checkout via `git -C`,
  `--git-dir`, or Git environment variables; sessions also stop landing in another project's stale
  worktree.** → 🟢 **direct isolation win.** Cassy already creates and records factory worktrees through
  its own worktree manager and enforces `CAS_CLONE_PATH` at SessionStart
  (`cas-cli/src/hooks/handlers/handlers_session.rs`). The host now closes bypasses beneath those
  guards. No Cassy change.
- **Resumed background agents now restore their agent prompt and tool restrictions, and a
  high-priority message no longer cancels a background subagent during startup.** → 🟢 **lifecycle
  win.** The task-verifier's restricted prompt/tool contract is load-bearing
  (`.claude/agents/task-verifier.md`); resume/wake behavior now preserves it instead of silently
  reverting to the default agent.
- **Long-session message normalization no longer grows quadratically with turn count.** → ✅ no
  action — factory supervisors and workers are deliberately long-lived, so the stall/resume fix rides
  free.
- **Skills changed during a session now appear without restart; plugin skill prefixes are preserved.**
  → ✅ no action; aligns with Cassy skill sync/hot-reload behavior and removes host-side stale discovery.

### 2.1.215 — no autonomous `/verify` or `/code-review`

Reviewed 2026-07-22. **Single-item release.**

- **Claude no longer invokes `/verify` or `/code-review` on its own.** → ✅ **no action — authority
  boundary clarified.** Cassy's close gate explicitly dispatches the `task-verifier` agent and the
  supervisor owns `cas-code-review`; neither depends on Claude opportunistically choosing a similarly
  named built-in skill (`docs/verifier-dispatch-trace.md`). Explicit Cassy lifecycle calls remain intact.

### 2.1.214 — hook blocking restored · tool heartbeat · permission hardening

Reviewed 2026-07-22.

- **Hooks exiting 2 now block even when their stdout JSON fails schema validation.** → 🟢 **direct
  hook-safety win.** Cassy uses exit-2 semantics on load-bearing SessionStart/SubagentStart and tool
  hooks (`cas-cli/src/hooks`); malformed diagnostic JSON can no longer accidentally turn a deny into
  continued execution. No Cassy change.
- **Long-running tool calls now emit periodic progress heartbeats.** → 🟢 **unattended-worker win.**
  A factory pane now visibly distinguishes a legitimately long tool operation from an apparently
  wedged worker. This complements Cassy process/session heartbeats rather than replacing them.
- **Permission checks were hardened for path globs, Bash redirects/long commands, PowerShell, remote
  prompts, and Docker daemon redirects.** → ✅ no action; host-side defense in depth. Factory workers
  normally use bypass permissions, while Cassy retains its own branch/path commit guards.
- **SessionStart reports source `"fork"` for forked sessions instead of `"resume"`.** → ✅ **already
  compatible.** Cassy's SessionStart dispatcher handles the shared event and does not require the old
  mislabeled source (`cas-cli/src/hooks/mod.rs`, `handlers_session.rs`).
- **Slow stream-json consumers now drain complete output at exit, and OTel logs gained correlation and
  tool-provenance fields.** → ✅ no action; improves headless evidence and observability without a Cassy
  protocol change.

### 2.1.213 — official changelog section absent

Reviewed 2026-07-22.

- Anthropic's official raw `CHANGELOG.md` jumps from **2.1.214** to **2.1.212**; there is no 2.1.213
  section or item to evaluate. → ⏭ **source gap.** Recorded explicitly to keep version coverage
  complete without inventing a release note or Cassy verdict.

### 2.1.212 — delegation budgets · long MCP backgrounding · hook/worktree correctness

Reviewed 2026-07-22.

- **Sessions now cap subagent spawns at 200 (reset by `/clear`), and WebSearch calls at 200.** → 🟢
  **runaway-loop safety win.** Cassy review/verification dispatch is intentionally bounded; normal runs
  stay far below these ceilings. `CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION` is available if a future large
  review fan-out proves constrained.
- **MCP calls longer than two minutes automatically move to the background.** → 👀 **watch — Cassy tool
  semantics.** Search, coordination, and close calls can be long-running on large stores. The session
  staying usable is beneficial, but if a caller assumes a foreground result, first inspect
  `CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS` when diagnosing an apparently detached Cassy call. No concrete
  defect observed, so no task filed.
- **A `continue:false` hook halt is no longer lost when a tool fails/completes mid-stream, and hook
  infrastructure errors are no longer reported as user rejection.** → 🟢 **direct hook-authority
  win.** Cassy PreToolUse guards rely on harness halts being final; this removes a race and preserves the
  distinction between policy and human decisions.
- **Worktree creation no longer follows a committed `.claude/worktrees` symlink outside the repo.** →
  ✅ no action / host hardening. Cassy factory uses `.cas/worktrees` via its own manager, while raw
  `Agent(isolation: "worktree")` is separately denied for factory supervisors.
- **Inter-agent `SendMessage` bodies are no longer duplicated in replayed history, agent JSON exposes
  `Needs input`, and SIGTERM kills print/SDK Bash process trees.** → ✅ no action — smaller context,
  clearer stall state, and cleaner unattended shutdown all ride free.

### 2.1.211 — subagent identity/model continuity · truthful completion · shared approvals

Reviewed 2026-07-22.

- **Explicit subagent model overrides now survive resume and follow-up messages.** → 🟢 **direct
  orchestration win.** Cassy's factory-worker model/effort pinning happens separately at PTY launch
  (`crates/cas-pty/src/pty.rs`); for native verifier/review subagents that do receive an explicit
  override, resume or follow-up no longer silently falls back to the parent model.
- **Background-agent result reporting now waits for real completion instead of fabricating results,
  and user-killed agents no longer auto-respawn with stale prompts.** → 🟢 **authority/lifecycle win.**
  This matches Cassy's rule that completion must be proven and launcher messages are task direction,
  not human approval. Factory workers remain tmux-managed, but native subagent verification benefits.
- **"Always allow" rules now save at repository root so approvals persist across worktrees.** → ✅ no
  action. Cassy factory normally bypasses host permission prompts and independently protects worker
  branch/path scope; interactive non-DSP sessions get more consistent worktree behavior.
- **`--forward-subagent-text` can include subagent text/thinking in stream-json.** → 👀 opportunity,
  not required. It could improve headless verifier diagnostics, but Cassy does not need to expose hidden
  reasoning and no concrete evidence gap warrants changing spawn flags.

### 2.1.210 — isolated git mutation · hook-timeout semantics · dead-worker cleanup

Reviewed 2026-07-22.

- **`isolation: "worktree"` subagents can no longer run git-mutating commands against the main
  checkout.** → 🟢 **direct isolation win.** Cassy blocks factory supervisors from raw worktree-isolated
  Agent spawns and routes workers through its own manager; the host now also closes the residual raw
  native-subagent cross-checkout mutation path. No Cassy change.
- **Hook callback timeouts are no longer described to the model as user rejection.** → 🟢 **unattended
  worker win.** Cassy relies on hooks for SessionStart context and tool guards; a timeout is infrastructure
  failure, not human denial. Correct classification prevents a worker from stopping to await approval
  that never occurred.
- **Dead background sessions no longer leave permanent git worktree locks; the periodic sweep releases
  locks after the owner exits.** → 🟢 **cleanup win.** Cassy's worktree manager remains authoritative for
  factory trees, while host cleanup reduces stale-lock interference around native subagents.
- **Plugin MCP servers survive mid-session re-sync, SDK-initialized MCP servers connect in the same
  turn, and Agent received indirect-prompt-injection hardening.** → ✅ no action — reliability and
  security improvements on surfaces Cassy uses without requiring a protocol or prompt change.

### 2.1.209 — dialogs unblocked in `claude agents` background sessions

Reviewed 2026-07-14 (w-claude-diary / cas-aeec9). Host on **2.1.209**.

- **"Fixed /model and other dialogs being blocked in `claude agents` background sessions (reverts an
  overly broad guard)."** → ⏭ **n/a for Cassy factory.** Factory workers are CC instances in **tmux
  panes**, not the `claude agents` background-daemon surface. No Cassy change.

### 2.1.208 — DSP catastrophic-rm prompt · hook/MCP long-session leaks · MCP tool-pool cache

Reviewed 2026-07-14.

- **"Catastrophic removals (e.g. `rm -rf ~`) in commands containing `$(…)`/backticks/`<(…)` now prompt
  in `--dangerously-skip-permissions` and auto mode, matching the plain form."** → 👀 **watch —
  factory permission surface.** Factory workers spawn with `--permission-mode bypassPermissions` /
  `--dangerously-skip-permissions` (`cas-pty` worker args). Plain catastrophic `rm` already prompted;
  substitution-wrapped forms now do too. Expected effect: a worker that builds `rm -rf $(…)` toward a
  home-path-ish target can stall on a permission dialog even under DSP — stall detection is the
  backstop. Not a break of the DSP path for normal work; recorded as the failure shape if a pane
  freezes on shell cleanup.
- **"Fixed several memory leaks in long sessions: MCP stdio server stderr accumulating up to 64 MB per
  server, … async hook output retained after backgrounding, …"** → ✅ **no action — direct win.** Cassy
  rides SessionStart/SubagentStart/Stop hooks + multi-server MCP for the whole factory lifetime;
  unbounded hook-output + MCP-stderr retention was exactly the long-session memory shape. Rides free.
- **"Reduced per-tool-call CPU overhead in print/SDK sessions with many MCP tools by caching tool-pool
  assembly (up to 7x faster tool rounds at high tool counts)."** → ✅ no action; Cassy sessions expose a
  large MCP tool surface (`cas__*`, plus host MCPs). Pure perf.
- **"Fixed the Agent tool launching with no tools when a subagent's `tools` list resolves to nothing —
  it now returns a clear error naming the unrecognized entries."** → ✅ no action; clearer failure for
  `cas-code-review` Workflow persona dispatch / named Agent spawns with bad tool allowlists.
- **"Fixed multi-second per-turn slowdowns in sessions with many permission deny/ask rules — rule
  matchers are now compiled once and cached."** → ✅ no action; host-side if anyone runs dense deny
  rules outside DSP.
- Screen reader / vim remaps / mouse / Remote Control / Bedrock SSO / large-table render → ⏭ n/a.

### 2.1.207 — agent-teams mailbox crash-loop · skill/worktree glob parse · plugin hook shell form

Reviewed 2026-07-14.

- **"Fixed a crash loop in agent teams where a malformed teammate mailbox message caused repeated
  errors every second until the mailbox file was manually deleted."** → 🟢 **direct factory win.** Cassy
  factory rides CC agent teams (memory `reference_cas_factory_uses_cc_agent_teams_cli_flags`); a
  corrupted mailbox previously required manual file surgery to unstick a pane. No Cassy change;
  shrinks a known zombie-team failure class (pairs with 2.1.198 teammate-"failed" reporting).
- **"Fixed malformed bracket patterns in rules globs, skill paths, `.ignore`, and `.worktreeinclude`
  breaking file reads, file suggestions, and worktree creation."** → ✅ **no action — de-flake.** Cassy
  ships skill paths and worktree isolation; bad bracket globs previously could break reads / worktree
  create rather than failing clearly. Rides free.
- **Plugin hooks/monitors/MCP headersHelper: `${user_config.*}` in shell-form commands is now rejected
  (shell-injection fix).** → ✅ **no action.** Cassy hooks are installed as shell-form `cas hook
  <Event>` without `${user_config.*}` interpolation (`cas init` SessionStart path). Plugin-authored
  hooks that did use that expansion must migrate to exec form / `$CLAUDE_PLUGIN_OPTION_*` — not a Cassy
  surface.
- Auto-mode default-on for Bedrock/Vertex/Foundry; `autoMode` no longer read from
  `.claude/settings.local.json` → ⏭ for factory (workers are DSP/bypassPermissions, not auto mode).
- Terminal freezes on long lists/tables, Remote Control, Bedrock SSO, Opus 4.8 cloud defaults → ⏭ /
  ✅ host-only.

### 2.1.206 — MCP request_timeout_ms · OAuth MCP refresh · EnterWorktree outside `.claude/worktrees/`

Reviewed 2026-07-14.

- **"Fixed MCP servers configured via `--mcp-config` or `.mcp.json` ignoring a per-server
  `request_timeout_ms`, which caused long-running MCP tool calls to time out at the 60s default in
  fresh sessions."** → ✅ **no action — reliability win.** CAS MCP tools (search, task list, heavy
  coordination) can exceed 60s on large stores; hosts that set a higher per-server timeout now get
  the configured value instead of a silent 60s clip. No Cassy code change.
- **"Fixed OAuth MCP servers requiring manual re-authentication after a single failed token refresh."**
  → ✅ no action; de-flakes host MCP OAuth (GitHub, etc.) mid-factory.
- **"`EnterWorktree` now asks for confirmation before entering a git worktree outside the project's
  `.claude/worktrees/` directory."** → 👀 **watch — path-shape note.** Cassy factory worktrees live under
  `.cas/worktrees/…`, not `.claude/worktrees/`. Cassy creates/isolates via its own worktree coordination
  path, not CC's `EnterWorktree` tool, so the confirm should not gate normal factory spawn. Residual
  risk: an agent that *calls* `EnterWorktree` into a Cassy worktree path may now get a human confirm
  prompt — same unattended-pane hang class as the 2.1.200 `AskUserQuestion` note. No Cassy change unless
  that shape shows up in the wild.
- Background-agent auto-upgrade after CC update, `/code-review` opus quality, agents-view Ctrl+X →
  ⏭ / adjacent (built-in `/code-review`, not `cas-code-review`).

### 2.1.205 — fabricated-approval denial · json-schema validity · verify-skills rewrite thrash

Reviewed 2026-07-14.

- **"Background task notifications now explicitly state that no human input has occurred, preventing
  fabricated in-transcript approvals from being acted on."** → 🟢 **authority-model win.** Same family
  as 2.1.166 SendMessage hardening and 2.1.198 "launcher messages ≠ user approval." Cassy factory already
  treats agent-to-agent direction as non-approval; this closes a background-notification path that could
  look like a human yes. No Cassy change.
- **"Fixed `--json-schema` silently producing unstructured output when the schema was invalid, and
  schemas using the `format` keyword being rejected."** → ✅ no action / residual of 2.1.187
  structured-output hardening. Helps Workflow `agent({schema})` paths used by `cas-code-review`
  (Phase C) fail loudly on bad schemas instead of returning free-form prose.
- **"Fixed project verify skills being rewritten on every session instead of only when a documented
  command changed."** → ✅ no action; skill hot-reload thrash reduction (pairs with 2.1.174
  re-announce-only-changed). Cassy skill sync already prefers stable skill bodies.
- **"Fixed background agents staying shown as 'failed' or 'completed' in the agent list after being
  resumed with `SendMessage`."** → ✅ no action for tmux factory; residual native agent-list correctness
  when anyone uses CC background agents + SendMessage.
- Auto-mode transcript-tamper block / `rm -rf` on unresolved vars → ⏭ (auto mode, not DSP factory).
- Reserved "Claude Browser" MCP name → ⏭ unless a host MCP collides on that exact name.

### 2.1.204 — SessionStart hook streaming in headless (idle-reap mid-hook)

Reviewed 2026-07-14. **Small release, high Cassy signal.**

- **"Fixed hook events not streaming during SessionStart hooks in headless sessions, which could cause
  remote workers to be idle-reaped mid-hook."** → 🟢 **direct win on the load-bearing Cassy surface.**
  SessionStart is how Cassy injects factory supervisor/worker guidance, task context, and session
  mapping (`cas hook SessionStart`). A headless/remote worker that looked idle while SessionStart was
  still running could be reaped before the hook finished — exactly the "worker died before it ever
  talked" shape. Interactive tmux factory panes were less exposed, but any headless/`claude -p`/remote
  Cassy path was. No Cassy change; rides free on the host bump. Pairs with 2.1.199 SessionStart stderr-on-
  exit-2 visibility for hook debuggability.

### 2.1.203 — many-worktree Bash · subagent worktree cwd · re-delegation reduction

Reviewed 2026-07-14.

- **"Fixed Bash failing with 'argument list too long' in repos with many git worktrees."** → 🟢
  **direct factory win.** Factory hosts accumulate `.cas/worktrees/*` (and related) by design; the
  same class of "many worktrees" pain as the 2.1.202 resume-picker fix. Shell ops that expanded
  worktree lists no longer blow `ARG_MAX`. No Cassy change.
- **"Fixed worktree-isolated subagents sometimes running shell commands in the parent checkout instead
  of their own worktree."** → 🟢 **isolation correctness win.** Cassy worktree-isolated workers and
  subagents depend on cwd staying inside the assigned tree; parent-checkout leakage is a silent
  cross-worker corruption risk. Rides free.
- **"Improved subagent behavior: agents are now less likely to re-delegate their entire task to another
  subagent."** → ✅ no action; quality win for task-verifier / `cas-code-review` persona fan-out (less
  nested Agent thrash).
- **"Fixed `TaskStop` and `TaskOutput` failing to find background agents spawned by another agent —
  errors now list running agents by id and description."** → ✅ no action; clearer nested-agent
  control surface.
- **"Added the session's additional working directories to MCP `roots/list`, with
  `notifications/roots/list_changed` when the set changes."** → ✅ no action / minor MCP correctness
  for multi-root sessions; CAS MCP servers that honor roots see the fuller set.
- Manual-mode footer ⏸ badge, background-session daemon recovery, `claude agents` UI polish → ✅ /
  ⏭ (badge is cosmetic for Manual mode; factory is DSP).
- Login-expiry warning before background sessions drop → ✅ host hygiene, not Cassy-owned.

### 2.1.202 — Workflow parse fixes · re-invoked skill dedup · worktree-heavy resume perf

Reviewed 2026-07-07 (patient-condor-18 / supervisor). Sweep of 2.1.197–2.1.202. Local host is on
**2.1.201** at review time, so everything through .201 is already live behavior here, not hypothetical.

- **"Fixed re-invoking an already-loaded skill appending a duplicate copy of its instructions to
  context."** → ✅ **no action — direct win.** Long supervisor sessions re-invoke Cassy skills
  (`cas-supervisor`, `verify-before-claim`, `cas-code-review`) repeatedly; each re-invoke was silently
  duplicating the skill body in context. This fix is pure context-bloat relief for exactly our usage
  pattern. Rides free on the bump.
- **"Fixed resuming a session by name, or opening the resume picker, taking minutes and using a large
  amount of memory in repositories with many git worktrees."** → ✅ **no action — direct win.** Factory
  hosts accumulate `cas-worktrees-*` dirs by design; this was our exact slow-resume shape. Nothing to
  change in Cassy.
- **"Fixed workflow scripts with unicode quote escapes in strings being corrupted before parsing;
  workflow parse errors now show the offending line."** + `workflow.run_id`/`workflow.name` OTel
  attributes. → ✅ no action; de-flakes + improves debuggability of the `cas-code-review` Workflow
  scripts (Phase C, cas-b667). No Cassy change.
- **"Changed `/review <pr>` back to a fast single-pass review; use `/code-review <level> <pr#>` for
  multi-agent."** → ✅ no action — CC's *built-in* review surfaces, distinct from `cas-code-review`
  (same disambiguation as the 2.1.196 token-cut note). Logged so nobody mistakes it for a Cassy change.
- "Dynamic workflow size" `/config` setting (advisory agent-count guideline) → ✅ no action; could be
  a host-side knob if `cas-code-review` fan-out ever feels over/under-sized, but it's advisory only.

### 2.1.201 — Sonnet 5 drops mid-conversation system role for harness reminders

Reviewed 2026-07-07.

- **"Claude Sonnet 5 sessions no longer use the mid-conversation system role for harness reminders."**
  → ✅ **no action, precision note.** Cassy hooks inject guidance via `additionalContext` /
  system-reminder-shaped payloads; this changes what *role* the harness wraps them in on Sonnet 5
  sessions, not whether they're delivered. Local host has run .201 with Cassy hooks active and
  SessionStart context + supervisor reminders demonstrably still land (this very session). Nothing to
  verify further.

### 2.1.200 — AskUserQuestion no auto-continue · "default" → "Manual" · tmux 3.4+ flicker fix

Reviewed 2026-07-07.

- **"Changed `AskUserQuestion` dialogs to no longer auto-continue by default; opt into an idle timeout
  via `/config`."** → 👀 **watch — unattended-pane hang class.** Previously an unanswered
  `AskUserQuestion` eventually auto-continued; now it blocks until answered. Cassy already steers this
  surface: the cas-e603 PreToolUse reminder (`pre_tool.rs:197-210`) tells factory *supervisors*
  AskUserQuestion routes to the human only, and `--yolo`-equivalent workers shouldn't be prompting at
  all. But if a factory agent does call it in an unattended pane, the pane now hangs indefinitely
  where it previously self-resolved — stall detection is the backstop, and the `/config` idle-timeout
  opt-in is the lever if this ever bites. No code change; recorded as the failure shape to recognize.
- **"default" permission mode renamed "Manual" (`--permission-mode manual` accepted alongside
  `default`).** → ✅ no action. Factory workers spawn with `--dangerously-skip-permissions`
  (bypassPermissions); Cassy never passes `--permission-mode default`, and the old spelling stays
  accepted anyway.
- **"Fixed rendering flicker under tmux 3.4+ by enabling synchronized terminal output."** → ✅ no
  action — direct win. Factory workers are CC instances inside tmux panes; less flicker in exactly our
  render path. (Adjacent to, not overlapping, the Konsole AlternateScrolling scrollback issue —
  that one is terminal config, memory `project_konsole_alternate_scrolling_breaks_factory`.)
- Startup crash on non-array `disabledMcpServers`; background-session stall/daemon-lock/roster fixes
  → ✅ no action / ⏭ n/a (Cassy factory uses tmux workers, not CC background agents).

### 2.1.199 — hook stderr visibility · SendMessage respawn-name misroute · subagent error reporting

Reviewed 2026-07-07.

- **"Fixed `SessionStart`, `Setup`, and `SubagentStart` hooks silently hiding stderr when exiting with
  code 2 — the error is now shown in the transcript."** → 🟢 **already aligned — debuggability win.**
  SessionStart (context injection) and SubagentStart (`matcher: "task-verifier"` unjail) are both
  load-bearing Cassy hook surfaces. A Cassy hook bug that exits 2 was previously invisible; now the stderr
  lands in the transcript. Pairs with the standing memory `feedback_verify_hook_runtime_via_jsonl` —
  the JSONL grep is still the ground truth, but first-line triage just got easier.
- **"Fixed `SendMessage` silently misrouting when a re-spawned agent reuses a previous agent's name —
  the tool now detects the mismatch and asks the caller to retarget."** → 🟢 **mostly covered, residual
  path helped.** In factory mode Cassy *intercepts* native SendMessage in PreToolUse and reroutes onto
  the Cassy prompt queue (`auto_route_send_message`, returns deny), so worker-to-worker traffic never
  rode the broken native path. The residual native surface (supervisor ↔ team-lead, non-factory
  teams) does respawn agents under reused display names (memory
  `feedback_reassign_collision_near_limit_worker`), so the mismatch detection is a genuine safety net
  there. No Cassy change.
- **"Subagents cut off by a rate limit or server error now return partial work / report the error to
  the parent instead of claiming success."** → ✅ no action — de-flakes `cas-code-review` Workflow
  persona dispatch (an API-errored persona previously looked like an empty-but-successful review).
  Same family as the 2.1.187 structured-output hardening.
- **Retry hardening: transient 429s auto-retry with backoff for subscribers;
  `CLAUDE_CODE_RETRY_WATCHDOG` raises retry ceilings.** → ✅ no action; strictly helpful for long
  factory runs. The env var is a lever if a factory host sits on a flaky network.

### 2.1.198 — subagents background by default · agent-teams failure reporting · launcher messages as direction

Reviewed 2026-07-07. **Most factory-relevant release in this sweep.**

- **"Subagents now run in the background by default, so Claude keeps working while they run" (was a
  gradual rollout).** → 👀 **watch — verify the close-time verification flow.** The task-verifier agent
  is spawned at task close; if that spawn is now backgrounded, a close flow could theoretically proceed
  while verification is still running. Mitigating evidence: local host has been on ≥2.1.198 through
  multiple shipped EPICs (2026-07-07 releases) with the verification jail + SubagentStart unjail
  visibly working, so no categorical breakage exists. The hook surface (SubagentStart firing) is
  unchanged by backgrounding. Recorded as the first thing to check if a task ever closes with a
  verification verdict that arrives "late".
- **"Agent teams: a teammate that dies on an API error now reports 'failed' to the lead, and messaging
  a stuck teammate wakes it to retry immediately."** → 🟢 **direct factory win.** Cassy factory rides CC
  agent teams (memory `reference_cas_factory_uses_cc_agent_teams_cli_flags`), and silent worker death
  is a documented pain class (memories `feedback_phantom_assignee_recovery`,
  `feedback_reassign_collision_near_limit_worker`). "Failed" now propagating to the lead + message-to-
  wake shrinks the zombie-worker window. No Cassy change; supervisor playbooks (check activity/task/git
  before respawning) still apply as defense in depth.
- **"Subagents now treat messages from the agent that launched them as normal task direction; an
  agent's message is still never treated as the user's approval."** → ✅ no action; matches how the Cassy
  supervisor already directs workers, and the approval carve-out is the same authority model as the
  2.1.166 SendMessage hardening.
- **Explore agent now inherits the main session's model (capped at opus) instead of haiku.** → ✅ no
  action; quality win for any supervisor using Explore for repo sweeps.
- Claude in Chrome GA, `/dataviz` skill, removed `/agents` wizard, background-agent draft-PR flow →
  ⏭ n/a (host surfaces Cassy doesn't ride).

### 2.1.197 — Claude Sonnet 5 GA: new CC default model, 1M context, promo pricing

Reviewed 2026-07-07.

- **Claude Sonnet 5 ships as the new Claude Code default model, with a native 1M-token context window
  and promotional pricing of $2/$10 per Mtok through 2026-08-31.** → 👀 **opportunity — model-selection
  calculus changed, no forced action.** Current Cassy posture: `STOCK_WORKER_MODEL = "gpt-5.5"`
  (`cas-cli/src/config/settings.rs:557`, the v2.27.0 Codex-default decision), and standing guidance
  "prefer high-effort Sonnet for long factory runs" (memory
  `feedback_codex_budget_prefer_sonnet_long_runs`) — written against the previous Sonnet. What
  changes:
  1. **Claude-path workers that don't pin a model already inherit Sonnet 5** — CC's default moved
     under us. That's a silent quality/context upgrade, not a break.
  2. **1M native context** materially reduces compaction pressure on long worker turns — relevant to
     the supervisor model-tier rubric (memory `project_supervisor_model_tier_rubric`), which should be
     re-scored with Sonnet 5 in the "standard/heavy" tiers.
  3. **Promo pricing ends 2026-08-31** — any cost comparison vs the gpt-5.5 default done before then
     bakes in a 5× discount that expires. Don't re-litigate `STOCK_WORKER_MODEL` on promo numbers
     alone.
  No task filed; the tier-rubric refresh is the natural vehicle when it next gets touched. Backlogged
  below.

Reviewed 2026-06-30 (eager-leopard-33 / supervisor). Sweep of 2.1.171–2.1.196.

- **Security: `claude mcp list`/`get` no longer spawn `.mcp.json` servers that a repo self-approved
  via a committed `.claude/settings.json`; untrusted workspaces show `⏸ Pending approval`.** → ✅ no
  action; strictly safer for anyone inspecting the CAS MCP registration. Cassy registers `mcp__cas__*`
  through user/project config on the *trusted* factory root, not via a committed self-approval, so the
  tightening doesn't touch worker startup. **Smoke on upgrade:** confirm workers in worktrees off the
  trusted root still get `mcp__cas__*` (worktrees inherit parent-repo trust → expected fine).
- **Streaming idle watchdog now ON by default for all providers — aborts + retries when a response
  stream produces no events for 5 min (`CLAUDE_ENABLE_STREAM_WATCHDOG=0` to disable).** → 👀 **watch.**
  A factory-worker turn that legitimately stalls >5 min inside a single long tool with no streamed
  output would now abort + retry. Low risk (Cassy turns stream tool calls regularly), but if a worker
  starts thrashing on a long build/test step, the disable env is the lever.
- **`/code-review` merged five cleanup finders into one (~−25% tokens).** → ✅ no action. That's CC's
  *built-in* `/code-review`; Cassy ships its own `cas-code-review` Workflow + skill (Phase C, cas-b667) —
  a separate surface, no shared code. Logged so the token-cut isn't mistaken for a Cassy change.

### 2.1.195 — hook matchers: hyphenated identifiers now exact-match

Reviewed 2026-06-30. **Highest-relevance item in this sweep for Cassy.**

- **"Fixed hook matchers with hyphenated identifiers (e.g. `code-reviewer`, `mcp__brave-search`)
  accidentally substring-matching — they now exact-match."** → 🟢 **already aligned — verify on
  upgrade.** Cassy registers a `SubagentStart` hook with **`matcher: "task-verifier"`**
  (`cas-cli/src/cli/hook/config_gen.rs:262`) to unjail the verification jail when the `task-verifier`
  agent spawns — a hyphenated identifier, exactly the affected class. Cassy's *intent* has always been
  exact-match (fire only for the `task-verifier` agent), so 2.1.195 makes the matcher behave as
  designed and removes any spurious substring hits. **No Cassy change needed**; on the bump, smoke-test
  that `cas hook SubagentStart` still fires when a task-verifier agent spawns (the close-time
  verification path depends on it).
- `CLAUDE_CODE_DISABLE_MOUSE_CLICKS` and the voice/plugin fixes are host-side → ⏭ n/a.

### 2.1.191 — comma-separated hook matchers never firing (fixed) · permanent stop-agent

Reviewed 2026-06-30.

- **"Fixed hooks with comma-separated matchers (e.g. `"Bash,PowerShell"`) silently never firing."** →
  ✅ **no action — Cassy was never on the broken path.** Cassy's broad tool hook uses a **regex
  alternation** (`matcher: "Read|Write|Edit|Glob|Grep|Bash|NotebookEdit"`, config_gen.rs), not a
  comma list, and its only other matcher is the single-token `task-verifier`. Grepped the generated
  config: no Cassy matcher uses commas, so none were silently dead. Recorded so the next person doesn't
  re-audit it.
- Permanent stop-agent + `/rewind`-before-`/clear` are host UX → ⏭ n/a.

### 2.1.187 / 2.1.186 — Agent(type) enforcement · schema structured-output reliability · subagent depth

Reviewed 2026-06-30. Two adjacent releases with the same Cassy angle.

- **`Agent(type)` deny rules and `Agent(x,y)` allowed-types restrictions are now enforced for named
  subagent spawns (2.1.186).** → ✅ no action. Cassy gates tools through its own PreToolUse hook + skill
  `disallowed-tools` frontmatter (cas-5be8), **not** host `Agent()` permission rules. Stricter host
  enforcement is orthogonal and strictly safer.
- **`--json-schema` / Workflow `agent({schema})` structured output hardened: the model can no longer
  re-call `StructuredOutput` indefinitely after a success, follow-up turns reliably return structured
  output (2.1.187), and schema-validation-failure loops now abort after 5 attempts (2.1.186).** → 👀
  **watch — benefits Cassy.** The `cas-code-review` Workflow and its Steps 3-4 persona dispatch use
  schema-validated `agent({schema})`; these fixes directly de-flake that path. No Cassy change; pick up
  the reliability win on the bump.
- **`claude mcp login <name>` / `logout <name>` CLI (2.1.186).** → ✅ no action; convenience for
  authenticating an MCP server from the CLI (Cassy's `mcp__cas__*` is local stdio, no OAuth, so N/A in
  practice but harmless).
- Subagent depth-tracking fixes + automatic cleanup of leaked agent-worktree registrations (2.1.187).
  → ✅ no action; Cassy factory uses its own tmux workers + worktrees, not CC background-agent worktrees.

### 2.1.183 — tmux teammate pane launch + spawn keystroke-leak fix · destructive-git auto-block

Reviewed 2026-06-30. **Touches the factory spawn path.**

- **"Fixed tmux teammate panes failing to launch when the shell has slow rc-file initialization, and
  keystrokes typed during agent spawn leaking into the new tmux pane instead of the leader prompt."** →
  👀 **touchpoint: factory tmux workers.** Cassy factory spawns workers in tmux panes (cas-pty PTY +
  agent-teams CLI flags; memory `reference_cas_factory_uses_cc_agent_teams_cli_flags`). Slow rc-file
  init + spawn-time keystroke leak is precisely the flake class we've hit. This host fix should *help*
  Cassy spawn reliability; **verify on upgrade** that worker panes come up clean and supervisor
  keystrokes typed during a spawn don't leak into the new worker pane.
- **Auto mode now blocks destructive git (`reset --hard`, `checkout -- .`, `clean -fd`, `stash drop`),
  amend of non-agent commits, and `terraform/pulumi/cdk destroy`.** → ✅ no action — same shape as the
  .160 sensitive-file note: factory workers run `--dangerously-skip-permissions` (bypassPermissions),
  which short-circuits auto-mode classification. Non-factory users get the safety net.
- **"Fixed background tasks started by a teammate being killed when the teammate finishes a turn."** →
  ✅ no action / 👀 noted. Relevant only if Cassy leaned on CC-native turn-scoped teammate background
  tasks — it doesn't; factory workers are long-lived tmux sessions.
- WebSearch-in-subagents fix; MCP auth-stub tools no longer exposed in headless/SDK → ✅ no action.

### 2.1.178 — Agent Teams: TeamCreate/TeamDelete removed → implicit team · nested skills · disallowedTools MCP specs

Reviewed 2026-06-30. **Strategically the most important entry in this sweep.**

- **"Agent teams: removed the `TeamCreate` and `TeamDelete` tools. With
  `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`, every session now has one implicit team — spawn teammates
  directly with the Agent tool's `name` parameter. The `team_name` parameter is still accepted but
  ignored."** → 🟢 **already covered — confirms Cassy's posture.** Cassy factory already rides CC agent
  teams through **CLI flags + PTY env** (`--team-name` / `--agent-id` at spawn; memory
  `reference_cas_factory_uses_cc_agent_teams_cli_flags`), **never** via the `TeamCreate`/`TeamDelete`
  tools. Their removal is a no-op for Cassy. Caveat to watch: the now-ignored *`team_name` tool
  parameter* is a different surface from the Cassy CLI `--team-name` *flag* — the flag is unaffected, but
  if any Cassy worker prompt still instructs a model to call `TeamCreate`, that's now dead. (We use the
  implicit-team model already; no such instruction found at review time — `teams.rs` is the only
  matcher hit and it's the factory daemon's own team wiring, not a `TeamCreate` call.)
- **Nested `.claude/skills` now load with `<dir>:<name>` on name clash; closest-dir
  agent/workflow/output-style wins.** → ✅ no action, 👀 namespace note. Cassy syncs builtin skills into
  `.claude/skills` (+ `.codex/skills`); nested-load + clash-qualification is host behavior Cassy rides.
  No collision expected (Cassy skills live at the project `.claude/skills` root).
- **MCP server-level specs (`mcp__server`, `mcp__server__*`, `mcp__*`) in subagent `disallowedTools`
  no longer silently ignored.** → 🟢 already covered; Cassy's disallowed-tools gating (cas-5be8) relies
  on these specs being honored, so the fix makes host enforcement match Cassy's intent.
- **Compaction now honors `--fallback-model`; Linux sandbox no longer fails when `.claude/skills` or
  `.claude/hooks` is a symlink.** → ✅ no action; the symlink fix is mildly relevant — Cassy's user-level
  skill fallback can symlink `.claude/skills` (memory `project_user_level_skill_fallback`).

### 2.1.174 — skill hot-reload only re-announces changed skills · Workflow agent() attribution

Reviewed 2026-06-30.

- **"Fixed skill hot-reload re-sending the entire skill listing when a single skill changed; only
  changed skills are now re-announced."** → 🟢 **already covered / direct win.** Cassy hot-syncs builtin
  `SKILL.md` into worker worktrees without a daemon restart (memory
  `project_skill_hot_sync_no_daemon_restart`); with this fix, editing one Cassy skill no longer re-floods
  the model with the full listing. Strictly better for that workflow.
- Workflow `agent()` subagents now carry per-agent attribution headers. → ✅ no action; improves
  `cas-code-review` Workflow dispatch readability. (2.1.175's `enforceAvailableModels` and 2.1.176's
  hook `if`-condition path matching are host-config niceties → ✅ no action.)

### 2.1.170 — Claude Fable 5 (Mythos-class model) GA · VS Code transcript-save fix

Reviewed 2026-06-10 (calm-crane-32 / supervisor).

**Verification note:** the "Claude Fable 5 / Mythos-class" headline tripped every skepticism wire
(naming doesn't match Opus/Sonnet/Haiku; hype phrasing; post-dates the Jan-2026 assistant cutoff),
so it was verified before recording. Multiple independent reputable sources — Anthropic news, CNBC,
NBC, TechCrunch, MacRumors, AWS/Amazon — corroborate a real launch on **2026-06-09**. (A WebFetch
summary labeled the page "fictional," but that is an April-2024-cutoff artifact of the summarizer
model, not a refutation — discounted.) **Conclusion: real model, genuinely launched.**

- **Claude Fable 5 — new top-tier model, GA 2026-06-09.** Public, safety-gated member of the
  "Mythos" class; positioned ABOVE Opus 4.8 (its safeguards fall back to Opus 4.8 on ~5% of sessions
  and block cyber/bio/chem topics). Pricing ~$10/Mtok in, ~$50/Mtok out. Heavy SWE claims
  (codebase-wide migrations in a day). → 👀 **opportunity, not yet actionable.** Relevant to Cassy
  worker/supervisor model selection (`STOCK_WORKER_MODEL` in `cas-cli/src/config/settings.rs`;
  `--model` passthrough for both claude + codex paths). Blockers before wiring anything:
  1. **Need the exact API model ID** — the announcement doesn't publish it; can't set a config
     default without the literal id string.
  2. **Cost** — ~5–10× current default; at most a supervisor / hard-task option, never a stock
     worker default.
  3. **Safeguard fallback** — Cassy is sometimes used for *authorized* security testing; Fable 5
     silently downgrades cyber/bio/chem prompts to Opus 4.8, so a security-focused worker may not
     actually run on Fable 5. Must be called out in any model-selection guidance.
  4. **Subscription window** — included in Pro/Max/Team/Enterprise only until **2026-06-22**, then
     usage-credits. Affects cost planning if adopted.
- **"Fixed sessions not saving transcripts (and not appearing in `--resume`) when launched from the
  VS Code integrated terminal or any shell that inherited Claude Code environment variables."** →
  👀 **watch — potential factory impact, highest-priority item in this release for us.** Cassy spawns
  workers via PTY with INHERITED + augmented Claude Code env (`CAS_*`,
  `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`, etc. — `crates/cas-pty/src/pty.rs::PtyConfig::claude`).
  If "inherited Claude Code env vars" was the trigger, factory worker sessions on ≤2.1.169 may have
  silently failed to save transcripts — which would undercut session-log mining + blame attribution
  (the per-session JSONL we rely on; see `reference_claude_session_log_paths`,
  `feedback_cross_project_log_mining`). **Checked 2026-06-10:** local CC is **2.1.168** (pre-fix),
  yet **161 worker session JSONLs exist** across `*cas-worktrees-*` dirs incl. recent runs — so
  factory workers ARE saving transcripts. The trigger is therefore narrower than generic CC-env
  inheritance (likely VS Code-terminal-specific, or a path var Cassy doesn't set). **Downgraded to low
  risk** — no evidence of categorical transcript loss; just pick up the fix on the normal 2.1.170
  bump. No standalone verify task needed.

### 2.1.169 — `--safe-mode` · `disableBundledSkills` · `/cd` · `agents --json` state/id

Reviewed 2026-06-09 (calm-crane-32 / supervisor). No item requires a Cassy change; three are
worth knowing about.

- **`--safe-mode` flag + `CLAUDE_CODE_SAFE_MODE`** — boots Claude Code with ALL customizations
  disabled (CLAUDE.md, plugins, skills, hooks, MCP servers) for troubleshooting. → ✅ no action,
  **👀 useful Cassy debugging lever.** When a Cassy hook/skill/MCP misbehaves, `--safe-mode`
  cleanly isolates "is it Cassy or the harness" in one flag — no manual settings surgery. Worth
  folding into a Cassy troubleshooting/onboarding doc as the first triage step. Note: it disables
  the whole Cassy surface, so factory mode won't run under it — it's a diagnosis tool, not a run mode.
- **`disableBundledSkills` setting + `CLAUDE_CODE_DISABLE_BUNDLED_SKILLS`** — hides Claude Code's
  *bundled* skills/workflows/built-in slash commands from the model. → ✅ no action, **👀 namespace
  hygiene option.** Cassy ships its own skill set; a user drowning in bundled + Cassy skills could set
  this to reduce menu clutter and slash-command collisions. Does NOT affect Cassy skills (those are
  project/user skills, not bundled). Optional ops choice, not a code change.
- **`claude agents --json` gains `--all`, `id`, `state`; now includes blocked + just-dispatched
  sessions.** → ✅ no action. Re-confirms the standing finding (memory `project_claude_agents_json_session_scoped`,
  `project_cc_2_1_145_150_compat_results`): `agents --json` tracks *background sessions*, not the
  factory's tmux workers, so the new `state`/`id` fields still don't give Cassy factory monitoring
  anything. Logged here so the next person doesn't re-test it expecting tmux visibility.
- **`/cd` (move cwd without breaking prompt cache); managed-MCP policy enforcement fixes (reconnect,
  IDE configs, `--mcp-config`, cold start); background agents now honor project `env` (e.g.
  `ANTHROPIC_MODEL`); untrusted-settings OTEL client-cert path now needs trust; `/workflows` opens
  mid-turn; native `TaskCreate` auto-repairs malformed input.** → ✅ no action. All host-side; Cassy
  factory uses tmux workers (not background agents) and its own `mcp__cas__task`, so the
  background-agent + native-TaskCreate items don't touch Cassy. Stricter MCP enforcement is strictly
  safer for the CAS MCP server registration.

### 2.1.166 — SendMessage authority hardening · `fallbackModel` · deny-rule globs

Reviewed 2026-06-08 (calm-crane-32 / supervisor).

- **Cross-session `SendMessage` no longer carries user authority** — receivers refuse
  relayed permission requests; auto mode blocks them. → **✅ no action.** Two
  independent reasons Cassy is off this vector:
  1. In factory mode Cassy *intercepts* native `SendMessage` in PreToolUse and reroutes
     it onto the Cassy prompt queue, returning `deny` so the native relay never executes
     (`auto_route_send_message`, `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs:169`,
     impl `:1044`). The native relay subsystem is effectively dead code in factory mode.
  2. Cassy grants worker permissions **hook-locally** via its own PreToolUse handler keyed
     on `CAS_AGENT_ROLE` (`FACTORY_AUTO_APPROVE_TOOLS` → `"allow"`, pre_tool.rs:113-119,
     :874, :1027) — never via a relayed cross-session message. That hook-local model is
     exactly what the hardening leaves intact. Informational coordination messages are
     unaffected (the change only restricts permission-request relays).
- **`fallbackModel` setting (up to 3 fallbacks); `--fallback-model` now interactive too.**
  → ✅ no action. Cassy doesn't pin the harness model selection; workers inherit it. Could
  *optionally* document a recommended fallback chain for factory hosts, but not required.
- **Deny rules support glob in tool-name position (`"*"` denies all).** → ✅ no action.
  Cassy enforces tool gating through its own PreToolUse hook + `disallowed-tools` skill
  frontmatter (cas-5be8), not through user deny rules. Glob deny rules are a host-config
  nicety orthogonal to Cassy's enforcement.

### 2.1.163 — Stop/SubagentStop `additionalContext` · managed version pinning · `/plugin list`

Reviewed 2026-06-08.

- **Stop and SubagentStop hooks can return `hookSpecificOutput.additionalContext`** to
  feed Claude and keep the turn going without being flagged a hook error. → **👀
  opportunity.** This is a cleaner channel than what `session-learn`'s Stop-hook auto-trigger
  uses today. Worth a spike: route session-learn / supervisor guidance through
  `additionalContext` instead of the current Stop-hook output path. Not urgent — current
  path works — but flagged so we evaluate before the next hook-surface EPIC. No task filed
  yet.
- **`requiredMinimumVersion` / `requiredMaximumVersion` managed settings.** → ✅ no action.
  Could be useful to pin factory hosts to a known-good CC range, but that's an ops choice,
  not a code change. Note for onboarding docs if we ever standardize a CC floor.
- **`/plugin list`, skill `\$` escape, `CLAUDE_CODE_SESSION_ID` to stdio MCP on resume.**
  → ✅ no action.

### 2.1.162 — Ctrl+V image paste fix · permission-rule correctness · MCP timeout fix

Reviewed 2026-06-08.

- **Fixed `claude agents` Ctrl+V image paste doing nothing in the dispatch/reply boxes.**
  → **🟢 already covered (different surface).** That fix is in Claude Code's *own* `claude
  agents` UI, which Cassy does not use — Cassy has its own factory TUI
  (`cas-cli/src/ui/factory/client.rs`). Cassy already handles image input there via bracketed
  paste: `Event::Paste(text)` → `contains_dropped_image_path()` → emits a `drop_image;…`
  control command (client.rs:202-214), with `file://` URI decode + tests (:464-472).
  **Precision:** Cassy handles dropped image *file paths* (drag-drop / pasted path text,
  incl. `file://`), NOT binary-clipboard Ctrl+V of an actual image with no path — which is
  what 2.1.162 specifically addressed. In practice terminals deliver drag-dropped images as
  paths through bracketed paste, so Cassy covers the realistic case. True binary-clipboard
  paste in the factory TUI would be net-new and is not currently justified.
- **WebFetch rules now apply to preapproved domains; Windows backslash/case path matching;
  Read deny rules hide files from Glob/Grep; sub-1000ms MCP `timeout` no longer floored to a
  1s watchdog; LSP `workspaceSymbol` returns results.** → ✅ no action (host-side correctness;
  Cassy unaffected).

### 2.1.161 — MCP secret redaction · parallel-tool isolation · telemetry labels

Reviewed 2026-06-08.

- **`claude mcp` list/get/add no longer print secrets** (`${VAR}` not expanded; credential
  headers + URL secrets redacted). → ✅ no action; strictly safer for anyone inspecting the
  CAS MCP server registration.
- **A failed Bash in a parallel batch no longer cancels siblings** — each tool returns its
  own result. → ✅ no action; mildly beneficial for factory workers issuing parallel calls.
- **`isolation:"worktree"` Workflow agents in background sessions can now edit their own
  worktree.** → ✅ no action, but relevant to the #6 Workflow-migration direction (see cc160
  note) — removes a friction point if Cassy skills author worktree-isolated Workflow agents.

### 2.1.160 — sensitive-file write prompts · "workflow" → "ultracode" rename

Reviewed 2026-06-08. (Note: the deep 2.1.152–160 hook-surface work is its own EPIC — see
[cc160 note](2026-06-02-cc160-hook-surface.md), shipped v2.18.0. Items below are the
remaining .160 changelog lines with a Cassy angle.)

- **`acceptEdits` now prompts before writing exec-granting config; shell-startup-file write
  prompts.** → ✅ no action — already characterized in EPIC cas-2f29 task **cas-f97d**:
  factory workers spawn with `--dangerously-skip-permissions` (`bypassPermissions`), which
  short-circuits the .160 sensitive-file check. Non-factory acceptEdits users will see the
  intended Claude Code prompt; that's harness hardening, not a Cassy bug.
- **Dynamic-workflow trigger keyword renamed `workflow` → `ultracode`.** → ✅ no action, but
  **terminology watch:** this renames the *trigger keyword* that fires a dynamic workflow,
  NOT the `Workflow` tool itself (still `Workflow`). Cassy skill/docs references to the
  `Workflow` tool are unaffected. If any Cassy prose tells users to "say workflow to trigger…",
  that guidance is now stale — none found at review time.
- **Removed `CLAUDE_CODE_OPUS_4_6_FAST_MODE_OVERRIDE` (now a no-op).** → ✅ no action; Cassy
  doesn't set it.

---

## Backlog of opportunities (not required, tracked)

- **Model-tier rubric refresh for Sonnet 5** (from 2.1.197) — re-score standard/heavy tiers
  against Sonnet 5's 1M context; ignore promo pricing (ends 2026-08-31) in any
  `STOCK_WORKER_MODEL` comparison. See 2.1.197 entry.
- **session-learn / guidance via Stop-hook `additionalContext`** (from 2.1.163) — evaluate
  before next hook-surface EPIC. See 2.1.163 entry.
- **Factory CC version floor** via `requiredMinimumVersion` (from 2.1.163) — ops/onboarding
  decision, not code.
