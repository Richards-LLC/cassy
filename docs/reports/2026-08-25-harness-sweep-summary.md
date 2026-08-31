# 2026-08-25 harness sweep — what changed, what Cassy changed, what's left

**State in one line:** All three harness diaries are current, both movable validated pins advanced
(Grok → 1.0.5, Codex → 0.149.1) with typed receipts, the Luna-default model rubric shipped, and the
OpenCode/Qwen3.8-Max assessment returned **conditional GO** — the epic PR (#585) is in the merge
queue to main; Slack announcement embargoed until 2026-08-31.

Audience: operator (Daniel). Type: status/release summary with a nested decision brief (OpenCode).
Source branch: `epic/epic-2026-08-25-harness-diary-sweep-claude-codex-g-cas-abfc` (epic cas-abfc),
tip `4bf0f6af`, base `17891528` (main). Date: 2026-08-25.

## Overview

| Workstream | Status | Was → Now |
| --- | --- | --- |
| Claude Code diary | ✅ merged | Current through 2.1.231 → current through **2.1.245** (installed 2.1.245) |
| Codex diary + pin | ✅ merged | Diary through 0.147.0, pin 0.146.0 → diary through **0.149.1**, pin **0.149.1** (live matrix + typed receipt) |
| Grok diary + pin | ✅ merged | Diary through 1.0.3, pin 0.2.114 → diary through **1.0.5**, pin **1.0.5** (live matrix + typed receipt) |
| Model rubric | ✅ merged | Terra default → **Luna `xhigh`-only default**; Terra suspended (operator-gated); light tier → Grok Composer/low |
| OpenCode assessment | ✅ merged | No position → **conditional GO**, 3 blockers, 7 sized tasks (decision needed) |
| Slack thread | ⏸ embargoed | Draft merged; posting held until 2026-08-31 (operator vacation), reminder #1088 set |
| Epic PR #585 | ✅ merged to main | Landed at `39a6c1d5` after one merge-queue rejection (six full-suite spawn tests still asserted Terra/high stock defaults — fixed under the rubric task) |

## What changed upstream (was → now)

### Claude Code 2.1.232 → 2.1.245 (14 versions)

- MCP reconnect/startup, cross-session delivery, hook matching, worktree/deleted-cwd recovery,
  skill reload (incl. UTF-8 BOM files), and background/subagent lifecycle all hardened.
- **No Cassy change required.** Verdicts are ✅/🟢 throughout; standing 👀 watches: MCP startup
  behavior and Cassy-synced skill-mirror precedence on upgrade.
- Source gaps recorded honestly: 2.1.242 and 2.1.244 have no sections in Anthropic's changelog;
  2.1.240/2.1.241 are detail-free rollups. No behavior invented for them.

### Codex CLI 0.148.0 → 0.149.1 (stables)

- 0.148.0: async/MCP hooks, MCP recovery + lazy start, skills-loader overhaul (legacy loader
  removed), fail-closed sandboxing, resumed sessions restore cwd/approval policy.
- 0.149.0: `codex agents` dashboard, rmcp 3.1.2 + MCP tool hooks, SDK exact config overrides and
  **new `max`/`ultra` reasoning-effort levels**, permission-profile restore across resume/fork.
- 0.149.1: patch with no release-note body.
- **Cassy response: validated the whole range live.** Full isolated `PtyConfig::codex` matrix passed
  on installed 0.149.1 (`--yolo` fresh + resumed, `xhigh` accepted, `developer_instructions`
  delivered, `mcp__cs__*` discovered before lifecycle work, mirrors visible). Pin advanced
  0.146.0 → 0.149.1; receipt `crates/cas-pty/conformance/codex-cli-0.149.1-2026-08-25.json`.
- Non-gating probe: `-c model_reasoning_effort=max` **is accepted** by 0.149.1 (exit 0).

### Grok Build 1.0.4 → 1.0.5

- 1.0.4: `GROK_SESSION_ID` exported to tools/MCP, auto-permission grants honored, headless MCP
  wait, subagent lifecycle out-of-order correctness, transcript rebuild from disk.
- 1.0.5: `GROK_CONFIG`/`GROK_CONFIG_PATH` overrides, automatic safe worktree reclaim under
  `~/.grok/worktrees`, hook-block vs user-cancel distinction, session recap on `/resume`.
- **Cassy response: validated live.** Full isolated `PtyConfig::grok` matrix passed on installed
  1.0.5 (bypassPermissions, fresh `--session-id` = `CAS_SESSION_ID` = transcript identity, `cas__*`
  discovery, `--rules` priming, env inheritance, worktree containment). Pin advanced
  0.2.114 → 1.0.5; receipt `crates/cas-pty/conformance/grok-build-1.0.5-2026-08-25.json`.
  Local changelog ends at 1.0.3; 1.0.4–1.0.5 attribution comes from x.ai's official page.

## What Cassy changed this session

| Change | Where | Receipt |
| --- | --- | --- |
| Three diaries brought current | `docs/notes/*-changelog-diary.md` | commits cbe28fb3, fb09cfe1, c5b64e3f |
| Grok pin 0.2.114 → 1.0.5 | `crates/cas-pty` conformance + pty.rs verification block | matrix 1/1 pass (22.24s), receipt `grok-build-1.0.5-2026-08-25`, branch CI green |
| Codex pin 0.146.0 → 0.149.1 | `crates/cas-pty` conformance + diary version status | matrix 1/1 pass (61.25s post-conflict re-run), conformance 5/5, parity 2/2, branch CI green |
| Model rubric: Luna default | all three builtin `cas-supervisor` flavors + reference-history | `cargo check` exit 0; builtins nextest 87/87 |
| OpenCode assessment | `docs/ideation/2026-08-25-opencode-harness-assessment.md` | cited per-touchpoint verdicts + search manifest |
| Slack draft (embargoed) | `docs/release-notes/2026-08-25-harness-diary-sweep-slack.md` | rubric-conformant; posting held |

Rubric detail (operator directives, both codified in the distributed skill, not just memory):

- Default/standard/taste worker: `cli=codex model=gpt-5.6-luna effort=xhigh`. **Luna is only ever
  run at its maximum effort**; `max`/`ultra` are documented as future-only until Cassy's effort
  vocabulary is extended.
- **Terra is suspended at every tier** (dated 2026-08-25, "operator decision pending") — slug kept
  documented, no active route. Light tier re-routed to Grok Composer/low.

## What Cassy still needs to change (decisions and follow-ups)

| Item | Why | Size | Blocked on |
| --- | --- | --- | --- |
| Extend effort vocabulary with `max` (and possibly `ultra`) | Codex 0.149.1 accepts them (probe verified); operator wants Luna at literal max; `Effort` in `crates/cas-mux/src/spec.rs` tops at `xhigh` | S–M | Operator go — pin is already validated |
| OpenCode implementation program (7 tasks) | Conditional GO; primitives all exist | ~15–24 worker-days | Operator go/no-go + `opencode` binary install |
| OpenCode blockers to engineer around | (1) no caller-chosen session ID at spawn; (2) shared SQLite session store needs a liveness/blame adapter; (3) Qwen3.8-Max supports only low/medium/xhigh effort — validate, don't silently remap | inside the 7 tasks | — |
| Post the #cas-internal diary thread | Mandatory publication duty | XS | Embargo lifts 2026-08-31 (reminder #1088) |
| Epic close (cas-abfc) | cas-9cf9 stays open until the post lands | — | Same embargo |

### OpenCode × Qwen 3.8 — nested decision brief

**Recommendation:** proceed with the 7-task adapter program when you're ready to fund ~15–24
worker-days; do **not** advertise `cli=opencode` before task 7's live conformance receipt.

- Launch contract is real: `opencode <dir> --model alibaba/qwen3.8-max --agent cassy-worker
  --prompt … --auto`, inline agent/MCP config via `OPENCODE_CONFIG_CONTENT`, env inheritance
  carries the full CAS identity set; MCP tools surface as `cas_*`.
- Qwen3.8-Max: provider `alibaba`, auth `DASHSCOPE_API_KEY`, 1M context, tool calling; effort
  variants exactly low/medium/xhigh (xhigh default).
- The recommended seam is the existing `cas_mux::Backend` + one OpenCode plugin — no new
  portability framework (deletion test favors it).
- Runner-up (minimum surface, no plugin) was rejected: it cannot satisfy transcript/blame or prove
  requested effort.

## Provenance

- Epic branch `epic/epic-2026-08-25-harness-diary-sweep-claude-codex-g-cas-abfc` @ `4bf0f6af`
  (base main @ `17891528`), PR https://github.com/Richards-LLC/cassy/pull/585 (merge queue).
- Tasks: cas-e59d, cas-d8ce, cas-d21f (diaries) · cas-444a, cas-b9a4 (matrices) · cas-3465
  (assessment) · cas-e352 (rubric) · cas-9cf9 (Slack, embargoed) — epic cas-abfc.
- Worker branch CI: Grok lane run 32859071876, Codex lane run 32859003475 (both green).
- All diary claims trace to the official Anthropic changelog, openai/codex GitHub releases, and
  x.ai/build/changelog; OpenCode claims to opencode.ai docs + pinned source `a7444bf9` and the
  Models.dev catalog (full manifest in the assessment doc).
