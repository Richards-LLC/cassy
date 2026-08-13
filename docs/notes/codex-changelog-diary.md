# Codex CLI Changelog Diary — CAS Response Ledger

A living, **newest-first** ledger of OpenAI Codex CLI releases and how CAS responded to
each. Sibling to `claude-code-changelog-diary.md` and `grok-changelog-diary.md` —
CAS supports three harnesses (`cli=claude` / `cli=codex` / `cli=grok`), so we track Codex drift too.

Codex has no `CHANGELOG.md`; release notes live on the GitHub **releases page**.

## How to update

When a new Codex stable ships:

1. Pull releases: `gh release list --repo openai/codex --limit 15` (stable tags look like
   `rust-v0.138.0`; `-alpha.N` tags are pre-releases — track stables, skim alphas).
2. Read notes: `gh release view rust-v<X.Y.Z> --repo openai/codex --json body --jq '.body'`
3. Verdict each item against CAS, focusing on the **touchpoints** below — that's the whole
   surface a Codex change can break for us. Most items (TUI, plugins, ChatGPT auth, Python
   SDK, Bedrock) are orthogonal → `⏭ n/a`.
4. Add a newest-first entry + index row. File a CAS task only when work is actually required.
5. **Version gap matters:** track the version CAS is validated against vs latest (below).
6. After the diary update merges, publish the mandatory shared **#cas-internal**
   harness thread: one parent plus exactly three replies ordered **Grok, Claude,
   Codex**. Follow [the release Slack rubric](../RELEASE_SLACK_RUBRIC.md), including
   its version-range, verdict/action, source-gap, and no-internal-narration rules.

**Verdict legend:** ✅ no action · 🟢 already covered · 👀 watch (touches a CAS dependency,
verify on upgrade) · 🔧 fix shipped · 🏗 EPIC · ⏭ n/a

## Version status

- **CAS validated against:** Codex CLI **0.146.0**. The exact interactive `PtyConfig::codex`
  matrix passed on 2026-07-30 after `cas-8c80` projected `mcp__cs` as a direct-only code-mode
  namespace and pinned Codex's restricted MCP subprocess to the pane's `CAS_ROOT`. The typed
  receipt is `crates/cas-pty/conformance/codex-cli-0.146.0-2026-07-30.json`.
- **Locally installed:** **0.147.0** (`codex-cli 0.147.0`, checked 2026-08-13).
- **Latest stable:** **0.147.0** (2026-08-07). 0.148.0 remains prerelease-only and is untracked
  under this diary's stable-only policy.
- **Gap:** the validated pin remains 0.146.0 while the local installation and latest stable are
  0.147.0. This review found no documented launch-contract rename, but a fresh `PtyConfig::codex`
  matrix remains required before calling 0.147.0 validated.
  The older entries below are a *triage pass* against the touchpoints, not a per-item code audit —
  upgrade-time re-verification is the trigger for promoting any 👀 to a task. (Contrast the Claude
  Code diary's .166/.162 entries, which were deep-verified for specific user questions.) The
  **0.130–0.135 block is a backfill** (lighter fidelity, consolidated) added 2026-06-30 to extend
  coverage below the original 0.136 seed floor. The **0.143–0.144.4 block is a backfill**
  (2026-07-14) catching the diary up from the previous 0.142.5 ceiling; 0.145.0 is the first
  subsequent stable tracked at release time.

## CAS ↔ Codex touchpoints (what a release can break)

The load-bearing surface, all in `crates/cas-pty/src/pty.rs::PtyConfig::codex` unless noted:

- **`--yolo`** — approval + sandbox bypass for factory workers (Codex's analogue of Claude's
  `--dangerously-skip-permissions`). If renamed/removed/semantically narrowed, workers can't act.
  Watch any "sandbox / approval / deny-rule enforcement" changelog line.
- **`-c model_reasoning_effort=<e>`** — effort is set via a **TOML `-c` override, not a flag**
  (vocabulary: none/minimal/low/medium/high/xhigh). Mapped from `Effort::as_codex_config()`
  (`crates/cas-mux/src/spec.rs:45`). Any "reasoning effort" changelog line is a 👀.
- **`--config developer_instructions="..."`** — supervisor/worker role instructions are injected
  via this config key. A rename breaks role priming.
- **`--no-alt-screen`** — required for the factory mux to render Codex panes.
- **`--model <m>`** — model selection passthrough.
- **`.codex/config.toml`** — registers the CAS MCP server (exposed to Codex as **`cs`**, e.g.
  `mcp__cs__task`, `mcp__cs__coordination` — note: `cs`, not `cas`). MCP dep bumps (e.g. rmcp)
  are 👀.
- **`.codex/skills/` + `.codex/agents/`** — the Codex mirror synced from `.claude/` by
  `cas integrate` / `cas update`. Codex "skills plumbing / malformed skills field" changes are 👀.
- **`AGENTS.md`** — Codex's workspace-instruction file (its CLAUDE.md analogue). Workers run in
  worktrees, so "AGENTS.md loading / symlinked workspace" changes are 👀.
- **`CAS_AGENT_ROLE` / `CAS_FACTORY_MODE` env** — drive the same hook-local auto-approve + jail
  exemptions as Claude workers (note: Codex has no Claude-style hook system; CAS relies on `--yolo`
  + env, not PreToolUse, on the Codex path).

## Index

| Codex version | Headline | CAS verdict | Pointer |
|---------------|----------|-------------|---------|
| 0.148.0-alpha | Pre-release as of 2026-08-13 — untracked under stable-only policy | — | — |
| 0.147.0 | MCP 2026-07-28 discovery/non-blocking startup · Agent Plugins · trust/approval hardening | 👀 upgrade validation | this doc |
| 0.146.1 | Safer cyber-model auto-review defaults | ✅ no action | this doc |
| 0.146.0 | **Live MCP refresh/reconnect** · executor skills and Agent Plugins · approval-state preservation | 🔧 validated/fix shipped | receipt + cas-8c80 |
| 0.145.0 | **Multi-agent V2 stable** · broader `/import` · MCP startup hardening · concurrent skill discovery · approval tightening | 👀 watch | this doc |
| 0.144.1–.4 | Installer/code-mode reliability · Guardian auto-review prompt revert · two empty patch releases | ✅ no action | this doc |
| 0.144.0 | **`writes` app-approval mode** · MCP auth elicitation default · plugin skill-loading perf · Ultra+multi-agent usage warn | 👀 watch | this doc |
| 0.143.0 | **`max` first-class reasoning effort** · MCP tool search by default · rmcp 1.8.0 · AGENTS.md env-reactive + skills/delegation · sandbox profile flag rename | 👀 watch | this doc |
| 0.142.5 | Single backport: WebSocket request payloads no longer written to trace logs | ✅ no action | this doc |
| 0.142.0–.4 | **Rollout token budgets (turns abort when exhausted)** · env-scoped command/network approvals · AGENTS.md from foreign envs · `SkillsManager→SkillsService` + skill-frontmatter repair | 👀 watch | this doc |
| 0.141.0 | **Hook trust bypass persists through `codex exec`; blocking PostToolUse rejects code-mode** · per-thread plugin stdio MCP activation · MCP tool timeout→300s | 👀 watch | this doc |
| 0.140.0 | **`/import` Claude Code setup/config/chats** · corrupted-SQLite auto-recover · encrypted MCP-OAuth secret storage · hooks.json unsupported-field warnings | 👀 watch | this doc |
| 0.139.0 | Sandbox preserves approval/escalation + proxy-only net · `oneOf`/`allOf` in tool schemas · `-P` profile alias · multi-agent v2 `interrupt_agent` | 👀 watch | this doc |
| 0.138.0 | Effort-order-from-model, skills→extension bridge, AGENTS.md symlink fix, multi-agent v2 catalog | 👀 watch | this doc |
| 0.137.0 | Skills plumbing → dedicated crates, malformed-skills-as-warning, permission env identity, multi-agent v2 | 👀 watch | this doc |
| 0.136.0 | deny-read enforced in approval-bypass paths, rmcp 1.7.0, command-safety hardening | 👀 watch | this doc |
| 0.130.0–0.135.0 | **Backfill (consolidated):** `--profile` becomes primary + legacy profile configs rejected · subagent identity in hook inputs · AGENTS.md invalid-UTF-8 warns-not-drops · MCP `$ref`/`$defs` + readOnlyHint concurrency · memory→dedicated SQLite | 👀 / ✅ | this doc |

---

## Entries

### 0.147.0 — MCP 2026-07-28, portable plugins, and approval/trust boundaries

Reviewed 2026-08-13. Triage pass vs touchpoints. Source: official
[`rust-v0.147.0` release](https://github.com/openai/codex/releases/tag/rust-v0.147.0)
(published 2026-08-07).

- **Codex supports the opt-in MCP 2026-07-28 protocol, paginated discovery, multi-round requests,
  cached tools before optional server startup, and non-blocking startup.** → 👀 **touchpoint: MCP
  (`cs`).** These are direct client-runtime changes. Upgrade validation must prove that the local
  stdio `cs` server is discovered under `mcp__cs__*`, is ready before required lifecycle calls, and
  never leaves a stale or partially paginated tool catalog.
- **Portable Agent Plugins and imported Cursor skills expand the plugin/skill sources, while host
  skill budgets and isolation were hardened.** → 👀 **touchpoint: `.codex/skills/`,
  `.codex/agents/`, and `AGENTS.md`.** CAS does not use plugins as its authority path; confirm its
  synced mirror remains discoverable and cannot be shadowed by an imported or plugin-provided skill.
- **`--approve-for-me`, explicit trust for unfamiliar projects, managed-auth enforcement, and
  fail-closed network/plugin policy behavior change approval and sandbox boundaries.** → 👀
  **touchpoint: `--yolo`, `CAS_AGENT_ROLE`, and `CAS_FACTORY_MODE`.** No documented removal or
  rename affects CAS, but a factory resume and fresh-worktree smoke is required before promotion.
- **Thread organization, rendering, remote compaction, and installer/package changes** are
  ⏭ **n/a** to the CAS PTY launch contract. **Source gaps:** none for the stable release.

### 0.146.1 — safer automatic-review defaults

Reviewed 2026-08-13. Source: official
[`rust-v0.146.1` release](https://github.com/openai/codex/releases/tag/rust-v0.146.1)
(published 2026-08-05).

- **Cyber-capable model automatic-review defaults and terminal permission explanation were made
  safer.** → ✅ **no action.** CAS starts factory workers with the explicit `--yolo` contract;
  no CLI spelling or MCP/skill/instruction behavior changed. **Source gaps:** none.

### 0.146.0 — live MCP refresh/reconnect · executor skills and Agent Plugins · approval continuity

Reviewed 2026-07-30. Triage pass vs touchpoints. Source: official `rust-v0.146.0` release
(`gh release view rust-v0.146.0 --repo openai/codex`; published 2026-07-29).

- **Upgrade validation:** 🔧 **passed after `cas-8c80`.** Codex 0.146 code mode keeps the CAS
  namespace top-level via
  `features.code_mode.direct_only_tool_namespaces=["mcp__cs"]`; the launch also passes `CAS_ROOT`
  explicitly to Codex's restricted MCP subprocess environment. The fresh real matrix proved root
  and follow-up `coordination`/`task` calls, code-mode `exec` coexistence, project discovery,
  model/effort/approval/cwd continuity, and interrupt/resume. The typed PASS receipt is
  `crates/cas-pty/conformance/codex-cli-0.146.0-2026-07-30.json`.

- **MCP connections and Apps tools now stay current after authentication or configuration changes,
  replace closed servers, and refresh without restarting healthy connections (#34952, #34957,
  #35028, #35144, #35146, #35151).** → 👀 **touchpoint: MCP (`cs`).** Live reconciliation should
  improve recovery for the local stdio server, but catalog/runtime refresh is load-bearing: on
  upgrade, change or explicitly refresh MCP configuration and verify `mcp__cs__task` and
  `mcp__cs__coordination` remain callable, with no stale catalog and no healthy `cs` connection
  needlessly restarted.
- **Codex can discover executor-provided skills and securely read their resources, including
  explicitly selected skills (#35184, #35198); tight context budgets retain more skills and warn
  before catalog truncation (#34732, #34738, #34997).** → 👀 **touchpoint: `.codex/skills/` +
  `.codex/agents/`.** This expands skill sources and changes budget-pressure behavior. Verify the
  CAS-synced mirror remains discoverable in a worktree, explicit resource reads stay within the
  selected skill, and truncation warnings do not silently hide required worker guidance.
- **Agent Plugins manifests and workspace publishing are now supported, with additional marketplaces
  for Amazon Bedrock and Claude Code (#35105, #35254, #34931, #34979).** → 👀 **touchpoint:
  `.codex/skills/` + `.codex/agents/`; multi-agent behavior.** CAS does not depend on Codex's plugin
  marketplace or native agent orchestration, so no integration change is required. On upgrade,
  confirm plugin-contributed agents/skills do not shadow the CAS mirror or relax the CAS
  supervisor/worker contract.
- **Approval settings and reviewer identity are preserved across interruptions, replay, imports, and
  forks (#34989, #34664, with related continuity fixes in #34839, #34777, #35524).** → 👀
  **touchpoint: `--yolo` + approval and multi-agent behavior.** Continuity is useful, but a resumed or
  forked factory session must retain the non-interactive bypass rather than restoring an unrelated
  interactive approval state; smoke interruption/resume once during upgrade validation.
- **Proxy routing now applies consistently across authentication, plugin downloads, MCP
  authorization, remote execution, WebSockets, redirects, and LM Studio (#34479, #34509, #34655,
  #34678, #35023, #35056, #35239).** → 👀 **touchpoints: MCP (`cs`) and sandbox/network behavior.**
  The local stdio `cs` server needs no proxy or OAuth, but host proxy policy must not prevent its
  startup or alter factory network access under `--yolo`.
- **Named/pinned/side-conversation sessions, paginated and ephemeral forks, remote Code Mode hosts,
  standalone web search for custom providers, terminal rendering, installers, macOS signing,
  enterprise update controls, and app-server performance work.** → ⏭ n/a to the documented CAS PTY
  launch contract. The release notes report no `--yolo`, `--no-alt-screen`, `--model`,
  `model_reasoning_effort`, `developer_instructions`, or `AGENTS.md` contract change.

### 0.145.0 — multi-agent V2 stable · broader `/import` · MCP startup and skills hardening

Reviewed 2026-07-22 (diary-codex / cas-c7f9). Triage pass vs touchpoints. Source: official
`rust-v0.145.0` release (`gh release view rust-v0.145.0 --repo openai/codex`; published 2026-07-21).

- **"Stabilized the opt-in multi-agent V2 experience" with configurable sub-agent models,
  reasoning levels, concurrency, restored roles, and improved navigation (#33550, #33631, #33657,
  #33841, #34383).** → 👀 **touchpoints: `--model`, `model_reasoning_effort`, CAS role/factory env,
  and multi-agent behavior.** CAS factory orchestration does not rely on Codex-native sub-agents, so
  no launch change is required. On upgrade, keep native delegation opt-in and verify configured
  model/effort overrides plus `developer_instructions` still govern the root worker; otherwise a
  Codex-native role or spawn default could diverge from CAS's supervisor/worker contract or rollout
  budget expectations.
- **`/import` now migrates Cursor and Claude Code settings, MCP servers, plugins, sessions, commands,
  and project-scoped memories (#31672, #33411, #33426, #33444).** → 👀 **touchpoints:
  `.codex/config.toml`, MCP (`cs`), `.codex/skills/`, and `AGENTS.md`.** This is an onboarding path,
  not a replacement for `cas integrate`: after any import, verify the local stdio `cs` registration,
  CAS-synced skills/agents, and workspace instructions were preserved rather than shadowed by imported
  configuration.
- **MCP startup/auth fixes add startup timeouts, avoid blocking OAuth discovery, serialize credential
  refresh, and safely reuse tool catalogs (#32229, #32781, #32825, #33184, #33297).** → 👀
  **touchpoint: MCP (`cs`).** The local stdio server has no OAuth, and catalog reuse should improve
  startup, but a new startup timeout is directly load-bearing: smoke `mcp__cs__task` and
  `mcp__cs__coordination` on a fresh factory worker so a slow CAS launch is not classified as failed
  and a cached catalog is not stale.
- **Skill/plugin discovery now scans concurrently and reuses inventory (#31566, #33369, #33423),
  while shared skill models moved into `codex-skills` (#34429).** → 👀 **touchpoint:
  `.codex/skills/` + `.codex/agents/`.** Internal plumbing changed again; verify the `cas integrate` /
  `cas update` mirror still enumerates both CAS skills and agent definitions from a worktree.
- **Approval/sandbox handling tightened full-access confirmation, forced-`rm` detection, rejection
  propagation, and Windows network-proxy enforcement (#32989, #33464, #34400, #32857, #34423).** → 👀
  **touchpoint: `--yolo` + `CAS_AGENT_ROLE` / `CAS_FACTORY_MODE`.** Safety improvements are welcome,
  but confirm a factory launch still receives non-interactive full access and does not regain approval
  prompts; Windows workers should also verify proxy enforcement does not narrow expected network access.
- **Bundled GPT-5.4 selections migrated to GPT-5.6 Terra/Luna variants (#33173), with GPT-5.6 Sol the
  Bedrock default (#32288).** → ✅ **touchpoint: `--model`.** CAS passes the selected model through and
  does not pin these bundled defaults, so no mapping change follows from this release. The new Bedrock
  default and audio/realtime, paginated-history, visualization, TUI-rendering, installer, and packaged
  ripgrep changes are otherwise ⏭ n/a to the CAS PTY launch contract (`--no-alt-screen` is unchanged).

### 0.144.1–.4 — installer/code-mode · Guardian review revert · empty patches

Reviewed 2026-07-14 (w-codex-diary / cas-64d6). Host is `codex-cli 0.144.4`. Consolidated: no CAS-touchpoint
signal in the patch band after 0.144.0.

- **0.144.4 (2026-07-14):** "No user-facing changes in this patch release." → ✅ **no action.**
- **0.144.3 (2026-07-13):** Version-only release; no merged PR changes since 0.144.2. → ✅ **no action.**
- **0.144.2 (2026-07-13):** "Restored the previous Guardian auto-review policy, request format, and tool
  behavior after rolling back a prompting regression" (#32672). → ✅ **no action** for factory. Guardian
  auto-review is Codex's review product surface, not CAS's `--yolo` worker launch path.
- **0.144.1 (2026-07-09):** Standalone-install GitHub metadata robustness + macOS code-mode host packaging
  + embedded runtime fallback when companion host binary missing (#31913). → ⏭ n/a (installer/code-mode
  packaging; CAS factory launches `codex` via PTY, not the standalone installer path).

### 0.144.0 — `writes` app-approval · MCP auth elicitation default · skills plugin ns · Ultra concurrency warn

Reviewed 2026-07-14 (w-codex-diary / cas-64d6). Triage pass vs touchpoints. Sources: `gh release view
rust-v0.144.0 --repo openai/codex`.

- **"Added a `writes` app-approval mode that allows declared read-only actions while prompting for
  writes" (#30482).** → 👀 **touchpoint: `--yolo`/approval.** New approval-mode vocabulary on the
  app-approval axis. CAS workers bypass via `--yolo` and do not set app-approval modes; still **verify
  on upgrade** that the new mode is not a default that reintroduces prompts under `--yolo`, and that
  any host-level defaults in `.codex/config.toml` don't pin `writes` for factory sessions.
- **"MCP tools can now request authentication interactively without an experimental opt-in"
  (#28772).** → 👀 **touchpoint: MCP (`cs`).** Auth elicitation is on by default for MCP tools that need
  it. Our `cs` server is local stdio with no OAuth, so expected impact is none — but **smoke
  `mcp__cs__*` load** after the bump in case the elicitation path changes MCP client startup ordering
  or hangs when a *second* MCP server in the same config needs auth.
- **"Reduced plugin skill-loading time on remote executors by resolving namespaces once per root"
  (#31348) + skill catalog/compaction parity tests.** → 👀 **touchpoint: `.codex/skills/`.** Perf/correctness
  on plugin skill discovery. Local `.codex/skills/` mirror (from `cas integrate`) should be unaffected;
  verify synced skills still list after upgrade.
- **"Selecting Ultra reasoning now warns when high multi-agent concurrency could increase usage
  quickly" (#31621).** → ✅ minor / TUI-adjacent. CAS sets effort via `-c model_reasoning_effort=<e>`
  (vocabulary still `…|xhigh`, not "Ultra"); this is a TUI warning, not a config-key change. No CAS
  action unless we start mapping a named "Ultra" product tier into the effort override.
- **"Windows sandbox sessions can delete files in writable roots and access the managed primary
  runtime" (#31138, #31574).** → 👀 minor **sandbox** (Windows host only). Factory `--yolo` path should
  still bypass; note for Windows workers if any.
- **MCP tool snapshot reuse within a sampling request (#31292); increase tool schema compaction
  threshold (#31497); round MCP timeout durations in error messages (#31612).** → 👀 **MCP (`cs`)**
  fidelity/perf. Schema compaction threshold up is usually helpful for large `cs` tool surfaces;
  smoke a few multi-arg tools on upgrade.
- **Usage-limit reset-credit picker, app-server hosted auth redirects, global pnpm install detection,
  Bedrock display names, TUI paste sanitization, code-mode host defaults.** → ⏭ n/a (orthogonal to the
  CAS launch surface).

### 0.143.0 — `max` reasoning effort · MCP tool search default · rmcp 1.8 · AGENTS.md / skills

Reviewed 2026-07-14 (w-codex-diary / cas-64d6). Triage pass vs touchpoints. Sources: `gh release view
rust-v0.143.0 --repo openai/codex`. This is the first stable after the diary's previous 0.142.5 ceiling.

- **"…first-class support for `max` reasoning effort" (#30467, #29899; Bedrock GPT-5.6 family
  #30285).** → 👀 **touchpoint: `-c model_reasoning_effort`.** Codex now treats `max` as a first-class
  effort level. CAS still maps `Effort::XHigh` → `xhigh` (`Effort::as_codex_config` in
  `crates/cas-mux/src/spec.rs`) and documents vocabulary `none/minimal/low/medium/high/xhigh`. **Verify
  on upgrade** that `xhigh` still accepts/works; if Codex deprecates `xhigh` in favor of `max` (or
  models only advertise `max`), CAS needs a mapping update. No evidence of rename in this release —
  additive `max` support is the safer reading.
- **"MCP tools now use tool search by default" (#29486) + ChatGPT-hosted MCP session auth
  (#29733).** → 👀 **touchpoint: MCP (`cs`).** Tool *search* as the default presentation path can change
  how tools are discovered vs always-listed. **Highest-risk 0.143 item for CAS:** confirm factory
  workers still see and invoke `mcp__cs__task` / `mcp__cs__coordination` (and the rest of the `cs`
  surface) without an extra search step that hides tools. Session-auth is for ChatGPT-hosted MCP, not
  local stdio `cs`.
- **"Update rmcp to 1.8.0" (#29634).** → 👀 **touchpoint: MCP (`cs`).** MCP client dep bump (diary
  already flags rmcp bumps). Smoke stdio MCP connect after upgrade; watch for protocol/schema
  regressions on large tool lists.
- **"core: make AGENTS.md react to environment changes" (#29810) + bounded AGENTS.md/Git root probes
  (#29870) + "allow AGENTS.md and skills to authorize delegation" (#30274).** → 👀 **touchpoint:
  AGENTS.md (+ skills).** Continuing AGENTS.md accuracy work (from 0.138/0.142). Env-reactive reload
  likely *helps* worktree workers; delegation-authorization via AGENTS.md/skills is informational for
  multi-agent v2 (CAS doesn't drive Codex-native delegation for factory workers today). Verify worker
  role priming still lands from worktree `AGENTS.md`.
- **Skills plumbing churn:** parallelize environment skill loading (#29990), project executor skills
  through World State (#30088), load executor skills without host path conversion (#29626), user-level
  `code-review-*` skills (#30143), model-metadata skill usage instructions (#29740). → 👀 **touchpoint:
  `.codex/skills/`.** Same subsystem churn pattern as 0.137–0.142. **Verify the `cas integrate`
  mirror still loads** post-bump.
- **"cli: rename sandbox permission profile flag" (#30095) + expose permission profile to shell tools
  (#29941); rm `AskForApproval::OnFailure` (#28418).** → 👀 **touchpoint: `--yolo`/approval.** Profile
  flag rename is only a 👀 if anything in CAS or host docs still references the old flag — workers use
  `--yolo`, not named profiles. Approval enum cleanup is internal; confirm `--yolo` still full-bypasses.
- **Rollout budget continuity:** surface budget exhaustion (#29715), rename to session budget error
  (#29744), raise token budget message limits (#29970). → 👀 carry-forward from **0.142 rollout token
  budgets** — still verify factory turns don't inherit a low default budget.
- **Remote plugins default-on, system proxy for auth/Responses, Bedrock models, `codex remote-control
  pair`, app-server env/thread APIs, Windows ConPTY.** → ⏭ n/a (plugins/proxy/Bedrock/remote; not on
  the CAS PTY launch surface). Cancelled-review MCP-busy fix (#31189) is a minor reliability win if a
  human runs `/review` in a shared Codex, not a factory concern.

### 0.142.5 — trace-log payload redaction backport

Reviewed 2026-07-07 (patient-condor-18 / supervisor). Locally installed at review time.

- **"Prevented full Responses WebSocket request payloads from being written to trace logs"
  (#30771, sole change).** → ✅ **no action.** Privacy/hygiene backport on Codex's own trace
  logging; no CAS touchpoint (not `--yolo`, effort, MCP, skills, or AGENTS.md). Recorded so the
  0.142.4 → 0.142.5 delta is known-empty for the pending 0.128 → 0.142 upgrade-validation
  checklist — nothing new to verify beyond the 0.142.0–.4 items.

### 0.142.0–.4 — rollout token budgets · env-scoped approvals · AGENTS.md from foreign envs · SkillsService

Reviewed 2026-06-30 (eager-leopard-33 / supervisor). Triage pass vs touchpoints. This is the version
band **currently installed locally** (`codex-cli 0.142.4`).

- **"Configurable rollout token budgets track usage across agent threads… and abort turns when
  exhausted" (#28746, #28494, #28707, #29423).** → 👀 **watch — most load-bearing item for the factory.**
  A Codex worker turn can now *abort* when a rollout token budget is exhausted. CAS doesn't set a budget
  (so the default applies, which should be unbounded/off), but **verify on the 0.142 bump** that factory
  workers don't pick up a low default budget that kills long turns mid-task. If Codex ever defaults this
  on, CAS needs to either raise it via `-c` or surface "turn aborted: budget" distinctly from a stall.
- **"Command approvals scoped by execution environment" (#28738) + "network approvals scoped by
  environment" (#28899) + "Report remote sandbox denials semantically" (#29424).** → 👀 **touchpoint:
  `--yolo`/approval.** Approvals are now keyed to the exec environment. CAS workers bypass via `--yolo`;
  confirm the env-scoping doesn't reintroduce a prompt on the bypass path for worktree/CAS-root access.
- **"core: load AGENTS.md from foreign environments" (#28958) + remote envs preserve AGENTS.md discovery
  (#28983, #29099).** → 👀 **touchpoint: AGENTS.md.** Worker priming rides AGENTS.md in worktrees; this
  is the continuing accuracy work from 0.138. Likely *helps*; verify worker role priming still lands.
- **`SkillsManager` → `SkillsService` (#28705) + "Repair invalid skill frontmatter scalars" (#28628) +
  "Support plugin manifest path lists / multiple skill paths" (#28790).** → 👀 **touchpoint:
  `.codex/skills/`.** The skills subsystem keeps churning (started 0.137). The frontmatter-repair is
  safer for our generated `.codex/skills/*.md`, but **verify the synced mirror still loads** post-bump.
- **"App-server clients can configure multi-agent delegation as disabled / explicit-request-only /
  proactive" (#28685, #28792, #29324) + "Parent agents receive terminal subagent errors instead of
  empty success" (#28375).** → 👀 strategic (multi-agent v2; same posture as 0.137/0.138 — feed it,
  don't compete). The terminal-error propagation is a genuine reliability win if CAS ever consumes
  Codex-native subagents.
- **Indexed web-search mode, scheduled UTC time reminders + current-time tool, `/usage` reset-credit
  redemption, plugin catalog sections.** → ⏭ n/a (orthogonal to the CAS launch surface).

### 0.141.0 — hook trust bypass in `codex exec` · per-thread plugin stdio MCP · MCP timeout 300s

Reviewed 2026-06-30. Triage pass.

- **"Hook trust bypass now persists through `codex exec` thread start and resume, while blocking
  `PostToolUse` hooks correctly reject code-mode tool calls" (#26434, #28365).** → 👀 **note: Codex
  hooks.** Codex now has its own hooks.json + PostToolUse path. CAS's Codex worker model relies on
  `--yolo` + env (no Codex hook system in the loop, per the touchpoints list), so this is mostly
  informational — but if CAS ever adopts Codex hooks for parity with the Claude path, the trust-bypass
  + code-mode-rejection semantics are the relevant surface.
- **"Selected executor plugins can activate their stdio MCP servers per thread" (#27870, #27884,
  #27893…).** → 👀 **touchpoint: MCP (`cs`).** How stdio MCP servers (our `cs`) get activated is moving
  to per-thread/plugin-scoped activation. **Verify `mcp__cs__*` still loads** from `.codex/config.toml`
  on every worker thread after the bump — this is the highest-risk item in 0.141 for CAS.
- **"`[mcp] Increase default tool timeout to 300 seconds" (#28234).** → ✅ helpful; longer ceiling for
  `cs` MCP calls (e.g. a slow `mcp__cs__search`), no downside.
- **"TUI input prompts can auto-resolve after inactivity" (#28235) + "let steer interrupt wait_agent"
  (#28341).** → 👀 minor: a `request_user_input` auto-resolution timer could auto-answer a worker's
  prompt — but `--yolo` workers shouldn't be prompting. Note in case a worker hangs on an input dialog.
- Noise relay E2E remote transport, P-521 TLS, bounded image cache. → ⏭ n/a (remote-exec/TUI).

### 0.140.0 — `/import` from Claude Code · corrupted-SQLite auto-recover · encrypted MCP-OAuth secrets

Reviewed 2026-06-30. Triage pass.

- **"Added `/import` for selectively importing setup, project configuration, and recent chats from
  Claude Code" (#27070, #27071, #27703).** → 👀 **strategic / onboarding.** Codex can now pull a Claude
  Code setup. Interesting for the cross-harness story (CAS supports both) and for onboarding a user who
  already has a CC config — but it imports *Codex-side* setup, not CAS's MCP/skill wiring, so it's not a
  substitute for `cas integrate`. Flag for the onboarding-doc pass; no code action.
- **"Corrupted SQLite state databases are now backed up and rebuilt automatically… including malformed
  database-directory cases" (#26859, #27719); 0.131 added fail-closed when state can't open.** → 👀
  minor. Codex's *own* state SQLite, separate from CAS's project-local `cas.db`
  (memory `reference_cas_project_local_dbs`). Echoes the CAS finding that a SQLite restore breaks a live
  MCP connection (memory `feedback_sqlite_restore_breaks_mcp_connection`) — different DB, same hazard
  class; no overlap expected.
- **"Managed Amazon Bedrock API-key auth and encrypted local storage for CLI and MCP OAuth credentials"
  (#27443, #27689, #27504, #27535, #27539, #27541).** → 👀 **touchpoint: MCP (`cs`).** Encrypted secret
  namespaces for MCP OAuth. Our `cs` server is local stdio with no OAuth, so N/A in practice — but
  verify the new secret-storage path doesn't change how `.codex/config.toml` MCP entries are read.
- **"hooks.json warns on unsupported top-level fields" (#26426) + "avoid duplicate hooks.json discovery
  with profiles" (#26418).** → 👀 note (Codex hooks; see 0.141). **Typing `@` opens unified mentions for
  files/plugins/skills (#27499); removed experimental `/realtime` voice (#27801).** → ⏭ n/a.

### 0.139.0 — sandbox preserves approval/escalation · oneOf/allOf tool schemas · `-P` profile alias

Reviewed 2026-06-30. Triage pass.

- **"Sandbox execution now preserves approved escalation decisions and enforces configured proxy-only
  networking more consistently" (#24981, #27035).** → 👀 **touchpoint: `--yolo`/sandbox.** Directly on
  the bypass path — verify `--yolo` workers keep full read/exec on the worktree + CAS root and that
  proxy-only networking (if a host sets it) doesn't block worker network access.
- **"Tool and connector input schemas now preserve `oneOf` and `allOf`, and large schemas keep more
  shallow structure when compacted" (#24118, #27084).** → 👀 **touchpoint: MCP (`cs`).** Our
  `mcp__cs__*` tool schemas pass through Codex's schema handling; richer `oneOf`/`allOf` preservation is
  a fidelity win for the CAS tool surface. Smoke `cs` tool calls on upgrade.
- **`cli: add -P sandbox permissions profile alias` (#27054); multi-agent v2 `close_agent`→
  `interrupt_agent` rename (#26994).** → ✅ no action (profile is an alias; CAS uses `--yolo`, not
  profiles) / 👀 strategic (multi-agent v2 naming churn — informational).
- **"Exclude external tool output from memories" (#26821).** → ✅ no action; Codex's own "memories"
  concept, orthogonal to CAS memory exposed via `cs`.

### 0.130.0–0.135.0 — backfill (consolidated, lighter fidelity)

Reviewed 2026-06-30. Added to extend coverage **below the original 0.136 seed floor** ("going back a
ways"). These versions are well behind both the 0.128 validated pin and the 0.142 install, so this is a
single consolidated triage rather than per-item entries — promote anything here to a real entry only if
an upgrade actually lands on one of these.

- **`--profile` made the primary profile selector across CLI/TUI/sandbox; legacy profile configs
  rejected through migration guidance (0.134, #23708…); managed `requirements.toml` permission profiles
  (0.133).** → 👀 **touchpoint: approval.** CAS workers use `--yolo`, not named profiles — but if any
  CAS-written or host `.codex` config still carries a *legacy* profile block, a 0.134+ Codex would
  reject it with a migration error. Verify no legacy profile config ships in the CAS-managed `.codex`.
- **Subagent identity now included in hook inputs + richer extension/hook context (0.134, #23963,
  #22882); subagent start/stop lifecycle events for extensions (0.133, #22782…).** → 👀 note (Codex
  hooks/extensions; same informational status as 0.140/0.141 — CAS Codex path doesn't use Codex hooks
  yet).
- **AGENTS instruction loading hardened: local global reads + warnings for invalid UTF-8 instead of
  silent drops (0.133, #23343, #23232).** → 👀 **touchpoint: AGENTS.md.** Strictly better — a malformed
  AGENTS.md now warns instead of silently dropping worker priming. The lineage of the 0.138 symlink fix
  and 0.142 foreign-env loading.
- **MCP: per-server env targeting + OAuth for streamable HTTP (0.134, #23583, #24120); `$ref`/`$defs`
  preserved + oversized schemas compacted (0.134, #23357); read-only MCP tools run concurrently with
  `readOnlyHint` (0.134, #23750); removed extra skills roots + string-keyed MCP tool maps (0.130).** →
  👀 **touchpoint: MCP (`cs`).** Schema-fidelity + concurrency improvements that the `oneOf`/`allOf`
  work (0.139) builds on; all strictly helpful to the CAS tool surface. The "extra skills roots
  removed" (0.130) is the early signal of the skills-subsystem consolidation that runs through 0.142.
- **State/SQLite safety: fail-closed when local state can't open + preserve SQLite data (0.131,
  #21831…); memory runtime state moved to a dedicated SQLite DB (0.135, #24591); memory summaries
  versioned/rebuilt when stale (0.132, #23148).** → ✅ no action; Codex's own state/memory DBs, separate
  from CAS `cas.db`. Same hazard class as the 0.140 auto-recover note, no overlap.
- **Git/worktree: "use root worktree hooks consistently, ignore repo hook/fsmonitor config in helper
  commands" (0.131, #21969, #22843).** → 👀 minor touchpoint: factory workers run in worktrees; this
  makes Codex's internal git helpers ignore repo-level hook/fsmonitor config, which is *safer* for the
  CAS factory-commit guard (memory `factory_commit_guard_blocks_main`) — no conflict expected.
- **`CODEX_NON_INTERACTIVE=1` install mode (0.135, #21567); bundled patched zsh helper (0.135).** → ✅
  no action; useful for scripted/CI Codex installs in an onboarding context. TUI/markdown/vim/Windows
  polish across all six → ⏭ n/a.

### 0.138.0 — reasoning-effort order · skills→extension bridge · AGENTS.md symlink fix

Reviewed 2026-06-09 (calm-crane-32 / supervisor). Triage pass vs touchpoints.

- **"Reasoning effort selection is more flexible… model-defined effort levels now flow through in
  the order advertised by the model" (#25623, #26444, #26446).** → 👀 **touchpoint: effort.** CAS
  sets effort via `-c model_reasoning_effort=<e>` and the 0.128 comment notes Codex had no `--effort`
  flag. On the 0.128→0.138 upgrade, verify (a) the `model_reasoning_effort` TOML key still exists and
  (b) our fixed vocabulary (none…xhigh) still validates against model-advertised levels. If 0.138
  added a first-class `--effort` flag, consider switching to it.
- **"Bridge host-loaded skills into the skills extension" (#26172).** → 👀 **touchpoint: `.codex/skills/`.**
  Codex is moving skills into an extension subsystem. Verify our synced `.codex/skills/*.md` still load
  on upgrade.
- **"Workspace instruction loading is more accurate for remote and symlinked workspaces, so the right
  `AGENTS.md` files are picked up" (#26205, #26465).** → 👀 **touchpoint: AGENTS.md.** Factory workers
  run in worktrees; this likely *helps* (more reliable pickup) but verify worker priming still lands.
- **"catalog multi-agent v2 config" (#26254) + multi-agent v2 work.** → 👀 **strategic.** Codex is
  building its own multi-agent orchestration — the same "cede the mechanism, own knowledge + quality"
  fork tracked for Claude Code (Workflow / Agent Teams). Same posture applies: CAS should feed Codex
  multi-agent, not compete with it. No action; flagged for the next strategy pass.
- **Startup resilience: `/usr/bin/bash` support (#26538), OAuth-backed MCP pre-refresh (#26482).** →
  ✅ no action (strictly helpful; the bash one echoes our shell-form vs exec-form lineage).
- **`/app` desktop handoff, local-image paths to model, plugin `--json`, Bazel worktree settings,
  forked-thread titles, TUI streaming whitespace.** → ⏭ n/a (orthogonal to the CAS surface).

### 0.137.0 — skills plumbing → dedicated crates · permission env identity · multi-agent v2

Reviewed 2026-06-09. Triage pass.

- **"Shared prompts, context fragments, and skills plumbing moved into dedicated crates/extension
  paths to reduce `codex-core` coupling" (#25151, #25953, #25959, #26106, #26122, #26167).** → 👀
  **touchpoint: `.codex/skills/`.** Internal refactor of how skills load; watch for format/location
  drift in our synced mirror across the upgrade.
- **"Plugin loading… treats malformed `skills` fields as warnings" (#25782).** → 👀 **touchpoint:
  skills.** If our generated `.codex/skills` frontmatter has a field Codex now scrutinizes, it
  degrades to a warning rather than hard-failing — safer, but verify nothing silently drops.
- **"Permission requests and approvals now carry environment identity" (#25850, #25858, #25862).** →
  👀 **touchpoint: `--yolo`/approval.** CAS workers bypass approvals via `--yolo`; confirm the new
  env-identity carrying doesn't reintroduce a prompt on the bypass path.
- **"Multi-agent v2 keeps runtime choice with each thread… cleaner follow-up and metadata defaults
  for spawned agents" (#25266, #25636, …).** → 👀 strategic (see 0.138 note).
- **"Moved repo review rules and contributor conventions into `AGENTS.md`" (#25682).** → ✅ no action
  (Codex repo's own convention; informs that AGENTS.md is the live instruction surface).
- **F13–F24 keybindings, enterprise credit limits, remote-control pairing, ChatGPT-auth, SQLite
  startup, Python SDK.** → ⏭ n/a.

### 0.136.0 — deny-read enforced in approval-bypass paths · rmcp 1.7.0 · command-safety hardening

Reviewed 2026-06-09. Triage pass.

- **"`deny` read rules stay enforced for safe-command and approval-bypass paths" (#22729, #19880,
  #23943).** → 👀 **touchpoint: `--yolo`.** Most relevant item in the seed: deny-read rules now hold
  even on approval-bypass paths. Verify our `--yolo` workers can still read everything they need (no
  default deny-read that blocks worktree/CAS-root access). Low risk but directly on the bypass path.
- **"Updated MCP dependencies to `rmcp` 1.7.0" (#24763).** → 👀 **touchpoint: MCP (`cs`).** Protocol
  is stable, but a Codex-side MCP client bump is worth a smoke test of `mcp__cs__*` tool calls on
  upgrade.
- **"Command-safety hardening: `/diff` won't run repo Git helpers/hooks; reject browser-origin
  exec-server websocket; no PowerShell parser exec on non-Windows" (#24954, #24946, #24947).** → ✅
  no action (security hardening; doesn't touch our launch surface).
- **"Move memories root setup out of core config" (#24758).** → 👀 minor — Codex has its own
  "memories" concept; confirm no collision with how CAS presents memory via MCP. Likely orthogonal.
- **`/archive` + `codex archive`, OSC 8 TUI links, Windows sandbox elevated setup, Bedrock region
  fallback, image-gen extension.** → ⏭ n/a.

---

## Backlog of opportunities (not required, tracked)

- **Effort flag migration:** if a stable Codex ships a first-class `--effort`, replace the
  `-c model_reasoning_effort=` TOML override (cleaner, version-stable). See 0.138 entry.
- **Multi-agent v2 strategic posture:** decide CAS's stance toward Codex's native multi-agent
  orchestration (mirror of the Claude Code Workflow/Agent-Teams fork). See 0.137/0.138 entries.
- **Future upgrade validation:** rerun the typed 0.146 matrix (effort key, skills/agents and
  AGENTS.md discovery, `--yolo`, direct `cs` MCP calls alongside code mode, interruption, and
  rollout-budget continuity) before advancing the validated pin.
- **Codex hooks adoption (optional):** Codex now has hooks.json + PostToolUse with trust-bypass
  semantics (0.140/0.141). If CAS ever wants Claude-path parity (PreToolUse auto-approve, jail
  exemptions) on the Codex side instead of relying solely on `--yolo` + env, that's the surface.
