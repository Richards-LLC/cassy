# L5 skills audit — engineering, release and workflow skills

**Date:** 2026-09-25 · **Reviewed at:** `4836e56f7` (v3.31.0) · **Task:** cas-9233 (EPIC cas-1660, lane L5) · **Rubric:** L1 v1.1 (`~/.cas/artifacts/cas-63c5/rubric.md`, 8 axes) · **Baseline:** `docs/analysis/2026-09-02-builtin-skills-review.md` · **Status:** findings only, no skill edited

## Verdict

We reviewed all 24 skills in scope, and **14 P0s mislead an agent today**. Each P0 is a small text fix, and each was checked against source, `--help` or a live run. Their causes fall into three groups:

- **The runtime has changed but the text has not:** the required `risk` on task creates, `cas integrate violet`, the QA evidence gate being triggered by paths, the `mcp_execute` call shape, and Viktor's `message` schema.
- **Procedures contradict themselves:** fallow's `|| true` rule, brainstorm's one-question-at-a-time rule against its whole-frontier rounds, and the release-notes template's Markdown against mecha-cassy's lint.
- **The session-learn prompt breaks its own Stop-hook parser.**

Since 2026-09-02 most of the older findings are **fixed**: the `disallowed-tools` contradiction, the `/plan` handoff, operator e-mails, `../../../../` links, NestJS leakage, twin drift, and opt-in sync for fallow. Two regressions came from over-correcting:

- **fallow:** its examples now obey a rule that is itself wrong.
- **release-notes → mecha-cassy:** the reply-count hard rule moved into mecha-cassy.

The largest token savings come from a few places:

| Where | Saving |
|---|---|
| cas-cut-release reading `failure-log.md` in full | −5.7 k per cut |
| fallow body | −4.9 k per invoke |
| fallow `cli-reference.md` | −20 k per read |
| Release trio content-policy copies | −1.5 k per announcement |
| cas-nuxt-playwright | −1.3 k per invoke |
| session-learn Stop prompt | −1.1 k per auto-extracting Stop |

| Severity | A fallow | B release | C QA | D ideation/docs | E tooling/method | Total |
|---|---|---|---|---|---|---|
| P0 | 1 | 3 | 2 | 5 | 3 | **14** |
| P1 | 7 | 6 | 6 | 8 | 8 | **35** |
| P2 | 9 | 10 | 23 | 20 | 21 | **83** |
| P3 | 3 | 8 | 9 | 16 | 17 | **53** |

## Scope and method

- **Skills (24):** fallow, cas-cut-release, cas-qa-craft, cas-nuxt-playwright, cas-playwright-debug, cas-brainstorm, cas-ideate, cas-to-questionnaire, mecha-cassy, release-notes, codemap, project-overview, session-learn, cas-github-issues, mcp-integration, cli-routing, cas-codex-exec, cas-viktor, cas-codebase-design, cas-servers, cas-tdd, cas-wizard, cas-diagnosing-bugs, cas-resolving-merge-conflicts. We read every file under `cas-cli/src/builtins/skills/<skill>/`, including `references/` and `scripts/`, about 430 KB in total.
- **Tools checked against (read-only `--help` or live runs):**
  - cas 3.31.0
  - codex-cli 0.156.0
  - gh 2.101.0
  - claude 2.1.282
  - grok 1.0.41 (`grok inspect --json`)
  - fallow 3.28.0 via npx; 3.15.0 is installed locally
  - Playwright 1.63.0 via npx: a real failing trace, every `trace` subcommand, and a `--debug=cli` session
- **Evidence:** MCP schemas are in `crates/cas-mcp/src/types/`, handlers in `cas-cli/src/mcp/`, and the CLI in `cas-cli/src/cli/`. Upstream changelogs and docs were read through exa-search.
- **Twin parity:** we diffed each canonical skill against its Codex and Grok twins after normalising the tool prefix (`mcp__cas__` → `mcp__cs__` / `cas__`). The residual diff is **0 lines in every file of all 24 skills**, with no extra or missing files, so the older `cas-wizard/template.sh` drift is fixed. The remaining parity defects are Claude-only content shipped unadapted (see Cross-lane).
- **Frontmatter:** `name` equals the directory name in 24 of 24. No reference file carries `name:`/`description:`. `disallowed-tools` appears in none of the 24 (older P0 #14 fixed). All 24 carry a top-level `managed_by: cas`, reported once here as P2; the portable form is `metadata: {managed_by: cas}`. Descriptions run 64–244 characters, so all are under the 250 target.
- **Known cross-lane P0s, referenced but not re-reported:**
  - (a) Bundled non-SKILL.md files never refresh after first install (`sync_builtin_detailed`). This affects every `references/` and `scripts/` finding below; for example, `~/.claude/skills` still holds the old `template.sh` and the orphaned `mocking.md`.
  - (b) In cas-src, Grok resolves skills from `.claude/skills`.
- **Work split:** five read-only sub-reviews (A–E). Their full reports, with every P2/P3 row, prior-review status and search manifest, are Appendices A–E. We re-checked every P0 below against source ourselves.

## Scores (1–5 on the rubric's 8 axes)

Axis 1 is capped at 4 for every skill by the top-level `managed_by` key.

| Skill | 1 FM | 2 Trigger | 3 Disclosure | 4 Wording | 5 Procedure | 6 Accuracy | 7 Parity | 8 Tokens | Verdict |
|---|---|---|---|---|---|---|---|---|---|
| fallow | 4 | 4 | 2 | 3 | 2 | 2 | 4 | 2 | revise M |
| cas-cut-release | 4 | 4 | 2 | 3 | 4 | 5 | 5 | 2 | revise S (tier the failure log) |
| release-notes | 4 | 3 | 4 | 4 | 3 | 3 | 5 | 4 | revise S (Grok name collision, template leak) |
| mecha-cassy | 4 | 3 | 3 | 3 | 4 | 2 | 5 | 3 | revise M |
| cas-qa-craft | 4 | 2 | 3 | 4 | 3 | 2 | 5 | 3 | revise M (trigger vs gate) |
| cas-nuxt-playwright | 3 | 4 | 2 | 3 | 2 | 3 | 3 | 2 | revise M |
| cas-playwright-debug | 4 | 5 | 5 | 5 | 4 | 4 | 5 | 4 | keep, fix one version claim |
| cas-brainstorm | 4 | 4 | 3 | 2 | 3 | 2 | 3 | 2 | revise M |
| cas-ideate | 4 | 4 | 4 | 3 | 3 | 2 | 3 | 3 | revise S |
| cas-to-questionnaire | 4 | 3 | 5 | 4 | 3 | 4 | 3 | 5 | keep |
| codemap | 4 | 5 | 4 | 4 | 4 | 3 | 4 | 3 | revise S |
| project-overview | 4 | 3 | 4 | 4 | 4 | 3 | 5 | 3 | revise S |
| session-learn | 4 | 4 | 2 | 3 | 2 | 1 | 3 | 2 | revise M (runtime prompt) |
| cas-github-issues | 4 | 5 | 4 | 4 | 5 | 2 | 3 | 4 | revise S |
| mcp-integration | 4 | 4 | 4 | 4 | 4 | 2 | 5 | 4 | revise M (allowlist, scopes) |
| cli-routing | 4 | 3 | 3 | 4 | 4 | 3 | 4 | 3 | revise S (shrink to fallback + gate) |
| cas-codex-exec | 4 | 4 | 5 | 4 | 4 | 3 | 5 | 4 | revise S |
| cas-viktor | 4 | 5 | 3 | 4 | 4 | 1 | 5 | 3 | revise S (call args) |
| cas-servers | 4 | 5 | 5 | 3 | 4 | 5 | 5 | 4 | keep, add a webServer subsection |
| cas-codebase-design | 4 | 4 | 2 | 4 | 3 | 3 | 5 | 3 | revise S |
| cas-tdd | 4 | 5 | 5 | 4 | 3 | 4 | 5 | 4 | revise S (factory Rust carve-out) |
| cas-wizard | 4 | 5 | 4 | 4 | 3 | 3 | 5 | 5 | revise S (`open_url`) |
| cas-diagnosing-bugs | 4 | 4 | 5 | 5 | 4 | 5 | 5 | 5 | keep |
| cas-resolving-merge-conflicts | 4 | 5 | 5 | 4 | 3 | 5 | 5 | 5 | keep |

## Prior-review delta (2026-09-02 → 2026-09-25)

| Status | Items |
|---|---|
| **Fixed** | `disallowed-tools` contradiction (brainstorm, ideate); `/plan` handoff; `codemap:187` gate claim; project-overview commit step; doc-family boilerplate moved to `codemap/references/doc-hygiene.md`; `pending_supervisor_review` in cas-github-issues; session-learn `include_str!` (the claim is now true, `handlers_session.rs:1678`); release-notes transport policy and `pippenz@gmail.com`; cli-routing operator e-mail (now `release.claude_account_allowlist`) and `../../../../` links; `-m gpt-5.5` pin and the second codex recipe; mcp-integration now teaches `cas mcp` and `proxy_*`; cas-viktor shows the call shape (but with wrong args, P0 below); NestJS leakage (cas-tdd, cas-codebase-design); tiny references inlined; `cas-wizard/template.sh` twin drift and `set -e` confirm; `note_type` in diagnosing-bugs and merge-conflicts; cas-nuxt-playwright opt-in is now `disable-model-invocation`, it names Firebase + Quasar, and the redundant `user-invocable` is gone; fallow procedure front-loaded and synced only into JS/TS projects (`builtins.rs:2782`) |
| **Regressed** | fallow `\|\| true`: the examples were made to match the rule, but the rule itself now contradicts Procedure step 2, and upstream reversed it (P0-1). The reply-count hard rule left release-notes but reappeared as `mecha-cassy/SKILL.md:76` "one reply", which contradicts the diary's 1+3 contract (P1-9). |
| **Still open (carry-over)** | fallow plugin count (now wrong everywhere: 127 on 3.28.0) and its MCP/Node sections (now also stale); the universal CLAUDE.md release-notes directive (`cli/init/docs_and_skill.rs:23`); content rules copied into 6 places; AskUserQuestion boilerplate (the hook enforces it, `pre_tool.rs:146-160`); ideate "v1" narration; to-questionnaire's undefined output location; codemap exit-status prose; cas-nuxt-playwright has no `Done when`, no cas-servers pointer and no stack-gated sync; cas-servers "never background" stated 3×; release-note posting restated in cli-routing |

## P0 — misleads an agent today

All 14 re-verified against source for this report. Δ is tokens (bytes ÷ 4). Surface: `always` = description, `per-invoke` = SKILL.md body, `on-demand` = references/scripts.

| # | Surface | file:line | Defect | Evidence | Fix | Δ |
|---|---|---|---|---|---|---|
| 1 | per-invoke | `fallow/SKILL.md:21-24`, `:67-68` (rule 2), `:192-195` | Step 2 requires `2>/dev/null \|\| true` on every command, then says "exit 1 means findings; exit 2 means failure". `\|\| true` forces status 0, so neither code is ever visible. A missing binary or npx failure gives empty stdout and rc 0, a silent pass. | `nosuchfallowbin --format json --quiet 2>/dev/null \|\| true` → rc 0, empty. `fallow fix` without `--yes` → `{"error":true,"exit_code":2}`, rc 2 (3.28.0). Upstream Agent Rule 2 now says "Preserve and interpret the exit status". | `fallow <cmd> --format json --quiet 2>"$err"; echo "exit=$?"`. Treat 0 and 1 as a finished analysis; any other code, or `"error": true`, is a stop. Drop `\|\| true` from rules and examples. | ≈0 |
| 2 | per-invoke / on-demand | `mecha-cassy/SKILL.md:65,78`; `references/registration.md:14,90` | Teaches `cas integrate mecha-cassy`. In 3.31.0 that is a hidden, deprecated alias of `cas integrate violet`, accepted for one release. | `cli/integrate/mod.rs:106-116` (`#[command(name = "mecha-cassy", hide = true)]`, "Deprecated name of `cas integrate violet`, accepted for one release (GH #963)"); doctor already says `cas integrate violet` | Replace with `cas integrate violet` in all 4 places and in both twins | ≈0 |
| 3 | on-demand | `mecha-cassy/references/registration.md:71` | Call shape `mcp__cas__mcp_execute server=mecha-cassy tool=mecha_read args={…}`; `mcp_execute` has only `code` and `max_length` | `crates/cas-mcp/src/types/ops_secondary.rs:1256-1272` (`ExecuteRequest { code, max_length }`); JSON dispatch is parsed from `code` (`crates/cas-mcp-proxy/src/lib.rs` `parse_dispatch`) | `mcp__cas__mcp_execute code='{"server":"mecha-cassy","tool":"mecha_read","args":{"channel":"<name>","since":"<RFC3339>","max_messages":50}}'` | +5 |
| 4 | per-invoke / on-demand | `release-notes/references/RUBRIC-template.md:33-45,88-95` vs `mecha-cassy/SKILL.md:36-37` | The template's example and shape use Markdown `**bold**`; mecha-cassy (release-notes step 5) lint-refuses any `**` and requires the two-line `*… — …*` header and `• *Label* — Was: … Now: …` bullets. A project following the template produces drafts the transport refuses. | Quoted lines; `release-train-announce.py` lint | Rewrite the template example in Slack mrkdwn and keep the lint in mecha-cassy format-agnostic (see the overlap map) | ≈0 |
| 5 | always + per-invoke | `cas-qa-craft/SKILL.md:3`, `:24-25` (and `cas-worker.md:24`) | The trigger and step 1 scope QA to a non-empty `demo_statement` ("skip otherwise"). The close gate also demands the full evidence bundle when the diff touches a `qa.user_facing_paths` glob or a catalog journey, so an agent following the skill has its close refused. | `qa_pass.rs:100-108` builds reasons `journeys:`, `path:`, `demo_statement`; `qa_evidence_gate.rs:81-93` maps `path:`/`journeys:` → `EvidenceTier::Bundle`; seed default in `config/meta/seed/qa.rs:90-105` | Description: "Use when a factory delivery needs QA evidence before close: a non-empty demo_statement, a changed user-facing path, or a touched journey." Step 1 runs the eligibility check instead of "skip if empty". Same fix in `cas-worker.md:24`. | +10 |
| 6 | on-demand | `cas-qa-craft/references/evidence-bundle.md:157-160` | Fallback "no `scripts/visual-qa.mjs` → set `visual_qa_status: "unavailable"`". The gate refuses anything but `"pass"` without a supervisor override, and no builtin ships the script, so every downstream web project hits a refused close. | `qa_evidence.rs:474-483` ("only \"pass\" closes without a supervisor override"); `find cas-cli/src/builtins -name 'visual-qa*'` → 0 | Either ship `visual-qa.mjs` as a qa-craft script, or say plainly: "without the script, `unavailable` needs a supervisor override; request it with blocker=true before closing". | +20 |
| 7 | per-invoke | `cas-github-issues/SKILL.md:122-126` | The per-issue `task_type=bug` create omits `risk`, so every create in the sweep is rejected | `mcp/tools/types/task.rs:124-140` ("TASK CREATE REJECTED: risk is required for task, bug, and feature tasks"), called from `core/task/lifecycle.rs:811` | Add `risk=<none\|platform\|concurrency\|blast-radius>` (+ `proof_targets` when blast-radius) | +10 |
| 8 | on-demand | `cas-brainstorm/references/handoff.md:58-61` | The "proceed directly to work" create omits `risk`, so it is rejected | same | Add `risk=`; better, delete the create (overlap map: supervisor owns creation) | +8 / −40 |
| 9 | on-demand | `cas-ideate/references/post-ideation-workflow.md:170-173` | The "Brainstorm: X" task create omits `risk`, so it is rejected; the task would also never be closed | same; brainstorm's handoff creates its own | Delete the create; the memory pointer (`:98-101`) already records the handoff | −45 |
| 10 | per-invoke | `cas-brainstorm/SKILL.md:32` vs `:41` | "Ask ONE question at a time… Never batch" contradicts "ask the full frontier… Number every frontier question" | quoted | Keep one rule: "Each round, ask only questions whose prerequisites are settled, numbered, each with a recommended answer (usually one)." | −60 |
| 11 | per-invoke + runtime | `session-learn/SKILL.md:70` | The body is the live Stop-hook prompt. It tells the model to "omit the rest of the body" when `dedup_hits` is non-empty, but `SessionLearnDraft` requires `signal`, `entry_type`, `scope`, `content` and `confidence`. One such draft fails `serde_json::from_str::<Vec<_>>` and the whole batch is dropped. | `hooks/handlers.rs:207-227` (no `serde(default)`); `handlers_session.rs:1678` (`include_str!`), `:1778-1779`; `stop_flow.rs:491` | Always emit `signal`, `entry_type`, `scope`, `confidence` and a one-line `content`; or add `#[serde(default)]` (code, other lane) | +10 |
| 12 | per-invoke | `cas-viktor/SKILL.md:28` | The example `ask_viktor` args are `{"question":…,"cas_task_id":…}`. Viktor requires `message` and has no `cas_task_id`, and the proxy does not remap. | Live `mcp__viktor__ask_viktor` schema: `required:["message"]`; props `message, metadata, idempotency_key, response_format, speed, timeout_seconds` | `"args":{"message":"<bounded question>","metadata":{"cas_task_id":"<task-id>"},"idempotency_key":"<task-id>-<n>"}` | +10 |
| 13 | per-invoke | `mcp-integration/SKILL.md:39-57` (whole skill) | Never mentions the proxy allowlist. `cas mcp add` and `proxy_add` do not write it, it is fail-closed, and a project `.cas/proxy.toml` list replaces the user list. Step 5's test call is denied after every fresh add, and creating a project `proxy.toml` silently drops user routes. | `crates/cas-mcp-proxy/src/config.rs:53-59` ("An empty list is intentionally fail-closed"), `:421-425`; `grep allowlist cas-cli/src/cli/mcp_cmd.rs` → 0 | Add a step: "add the `(server, tool)` route to the `allowlist` of the proxy.toml that wins (project replaces user)", with the replace-not-merge warning | +60 |
| 14 | per-invoke | `mcp-integration/SKILL.md:24-28` | Applies Claude's `-s local` semantics to `cas mcp`. For `cas mcp add`, `local` (default) and `project` both write `<cas_root>/proxy.toml`, which every worktree shares, so "choose `user` for workers" needlessly publishes a project server machine-wide. | `cli/mcp_cmd.rs:54-55`, `:118-124`; `store/detect.rs:50-70` | "For `cas mcp add`, `local` and `project` are the same file and already reach every worktree; use `user` only for a server every project on the machine should see." | ≈0 |

## P1 — routing, format, stale third-party claims

Condensed; full evidence is in the appendix rows.

| # | Surface | file:line | Defect → fix | Δ |
|---|---|---|---|---|
| 1 | per-invoke | `cas-cut-release/SKILL.md:13` | "Read `references/failure-log.md` in full": 31.9 KB (≈8 k tokens) per cut, append-only, and 63 of 88 entries are already enforced by `release-gate.sh`. Read only the 25 `manual:*` entries (≈9.1 KB); update the pin `builtins.rs:8315`. | −5,700/cut |
| 2 | always | `release-notes` (Grok twin) | Name collides with Grok's built-in `/release-notes`; Grok exposes it only as `user:release-notes` (`grok inspect --json`). Rename to `cas-release-notes`, update `docs_and_skill.rs:23` and the pins. | 0 |
| 3 | on-demand | `release-notes/references/RUBRIC-template.md:62-76` | cas-src release-train receipt rules (`release-report.receipt`, Slack file ids) shipped to every project as "may add, never relax". Reduce to "after a published version run cas-release-report and link its artifacts". | −200 |
| 4 | always | `cli/init/docs_and_skill.rs:23` | Carry-over: universal staging/main Slack duty. Use "Release-note duties, if any, are defined by `docs/release-notes/RUBRIC.md`." | −10 |
| 5 | per-invoke | `mecha-cassy/SKILL.md:9` vs `:35-37` | "Owns only transport", yet carries ≈1.8 KB of cas-src content policy (forbidden words, `Cassy vX.Y.Z` label, reply grammar), so downstream posts are labelled "Cassy". Keep only the mrkdwn lint and move the content policy to the rubric. | −450 |
| 6 | always / per-invoke | `mecha-cassy/SKILL.md:3` vs `:39,76` | Description promises diary posts; the procedure is runtime-only and hard-codes one reply per thread (diary is 1+3, Grok→Claude→Codex, `RELEASE_SLACK_RUBRIC.md:227-247`). Generalise to "parent → replies in rubric order". Regression of the older reply-count item. | −20 |
| 7 | per-invoke | `cas-qa-craft/SKILL.md:86-104` vs `:20-31` | The telemetry sweep says "first QA step", but it sits after the procedure; journeys are also outside it. Fold both into the numbered steps and delete the restatements. | −150 |
| 8 | per-invoke | `cas-nuxt-playwright/SKILL.md:276` | `cli attach` then bare `step-over`; every command after attach needs `-s=<session>` (live: "browser 'default' is not open"). Point to cas-playwright-debug §2. | −40 |
| 9 | per-invoke | `cas-nuxt-playwright/SKILL.md:211,297` | Claims `<q-btn>` breaks `getByRole('button')` and recommends `getByText`. It does not (live run); the real case is q-btn with `to`/`href` → role link. | 0 |
| 10 | always (Codex) | `cas-nuxt-playwright/SKILL.md:5` | `disable-model-invocation` is not read by Codex (needs `agents/openai.yaml` `policy.allow_implicit_invocation:false`, never generated). Cross-lane parity fix for every opt-in skill. | 0 |
| 11 | routing | `cas-frontend-engineering/SKILL.md:11,100-101` | Routes the model to the model-uninvocable cas-nuxt-playwright and credits it with locks/retries that live in cas-playwright-debug §4. (L4 scope; flagged here because of the overlap.) | +10 |
| 12 | per-invoke | `cas-playwright-debug/SKILL.md:13-15,61-67` | "trace CLI and `--debug=cli` need 1.59+", but `npx playwright cli attach` ships only from **1.62** (1.62 release notes). State both floors. | +20 |
| 13 | per-invoke | `fallow/SKILL.md:114-143` | MCP tool table lists 22 tools; 3.28.0 has 38 (`fallow schema`). Delete it; point at `fallow schema`. | −1,000 |
| 14 | per-invoke | `fallow/SKILL.md:145-165` | Node-bindings section (library embedding, and "six functions" when there are eight). Delete it. | −340 |
| 15 | per-invoke + on-demand | `fallow/SKILL.md:15,228,330,379`; `gotchas.md:26`; `patterns.md:599` | "90"/"91" plugins; the real count is 127 (3.28.0) / 123 (3.15.0). Drop the number. | −10 |
| 16 | per-invoke | `fallow/SKILL.md:315-326` | Exit-code table lists 0–2; 3.x defines 0–8 and 10–13. Point at `fallow schema` → `exit_codes`. | +20 |
| 17 | on-demand | `fallow/references/*` | The vendored snapshot is ≈2.57-era (`"version": "2.57.0"`, schema 3 vs live 9). It misses `review --brief`, `inspect`, `trace`, `guard`, `security`, `--type-aware`. Re-vendor from `fallow schema`/`--help` or trim to pointers; pin the upstream commit. | −20,500/read (trim) |
| 18 | on-demand | `fallow/references/patterns.md:675-762` | Recommends deprecated `fallow setup-hooks`. Its successor `fallow agent install` writes its own skill and hooks into `.claude/`, colliding with CAS-managed files. Tell agents not to run either. | −400 |
| 19 | on-demand | `cas-brainstorm/references/handoff.md:33` vs `:46-53` | Brainstorm creates the epic, and cas-supervisor `intake.md:23` also creates one, giving duplicate epics. Hand the doc path to the supervisor. | −60 |
| 20 | per-invoke | `cas-ideate/SKILL.md:90` | "Light lane, e.g. GPT-6 Luna/xhigh" is a factory `spawn_workers` lane, not an Agent-tool option. Say "the harness's smallest model". | −5 |
| 21 | per-invoke | `codemap/SKILL.md:110,122`; `project-overview/SKILL.md:117-123` | "One build turns the doc into a knowledge page / at most one model call" is false with a stale ledger: pending sources are cut to `--max-sources` in path order (`knowledge/pipeline.rs:276-279`); cas-src dry run → 1,120 pending. Gate on `cas knowledge build --dry-run`. | −70 |
| 22 | per-invoke / on-demand | `codemap/SKILL.md:104`; `doc-hygiene.md:25-32` | "Update the pointer if it exists" has no find call; codemap's title adds `.md`; cas-src has 4 duplicate codemap pointers. Specify search-then-update and drop the suffix. | +30 |
| 23 | runtime | `session-learn/SKILL.md:44` | Hook mode runs with `max_turns(1)` (`handlers_session.rs:1758-1766`), so it cannot "scan memory via `mcp__cas__search`". Mark this step interactive-only. | +10 |
| 24 | per-invoke | `session-learn/SKILL.md:3` vs `:19-21` | When the user invokes it, no step stores the drafts ("the caller writes", but the agent is the caller). Add a store step and `Done when`. | +30 |
| 25 | per-invoke | `cas-github-issues/SKILL.md:45` (and cas-worker, cas-supervisor, CLAUDE.md snippet) | `issues.components.mecha_cassy` is deprecated in favour of `issues.components.violet` for one release (`config/access/mod.rs:9-12`). | 0 |
| 26 | on-demand | `cli-routing/references/routing.md:14-16` + `cas-codex-exec/SKILL.md:17,34` | Claims cas-codex-exec's recipe closes stdin; it does not. `codex exec` appends piped stdin (`--help`). Add `< /dev/null` at the owner. | +3 |
| 27 | on-demand | `cli-routing/references/routing.md:18-21` | Recommends `--dangerously-bypass-approvals-and-sandbox` for a small write; codex 0.156 has `-s workspace-write` + `--add-dir`. | +10 |
| 28 | always | `cli-routing/SKILL.md:3` vs `cas-codex-exec/SKILL.md:3` | Both trigger on "one-shot `codex exec`", so both fire. Narrow cli-routing to capacity/auth fallback and the Claude gate. | +5 |
| 29 | per-invoke | `cas-viktor/SKILL.md:35-39`; `gateway.md:49-50` | Forbids retrying an uncertain start but never uses Viktor's `idempotency_key`, which makes retries safe. | +25 |
| 30 | per-invoke | `cas-codebase-design/SKILL.md:106-107` | Points to a "cas-update-and-doctor-read-like-reports" precedent that exists nowhere; replace it with a cas-cli-craft pointer. | −10 |
| 31 | per-invoke | `cas-tdd/SKILL.md:18,40,43` | Requires red/green test runs; factory workers on Rust lanes are denied cargo. Add a one-line carve-out pointing at cas-worker and the `ASSEMBLY_PROOF` (same P2 in diagnosing-bugs and merge-conflicts). | +35 |
| 32 | on-demand | `cas-wizard/scripts/template.sh:9` | `open_url` chains `A && B \|\| C && D \|\| E`; after a successful `xdg-open` it also runs `open` (reproduced: URL opens twice; `open` is `openvt` on Debian). Use if/elif. | +10 |

The remaining P1 rows are in Appendix B (release trio) and Appendix C (QA). They restate P1s 1–12 per skill and add no new defects.

## Overlap map — who should own what

| Cluster | Topic | Current copies (file:lines) | Owner | Others |
|---|---|---|---|---|
| **codex-exec vs cli-routing** | Trigger "one-shot `codex exec`" | codex-exec `:3`; cli-routing `:3` | cas-codex-exec | cli-routing narrows to capacity/auth fallback + Claude account gate |
| | Codex flags (sandbox, `-C`, `-o`, stdin, write mode, `--output-schema`, reasoning effort) | codex-exec `:11-35`; routing.md `:12-32` | cas-codex-exec | Move routing.md `:18-32` to codex-exec (≈ +80 there, −500 across both) |
| | Claude `claude -p` + account gate | cli-routing `:20-27`; routing.md `:34-79` | cli-routing | — |
| | Release-note posting | cli-routing `:32-40`, routing.md `:81-96` | release-notes + mecha-cassy | Delete from cli-routing |
| **brainstorm vs ideate vs to-questionnaire** | Routing / pipeline position | brainstorm `:9-15`; ideate `:9-15,23`; cas-supervisor `intake.md:29-47` | cas-supervisor intake.md | Each skill keeps a one-line output contract (−400) |
| | Question mechanics (frontier, one-at-a-time) | brainstorm `:28-43,165-178`; ideate `:26-28`; handoff.md `:13,19` | cas-brainstorm (after P0-10) | ideate links; drop the AskUserQuestion sentences (hook enforces them) |
| | Divergent idea generation | ideate `:137-140`; brainstorm `:184-187` | cas-ideate | brainstorm keeps "2–3 approaches, one non-obvious" |
| | Third-party facts/decisions | to-questionnaire `:12`; brainstorm `:43` | cas-to-questionnaire | brainstorm handoff 4.1 offers it |
| | Task/epic creation after ideation | handoff.md `:46-61`; post-ideation `:170-173`; intake.md `:23`; planning.md `:46-98` | cas-supervisor planning.md | brainstorm and ideate create nothing; hand over doc path + memory pointer (fixes P0-8/9 and P1-19) |
| | Repo grounding | ideate `:84-117`; brainstorm `:118-140` | codemap / project-overview | Read CODEMAP / PRODUCT_OVERVIEW first |
| **playwright-debug vs nuxt-playwright vs qa-craft** | Trace-CLI triage, `--debug=cli`, flake control | debug `:18-123`; nuxt `:217,259-279,301-302,321,339`; frontend-eng `:100-101` | cas-playwright-debug | nuxt keeps Nuxt/Firebase/Quasar rows only (−1.3 k); frontend-eng repoints |
| | Acceptance assertions, component `mount` | frontend-eng `:67-96`; nuxt `:281-288` | cas-frontend-engineering | nuxt deletes |
| | QA close gate + bundle file list | qa SKILL `:41-53,62-75`; evidence-bundle `:17-49`; independent-pass `:101-115`; journeys `:43-50`; cas-worker close-gate.md `:68-74` | `evidence-bundle.md` (the code's `CONTRACT_REFERENCE`, `qa_evidence.rs:23`) | One line + pointer elsewhere |
| | Matrix quotas | qa SKILL `:26-31`; matrix-builder `:9-25`; independent-pass `:51-61`; epic-flow-walk `:21-27` | matrix-builder.md | "per matrix-builder" + local override |
| | webServer lifecycle | cas-servers `:3,23` (advertised, 1 sentence); nuxt template `:234-242` (`reuseExistingServer:true`); qa `:35-36`; journeys `:35-39` = independent-pass `:29-34` | cas-servers (new 5-line subsection) | Template and qa-craft point to it |
| **release-notes vs cut-release vs mecha-cassy (vs release-report)** | Content rules (Was→Now, no tickets, one punch, forbidden words) | release-notes `:34-40`; template `:47-60`; mecha-cassy `:35-37,76`; cut-release `:37-39`; `RELEASE_SLACK_RUBRIC.md:184-225` | project rubric | Six copies → one; three different forbidden-word lists today |
| | Format / mrkdwn lint | template `:33-45,88-95` (Markdown); mecha-cassy `:36-37` | rubric (format), mecha-cassy (format-agnostic lint) | Fixes P0-4 |
| | Thread order / reply count | template `:29-31`; mecha-cassy `:39,76`; cut-release `:55-56,72` | rubric | mecha-cassy runs a generic parent→replies loop |
| | Transport, preflight, `## POSTED` receipt | mecha-cassy `:9-72` (owner); release-notes `:24-32`; release-report `:57-58` (wrongly credits release-notes) | mecha-cassy | release-notes hands off; release-report fixes `:57-58` |
| | Release mechanics (PR, queue, tag, publish) | cut-release `:19-79`; `RELEASE_SLACK_RUBRIC.md:38-91,132-177` (manual `gh pr merge --merge`, contradicts the merge-queue enqueue in `release-train.sh:531-535`) | cas-cut-release | Rubric points to train stages (doc fix, P1) |
| **mcp-integration vs cas-viktor** | Proxy ladder, credentials, retry classes, allowlist | mcp-integration `:53-70` (allowlist missing); viktor `:13-54`; gateway.md `:5-53` | mcp-integration | cas-viktor keeps its call shape, 9 routes, watch/cost |
| **other** | Printed-output critique | codebase-design `:83-123`; cas-cli-craft `:17-44` | cas-cli-craft | codebase-design points |
| | Long-lived processes vs `&` | codex-exec `:30-35` (`&`); cas-servers `:9,85-87` ("never `&`") | cas-servers | codex one-shots use the harness background runner |
| | Factory Rust no-build rule | tdd `:18,40-43`; diagnosing-bugs `:21,54-57`; merge-conflicts `:18` | cas-worker discipline.md `:8-26` | One-line carve-out pointer each |

## Cross-lane items (outside this lane's files)

1. **Codex opt-in parity.** `disable-model-invocation` has no Codex mapping. CAS never generates `agents/openai.yaml` (`policy.allow_implicit_invocation: false`), so cas-nuxt-playwright and cas-to-questionnaire are model-invocable in Codex. This belongs to the parity lane.
2. **`mcp_search`/`mcp_execute` schema text.** `ops_secondary.rs:1260-1262` describes `code` as "TypeScript code…", while the tool descriptions and proxy `parse_dispatch` accept JSON dispatch. Agents copy the wrong form (the root cause of P0-3 and P0-12). This is a code fix in cas-mcp.
3. **`SessionLearnDraft` fragility.** Adding `#[serde(default)]` in `hooks/handlers.rs:207-227` would make P0-11 non-fatal regardless of prompt wording.
4. **Deprecated names ahead of removal:** `cas integrate mecha-cassy` and `issues.components.mecha_cassy`. Both expire next release, so sweep every builtin (cas-worker.md `:72`, cas-supervisor.md `:67`, `filing-cas-bugs.md:19,33`, the CLAUDE.md snippet) before the alias is removed.
5. **`cas-worker.md:24`** repeats the demo_statement-only QA trigger (P0-5). This is the always-loaded worker guidance, so it belongs to the L2/L3 lane.
6. **`docs/RELEASE_SLACK_RUBRIC.md`** has 8 contradictions with the release train (Appendix B §Contradictions). The main one is the manual `gh pr merge --merge` vs merge-queue enqueue.
7. **Claude-only content in twins.** AskUserQuestion, Glob, `.claude/scheduled_tasks.json`, `claude --resume` (D) and "Bash tool cancels parallel commands" / `$ARGUMENTS` (fallow) ship unadapted to Codex and Grok.

## Token economy — ranked by delta × multiplier

| Rank | Change | Surface | Δ per use | Multiplier |
|---|---|---|---|---|
| 1 | cas-cut-release reads only `manual:*` failure-log entries | per-invoke | −5,700 | every release cut (and the log grows) |
| 2 | session-learn runtime prompt trimmed to the hook contract | runtime | −1,100 | every auto-extracting Stop hook |
| 3 | fallow body → procedure + pointers to `fallow schema` / `--help` | per-invoke | −4,900 | JS/TS projects only (synced by stack) |
| 4 | Release trio single-owner split | per-invoke | −1,500 | every announcement |
| 5 | cas-nuxt-playwright duplicates removed | per-invoke | −1,300 | opt-in only |
| 6 | cas-brainstorm / cas-ideate dedupe (question rules, anti-pattern lists, pipeline prose) | per-invoke | −900 / −500 | every brainstorm / ideation |
| 7 | fallow `cli-reference.md` (82 KB) trimmed to pointers | on-demand | −20,500 | per read |
| 8 | cli-routing shrink / codex-exec consolidation | per-invoke | −500 | per one-shot |
| 9 | `managed_by` → `metadata` | always | ≈0 | portability only |

No always-loaded description in scope is over budget (max 244 chars).

## Appendices — sub-review reports

Appendices A–E hold the five sub-reviews as written, with their headings demoted two levels. Each contains the per-skill sizes, prior-review status, all P0–P3 rows (`Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens`), twin-parity notes and its own search manifest. Paths in those tables are relative to `cas-cli/src/builtins/skills/<skill>/` unless prefixed.

### Appendix A — A-fallow — L5 audit of skill `fallow` (task cas-9233)

Repo HEAD 4836e56f7 (v3.31.0). Read-only. Third-party baseline: fallow **3.28.0** (npm latest, crates.io `fallow-cli` 3.28.0 published 2026-09-22) via `npx -y fallow@3.28.0 <cmd> --help` and `fallow schema`. Local global install is 3.15.0 (`~/.nvm/.../bin/fallow`). Upstream skill: github.com/fallow-rs/fallow-skills, now at `fallow/skills/fallow/` (last commit 2026-09-24, #52).

#### Size

| File | Bytes | Lines | ≈tokens | Surface |
|---|---|---|---|---|
| SKILL.md (frontmatter description, 172 chars) | 172 | 1 | 43 | always |
| SKILL.md body | 27,032 | 396 | 6,760 | per-invoke |
| references/cli-reference.md | 81,980 | 1,605 | 20,500 | on-demand |
| references/gotchas.md | 21,722 | 611 | 5,430 | on-demand |
| references/patterns.md | 20,420 | 762 | 5,100 | on-demand |
| **Total bundle** | **151,154** | 3,374 | **37,800** | |

SKILL.md body by section (bytes): Procedure 713 · When to Use 565 · When NOT 269 · Prerequisites 299 · Agent Rules 914 · Commands 3,500 · Issue Types 1,912 · **MCP Tools 5,267** · **Node.js Bindings 1,399** · References 307 · **Common Workflows 5,696** · Exit Codes 391 · **Configuration 3,390** · Key Gotchas 781 · Instructions 653.

Upstream for comparison (fetched 2026-09-25): SKILL.md 30,188 B; references 325 KB (cli-reference alone 189,942 B), plus new mcp.md, issue-types.md, node-bindings.md, similar-code.md. Re-vendoring would double the bundle.

#### Scores (1–5)

| Skill | 1 Frontmatter | 2 Trigger | 3 Disclosure/size | 4 Wording | 5 Procedure/completion | 6 Accuracy | 7 Parity | 8 Tokens |
|---|---|---|---|---|---|---|---|---|
| fallow | 4 | 4 | 2 | 3 | 2 | 2 | 4 | 2 |

#### Prior-review status (docs/analysis/2026-09-02-builtin-skills-review.md :67, :108 #22, :153, :168, :178, :407, :466, :495)

- `|| true` rule vs examples (#22, P0): **FIXED as written** — 31 `|| true` in SKILL.md now, every example uses it (commit 5c6676d9a). **But the rule is now itself wrong**: it makes Procedure step 2 contradictory, and upstream reversed it. The new finding is F1 below, not a carry-over.
- Procedure last (`:373-382`): **FIXED** — `## Procedure` at SKILL.md:17-29 with a completion sentence. **Leftover**: the old `## Instructions` (:387-396) still duplicates it (F8).
- 90 vs 91 plugins: **STILL OPEN (carry-over), and worse** — "91" at :15,:228,:379; "90" at :330, gotchas.md:26, patterns.md:599. Real counts: 3.15.0 = 123, 3.28.0 = 127 (`fallow schema` → `plugins.count`). Upstream dropped the number (F4).
- MCP/Node sections irrelevant to a CLI agent (`:100-151`): **STILL OPEN (carry-over)** — now :114-165, 6.7 KB, and stale as well (F3).
- Stack-specific skill shipped to every project: **FIXED at project level.** `OPTIONAL_PROJECT_SKILLS` (builtins.rs:2782) together with `enabled_optional_project_skills` (:2865-2920) syncs fallow only when the project has `package.json`, contains JS/TS within depth 4, or sets `[skills].optional`. **Residual**: user-level sync keeps the full catalog (builtins.rs:2789 doc comment). It is installed and byte-identical in `~/.claude`, `~/.claude-daniel@…`, `~/.codex` and `~/.grok` `/skills/fallow`, so the description is paid in every session. That costs 43 tok, which is acceptable (P3).
- Three install paths, no detection (`:39-49`): **PARTLY FIXED** — Procedure step 1 says "available (or use `npx fallow`)". It still gives no concrete detection command and no version check (F9).

#### Findings

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P0 | per-invoke | SKILL.md:21-24, :67-68 | Procedure contradicts itself. Step 2 mandates `2>/dev/null \|\| true`, then says "exit code 1 means findings; exit code 2 means the command failed". `\|\| true` forces status 0, so the agent can never see 1 or 2. With `2>/dev/null` a missing binary or npx failure yields empty stdout and rc 0, which fails silently. | `nosuchfallowbin --format json --quiet 2>/dev/null \|\| true` gives rc=0 and empty output. `fallow fix` without `--yes` gives `{"error":true,…,"exit_code":2}`, rc=2 (measured on 3.28.0). Upstream Agent Rule 2 now reads: "Preserve and interpret the exit status… Do not force a successful status, because that hides validation, license, setup, network, and security-gate outcomes". Upstream Rule 1 also says keep stderr separate, not `/dev/null`. | Replace both rules and step 2 with `fallow <cmd> --format json --quiet 2>/tmp/fallow.err; echo "exit=$?"`. `echo` keeps the Bash call successful, so parallel calls are not cancelled, and the code stays visible. 0/1 = analysis OK; any other code, or `"error": true` on stdout, means read /tmp/fallow.err and stop. Then drop `\|\| true` from all 31 examples. | −120 |
| P1 | per-invoke | SKILL.md:192-195 | Example pairs `--fail-on-issues` with `\|\| true`, then claims "Exit code 1 if new dead code is introduced". The exit code is swallowed, so the example cannot do what the prose says. | Line text. Root help: `--fail-on-issues  Exit with code 1 if issues are found`. | Covered by the F1 rewrite, or drop `--fail-on-issues` (the JSON `total_issues` is the agent signal). | −5 |
| P1 | per-invoke | SKILL.md:114-143 | MCP tool table is stale: it lists 22 tools. The skill never tells the agent to register or call `fallow-mcp`. | 3.28.0 `fallow schema` → `mcp_tools.tools` = 38 (new: `code_execute`, `security_candidates`, `find_similar_code`, `inspect_similar_code`, `inspect_target`, `guard`, `get_cloud_runtime_context`, `get_token_blast_radius`, `decision_surface`, `recommend`, `list_suppressions`, `impact`, `impact_all`, `trace_symbol`, `symbol_impact`, `trace_import_path`, `trace_error`, `impact_closure`). 3.15.0 = 33. Upstream moved MCP to references/mcp.md. | Delete the section. Add one line: "If a `fallow-mcp` server is registered, prefer its tools; the list is in `fallow schema` → `mcp_tools`." | −1,300 |
| P1 | per-invoke | SKILL.md:145-165 | Node.js bindings section is for people embedding fallow in their own code, not for an agent running the CLI. It is also stale ("Six async functions"). | Upstream node-bindings.md: "Eight async functions" (adds `detectSimilarCode`, `detectFeatureFlags`); `@fallow-cli/fallow-node` 3.28.0 on npm. | Delete. Optionally keep the docs URL in References. | −340 |
| P1 | per-invoke + on-demand | SKILL.md:15,:228,:330,:379; gotchas.md:26; patterns.md:599 | Plugin count is hard-coded, inconsistent (90 and 91), and wrong for every shipped fallow. | `fallow schema` → `plugins.count`: 127 on 3.28.0, 123 on 3.15.0. | Remove the number ("built-in framework plugins; see `fallow list --plugins`"), as upstream did. | −10 |
| P1 | per-invoke | SKILL.md:315-326 | Exit-code table lists only 0/1/2. fallow 3.x defines 0–8 and 10–13. For example, 3 means `config --path` found no config or a license problem, 7 is a network failure, and 8 is a security gate. | `fallow schema` → `exit_codes` (14 entries); `config --help`: "`--path` exits 3 when no config file exists". | Keep 0/1/2 and add "other codes: see `fallow schema` → `exit_codes`". | +20 |
| P1 | on-demand | references/* (whole) | Vendored reference snapshot is about 2.57-era, while the current release is 3.28.0. JSON samples are pinned to `"version": "2.57.0"`, `schema_version: 3` (8 hits). Live output is `schema_version: 9`, `version: 3.28.0` and carries `kind` and `next_steps`. It misses the 3.x agent surface: `review --brief`, `inspect`, `trace`, `guard`, `security`, `similar-code`, `doctor`, `agent install`, `hooks`, `--type-aware`, `--diff-file`, `--output-file`. Upstream root help now opens with a "When the agent is about to…" routing table, and `fallow schema` ships `task_matrix`. | `grep -c '"version": "2' references/*.md` = 8. Live run of `fallow@3.28.0 dead-code --format json` on a /tmp copy of hub-web showed `schema_version 9`. The root `--help` "Analysis/Project inspection" lists. | Make the tool the reference. Drop cli-reference.md and replace it with "run `fallow <cmd> --help` or `fallow schema` (commands, flags, issue_types, exit_codes, task_matrix; always JSON)". Those are version-matched by construction. Keep a trimmed gotchas.md. | −20,500 on-demand |
| P1 | on-demand | patterns.md:675-762 | Tells the agent to run `fallow setup-hooks`, which is deprecated in 3.28.0. It also writes a PreToolUse hook into `.claude/settings.json`, a file CAS manages. The successor, `fallow agent install`, additionally writes a fallow skill into `.claude/skills/` or `.agents/skills/` and a CLAUDE.md import. That collides with the CAS-managed `skills/fallow` and with CAS hook config. | `setup-hooks --help`: "Deprecated: use `fallow agent install` … or `fallow hooks install --target agent`". `agent install --help`: "skill: The fallow skill under `.claude/skills/` or `.agents/skills/`", plus `--force` "Replace skills… fallow did not write". | Replace with: "In CAS projects do not run `fallow agent install`/`setup-hooks`; ask before `fallow hooks install --target git`." | −900 on-demand |
| P2 | always/frontmatter | SKILL.md:4 | Top-level `managed_by: cas` is a non-standard key. `metadata:` already exists at :6-10. | rubric Axis 1 | Move it to `metadata.managed_by: cas`. | 0 |
| P2 | per-invoke | SKILL.md:387-396 vs :17-29 | `## Instructions` repeats the Procedure (dry-run, report, suppressions). Line 390 contradicts the Procedure (`--format json --quiet` with no stderr handling). | Side-by-side read. | Delete :387-394 and fold the `$ARGUMENTS` line into Procedure step 2. | −160 |
| P2 | per-invoke | SKILL.md:173-313 | Common Workflows is 5.7 KB of 14 near-duplicate example blocks, which models copy literally (see F1). They overlap patterns.md (Full audit :27, PR check :67, auto-fix :459, debugging :531) and the cli-reference examples. | Section byte count; patterns.md headings. | Keep 4 canonical invocations (dead-code, dupes, fix dry-run→yes, trace), each in the F1 form. Put the rest behind `fallow --help`'s "When the agent is about to…" table. | −1,100 |
| P2 | per-invoke | SKILL.md:75-113 | Commands table (3.5 KB; health row alone ≈1.2 KB of flags and Angular suppression prose) and Issue Types table (1.9 KB) restate `--help` and `fallow explain`. They are incomplete for 3.28: they miss `review`, `inspect`, `trace`, `guard`, `security`, `similar-code`, `doctor`, `suppressions`, `workspaces`, `viz`, `hooks`, `agent`. | Root `--help`; `fallow schema` → 42 commands and 117 issue types. | Replace with a 6-row intent→command table taken from `fallow --help`'s agent routing block, plus "`fallow explain <issue-type>` for any finding". | −1,000 |
| P2 | per-invoke | SKILL.md:328-374 | The 3.4 KB Configuration block (config-field essays for `ignoreExportsUsedInFile`, `usedClassMembers`, `resolve.conditions`) is reference material in the body, and it conflicts with "zero config by default" (:379). It omits that `fix` auto-creates `.fallowrc.json` when none exists. | `fix --help`: "When no fallow config exists … a fresh `.fallowrc.json` is created … Pass `--no-create-config`". | Keep the precedence line plus the suppression-comment snippet. Add "`fix` creates `.fallowrc.json` unless `--no-create-config`". Move field docs to `fallow config-schema`. | −700 |
| P2 | per-invoke | SKILL.md:13-15, :31-51 | Stance before procedure is gone from the top, but the tagline paragraph (:15) repeats the description and :31-42 "When to Use" repeats the frontmatter trigger. | Read. | Drop :15 and fold "When NOT" into one line under Procedure. | −250 |
| P2 | per-invoke | builtins.rs:580-584 (vendoring comment) + SKILL.md:6-10 | Vendoring provenance is false and unpinned. The comment says "only `managed_by: cas` is injected", but CAS rewrote the description, Procedure, and examples (5c6676d9a, f0682a43f). There is no upstream commit or fallow version pin (`metadata.version: 1.0.0`), so a future "refresh from upstream" would silently clobber the CAS edits or re-import `\|\| true`-era text. | `git log -- cas-cli/src/builtins/skills/fallow`: vendored bff70af21 2026-04-30, local edits since. Upstream has `source-lock.json`. | Declare the skill CAS-authored (`metadata.upstream-basis: fallow-skills@<sha>, fallow 3.x`) and fix the comment. Add a verification line: "skill written for fallow ≥3.15; if `fallow --version` is older, prefer `npx -y fallow@latest`". | +30 |
| P2 | per-invoke | SKILL.md:19-20, :53-63 | No concrete detection or version step. The local global install (3.15.0) lags npm by 13 minors, so plain `npx fallow` resolves the stale global binary. | `fallow --version` gives 3.15.0; `npm view fallow version` gives 3.28.0. | Step 1: `command -v fallow && fallow --version \|\| echo "use npx -y fallow@latest"`. Delete the Prerequisites install block (cargo path not needed by agents). | −60 |
| P2 | on-demand | references/gotchas.md (611 lines) | No contents list (patterns.md and cli-reference.md have one). The body links it (:170, :385) without saying when to read it. | `grep -n 'Table of Contents'` hits only patterns.md:7 and cli-reference.md:7. | Add a TOC, and a "read when a finding looks like a false positive" pointer in SKILL.md. | +60 on-demand |
| P3 | per-invoke | SKILL.md:86, :247, :258 | ALL-CAPS emphasis on non-safety text ("AND", "BOTH"); bold "Always/Never" on routine rules (:67-73). | grep | Plain wording. Keep "Never run `fallow watch`" (it has a reason). | 0 |
| P3 | per-invoke | SKILL.md:380 | "Syntactic analysis only. No TypeScript compiler" is imprecise. 3.x has opt-in `--type-aware` semantic analysis (also present in 3.15.0). | Root `--help` `--type-aware`. | Change to "Syntactic by default; `--type-aware` opts into TS semantic evidence". | +10 |
| P3 | per-invoke (Codex/Grok twins) | codex/…/SKILL.md:68, :396 | The twins are byte-identical to the Claude copy. The `\|\| true` rationale ("the Bash tool … cancels parallel commands") is Claude Code behaviour, and `$ARGUMENTS` is a Claude/Grok slash-arg token; Codex does not substitute it. | `diff -q` shows all 4 files identical across skills/, codex/, grok/. | Resolved by F1 (rationale removed) and F8 (`$ARGUMENTS` line folded, phrased "if invoked with an argument"). | 0 |

Known cross-lane P0 (a) applies but is not re-reported here: fallow's 3 references are non-SKILL.md files, so they never refresh after first install. Installed copies currently match source because the references are unchanged since vendoring (see manifest).

Net if all per-invoke fixes land: body ≈27 KB → ≈8 KB, **≈ −4,900 tokens per invoke**. On-demand: **≈ −21,400 tokens** (cli-reference replaced by `fallow schema`/`--help`; setup-hooks recipe removed).

#### Twin parity (Axis 7)

`diff -q` of canonical vs `builtins/codex/skills/fallow/*` and `builtins/grok/skills/fallow/*`: all 4 files byte-identical. No tool-prefix substitution is needed (the skill names no CAS MCP tools). Installed copies (`cmp`) are identical to source in `~/.claude`, `~/.claude-daniel@petrastella.io`, `~/.codex`, `~/.grok`, `cas-src/.claude` and `cas-src/.codex`. `cas-src/.grok/skills/fallow` is absent, consistent with known P0 (b): Grok resolves `.claude/skills`. Drift is only the semantic Claude-isms noted in the last P3 row.

#### Overlap

- **Within the skill (main waste):** SKILL.md:173-313 Common Workflows ≈ patterns.md:27-596 and the cli-reference.md "Examples" subsections (:76-127, :164-187, :203-213, :344-375). SKILL.md:376-385 Key Gotchas ≈ gotchas.md:7-40 and further (fix `--yes`, zero-config, changed-since). SKILL.md:328-374 Configuration ≈ cli-reference.md "Configuration File Format"/"Inline Suppression Comments" (TOC :27-28). SKILL.md:387-396 ≈ SKILL.md:17-29. SKILL.md:75-113 ≈ cli-reference.md :32-375 flag tables.
- **Cross-skill:** none. `grep -rln fallow cas-cli/src/builtins/{skills,agents}` outside `skills/fallow/` gives 0 hits (the old cas-code-review fallow persona is gone). Topic-adjacent but not duplicated: cas-codebase-design (architecture/testability, no tool recipe), cas-playwright-debug (JS stack, different concern).

#### Search manifest

| Command | Hits / result |
|---|---|
| `find cas-cli/src/builtins -path '*fallow*' -type f \| xargs wc -c -l` | 12 files (4 × 3 catalogs) |
| `grep -n -i fallow docs/analysis/2026-09-02-builtin-skills-review.md` | 9 lines (:67,:108,:117,:153,:168,:178,:407,:466,:495) |
| `diff -q` canonical vs codex / grok (4 files each) | 0 differences |
| `cmp` source vs 8 installed dirs | 24/24 same; 2 dirs absent |
| `grep -c '\|\| true' SKILL.md` | 31 |
| `grep -n '\|\| true\|2>/dev/null' references/*.md` | 1 (patterns.md:671, CI context) |
| `grep -n 'fallow' cas-cli/src/builtins.rs` | 37 (sync entries :580-600, :1144-1161, :1705-1719; gating :2782-2892) |
| `git log -- cas-cli/src/builtins/skills/fallow` | 4 commits (bff70af21 vendored 2026-04-30 … 5c6676d9a 2026-09-02) |
| `git diff 0489d956 HEAD --stat -- …/skills/fallow` | SKILL.md +46/−32, cli-reference.md ±2 |
| flag sweep: every `--flag` in SKILL.md + 3 references vs 3.28.0 `--help` of 20 subcommands + 18 nested | SKILL.md 0 missing; references 0 fallow-flag misses (`--save-dev`, `--reporter`, `--no-install` are npm/knip; `--vendor` is under `ci-template gitlab`) |
| per-row check of SKILL.md Commands table flags vs that command's `--help` | 0 missing (sanity negative control: 3 misses when checked against the wrong command) |
| `fallow schema` (3.28.0) | plugins.count 127, mcp_tools 38, commands 42, issue_types 117, exit_codes 14, output_formats 16 |
| `fallow schema` (3.15.0 local) | plugins 123, mcp_tools 33 |
| `grep -n '"version": "2' references/*.md` | 8 (2.57.0 samples) |
| `grep -n 'setup-hooks' SKILL.md references/*.md` | 6 (patterns.md :697-762) |
| `grep -n -i 'table of contents' references/*.md` | 2 (gotchas.md has none) |
| `grep -rln fallow builtins/skills builtins/agents` (excluding own dir) | 0 |
| live run `npx -y fallow@3.28.0 dead-code --format json --quiet --no-cache` on /tmp copy of hub-web | exit 1, `kind: dead-code`, `schema_version: 9`, 98 issues |
| live `fallow fix --format json` (non-TTY, no `--yes`) | exit 2, `{"error":true,…}` on stdout |
| `nosuchfallowbin … 2>/dev/null \|\| true; echo $?` | rc 0, empty stdout (silent failure) |
| upstream `api.github.com/repos/fallow-rs/fallow-skills` commits/contents | last commit 2026-09-24; skill at `fallow/skills/fallow/`, 7 reference files, 325 KB |
| `npm view fallow version` / `@fallow-cli/fallow-node` / crates.io `fallow-cli` | 3.28.0 / 3.28.0 / 3.28.0 |

### Appendix B — B-release — cas-cut-release · release-notes · mecha-cassy (+ overlap with cas-release-report)

Auditor: sub-reviewer B-release, task cas-9233. Repo HEAD 4836e56f7 (v3.31.0). Read-only. Rubric: `~/.cas/artifacts/cas-63c5/rubric.md` v1.
Paths below are relative to `cas-cli/src/builtins/skills/` unless they start with `docs/`, `scripts/`, `crates/` or `cas-cli/`.

#### Sizes

| File | Lines | Bytes | ≈ tokens |
|---|---|---|---|
| cas-cut-release/SKILL.md | 79 | 5,184 | 1,296 |
| cas-cut-release/references/failure-log.md | 88 | 31,930 | 7,983 |
| release-notes/SKILL.md | 40 | 2,161 | 540 |
| release-notes/references/RUBRIC-template.md | 95 | 4,699 | 1,175 |
| mecha-cassy/SKILL.md | 80 | 12,029 | 3,007 |
| mecha-cassy/references/registration.md | 170 | 7,611 | 1,903 |
| (overlap only) cas-release-report/SKILL.md | 58 | 3,688 | 922 |

Descriptions: cas-cut-release 64 chars, release-notes 137, mecha-cassy 209 (all ≤ 250). None of the reference files has frontmatter. No retired vocabulary, no `/home/<user>`, no e-mail addresses, no `../../../../` links (see manifest). None of the skills has `disallowed-tools`.

#### Scores (1–5; A1 frontmatter · A2 description · A3 disclosure/size · A4 wording · A5 procedure/completion · A6 accuracy · A7 parity · A8 token economy)

| Skill | A1 | A2 | A3 | A4 | A5 | A6 | A7 | A8 | Verdict |
|---|---|---|---|---|---|---|---|---|---|
| cas-cut-release | 4 | 4 | 2 | 3 | 4 | 5 | 5 | 2 | revise S: tier the failure log |
| release-notes | 4 | 3 | 4 | 4 | 3 | 3 | 5 | 4 | revise S: Grok collision, template leak |
| mecha-cassy | 4 | 3 | 3 | 3 | 4 | 2 | 5 | 3 | revise M: stale command, wrong call shape, content-policy leak |

#### Twin parity (A7)

`diff` of canonical against the codex/ and grok/ twins for all six files: cas-cut-release/SKILL.md has 1 hunk (`:15` `mcp__cas__memory` → `mcp__cs__memory` / `cas__memory`). failure-log.md has 0. release-notes SKILL and template have 0. mecha-cassy/SKILL.md has 1 hunk (`:78` `mcp__cas__mcp_execute` prefix). registration.md has 2 hunks (`:71`, `:83` prefix). **All of it is tool-prefix substitution, so there is no drift.** The installed copies in `~/.grok/skills/` and `~/.claude/skills/` are byte-identical to the source for all six files. Known cross-lane P0s (a) and (b) are not re-reported.

---

#### Prior-review status (docs/analysis/2026-09-02-builtin-skills-review.md)

| Prior item | Status | Evidence |
|---|---|---|
| P0 #16 / §release-notes P0: `release-notes/SKILL.md:18-30` cas-src transport policy (`pippenz@gmail.com` profile, `docs/SLACK_POSTING_RUNBOOK.md`, "Default Codex workers") | **FIXED** in the skill | `grep -rn "SLACK_POSTING_RUNBOOK\|pippenz@gmail" cas-cli/src/builtins/skills/{release-notes,mecha-cassy,cas-cut-release}` returns 0. The skill now points to mecha-cassy (`:24-30`). Residual: `docs/SLACK_POSTING_RUNBOOK.md:110-119` still has a "Historical" table whose Decision cell reads **"Canonical route."** for the `pippenz@gmail.com` Claude.ai Slack profile. This is a doc, not a builtin, so it is P3. |
| P1: reply-count hard rule (`:43,75`, template `:21`) vs `docs/RELEASE_SLACK_RUBRIC.md` | **FIXED in release-notes; carry-over moved to mecha-cassy** | `release-notes/SKILL.md:19` "reply-count default". `RUBRIC-template.md:29-31` "Default: one threaded reply … A project may explicitly document a different reply count". The hard rule now lives in `mecha-cassy/SKILL.md:76` "one punch per top-level message with its detail in one reply" and in the 4-write-only steps `:39`. cas-src's own `docs/release-notes/RUBRIC.md:40` still says "**exactly one** threaded reply", while `RELEASE_SLACK_RUBRIC.md:234` requires three for a diary (its `:6-7` says RELEASE_SLACK wins). |
| P2: five content rules restated 4× | **STILL OPEN (carry-over), now 6 copies** | `release-notes/SKILL.md:34-40`, `RUBRIC-template.md:47-56`, `mecha-cassy/SKILL.md:35-37,76`, `docs/RELEASE_SLACK_RUBRIC.md:184-225,257-262`, `docs/release-notes/RUBRIC.md:55-63`, and the CLAUDE.md user-level block. |
| P3: `git log <last-release>..HEAD` unresolvable for staging | **FIXED** (command removed) | `:15-17` now "Read the commits and merged change set since the last release". It is still vague for a staging merge (see R-4). |
| P1: CLAUDE.md directive (`docs_and_skill.rs:22`) ships a Petrastella Slack policy to every project | **STILL OPEN (carry-over)** | `cas-cli/src/cli/init/docs_and_skill.rs:23` is unchanged: "Release notes: when a merge reaches `staging` or `main`, use the `release-notes` skill and follow docs/release-notes/RUBRIC.md." It is pinned by the test at `:314-323`. Combined with `release-notes/SKILL.md:11-14` (create the rubric if it is missing), every downstream project gets a mandatory Slack duty. |
| cas-cut-release, mecha-cassy | not in prior review | Both skills were added after 2026-09-02. |

---

#### cas-cut-release

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P1 | per-invoke → on-demand | cas-cut-release/SKILL.md:13; references/failure-log.md (all) | "Read `references/failure-log.md in full`" costs about 8k tokens per invoke, and the log only grows (88 entries, append-only via `--learn`). 63 of the 88 entries name a gate `check-id` that `release-gate.sh` already enforces mechanically. Only the 25 `manual:*` entries (about 9.1 KB) describe hazards that no row catches. Some entries run to 1,431 chars on a single line. | `grep -c '^- '` → 88; `grep manual: \| wc -c` → 9,099 B; non-manual → 22,831 B; `awk length` max 1,431. The marker is pinned by `cas-cli/src/builtins.rs:8315` (`"references/failure-log.md in full"`). | Change step 1 to: "Read the `manual:*` entries in `references/failure-log.md`. On a gate failure, grep the log for the failing row id." Update the pin at `builtins.rs:8315`. Optionally split into `failure-log.md` (automated rows) and `manual-hazards.md`. | −5,700 per invoke |
| P2 | per-invoke | cas-cut-release/SKILL.md:13-18 | Step 1 tells the agent to "mirror the entry" and to "regenerate [the ledger] again after the final merge as the last prep step". Both are already automatic: `--learn` appends to all three mirrors and regenerates the ledger, and `--cut` runs a `ledger` stage. The manual instructions invite double edits. | `scripts/release-gate.sh:44-63` loops over the canonical, codex and grok logs and runs `gen-builtin-reference-history.sh`. `scripts/release-train.d/ledger.sh:129` and `release-train.sh:131` include `ledger` in the cut stages. | Reword to: "`--learn` writes all three mirrors and the ledger; commit them with the new check. Then store the same text with `mcp__cas__memory action=remember entry_type=learning tags=release`." Keep the pinned substrings `release-gate.sh --learn` and `ledger is the last prep step` (`:52`). | −40 |
| P2 | per-invoke | cas-cut-release/SKILL.md:24-46 | Step 3 is a 23-line run-on (1,655 B) that asks the agent to "confirm" about ten prerequisites which the `preflight` stage checks and blocks on by name: competing release, merge queue, scratch base, `CAS_RELEASE_ENV_FILE`, Zig/toolchain, cut date. It mixes lane rules (fixture `9.99.x`, `runtime_fixture_parent`, snapshot policy) with operator steps. There is no first-read ordering. | `scripts/release-train.d/preflight.sh:36-72` (competing-release, GraphQL mergeQueue), `:78` (release.env), `:110-137` (scratch-space); `prep.sh:72-74` (journey-evaluation). | Split into (a) "Before `--cut`: start a clean detached or `release/` worktree from `origin/main`. If hub-web/dist changed, commit a journey evaluation. `preflight` names any other missing prerequisite." and (b) move the fixture/snapshot/version rules into a short "Lane rules" list, or into the failure log where they already exist (`failure-log.md:51-53`). Keep the pinned markers. | −200 |
| P2 | per-invoke | cas-cut-release/SKILL.md:37-39 | The User-thread forbidden-word list is a third, divergent copy: `agent, worker, supervisor, daemon, factory`. mecha-cassy `:35` and RELEASE_SLACK_RUBRIC `:200-202` omit `daemon`. The enforced list is the union. | `scripts/release-train-announce.py:19-38` `USER_FORBIDDEN` includes `daemon`, `harness`, `lane`, `epic` and 15 more. | Replace with: "`announce` lint (`scripts/release-train-announce.py`) is the authority for User-thread wording; preflight runs the same lint." Delete the word list. | −25 |
| P2 | always | cas-cut-release/SKILL.md:1-5 | Top-level `managed_by: cas` (all three skills). | Rubric Axis 1. `is_managed_by_cas` is a substring check. | Use `metadata: { managed_by: cas }`. This is a cross-skill mechanical change. | 0 |
| P3 | always | cas-cut-release/SKILL.md:3 | The skill ships to every project (`builtins.rs:174-180`, no gate), but every step drives cas-src-only `scripts/release-train.sh` and `release-gate.sh`. The description is correctly scoped ("Cassy runtime release"), so the only cost is a listing line in downstream projects. | `ls scripts/release-train.sh` exists only in cas-src. No `cas-cut-release` gating in `builtins.rs`. | Accept, or gate installation to the CAS source repo, like other cas-src-only surfaces. | −16 always (downstream) |
| P3 | per-invoke | cas-cut-release/SKILL.md:13 | Backticks wrap the prose "in full" (`references/failure-log.md in full`) only to satisfy a substring pin. | `builtins.rs:8315`. | This goes away with the P1 fix. | 0 |
| P3 | per-invoke | cas-cut-release/SKILL.md:65-79 | There is no literal `Done when`. Step 6's checklist works as one. | Rubric Axis 5. | Prefix step 6 with "Done when". | +3 |

Verified OK (A6): `--check-lane`, `--cut [--resume]`, `--status`, `--gate --only <row,row>` (`release-train.sh:22-35,58`). Stage order `preflight…host-update` (`:131`, `ledger.sh:129`). `release-gate.sh --learn "<symptom>" "<cause>" "<check-id>"` (`release-gate.sh:29,68`). `scripts/journey-eval.sh` and `docs/qa/journey-evaluation.md:87-102`. The `prep` stop id `journey-evaluation` (`prep.sh:74`). `cargo update --workspace --offline` (`prep.sh:29`). Mergeability 60×5 s (`release-train.sh:465-466`). `/Users/Shared/cas-release-gate` (`release-portable.sh:137-144`). `run.env started_at` (`preflight.sh:9`). `release.tag-complete.epoch` (`release-train.sh:714`). `release-published.receipt` (`post-publication.sh:94`). `receipts.commit` (`receipts-common.sh:10`). `cas::test_paths::runtime_fixture_parent` (`cas-cli/src/test_paths.rs:140`). `stranded_branch_override` (`crates/cas-mcp/src/types.rs:317`). `refresh_binary_version` (`scripts/release-host-update.py`). `CAS_RELEASE_ENV_FILE` (`release-train.sh:53`).

#### release-notes

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P1 | always | release-notes/SKILL.md:2 (grok twin) | The name collides with Grok's built-in `/release-notes` command ("View release notes for the current version"). In Grok the skill is advertised only as `user:release-notes`. The universal CLAUDE.md directive says "use the `release-notes` skill", so a Grok agent or user typing `/release-notes` gets Grok's own changelog. | `grok inspect --json` → `{"name":"release-notes","collidesWith":"release-notes","invocableAs":"user:release-notes"}`. `~/.grok/docs/user-guide/08-skills.md:172`. Grok 1.0.41. | Rename to `cas-release-notes` in all three catalogs, update the directive (`docs_and_skill.rs:23`) and the pin at `:314-323`, and keep a doctor retired-name mapping (as for `mecha-cassy-post`, `doctor.rs:361`). | +2 always |
| P0 | per-invoke / on-demand | release-notes/references/RUBRIC-template.md:33-45,88-95 vs mecha-cassy/SKILL.md:36-37 | The procedures contradict each other on message format. The template's example and shape use Markdown bold (`**User**`, `` `Live on production` · **User** — … ``) and a free-form reply. mecha-cassy, which release-notes step 5 requires, has a lint that **refuses to post** any body containing `**`. It also requires exactly two lines `*Live on production — User — Cassy vX.Y.Z*` + a punch of 25 words or fewer, and `• *Label* — Was: … Now: …` bullets. A downstream project that copies the template drafts text the transport then rejects. | `RUBRIC-template.md:90-95`; `mecha-cassy/SKILL.md:36-37`; enforced for cas-src by `scripts/release-train-announce.py:58` and the `TOP_LEVEL_LABEL` regex `…— Cassy(?: vX.Y.Z)?\*`. | Pick one owner for format. Recommended: the template owns format and gets a mrkdwn example (single `*bold*`, `•` bullets). mecha-cassy lints only transport-level hazards (`**`, `#` headings, `-` (dash-space) bullets) and defers label and shape to the rubric. | ±0 (rewrite) |
| P1 | on-demand | release-notes/references/RUBRIC-template.md:62-76 | The universal template carries cas-src release-train mechanics: "Save `release-report.receipt` in the release-train run directory with both report paths, both SHA-256 values, both Slack file ids …" and "Carry these post-publication artifacts into the next release-prep commit". Downstream projects have no release train. The rules are also presented as framework rules a project "may add to, never relax" (`:4-5`). | `scripts/release-train.sh:806-900` is the only consumer of `release-report.receipt`. No downstream equivalent exists. | Reduce to 3 lines: "After a published version, run `cas-release-report` and link its HTML/PDF from the Dev/User threads." Move the receipt field list to `docs/RELEASE_SLACK_RUBRIC.md`, which already has it at `:104-128,299-303`. | −170 |
| P1 | always | cas-cli/src/cli/init/docs_and_skill.rs:23 | Carry-over: every `cas init` project gets a mandatory staging/main Slack duty via the MechaCassy hub (release-notes `:24-27` "Use only the MechaCassy hub/bot"), even when it has no hub registration. | Prior review #22. The line is unchanged. | Use the prior proposal: "Release-note duties, if any, are defined by `docs/release-notes/RUBRIC.md`." Make step 1 create a rubric only on request. | −10 always |
| P2 | per-invoke | release-notes/SKILL.md:24-32 | Step 5 restates mecha-cassy transport rules (preflight, dedupe, integrity, parent id, failure handling). Step 6 restates the `## POSTED` block with fewer fields than mecha-cassy `:41-54` (no `message_id`). The two receipt contracts differ. | `mecha-cassy/SKILL.md:41-54` requires `message_id` + permalink. release-notes `:31-32` requires only timestamp, channel and permalink. | Step 5: "Post through [mecha-cassy](../mecha-cassy/SKILL.md) steps 3–6. They own preflight, order, integrity and the `## POSTED` receipt." Delete step 6. | −110 |
| P2 | per-invoke | release-notes/SKILL.md:34-40 | The "Quality bar" duplicates the rubric hard rules (`RUBRIC-template.md:47-56`), which step 1 already made the contract. | Carry-over of prior P2. | Delete. Step 3 already says "Draft the messages from the rubric". | −75 |
| P3 | per-invoke | release-notes/SKILL.md:15-17 | "since the last release" is undefined for a staging merge, and no command is given. | Prior P3, partly fixed. | "Read the merged PR (`gh pr view <n> --json title,body,commits,files`)". | +10 |
| P3 | per-invoke | release-notes/SKILL.md (end) | There is no `Done when`, and no stop condition of its own (it relies on mecha-cassy). | Rubric Axis 5. | "Done when the draft is saved and has a `## POSTED` block from mecha-cassy, or a blocked report with partial receipts." | +20 |

#### mecha-cassy

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P0 | per-invoke / on-demand | mecha-cassy/SKILL.md:65,78; references/registration.md:14,18,90 | Tells the agent to run `cas integrate mecha-cassy`. In 3.31.0 that is a deprecated alias of `cas integrate violet`, "accepted for one release", and it prints a deprecation warning. Next release it is gone. Doctor remediation already prints `Run \`cas integrate violet\``. | `cas integrate mecha-cassy --help` → "Deprecated name of `cas integrate violet`, accepted for one release (GH #963)". `cas-cli/src/cli/integrate/mod.rs:106-116`. `doctor.rs:8283-8296`. Merge 41c4b0593. | Replace with `cas integrate violet` in the five places (and in the three twins). Add one line saying the proxy server/tools are still `mecha-cassy` / `mecha_read` / `mecha_post` until the hub serves `violet_read` / `violet_post` (`crates/cas-mcp-proxy/src/config.rs:39-46`). | +15 |
| P0 | on-demand | mecha-cassy/references/registration.md:71 (+ twins) | Wrong call shape: `mcp__cas__mcp_execute server=mecha-cassy tool=mecha_read args={…}`. `mcp_execute` has no `server`/`tool`/`args` parameters. Its only parameters are `code` (string) and `max_length`. The JSON dispatch goes inside `code`. The tool description mitigates this, but an agent copying the example sends invalid params. | `crates/cas-mcp/src/types/ops_secondary.rs:1256-1272` (`ExecuteRequest { code, max_length }`). `crates/cas-mcp-proxy/src/lib.rs:844-851,2040-2061` (JSON `{"server","tool","args"}` parsed from `code`). | `mcp__cas__mcp_execute code='{"server":"mecha-cassy","tool":"mecha_read","args":{"channel":"<name>","since":"<RFC3339>","max_messages":50}}'` | +8 |
| P1 | per-invoke | mecha-cassy/SKILL.md:9 vs :35-37,76 | Says "this skill owns only transport", then spends about 1.8 KB on content policy: the User-thread forbidden-word list, the read-aloud test, the two-line top-level format hard-coded to **`Cassy vX.Y.Z`**, reply-bullet grammar, install trailer and ~12-bullet grouping. This is cas-src's `RELEASE_SLACK_RUBRIC.md:194-225` copied into a universal builtin, so a downstream project's announcement would be labelled "Cassy vX.Y.Z". | `RELEASE_SLACK_RUBRIC.md:196-222` is near-verbatim. `release-train-announce.py:19-45` regex pins "Cassy". | Keep only the mrkdwn transport lint (no `**`, no `#` lines, no `-` (dash-space) bullets, bullets ≤ 2 lines). Move wording and label rules to the project rubric. Step 2 becomes: "Draft per the project rubric and save the exact fenced bodies to `docs/release-notes/<date>-<topic>-slack.md`." | −330 |
| P1 | always / per-invoke | mecha-cassy/SKILL.md:3 vs :39,76 | The description promises "a diary update", but the procedure only covers the runtime 4-write order. `:76` hard-codes "its detail in one reply". The cas-src diary contract is 1 parent + exactly 3 replies ordered Grok → Claude → Codex. An agent posting a diary via this skill follows the wrong order and count. | `docs/RELEASE_SLACK_RUBRIC.md:227-247`. `docs/SLACK_POSTING_RUNBOOK.md:50-51` knows about the diary; the skill does not. | Generalise step 4: "Post each thread in the rubric's order: parent → save `message_id` → each reply with `reply_to=<parent>`; ≥ 1 s between writes." Delete `:76`, or change it to "Content rules come from the rubric." Drop "diary update" from the description, or keep it and have the rubric supply the order. | −40 |
| P2 | per-invoke | mecha-cassy/SKILL.md:18,30,40 | cas-src release-adapter detail lives in the per-invoke body. `:18` names `scripts/release-report-post.py`, a cas-src-only script. `:40` is a 1,609-byte single-line step 5 covering download origin rules, bearer/bypass forwarding, loopback exception, PIL verify and PDF page count. `:30` repeats the PDF-to-User / HTML-to-Dev rule, which appears again in RUBRIC-template `:67-68` and RELEASE_SLACK_RUBRIC `:106-108`. | `wc -c` of line 40 is 1,609 and of line 18 is 958. `scripts/release-report-post.py:34,453` exists only in cas-src. | Move file-upload verification to `references/file-upload.md` (on-demand, read only when posting a file). Keep a 2-line pointer. Drop the release-report specifics from `:18,30`. | −600 per invoke |
| P2 | per-invoke | mecha-cassy/SKILL.md:38 | Step 3 (1,366 B) folds the read-outage fallback, a write-safe exception and envelope redaction into one line. The same fallback is restated at `:64`. | `:38`, `:64`. | Split into 3a preflight, 3b bounded read (`since`, 3×10 s), 3c outage fallback. Delete the `:64` restatement and keep a pointer. | −60 |
| P2 | on-demand | mecha-cassy/references/registration.md:20,44,143 | Points to cas-src/hub-only artifacts that are not shipped: `docs/MECHA_CASSY_ONBOARDING.md` (only in cas-src), `mecha-cassy#5` (hub issue), and the hub project's `scripts/slack-post.sh`, which cannot be verified from this repo. | `ls docs/MECHA_CASSY_ONBOARDING.md` exists only in cas-src. `integrate/mecha_cassy.rs:55` (`HUB_CLIENT_ISSUE`). | Replace with the command-level facts (`cas integrate violet --help`). Name the proxy-less script by its hub repo URL, or drop it. | −30 |
| P2 | per-invoke | mecha-cassy/SKILL.md:34 | The channel rule `^[a-z0-9-]+-internal$` is enforced (hub/`slack-post.sh` "same channel rule", `registration.md:144`), but `RUBRIC-template.md:9` offers a free `<#channel-name>` and never mentions the constraint. A downstream rubric naming `#releases` fails only at post time. | `RUBRIC-template.md:9`. | Add "must match `*-internal` or be allowlisted by the hub" to the template's Channel line. | +12 |
| P3 | per-invoke | mecha-cassy/SKILL.md:40 | Step 5 is indented one space (a leading space before `5.`), which breaks the ordered list in some renderers. | `cat -n` line 40. | Remove the leading space. | 0 |
| P3 | per-invoke | mecha-cassy/SKILL.md:80 | A maintenance note ("If a machine still carries a separate user-level `mecha-cassy-post` skill … delete it") is already enforced by doctor. | `doctor.rs:361` `RETIRED_USER_SKILLS`. | Delete. | −45 |
| P3 | (source comment) | cas-cli/src/builtins.rs:529-530 | The comment still says the hub "exposes four tools". | `crates/cas-mcp-proxy/src/config.rs:39` has 2 tools. | Fix the comment. | 0 |

Verified OK (A6): the `mecha_read` input schema matches `SKILL.md:15` exactly (live schema: `channel` required; `since` date-time; `max_messages` ≤ 500 default 200; `max_files` ≤ 50; `max_file_bytes` ≤ 4,194,304; `max_bytes` ≤ 8,388,608; `mentions_only`, `include_threads`, `include_files`). `MECHA_CASSY_TOOLS = ["mecha_read","mecha_post"]` (`config.rs:39`). The `supervisor:` allowlist prefix (`config.rs:135`). `callable tools: [...]` log line (`cas-mcp-proxy/src/lib.rs:975`). `mcp__cas__system action=proxy_health` (`types/ops_secondary.rs:252`). `--label`, `--token-env`, `--bypass-env`, `/api/clients`, `/api/bypass`, `label_taken` (`integrate/mecha_cassy.rs:53-56,383`). 1 MiB file limit and `file_too_large` (`scripts/release-report-post.py:40,453`). Observation only: this Claude session's direct `mecha-cassy` registration exposes only `mcp__mecha-cassy__mecha_read` (no `mecha_post`), consistent with posting via the proxy.

---

#### Overlap map: who owns what

| Concern | release-notes | mecha-cassy | cas-cut-release | cas-release-report | docs/RELEASE_SLACK_RUBRIC.md (cas-src) | Should own |
|---|---|---|---|---|---|---|
| Content rules (Was→Now, no tickets, no process talk, one punch) | SKILL :34-40; template :47-60 | :76; :35 (User wording) | :37-39 (word list) | :15 (Was→Now in report) | :184-203, :257-262 | project rubric (template) |
| Message format / mrkdwn / label | template :33-45, :88-95 (Markdown `**`) | :36-37 (mrkdwn, `Cassy vX.Y.Z`, lint) | — (announce lint runs it) | — | :205-225 | rubric. The lint lives in the transport, shape-agnostic |
| Thread order / reply count | SKILL :18-19 (defers to rubric); template :29-31, :45 | :39 (4-write runtime only), :76 (one reply) | :55-56, :72 ("four Slack POSTED") | — | :31-36 runtime; :227-247 diary 1+3 | rubric states order and count; mecha-cassy executes a generic parent→replies loop |
| Transport (hub only, preflight, dedupe, pacing, failures, credentials) | SKILL :24-30 (restated) | :9-18, :32-72 (owner) | :72-73, :78-79 | :57-58 ("release-notes owns … posting authorization": wrong, release-notes delegates it) | :15-29 + SLACK_POSTING_RUNBOOK :13-51 (second authority) | mecha-cassy alone |
| `## POSTED` receipt | SKILL :31-32 (ts, channel, permalink) | :41-54 (+ message_id) | :55-58 (announce stage writes it) | — | :264-269 (ts, channel, permalink) | mecha-cassy (single field list) |
| Report PDF/HTML delivery + `release-report.receipt` | template :62-76 | :18, :30, :40 | :74 | :50-58 (artifacts only) | :93-130, :296-303 | cas-release-report makes the artifacts; RELEASE_SLACK_RUBRIC owns the cas-src receipt; mecha-cassy owns upload integrity only |
| Draft path `docs/release-notes/<date>-<topic>-slack.md` | SKILL :22-23; template :78-81 | :36 | :35-36 (cut-date pinning) | :53 | :73-81 (runtime template) | rubric |
| Release mechanics (PR, queue, tag, publish) | — | — | :19-79 (owner, "only release procedure") | — | :38-64, :66-91, :132-177 (manual procedure) | cas-cut-release; RELEASE_SLACK_RUBRIC should point to it, not restate it |

##### Contradictions with docs/RELEASE_SLACK_RUBRIC.md

1. **Merge path:** RELEASE_SLACK_RUBRIC `:46-51` says "`gh pr merge "$PR_URL" --merge`. Do not use `--auto`". cas-cut-release `:62-64`, and `release-train.sh:531-535`, enqueue through the merge queue with a GraphQL `enqueuePullRequest` and call themselves "the supervisor's only release procedure" (`:9`). An agent reading the rubric merges by hand, and the merge queue is bypassed or refused. **P1** (doc).
2. **Report timing:** RELEASE_SLACK_RUBRIC `:95-110` says to run `cas release report --pdf` *before* announcements, then export `CAS_RELEASE_TRAIN_REPORT_{USER,DEV}_THREAD_TS` by hand and run `--report`. The `--cut` stage order is `announce` → `report`, and the report stage reads the parent id from the announce receipt automatically (`release-train.sh:996-1014`). It is the same end state but describes two procedures. **P2**.
3. **Draft fill:** RELEASE_SLACK_RUBRIC `:73-81` has you `cp` the runtime template and run `release-published-receipt.sh --write-draft` by hand. The `post-publication` stage does this (`release-train.d/post-publication.sh:67-94`). **P2**.
4. **Transport authority:** RELEASE_SLACK_RUBRIC `:6-9` sends transport to `SLACK_POSTING_RUNBOOK.md`, while `:17-23` says mecha-cassy owns it. `docs/release-notes/RUBRIC.md:16-17,21-22` cites both. The runbook `:117` still labels the Claude.ai Slack `pippenz@gmail.com` profile "Canonical route" in a table marked historical. **P3**.
5. **Diary vs transport:** the diary thread is 1 parent + 3 replies (Grok, Claude, Codex) (`:227-247`). mecha-cassy (`:3` claims diary coverage; `:39`, `:76` one reply) and cas-src `docs/release-notes/RUBRIC.md:40` ("exactly one") disagree. **P1** (mecha-cassy row above).
6. **Forbidden User words:** there are three lists. RELEASE_SLACK_RUBRIC `:200-202` (+`registrations`, −`daemon`), mecha-cassy `:35` (−`daemon`), and cas-cut-release `:38-39` (5 words, +`daemon`). The enforced list in `release-train-announce.py:19-38` has `daemon`. **P2**.
7. **"Two distinct top-level posts (not threaded replies)"** (`:33`) reads as if replies are forbidden. `:101` and every skill post 4 writes (2 parents + 2 replies). **P3**: reword to "two top-level posts, each with its reply".
8. **POSTED fields:** RELEASE_SLACK_RUBRIC `:266-269` and release-notes `:31-32` omit `message_id`, which mecha-cassy `:41-54` requires. **P3**.

##### Recommended single-owner split (≈ −1,500 tokens per announce across the three skills)

- **release-notes:** procedure only. Ensure the rubric exists, gather the merge, draft per the rubric, save, then hand off to mecha-cassy. The template owns format (mrkdwn example), order and reply count, and has no release-train receipts.
- **mecha-cassy:** transport only. Channel rule, preflight/read, a generic parent→replies loop with pacing, a format-agnostic mrkdwn lint, `## POSTED` (the only field list), failure classes and credentials. Upload integrity moves to `references/file-upload.md`.
- **cas-cut-release:** train only. Announce, report and receipts are stages, and it points to RELEASE_SLACK_RUBRIC for content.
- **cas-release-report:** artifacts only. Fix `:57-58` to "mecha-cassy owns posting; the project rubric owns wording".
- **RELEASE_SLACK_RUBRIC.md:** cas-src content (runtime + diary) and the receipt field list. Replace `:38-64`, `:73-81` and `:95-130` with pointers to cas-cut-release stages.

---

#### Search manifest

| Command | Hits |
|---|---|
| `grep -n -i -E "release-notes\|cut-release\|mecha\|release-report" docs/analysis/2026-09-02-builtin-skills-review.md` | 8 relevant (release-notes only; 0 for cas-cut-release/mecha-cassy) |
| `grep -rn "SLACK_POSTING_RUNBOOK\|pippenz@gmail" cas-cli/src/builtins/` | 0 in the B skills (hits only in cas-html-reports example HTML) |
| `grep -n -E "pending_supervisor_review\|bypass_code_review\|/epic-spec\|/plan\b\|cas-code-review\|Phase [12]\|verified on this machine\|/home/\|@gmail\|\.\./\.\./\.\./" -r cas-cut-release release-notes mecha-cassy` | 0 |
| `grep -n -E "\b(NEVER\|MUST\|ALWAYS\|CRITICAL\|IMPORTANT\|ONLY)\b" -r cas-cut-release release-notes mecha-cassy` | 0 |
| `head -c4 <3 reference files> \| grep -c -- ---` (frontmatter in references) | 0 |
| `diff` canonical vs codex/grok twins (6 files × 2) | 10 changed lines, all prefix substitution |
| `diff -q` installed `~/.grok/skills`, `~/.claude/skills` vs source (6 files) | 0 differences |
| `grok inspect --json` (B skills) | 1 collision: `release-notes` → `user:release-notes` |
| `strings ~/.grok/bin/grok \| grep release-notes` | built-in `/release-notes` "View release notes for the current version" |
| `cas integrate mecha-cassy --help` | "Deprecated name of `cas integrate violet`, accepted for one release (GH #963)" |
| `cas release --help` / `cas release report --help` | `report <VERSION> [--out] [--pdf] [--refresh-sources]`. The only subcommand is `report`. No B skill uses it (`grep -rn "cas release report" skills/` → 0) |
| `grep -n "struct ExecuteRequest" -A20 crates/cas-mcp/src/types/ops_secondary.rs` | fields `code`, `max_length` only |
| `grep -n "MECHA_CASSY_TOOLS\|VIOLET_TOOLS" crates/cas-mcp-proxy/src/config.rs` | 2 (`:39`, `:46`) |
| `sed -n 21,58p scripts/release-train.sh` (usage) | all skill flags present |
| `grep -n "cut_stages\|for stage in" scripts/release-train.d/ledger.sh` | 1 (`:129`, 13 stages match the skill) |
| `grep -n -- "--learn" scripts/release-gate.sh` | 4 (`:29,36,63,68`); writes 3 mirrors (`:44-58`) |
| `grep -c '^- ' failure-log.md` / `grep -c manual:` | 88 / 25 |
| `grep -n "USER_FORBIDDEN" -A20 scripts/release-train-announce.py` | 19 words incl. `daemon` |
| `grep -n "enqueuePullRequest" scripts/release-train.sh` | 1 (`:533`) |
| `grep -n "write-draft" scripts/release-train.d/post-publication.sh` | 1 (`:86`) |
| `grep -n "cas-cut-release" cas-cli/src/builtins.rs` (install gating) | registrations + test only, no cas-src gate |
| `sed -n 1,23p cas-cli/src/cli/init/docs_and_skill.rs` | directive `:23` unchanged |
| `ToolSearch select:mcp__mecha-cassy__mecha_read` (schema only, not called) | schema matches SKILL.md:15 |

### Appendix C — C-qa — cas-qa-craft · cas-nuxt-playwright · cas-playwright-debug

HEAD 4836e56f7 (v3.31.0). Read-only audit. Playwright claims checked against `playwright@1.63.0` / `@playwright/test@1.63.0`: `--help` output, the shipped `.d.ts` files, and **live runs** in `/tmp/pwq` (a failing trace, every `trace` subcommand, a `--debug=cli` session, and an evidence-bundle run). Release notes came from playwright.dev/docs/release-notes (fresh crawl) and the GitHub v1.59.0 release. Codex/Grok twins: `diff -r` is empty for all three skills (see Parity).

#### Scores (1–5; axes: 1 frontmatter · 2 description · 3 disclosure/size · 4 wording · 5 procedure/completion · 6 accuracy · 7 parity · 8 token economy)

| skill | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 |
|---|---|---|---|---|---|---|---|---|
| cas-qa-craft | 4 | 2 | 3 | 4 | 3 | 2 | 5 | 3 |
| cas-nuxt-playwright | 3 | 4 | 2 | 3 | 2 | 3 | 3 | 2 |
| cas-playwright-debug | 4 | 5 | 5 | 5 | 4 | 4 | 5 | 4 |

---

#### cas-qa-craft

Size: SKILL.md 110 lines / 6,321 B (≈1.6k tok per-invoke). refs: evidence-bundle 197/9,415 · independent-pass 164/6,921 · journeys 73/2,940 · telemetry-sweep 76/3,015 · exemplar 59/3,150 · evidence-ledger 42/1,859 · matrix-builder 25/1,581. Description: 132 chars.
Prior review (2026-09-02): the skill is not mentioned there (it is new or was restructured since). No carry-overs.

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P0 | always | SKILL.md:3, :24-25 | The trigger is scoped to a "non-empty demo_statement", and step 1 says to skip the skill when it is empty. The close gate still demands the **full web bundle** when a diff touches a `qa.user_facing_paths` glob (default `**/*.vue,*.tsx,*.html,*.css…`) or a catalog journey, even when there is no demo_statement. An agent following the skill skips it, then gets its close refused. | `qa_pass.rs:100-108` (reasons `journeys:` / `path:` / `demo_statement`); `qa_evidence_gate.rs:81-93` (`path:`/`journeys:` → `EvidenceTier::Bundle`); `config/meta/seed/qa.rs:90-105` | Description: "Use when a factory delivery needs QA evidence before close: a non-empty demo_statement, a diff touching user-facing paths or catalog journeys, or a `qa-pass` review task." Replace :24-25 with "If the gate names no reason (see `task action=show`), stop." | +25 always |
| P0 | on-demand | references/evidence-bundle.md:157-160 (with SKILL.md:50-51) | The fallback "no `scripts/visual-qa.mjs` → set `visual_qa_status: "unavailable"`" always fails the implementer's close. The gate accepts only `"pass"` (anything else needs a supervisor override) and needs `visual-qa.mjs`'s own JSON report. The script is not shipped by any builtin skill, so every downstream web project hits this. gabber-studio and gabber-qa-epic carry hand-copied scripts; other projects have none. | `qa_evidence.rs:475-483` ("only \"pass\" closes without a supervisor override"), `:512-526`, `:616-660`; `find cas-cli/src/builtins -name 'visual-qa*'` → 0; `ls ~/Petrastella/*/scripts/visual-qa.mjs` → only cas-src, gabber-studio, gabber-qa-epic | Say it plainly: without the script, close is refused until the supervisor overrides it, so message the supervisor with the four renders attached. Longer term, ship `visual-qa.mjs` as `cas-qa-craft/scripts/visual-qa.mjs` (it is subject to cross-lane P0 (a): bundled files never refresh) and point `<repo>/scripts/…` at the installed skill path. | +40 on-demand |
| P1 | per-invoke | SKILL.md:86-104 vs :20-31 | The telemetry sweep says "Before building the matrix … run it as the first QA step", but it sits after Procedure and Close gate, and the numbered steps never include it. An agent working in step order builds the matrix first. User journeys (:106-110) likewise sit outside the procedure, although journeys.md:25 makes them matrix rows. | Line order in SKILL.md | Fold both into the Procedure: step 1b "Run `cas config get qa.telemetry_sweep`; if set, run it per references/telemetry-sweep.md", and step 2 "add touched journeys (references/journeys.md) as rows". Delete the two trailing sections: :96-104 restate telemetry-sweep.md:10-30, and :108-110 restate journeys.md. | −230 per-invoke |
| P2 | per-invoke | SKILL.md:62-75 | The close-gate paragraph repeats cas-worker/references/close-gate.md:68-74 almost word for word, and its bullets repeat evidence-bundle.md:35-49. It also omits two refusals the validator makes: any **failing** Expect in the trace, and every listed file older than the delivered commit. | `qa_evidence.rs:446-457` (failed>0 refused), `:419-431` (per-file mtime); close-gate.md:68-74 | Keep one sentence here ("close validates `qa/bundle.json` against the contract in evidence-bundle.md; rejections print the producing command"). Move the full list into evidence-bundle.md, which the code already names as the contract (`qa_evidence.rs:23` CONTRACT_REFERENCE), and add "0 failing Expect" and "every file newer than the commit" there. | −170 per-invoke |
| P2 | per-invoke | SKILL.md:41-53 | The bundle contents list repeats the evidence-bundle.md:17-27 table. independent-pass.md:103-115 and journeys.md:43-50 repeat it again. | Side-by-side read | Replace with "write the bundle per references/evidence-bundle.md". Have independent-pass list only its differences: producer, F0N names, `journeys/` subfolders. | −130 per-invoke, −120 on-demand |
| P2 | per-invoke / on-demand | SKILL.md:67-68; evidence-bundle.md:164; SKILL.md:22 | The `task action=notes …` example omits `id=`, which the handler requires. The gate's own refusal text includes it. | `service/core.rs:602` (`missing_id("task","notes")`); `qa_evidence.rs:152` (`task action=notes id={task} …`) | Write `task action=notes id=<task-id> note_type=platform_proof notes="qa-bundle: …"` and `task action=show id=<task-id>`. | +6 |
| P2 | on-demand | evidence-bundle.md:147-149 | Line 141 says to use `npx --prefix <project> playwright` outside the project, but the example runs bare `npx playwright trace …` right after `cd "$QA"` (which is outside the project). In a non-TTY shell, npx then silently fetches the latest `playwright` instead of the pinned version. | Lines 139-149; npm exec treats non-TTY as `--yes` | Use `npx --prefix <project> playwright trace …` on lines 147-149 and 171-177. | +10 |
| P2 | on-demand | references/exemplar.md:25-56 | A nested fence: a ```` ```markdown ```` block contains ```` ```text ```` blocks. The ```` ``` ```` at :32 closes the outer fence, and :56 opens an unclosed fence that runs to EOF, so the file renders broken. CLAUDE.md names this exact shape as the Ink-crash tripwire. | Lines 25, 30, 32, 37, 39, 56 | Drop the outer ```` ```markdown ```` wrapper and write the sections as real headings. | 0 |
| P2 | on-demand | exemplar.md:10-16 vs evidence-bundle.md:23, :162 | The exemplar's evidence paths are `step-01.png`, but the contract names captures `M01.png` and cites them as `qa/M01.png`. Models copy details from examples. | Lines cited | Use `qa/M01.png`… in the exemplar rows. | 0 |
| P2 | on-demand | exemplar.md:6 | Operator-specific absolute path `/home/pippenz/.cas/artifacts/cas-1234/LEDGER.md`. | Line 6 | `~/.cas/artifacts/cas-1234/LEDGER.md` | −2 |
| P2 | on-demand | journeys.md:6, :35-39; independent-pass.md:18-19, :29-34 | cas-src/hub-web-only facts (hub-web ports, `npm run build` + `hub-web/dist`) are shipped to every project, and the ports paragraph appears twice. | `hub-web/playwright.config.ts:13-20,63,68` | Keep one copy under "Cassy (cas-src) only:" in journeys.md and have independent-pass point to it. | −110 on-demand |
| P2 | on-demand | evidence-ledger.md:37-42 vs SKILL.md:54-60 | The constants grep and defect rule are restated almost word for word. | Side-by-side read | Keep them in SKILL step 5 and cut the ledger-reference restatement down to the table headers. | −95 |
| P2 | on-demand | matrix-builder.md:9-25 vs SKILL.md:26-31 | The matrix quotas (≥3 unmentioned conditions, ≥1 adjacent surface, cap 8, no replays) appear in both files. epic-flow-walk.md:21-27 and independent-pass.md:51-61 repeat them again. | Side-by-side read | Put the quotas only in matrix-builder.md and have SKILL step 2 say "per matrix-builder.md (8-cell cap)". | −70 per-invoke |
| P2 | on-demand | evidence-bundle.md:4-7, :184 | Dated verification narration ("checked … on 2026-09-23"; "Measured gotchas (1.63.0)"). The gotchas themselves are correct: I reproduced all four (chapter blocked 1,306 ms for a 1,000 ms duration; `start()` without size recorded 800×450; `trace screenshot` said "No screenshot found" with `screenshots:false`; emulateMedia kept the omitted `reducedMotion`). | Live run `/tmp/pwq/ev` | Replace the narration with "Requires `@playwright/test` ≥1.63" and rename the heading to "Gotchas". | −25 |
| P3 | on-demand | evidence-bundle.md:172 | "`trace actions --errors-only` must print no rows" is ambiguous: it always prints the two header lines. | Live run: header printed, rc=0 | "…prints only the header". | +3 |
| P3 | on-demand | evidence-bundle.md (197 l), independent-pass.md (164 l) | Both references are over 100 lines and have no contents list (rubric Axis 3). | wc -l | Add a 3-line contents list to each. | +40 |
| P3 | per-invoke | SKILL.md:65 | Internal ticket label `cas-0cd5` in the body. | grep | Drop it. | −3 |
| P3 | per-invoke | SKILL.md:32, :42 vs :66 | The file hardcodes `~/.cas/artifacts` in two places and uses `<artifacts>` in another. The real root is `[factory] artifacts_root` (default `~/.cas/artifacts`). | `config/settings.rs:786-798` | Use `<artifacts>` throughout and define it once. | +10 |
| P3 | per-invoke | SKILL.md (no end criterion) | There is no `Done when …` line. Step 6 and Close gate imply one. | Rubric Axis 5 | Add "Done when: the ledger and bundle are cited in a platform_proof note and close accepts them, or the NOT EXERCISED cells are listed in Honesty and defect tasks are filed." | +35 |
| P3 | on-demand | matrix-builder.md:5-7; telemetry-sweep.md:32-63 | Provenance narration (GH #759). The PostHog example labels every row `RISING`, and `.results[]` rows are arrays, so `.event` is wrong. Models copy examples. | Lines cited | Delete the provenance line. Either fix the jq or cut the example down to the output contract. | −300 on-demand |
| P2 | always | SKILL.md:4 | Top-level `managed_by: cas` (rubric Axis 1). | frontmatter | `metadata: { managed_by: cas }` | 0 |

#### cas-nuxt-playwright

Size: SKILL.md 343 lines / 19,538 B (≈4.9k tok per-invoke; it was 228 lines at the prior review, so it grew 50%). refs: auth-fixture-template 302/8,822. Description: 177 chars. `disable-model-invocation: true`.

Prior-review status (docs/analysis/2026-09-02-builtin-skills-review.md:68, :126, :408, :495):

- Shouted opt-in → `disable-model-invocation`: **FIXED** (SKILL.md:5; pinned by `builtins.rs:7533-7556`). Codex does not honour it; see P1 below.
- Firebase+Quasar specificity vs a "Nuxt + Playwright" description: **FIXED** (SKILL.md:3 now names Firebase auth and Quasar UI).
- Redundant `user-invocable: true`: **FIXED** (absent; the test asserts it stays absent).
- No completion criterion: **STILL OPEN** (carry-over), see the table.
- Should point at cas-servers for webServer: **STILL OPEN** (carry-over); `grep -c cas-servers` → 0 in both files.
- Stack-specific skill synced universally (opt-in tier, review #16): **STILL OPEN** (carry-over). `grep -rn nuxt cas-cli/src/sync` → 0. The Claude listing cost is now nil; sync bytes and the Codex listing remain.

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P1 | per-invoke | SKILL.md:276 | The debugging table says "`npx playwright cli attach` and `step-over` / `resume`" without the session flag. Every command after attach needs `-s=<session>`. | Live run: bare `npx playwright cli step-over` → "The browser 'default' is not open, please run open first"; `-s=tw-1c6c89 step-over` worked | Delete the row and point to cas-playwright-debug §2, which already has the correct `-s=` form. | −40 |
| P1 | per-invoke | SKILL.md:211, :297 | The claim "`<q-btn>` nested `<span>` breaks `getByRole('button')`" is wrong, and the recommended fix (`getByText`) contradicts :185 and cas-playwright-debug:87. The real failure is a q-btn with `to`/`href`, which renders `<a>` with role **link**. | Live run with Quasar-2-shaped markup (approximated): `getByRole('button',{name:'Save'})` → 1; for the `<a class="q-btn">` variant, button → 0 and link → 1 | Row: "`<q-btn>` with `to`/`href` renders a link → `getByRole('link', { name })`; otherwise `getByRole('button', { name })`." Fix the :297 symptom row the same way. | 0 |
| P1 | always (Codex) | SKILL.md:5 (Codex twin identical) | The opt-in uses `disable-model-invocation`, which only Claude Code and Grok read. Codex controls implicit invocation through `agents/openai.yaml` `policy.allow_implicit_invocation: false`, which CAS never generates. In Codex the skill therefore stays model-invocable, and its description competes with cas-playwright-debug. | developers.openai.com/codex/skills; `grep -rn "openai.yaml\|allow_implicit_invocation" cas-cli/src --include=*.rs` → 0 | Generate `agents/openai.yaml` with that policy for Codex twins of every `disable-model-invocation` skill. This is cross-cutting, so raise it once in the parity lane. | 0 |
| P1 | routing (cross-skill) | cas-frontend-engineering/SKILL.md:11, :100-101 | Two model-invocable skills route the model to cas-nuxt-playwright, which the model cannot invoke. They also claim it owns "locks, and retries", but those are generic and live in cas-playwright-debug §4 (:93-123). | `disable-model-invocation: true` at SKILL.md:5 | In frontend-engineering, say "`cas-playwright-debug` owns harness debugging, locks, and retries. For Nuxt+Firebase+Quasar, ask the user to run `/cas-nuxt-playwright`." | +10 |
| P2 | per-invoke | SKILL.md:259-266, :268-279, :309-322, :324-332, :281-288, :290-307 | About 30% of the body repeats sibling skills or other parts of this body. Flake control repeats debug §4. Debugging failures repeats debug §1-2. Retired idioms overlap the debug §3 table. The anti-patterns list restates :26, :103-105, :164-172, :158-160, :71-76, and :332. Component tests repeat cas-frontend-engineering:83. The diagnostic table restates rules given above it. | Byte counts: 1,053 + 881 + 858 + 652 + 832 + ~1,000 of 1,883 | Keep only the Nuxt/Firebase/Quasar-specific parts: SSR decision tree, auth patterns, Firebase facts, navigateTo, hydration, Quasar table, Firebase mock rules, and the diagnostic rows that are stack-specific. Replace the rest with one line: "Generic triage, flake control and idioms: cas-playwright-debug; acceptance assertions and `mount`: cas-frontend-engineering." | ≈ −1,300 per-invoke |
| P2 | per-invoke / on-demand | SKILL.md:130-160, :219-233 vs auth-fixture-template.md:86-114 | The `navigateTo()` and `waitForHydration` code appears verbatim in both the body and the template. | diff by eye | Keep the code in the template. In the body, keep the "why" and "when you do not need it" lines plus the pointer. | −330 per-invoke |
| P2 | per-invoke | SKILL.md:217 vs :339 | One line tells the agent to use `npx playwright test --debug`, which is the Inspector: a GUI that also forces `--headed`. The other says `--debug=cli` is the agent path. | `test --help`: `--debug [mode] … choices: "inspector","cli", preset: "inspector"` | Change :217 to `--debug=cli` (cas-playwright-debug §2). | 0 |
| P2 | per-invoke | SKILL.md (no end criterion) | Carry-over: no `Done when`. | grep "Done when" → 0 | Add "Done when: the spec passes `--repeat-each=3` on the target env, the diff has no `networkidle`, sleep, or raised timeout, and shared-account tests carry a `lock`." | +40 |
| P2 | on-demand | auth-fixture-template.md:234-242 | Carry-over: the template has `webServer` run `pnpm dev` with `reuseExistingServer: true`. That silently reuses whatever holds :3000, possibly another checkout's server; hub-web deliberately forbids reuse (`hub-web/playwright.config.ts:63,68`). A dev server is also not the "real build" that cas-qa-craft requires. There is no cas-servers pointer. | Lines cited; cas-servers/SKILL.md:3, :23 | Use `reuseExistingServer: false` and a per-checkout port. Add: "For a server that must outlive the run, start it with cas-servers `coordination action=server_start` and drop `webServer`." | +40 on-demand |
| P2 | per-invoke | SKILL.md:83, :92, :103-104, :128, :158, :326-332 | Shouting: BEFORE, ALL, EVERY, ONLY, DO NOT, and seven bold **Never** bullets on non-safety rules (rubric Axis 4). | grep | Use plain imperatives plus the reason. The anti-pattern list disappears with the dedup above. | −40 |
| P2 | per-invoke | SKILL.md:53-67 vs :125-127 | Internal tension: the body says Firebase keeps tokens in IndexedDB and that `storageState({ indexedDB: true })` captures them, yet Pattern B hand-seeds `firebase:authUser:*` into **localStorage**. That only works when the app sets `browserLocalPersistence`. | `types.d.ts:10700` (`indexedDB?: boolean` on storageState) | Pattern B: "Prefer UI login + `storageState({ path, indexedDB: true })`. Hand-seed localStorage only when the app uses `browserLocalPersistence`." | +20 |
| P3 | per-invoke / on-demand | SKILL.md:10; auth-fixture-template.md:3 | Narration ("Grounded in real failures across production projects") and an operator-specific reference ("Modeled after the gabber-studio production test suite"). | grep | Delete both. | −30 |
| P2 | always | SKILL.md:4 | Top-level `managed_by: cas`. | frontmatter | Move it to `metadata`. | 0 |

Third-party checks that passed (1.63.0 types, live runs, and release notes):

- 1.60: `getByRole` `description`, `expect(page).toMatchAriaSnapshot`, `locator.drop({files})`.
- 1.61: `page.localStorage` (WebStorage), `context.credentials` install/create/get, video `retain-on-failure-and-retries`.
- 1.62: `retryStrategy: 'isolated'`, `.webp` `toHaveScreenshot`, stories/gallery `mount` + `reuseContext`.
- 1.63: `lock`, `locator.visible()`, `frameLocator()` with no selector, `test.step` `subtitle`/`params`, trace `snapshots:{dom,aria,screen}`.
- Also: `page.clock.install/fastForward/setFixedTime`, `failOnFlakyTests`, and `storageState({indexedDB})` all exist.
- The experimental-ct deprecation is a 1.63 announcement; the skill's wording is consistent with it.

#### cas-playwright-debug

Size: SKILL.md 142 lines / 6,715 B (≈1.7k tok per-invoke). No references. Description: 176 chars. Prior review: this skill had not been built yet (review line 472 says the Playwright debugging content lived in cas-nuxt-playwright). It is new, so there are no carry-overs.

Every trace subcommand and flag in §1 was verified live: `open`, `actions --errors-only/--grep`, `action <id>`, `snapshot <id> --phase before|after`, `snapshot <id> -- eval "…"` (returned `"Saved!"`), `requests --failed`, `console --errors-only`, `errors`, and `close`. `error-context.md` is written beside the trace. The `--debug=cli` log format matches `Run "playwright-cli attach tw-XXXXXX"` exactly. `pause-at`, `snapshot`, `eval`, `console`, and `resume` work with `-s=`. Stale refs error out, as the skill says ("Ref e6 not found … capture new snapshot"). The `--repeat-each`, `--workers`, and `--last-failed` flags and the 1.63 config keys exist.

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P1 | per-invoke | SKILL.md:13-15, :61-67 | The skill says "the trace CLI and `--debug=cli` need 1.59+", but its attach commands use `npx playwright cli …`, which only ships from **1.62**. On 1.59–1.61 the attach step fails, because the separate `playwright-cli` was needed. | Release notes 1.62 "Command line & MCP: Playwright now bundles … `playwright-cli`, runnable via `npx playwright mcp` and `npx playwright cli`". The v1.59.0 release introduced `--debug=cli` "over `playwright-cli`". | "`trace` CLI and `--debug=cli`: 1.59+. `npx playwright cli attach`: 1.62+ (on 1.59–1.61 use `npx @playwright/cli attach`)." Alternatively, make 1.62 the floor. | +20 |
| P2 | per-invoke | SKILL.md:59, :76 | Under `--debug=cli`, a config with `retries>0` re-pauses the failed test on its retry with a **new** session id after `resume`, so the background run never ends. The skill's own §4 config uses `retries: process.env.CI ? 2 : 0`, which hits this under CI env. | Live run: after `resume`, the log printed a second `attach tw-640f8c`, and the process had to be killed | Add `--retries=0` to the :59 command. | +3 |
| P2 | per-invoke | SKILL.md:24-35, :58-68 | Both `trace open` and `cli attach` write `.playwright-cli/` into the **current directory**: `trace close` leaves an empty directory, and attach leaves `page-*.yml`. In a worktree this is untracked litter that can get committed. evidence-bundle.md:139-140 knows this; this skill does not. | Live run: `/tmp/pwq/.playwright-cli/page-2026-09-25T13-44-04-470Z.yml` persisted | Add: "Both write `./.playwright-cli/`; remove it (`rm -rf .playwright-cli`) before committing." | +20 |
| P3 | per-invoke | SKILL.md:138-141 | "The task note records …" does not give the Cassy call shape. | Rubric Axis 5 | `task action=notes id=<task> note_type=decision notes="action <id>: <cause> → <fix>"` | +10 |
| P3 | per-invoke | SKILL.md:16 | The fallback for versions below 1.59, `show-trace`, is a GUI. A headless agent cannot read it. | `show-trace --help` (serves a viewer) | "Read `error-context.md` and the reporter's call log. Upgrade to use the trace CLI." | +5 |
| P2 | always | SKILL.md:4 | Top-level `managed_by: cas`. | frontmatter | Move it to `metadata`. | 0 |

---

#### Overlap map and proposed ownership

| Topic | Occurrences (file:lines) | Owner | Action for the others |
|---|---|---|---|
| Trace-CLI triage | debug:18-51 · nuxt:268-279 · evidence-bundle:167-182 (supervisor review) · independent-pass:128 | **cas-playwright-debug** | nuxt: delete and point here. evidence-bundle: keep its 4-step review, since that is review rather than diagnosis, but link §1. |
| `--debug=cli` stepping | debug:53-77 · nuxt:217, :276, :339 | **cas-playwright-debug** | nuxt: one pointer line (its :276 is currently wrong). |
| Flake control (retries, retryStrategy, failOnFlakyTests, lock, trace retain) | debug:93-123 · nuxt:259-266, :301-302, :321 · auth-fixture-template:203-221 · frontend-eng:100-101 (mis-attributes it to nuxt) | **cas-playwright-debug** | nuxt: keep only the lock example for the shared account in the template. frontend-eng: repoint. |
| Web-first/retired idioms | debug:79-91 · nuxt:183-197, :309-322, :332 · frontend-eng:67-84 | **debug** owns fix idioms; **frontend-eng** owns acceptance assertions (description, aria snapshot, webp, a11y modes) | nuxt: keep only the Quasar `.visible()` row and the Nuxt hydration rows. |
| Component tests (`mount`, stories, `reuseContext`) | nuxt:281-288 · frontend-eng:83 | **cas-frontend-engineering** | nuxt: delete. |
| Route-mock origin rule | debug:127-133 (generic) · nuxt:162-181 (Firebase endpoints) | Generic rule in **debug**; Firebase endpoint list in **nuxt** | nuxt Rule 1 → pointer. |
| Trace `snapshots:{dom,aria,screen}` config | evidence-bundle:53-80 (`mode:'on'`, evidence) · debug:107-112 · nuxt:266 · template:215-218 (`retain-on-failure-and-retries`) | Evidence mode → **cas-qa-craft**; failure mode → **debug** | Template keeps its config (it is copy-ready); the nuxt body drops the prose. |
| a11y media modes | evidence-bundle:123-133 (emulateMedia plus a matchMedia proof) · frontend-eng:78-79, :86-96 (projects) · independent-pass:144-147 | Proof of an exercised mode → **qa-craft**; test projects → **frontend-eng** | Keep both; cross-link once. 1.63 also has standalone `testOptions.reducedMotion/forcedColors/contrast`. |
| webServer / server lifecycle | cas-servers:3, :23 (the description advertises "Playwright webServer", but the body has only one sentence on it) · template:234-242 · qa SKILL:35-36 · independent-pass:18 · journeys:35-39 = independent-pass:29-34 (hub-web ports) | **cas-servers** | Gap: cas-servers needs a 5-line "Playwright webServer" subsection covering per-run webServer (`reuseExistingServer:false`, a per-checkout port) versus a registered `server_start` plus `baseURL`. The nuxt template and qa-craft point to it. Keep the hub-web ports once, in journeys.md. |
| QA close gate | qa SKILL:62-75 · evidence-bundle:35-49 · cas-worker/references/close-gate.md:68-74 · cas-worker.md:24 ("for non-empty demo_statement" repeats the P0 trigger gap) | **evidence-bundle.md** (it is `CONTRACT_REFERENCE`, `qa_evidence.rs:23`) | qa SKILL and close-gate.md: one line plus a pointer. Fix cas-worker.md:24 to cover path- and journey-gated deliveries. |
| Bundle file list | qa SKILL:41-53 · evidence-bundle:17-27 · independent-pass:101-115 · journeys:43-50 | **evidence-bundle.md** | Others list deltas only. |
| Matrix quotas | qa SKILL:26-31 · matrix-builder:9-25 · independent-pass:51-61 · cas-supervisor/references/epic-flow-walk.md:21-27 | **matrix-builder.md** | Others: "per matrix-builder (cap 8)" plus their override (60 min / epic). |
| Rejection bar | independent-pass:132-147 · `qa_pass.rs:572-580` (generated task text) | Code-generated task text (intentional, `qa_pass.rs:566-571`) | Keep the reference copy, because installed refs may be stale (cross-lane P0 (a)). |
| navigateTo / hydration code | nuxt:130-160, :219-233 · template:86-114 | **template** | Body keeps the rationale only. |

#### Twin parity (axis 7)

`diff -r` of canonical against `codex/skills/<s>` and `grok/skills/<s>`: 0 differing lines for all three skills and all eight reference files. The skills use bare `task action=…` and `verification action=…` with no `mcp__cas__` prefix, so no substitution is expected and none occurs. The one parity defect is not textual: cas-nuxt-playwright's `disable-model-invocation` is ignored by Codex (P1 above). Both known cross-lane P0s apply here and are not re-reported: (a) bundled references never refresh after the first install (`qa_pass.rs:566-571` admits "an installed skill may predate `references/independent-pass.md`"); (b) Grok in cas-src resolves `.claude/skills`.

#### Search manifest

| Command | Hits |
|---|---|
| `grep -n -i -E "qa-craft\|nuxt-playwright\|playwright-debug" docs/analysis/2026-09-02-builtin-skills-review.md` | 10 (all nuxt, plus the "playwright-debug never built" line) |
| `npx -y playwright@1.63.0 --help` / `test --help` / `show-trace --help` / `trace --help` / `trace {actions,action,requests,console,errors,snapshot,screenshot,open,close} --help` / `cli --help` | all commands and flags the skills cite exist; `--debug [mode]` choices `inspector,cli` |
| `grep` in `playwright/types/test.d.ts`: retryStrategy / failOnFlakyTests / `lock?:` / reuseContext / retain-on-failure-and-retries / `snapshots?:` / subtitle / mount | 2 / 4 / 1 / 2 / 4 / 1 / 2 / 30 |
| `grep` in `playwright-core/types/types.d.ts`: Screencast start/showActions/showChapter/stop | 18651–18738; `visible()` 17247; `frameLocator(selector?)` 3019; `description?` 3205; `localStorage: WebStorage` 5831; `credentials` 10983; `ariaSnapshotJSON` 2149/14585; `drop(` 15362; `indexedDB?` 10700 |
| Live: failing test + `trace open/actions/--errors-only/--grep/action/snapshot --phase/snapshot -- eval/requests --failed/console --errors-only/errors/screenshot/close` | all OK |
| Live: `--debug=cli` + `cli attach` / bare `step-over` / `-s= step-over,snapshot,pause-at,eval,console,generate-locator,resume` | bare `step-over` fails; retry re-pause reproduced; headless (DISPLAY unset) works |
| Live: evidence-bundle example (screencast, chapter, ariaSnapshotJSON, emulateMedia reset) | all 4 gotchas reproduced |
| `exa-search --contents --fresh https://playwright.dev/docs/release-notes`; `exa-search "Playwright 1.59 … trace CLI --debug=cli"` | 1.60–1.63 sections; v1.59.0 GitHub release |
| `exa-search "Codex CLI skills … allow_implicit_invocation"` | developers.openai.com/codex/skills (`agents/openai.yaml` policy) |
| `grep -rn --include=*.rs` evidence_gate / terminal_render_paths / pass_timeout_mins / telemetry_sweep / qa_record / cas-allow-skip / platform_proof / `qa-bundle:` / visual_qa_status / critique_score / trace-actions / ledger_path / demo_statement / qa-pass / QaPass | 18 / 12 / 12 / 20 / 28 / 8 / 25 / 15 / 16 / 10 / 7 / 53 / 299 / 6 / 135 |
| `cas config get qa.telemetry_sweep`; `cas config get qa.pass_timeout_mins` | "" (unset); 45 |
| `find cas-cli/src/builtins -name 'visual-qa*' -o -name 'terminal-qa*'` | **0** |
| `ls ~/Petrastella/*/scripts/visual-qa.mjs` | 3 (cas-src, gabber-studio, gabber-qa-epic) |
| `grep -rn "openai.yaml\|allow_implicit_invocation" cas-cli/src --include=*.rs` | **0** |
| `grep -rn -i nuxt cas-cli/src/sync` | **0** (no stack gating) |
| `grep -c cas-servers` in the nuxt SKILL/template and the debug SKILL | **0 / 0 / 0** |
| `grep -l "^name:\|^description:"` over the references | **0** |
| Retired vocabulary grep (pending_supervisor_review, bypass_code_review, /epic-spec, /plan, cas-code-review, Phase 1/2) | **0** |
| `diff -r` canonical vs codex and grok twins (3 skills × 2) | **0** lines |
| `grep -n "\-s=" cas-nuxt-playwright/SKILL.md` | **0** (confirms the missing session flag) |
| `grep -n ".playwright-cli" cas-playwright-debug/SKILL.md` | **0** |

### Appendix D — D-ideation-docs — L5 skills audit (cas-9233)

HEAD 4836e56f7 (v3.31.0). Canonical dir `cas-cli/src/builtins/skills/`. All paths below are relative to it unless prefixed.
Axes: FM=frontmatter, Desc=description, PD=progressive disclosure, Word=wording, Proc=procedure/completion, Acc=accuracy, Par=parity, Tok=token economy (1-5).

#### Cross-group summary

- **P0: three task-create examples omit `risk`.** A task/bug/feature create without `risk` is rejected. Evidence: `cas-cli/src/mcp/tools/types/task.rs:124-140` (`TASK CREATE REJECTED: risk is required…`), called unconditionally from `mcp/tools/core/task/lifecycle.rs:811` and `service/core.rs:249`, with a pin test at `lifecycle.rs:2790`. Affected: `cas-github-issues/SKILL.md:122-126`, `cas-brainstorm/references/handoff.md:60`, `cas-ideate/references/post-ideation-workflow.md:172`. Epic creates are exempt.
- **P0: `session-learn` breaks its own runtime parser.** The skill body is now the live Stop-hook prompt (`include_str!` at `hooks/handlers/handlers_session.rs:1678`). `:70` tells the model to omit the body when `dedup_hits` is non-empty. But `SessionLearnDraft.content/signal/entry_type/scope` have no `serde(default)` (`hooks/handlers.rs:207-227`). One such draft makes `serde_json::from_str::<Vec<_>>` fail (`handlers_session.rs:1778-1779`), and the whole batch is dropped with an error log (`stop_flow.rs:491`).
- **Cross-lane P1: deprecated config key.** `issues.components.mecha_cassy` is deprecated in favour of `issues.components.violet` and is accepted for one more release only (`config/access/mod.rs:9-12`; `cas config get` prints the warning). It appears in `cas-github-issues:45`, `cas-worker.md:72`, `cas-supervisor.md:67`, `cas-supervisor/references/filing-cas-bugs.md:19,33` and the project CLAUDE.md snippet.
- **Twins:** all 7 skills are byte-identical to their Codex and Grok twins after tool-prefix normalisation, so there is no textual drift. Parity defects are Claude-only content shipped unadapted (AskUserQuestion, Glob, `.claude/scheduled_tasks.json`, `claude --resume`, `disable-model-invocation`).
- **All 7 skills:** top-level `managed_by: cas` (rubric P2; portable form is `metadata: {managed_by: cas}`). Reported once here, not per skill. `name` equals the directory name in all 7. No reference file carries `name:`/`description:`.
- **Known cross-lane P0 (a):** `references/*` never refresh after first install. This applies to `doc-hygiene.md`, `handoff.md`, `requirements-capture.md` and `post-ideation-workflow.md`. Not re-reported.

#### Scores

| Skill | FM | Desc | PD | Word | Proc | Acc | Par | Tok |
|---|---|---|---|---|---|---|---|---|
| cas-brainstorm | 4 | 4 | 3 | 2 | 3 | 2 | 3 | 2 |
| cas-ideate | 4 | 4 | 4 | 3 | 3 | 2 | 3 | 3 |
| cas-to-questionnaire | 4 | 3 | 5 | 4 | 3 | 4 | 3 | 5 |
| codemap | 4 | 5 | 4 | 4 | 4 | 3 | 4 | 3 |
| project-overview | 4 | 3 | 4 | 4 | 4 | 3 | 5 | 3 |
| session-learn | 4 | 4 | 2 | 3 | 2 | 1 | 3 | 2 |
| cas-github-issues | 4 | 5 | 4 | 4 | 5 | 2 | 3 | 4 |

---

#### cas-brainstorm

Size: `SKILL.md` 14,527 B / 237 L (about 3.6k tok per invoke). `references/handoff.md` 4,601 B / 108 L. `references/requirements-capture.md` 7,392 B / 158 L. Description is 125 chars. The first actionable step is at `:53` / `:67`, after 50 lines of stance.

Prior review:

- **FIXED:** `disallowed-tools` (the frontmatter at `:1-5` no longer has it).
- **FIXED:** `/plan` handoff (`handoff.md:87` and `requirements-capture.md:76` now name cas-supervisor).
- **STILL OPEN, carry-over:** AskUserQuestion boilerplate. The factory-blocked sentence now appears once (`:33`), but AskUserQuestion is still named at `:33`, `:81`, `:167`, `handoff.md:13` and `:19`.
- **STILL OPEN:** the interaction rules are stated three times (`:28-37`, `:165-176`, `:228-237`).
- **STILL OPEN:** MIT attribution in the body (`:41`).
- **STILL OPEN:** `task action=list status=closed` with no query (`:129`).

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P0 | on-demand | references/handoff.md:58-61 | The "Proceed directly to work" create has no `risk`, so it is rejected. | `types/task.rs:134-138`; the create defaults to task_type=task, which requires risk | Add `risk=<none\|platform\|…>` (plus `proof_targets` if blast-radius). | +8 |
| P0 | per-invoke | SKILL.md:32 vs :41 | The procedure contradicts itself. "Ask ONE question at a time… Never batch" conflicts with the frontier round: "ask the full frontier… Number every frontier question". | text | Pick one. Suggested: "Each round, ask only the questions whose prerequisites are settled, numbered, each with a recommended answer. Usually that is one." Delete the other rule. | −60 |
| P1 | on-demand | references/handoff.md:33 vs :46-53 | Epic ownership conflicts. `:33` says to hand off to cas-supervisor "to create an epic". Step 2 then creates the epic itself. cas-supervisor `intake.md:23` also creates the EPIC, so the result is duplicate epics. | `cas-supervisor/references/intake.md:23,42-47`; `planning.md:56,75` consumes R-IDs from the doc | Drop the brainstorm epic create. Hand cas-supervisor the doc path. Store only the memory pointer. | −60 |
| P2 | per-invoke | SKILL.md:28-37,165-176,228-237 | The same interaction rules appear three times (rules, 1.3 guidelines, anti-patterns). | text | Keep `:28-37`. Cut 1.3 guidelines to two lines and delete the ❌ list. | −350 |
| P2 | per-invoke | SKILL.md:30,33,81,167; handoff.md:13,19 | Restates a harness-enforced rule. In factory mode AskUserQuestion is already denied with the correct role-specific guidance, and the skill's "director relays" wording is supervisor-only (workers get a coordination route). | `hooks/handlers/handlers_events/pre_tool.rs:146-160` | State once: "Ask with AskUserQuestion; the harness redirects in factory mode." | −80 |
| P2 | per-invoke | SKILL.md:9-17,45-49 | Stance before procedure. Pipeline positioning duplicates `cas-ideate:9-15` and `intake.md:29-47`. The repo-relative-path rule is stated twice (`:17` in bold IMPORTANT and `:48`). | text | Start with Phase 0. Use one line for path hygiene. | −250 |
| P2 | per-invoke | SKILL.md:11,28,224; handoff.md:15,24 | Shouting on non-safety rules ("NON-NEGOTIABLE", "IMPORTANT", "CRITICAL", "single most important guardrail"). | rubric Axis 4 | Use a plain imperative plus the reason. | −30 |
| P2 | always/per-invoke | SKILL.md:33,81,167 (twins) | Parity: AskUserQuestion is a Claude Code tool, shipped verbatim in the Codex and Grok twins. | the codex/grok diff shows only prefix changes | Use harness-neutral wording ("the harness's blocking-question tool, else plain text and end the turn"). | 0 |
| P2 | per-invoke | SKILL.md (no line) | There is no `Done when …` criterion. Completion is spread across `:178`, Phase 3 and handoff. | grep `Done when` = 0 | Add: "Done when the doc has no Resolve-Before-Planning items (or the pause is recorded), the memory pointer is stored, and the handoff option is executed." | +40 |
| P3 | per-invoke | SKILL.md:129 | `task action=list status=closed` has no query filter. It returns the 20 most recent closed tasks, unrelated to the topic. | `TaskListRequest` has no query field; `query.rs:655-658,729` (limit defaults to 20) | Replace with `search action=search query="<topic>" doc_type=task`. | −5 |
| P3 | per-invoke | SKILL.md:41 | MIT provenance line in the always-invoked body. | text | Move it to a trailing comment or NOTICE. | −25 |

#### cas-ideate

Size: `SKILL.md` 9,108 B / 172 L (about 2.3k tok). `references/post-ideation-workflow.md` 7,998 B / 208 L. Description is 180 chars. The first action is at `:49`.

Prior review:

- **FIXED:** `disallowed-tools`.
- **FIXED:** the Haiku-scan vs "do NOT tier down" contradiction. The scan (`:90`) and the ideation agents (`:123`) are now labelled separately. `:90` introduces a new defect, below.
- **STILL OPEN:** "v1" narration (`:117`).
- **STILL OPEN:** AskUserQuestion boilerplate (`:28`, `:63`, `post:154`).
- **STILL OPEN:** volume math stated twice (`:79` and `:123`).

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P0 | on-demand | references/post-ideation-workflow.md:170-173 | The task create has no `risk`, so it is rejected. It also creates a "Brainstorm: X" task that nothing ever closes, while brainstorm's handoff creates its own task or epic. | `types/task.rs:134-138`; `handoff.md:49-61` | Delete the task create. The memory pointer (`post:98-101`) and the `Explored` marker already record the handoff. | −45 |
| P1 | per-invoke | SKILL.md:90 | "light lane… e.g. GPT-6 Luna/xhigh" is a factory `spawn_workers` lane, not something a Claude Agent-tool subagent can run. Claude's canonical text names a Codex model. | `cas-supervisor.md:23`; `model-selection.md:13` | "dispatch a cheap read-only subagent (the harness's smallest model)". | −5 |
| P2 | per-invoke | SKILL.md:88-109 | The grounding scan re-derives what `.claude/CODEMAP.md` and `docs/PRODUCT_OVERVIEW.md` already hold, via a subagent. | codemap:9; project-overview:9 | Step 1: read CODEMAP and PRODUCT_OVERVIEW if present (and `cas knowledge search`). Dispatch the scan agent only when both are missing. | −80 |
| P2 | per-invoke | SKILL.md:92 | Names a `Glob` tool. This Claude Code 2.1.282 session exposes no Glob/Grep tool, and Codex and Grok have none either. | this session's tool list; the twins ship `:92` verbatim | "list the top-level layout (`ls`, `git ls-files \| cut -d/ -f1-2 \| sort -u`)". | 0 |
| P2 | per-invoke | SKILL.md:9-17,163-172 | Pipeline preamble (a third copy, see the overlap section). The ❌ list restates principles `:21-24` and phase rules `:121` and `:159`. | text | Use a one-line output contract (prior-review "Worst" rewrite). Delete the anti-pattern list. | −320 |
| P2 | per-invoke | SKILL.md:28,63; post:154 | AskUserQuestion restated, with the same factory note as brainstorm. Same parity defect for Codex and Grok. | `pre_tool.rs:146-160` | Point to brainstorm's single rule. | −60 |
| P3 | per-invoke | SKILL.md:117 | "Do not do external research in v1 of this skill" is phase narration. | text | "Do not do external research." | −5 |
| P3 | per-invoke | SKILL.md:79,123; post:44 | Volume defaults stated three times. | text | State them once in 0.2. | −40 |
| P3 | on-demand | post:192-193 vs handoff.md:48 | Ideate says "Do not commit". Brainstorm says to make sure the doc is "committed/saved". The two artifact policies differ. | text | Pick one rule for `docs/ideation` and `docs/brainstorms`. | 0 |

#### cas-to-questionnaire

Size: `SKILL.md` 1,346 B / 16 L. Description is 108 chars. Uses `disable-model-invocation: true`.

Prior review:

- **STILL OPEN:** "user-approved output location" is undefined (`:14`).
- The prior "keep" verdict stands.

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P2 | always (Codex) | SKILL.md:3-4 | `disable-model-invocation` is honoured only by Claude and Grok. Codex gets it verbatim with no implicit-invocation mapping, so it is model-invocable there, and its description does not lead with "Use when". | `sync/skills.rs:119-121` is Claude-only; grep `allow_implicit_invocation` = 0 hits | Use "Use when a decision needs input from a third party the user can name; drafts a discovery questionnaire." Add Codex projection of the flag. | +5 |
| P2 | per-invoke | SKILL.md:12-16 | No entry point from cas-brainstorm. Brainstorm's `Resolve Before Planning` items that need a third party have no route here. | grep `questionnaire` in brainstorm = 0 | Add a handoff option in `handoff.md` 4.1: "Needs someone else's answer → /cas-to-questionnaire". | +25 |
| P3 | per-invoke | SKILL.md:14 | "user-approved output location" is undefined (carry-over). | text | "Propose `docs/questionnaires/YYYY-MM-DD-<recipient>-<topic>.md`; write after the user confirms." | +10 |
| P3 | per-invoke | SKILL.md:10 | Provenance line. | text | Move it to the footer. | −15 |

#### codemap

Size: `SKILL.md` 8,963 B / 146 L. `references/doc-hygiene.md` 1,935 B / 48 L (shared with project-overview and design-spec). Description is 85 chars. The first action is at `:24`.

Prior review:

- **FIXED:** the `:187` gate claim. `:132-133` now says Missing produces only a high banner and the gate needs SignificantlyStale, for a supervisor, on task create or spawn. This matches `pre_tool.rs:472-494` and `codemap.rs:237-242`.
- **FIXED:** the git-sole-authority statement was stated 3×; it is now stated once, at `:105`. This matches `codemap.rs:340-346`; the mtime fallback applies only when no CODEMAP is committed (`:419-425`).
- **FIXED:** the doc-family boilerplate is shared via `references/doc-hygiene.md`.
- **STILL OPEN:** exit-status capture prose (`:112-122`).

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P1 | per-invoke | SKILL.md:110,122 | Two build claims are false when the ledger is not current: "one build turns the doc you just wrote into a knowledge page" and "costs at most one model call". Pending sources are truncated in path order to `--max-sources`. In cas-src the ledger has 2 sources and a dry run shows 1,120 pending, so CODEMAP may be deferred. | `knowledge/pipeline.rs:276-279`; `cas knowledge build --dry-run --max-sources 5` gave "would distill: 1120"; `cas knowledge status` shows sources: 2 | "Run `cas knowledge build --dry-run`; if more than a handful of sources are pending, skip seeding (the doc is the artifact) and note it." Delete the one-call claim. | −40 |
| P1 | per-invoke / on-demand | SKILL.md:104; doc-hygiene.md:25-32 | Pointer-memory dedupe has no mechanism. "If a pointer with that title already exists, update it" gives no find/update call. The codemap title adds a `.md` suffix; project-overview and design-spec do not. | `search query="project_cas_codemap…" doc_type=entry` returns 4 cas-src pointers (2026-05-14-12, 07-01-4, 08-03-1, 09-14-1). The knowledge store holds both `project_cas_codemap.md` and `project_cas_codemap` pages. | Title `project_<slug>_codemap`. In doc-hygiene: "`search action=search query=<title> doc_type=entry`; if a hit, `memory action=update id=<id>`, else `remember`." | +30 |
| P2 | per-invoke | SKILL.md:112-122 | 11 lines of shell exit capture plus Rust internals ("terminates/reaps the active provider process group"). project-overview uses the plain command, so the doc family is inconsistent. | text; `cas knowledge build --help` (timeout default 90) | Use the prior-review "Worst" rewrite; the same two lines in both skills (move them into doc-hygiene §5). | −200 |
| P3 | per-invoke | SKILL.md:120 | "record the durable receipt in task notes" assumes a task exists, but a manual `/codemap` has none. | text | "…in task notes if running under a task." | +3 |
| P3 | on-demand | doc-hygiene.md:3-6 vs SKILL.md:101 | The parents say "three steps". The reference has four sections (plus a report-back that the parents override). | text | "four steps", or drop §4. | 0 |
| P3 | per-invoke | SKILL.md:52 | The template says "Regenerate with `/codemap`". Codex invokes skills as `$codemap`. | Codex skill invocation convention; the twin ships verbatim | "Regenerate with the codemap skill". | 0 |

#### project-overview

Size: `SKILL.md` 8,193 B / 157 L. Description is 150 chars. The Codex and Grok twins are byte-identical (no MCP calls).

Prior review:

- **FIXED:** the commit step (`:106-111`).
- **FIXED:** bounded build framing (`:123`).
- **FIXED:** the `.md` suffix on the memory title. It moved: codemap still has it.
- **FIXED:** `docs/plans/`.
- **Verified:** `clear` is still required. `check_freshness` reads `project-overview-pending.json` before git (`handlers_events/project_overview.rs:520-527`). This matches `cas project-overview --help`.

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P1 | per-invoke | SKILL.md:117-123 | Same `--max-sources 5` false claims as codemap. `docs/…` sorts late, so PRODUCT_OVERVIEW is the source most likely deferred. | `pipeline.rs:276-279`; dry run gave 1,120 pending | Same as codemap (dry-run gate). | −30 |
| P2 | always | SKILL.md:3 | "Use when asked what a project is" over-triggers. A one-line "what is this repo?" question fires a doc-write-commit workflow. | description text | "Use when asked to create or refresh docs/PRODUCT_OVERVIEW.md, or when SessionStart reports it missing or stale; not for answering a quick 'what is this project'." | +10 |
| P3 | per-invoke | SKILL.md:106-111 | The commit block duplicates doc-hygiene §3. | `doc-hygiene.md:34-39` | Keep only the sentence about why the commit matters. | −30 |
| P3 | per-invoke | SKILL.md:46 | Lists `docs/requests/` as in-flight planning. It is deprecated for new requests. | `cas-github-issues:201` | Drop it, or say "legacy". | −5 |
| P3 | per-invoke | SKILL.md:154; codemap:142; design-spec; doc-hygiene:20-21 | "Destroying hand-edits is a trust breaker" is stated 4× across the family. | text | Keep it only in doc-hygiene. | −40 |

#### session-learn

Size: `SKILL.md` 9,660 B / 141 L. It is also the Stop-hook runtime prompt (about 2.4k input tokens on every auto-extracting Stop). Description is 207 chars.

Prior review:

- **FIXED:** the `include_str!` claim is now TRUE. `handlers_session.rs:1678` embeds the body; a test at `:1793` pins it.
- **FIXED:** the description rewrite.
- **STILL OPEN:** third-brain provenance (`:9`, `:15`, `:141`).
- **STILL OPEN:** design notes (`:76-97`).
- **STILL OPEN:** "You do NOT write" (`:21`) conflicts with the user-invoked path.

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P0 | per-invoke + runtime | SKILL.md:70 | "omit the rest of the body" when `dedup_hits` is non-empty fails the parse. `content`, `signal`, `entry_type` and `scope` are required, so one omitted draft drops the entire batch. | `hooks/handlers.rs:207-227` (no `serde(default)` on those fields); `handlers_session.rs:1778-1779`; `stop_flow.rs:491` | "When `dedup_hits` is non-empty, still emit `signal`, `entry_type`, `scope`, and a one-line `content`." Or default the fields in Rust. | +10 |
| P1 | runtime | SKILL.md:44 | Hook mode cannot perform "scan the existing memory store via `mcp__cas__search`". The hook calls `traced_prompt(... .max_turns(1))`, which leaves no turn to call a tool and read the result. Dedup in hook mode is really `find_similar_entry` (BM25). | `handlers_session.rs:1758-1766`; `stop_flow.rs:430-439` | "Interactive only: search first. Hook mode: leave `dedup_hits` empty; Rust dedupes." | +10 |
| P1 | per-invoke | SKILL.md:3 vs :19-21 | The user-invoked path has no store step. The description says it "hands each accepted draft to cas-memory-management". The body says "You do NOT write… the caller writes", but when the user invokes it the agent is the caller. | text | Use the prior-review "Worst" rewrite, and add `Done when every draft is stored, corroborated, or dropped`. | +30 |
| P2 | runtime | SKILL.md:19,70 | Wrong runtime description. It says the caller "routes them through `mcp__cas__memory action=remember`" and that `dedup_hits` makes the caller "record corroboration". The hook uses `store.add` directly and silently drops drafts with `dedup_hits`. | `stop_flow.rs:428-471` | Describe the actual behaviour, or implement corroboration. | 0 |
| P2 | per-invoke + runtime | SKILL.md:9,13-15,76-97,137-141 | Provenance, "v1" narration, the in-process vs subprocess decision, and see-also are all sent to the classifier on every Stop. | `include_str!` whole file | Move them to a doc comment in `handlers_session.rs`. The body keeps signals, rules, schema and one example. | −1,100 per Stop |
| P2 | runtime | SKILL.md:101,128 | The worked example carries a ticket ID (`cas-ec8f`) and supervisor/cherry-pick jargon. Models copy example details into memories. | rubric Axis 4 | Use a neutral example. | −20 |
| P3 | per-invoke | SKILL.md:43 vs `handlers_session.rs:1733` | Floor description is inaccurate. The actual floors are observations ≥5 (`stop_flow.rs:398`) and a transcript of at least 500 bytes. "tool calls" is approximate. | code | "fewer than 5 recorded observations". | 0 |
| P3 | per-invoke (twins) | SKILL.md:80 | `claude --resume … --skill` is Claude-only and not a real flag set. Codex and Grok twins ship it verbatim. | text | Delete it (covered by the P2 row above). | incl. |

#### cas-github-issues

Size: `SKILL.md` 10,834 B / 244 L. Description is 128 chars. The first action is at `:20`, and the file has sharp stop conditions (`:14-16`, `:228-244`).

Prior review:

- **FIXED:** `pending_supervisor_review` (`:37` now lists `awaiting_merge`). The substring claim is re-verified at `query.rs:655-658`.
- **FIXED:** the preamble (`:18-34` is now "Before you start" procedure).
- **FIXED:** date narration.
- **STILL OPEN, P3:** the scheduled-task expiry claim (`:218-221`). It is now verified as Claude behaviour: the claude 2.1.282 binary contains "Recurring tasks auto-expire after" and `scheduled_tasks.json`.

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P0 | per-invoke | SKILL.md:122-126 | The per-issue `task_type=bug` create has no `risk`, so every create in step 4 is rejected. | `types/task.rs:124-140`; `lifecycle.rs:811` | Add `risk=<none\|blast-radius…> proof_targets=<when blast-radius>` to the template. | +10 |
| P1 | per-invoke | SKILL.md:45 | `issues.components.mecha_cassy` is deprecated in favour of `issues.components.violet` and accepted for one release only. | `config/access/mod.rs:9-12`; `cas config get` prints the deprecation warning | Rename it here and in the cross-lane copies. | 0 |
| P2 | per-invoke | SKILL.md:78-80,92-94 | Two commands where gh 2.101 needs one. `gh issue close --duplicate-of <keeper>` links the issues natively. `-c` adds the comment. | `gh issue close --help`: `--duplicate-of`, `-r {completed\|not planned\|duplicate}`, `-c` | `gh issue close <dup> --duplicate-of <keeper> -c "Same defect, tracking there."` and `gh issue close <n> -r completed -c "<evidence>"`. | −30 |
| P2 | per-invoke | SKILL.md:201 | Operator-specific fact in a universal builtin ("Richards-LLC team's issue board"). | rubric Axis 6 | "the receiving team's configured tracker (`issues.*`)". | −5 |
| P2 | per-invoke (twins) | SKILL.md:216-226 | Parity: `.claude/scheduled_tasks.json` cron and `lastFiredAt` are Claude Code only. They ship verbatim to Codex and Grok, which have no such file. | normalised twin diff = identical | Say "Claude Code: …; other harnesses: whatever scheduler armed the sweep." | +15 |
| P3 | per-invoke | SKILL.md:22,31 | `--limit 100` and `list limit=100` silently truncate large backlogs, with no rule for handling it. | gh default is 30; `query.rs:729` | "If the result count equals the limit, page or raise it." | +15 |

---

#### Overlap map and ownership

| Topic | Where it lives (lines) | Owner | Others should |
|---|---|---|---|
| Pipeline positioning (ideate → brainstorm → plan) | brainstorm:9-15; ideate:9-15, :23; `cas-supervisor/references/intake.md:29-47` | **cas-supervisor intake.md** (routing). The skill descriptions already route. | Cut each skill to a one-line output contract (−400 total). |
| When to brainstorm or skip | brainstorm:84-94 (0.2), :96-114 (scope); intake.md:39-47 | brainstorm 0.2 for the "skip" test; intake.md for the trigger | intake.md keeps the trigger bullets only; drop its Output/Handoff restatements. |
| Pressure test / request challenge | brainstorm:142-163; intake.md:11-21 (Intake Gate: goal clarity, assumption surfacing, "why now") | Split by stage: intake.md for requests going straight to EPIC; brainstorm 1.2 once brainstorm fires | intake.md: "if brainstorm fires, its 1.2 replaces gates 3, 4 and 7". |
| Question mechanics (one-at-a-time, frontier, AskUserQuestion) | brainstorm:28-43, :165-178; ideate:26-28; handoff.md:13,19; post:154 | **cas-brainstorm** (after fixing the :32/:41 contradiction) | ideate links to it; delete the factory-mode sentences (the hook enforces them, `pre_tool.rs:146-160`). |
| Facts vs decisions / who answers | brainstorm:43 ("facts are agent work, decisions are user work"); to-questionnaire:12 ("grill the send… don't ask the user for recipient facts") | brainstorm handles the user's decisions; **to-questionnaire** handles third-party facts and decisions | brainstorm handoff 4.1 adds a to-questionnaire option. |
| Resume-prior-work check | brainstorm:65-82; ideate:47-69 (same ls + `search doc_type=entry` + "continue or start fresh?") | Each keeps its own; the shape is identical | Share a 3-line pattern; no action needed beyond trimming. |
| Divergence prompts | brainstorm:184-187 (inversion, constraint removal, analogy); ideate:137-140 (inversion, assumption-breaking, leverage) | **cas-ideate** owns divergent generation | brainstorm Phase 2 keeps "2-3 approaches, one non-obvious". |
| Repo grounding | ideate:84-117 (scan subagent); brainstorm:118-140; codemap; project-overview | **codemap / project-overview** own durable grounding | ideate and brainstorm read CODEMAP and PRODUCT_OVERVIEW first. |
| Epic/task creation after brainstorm | handoff.md:46-61; post:170-173; intake.md:23; planning.md:46-98 | **cas-supervisor planning.md** | brainstorm and ideate create no tasks; they hand over the doc path and a memory pointer (fixes both `risk` P0s and the duplicate epic). |
| Memory pointer after an artifact | brainstorm:214-218; handoff.md:104-108; post:98-101; doc-hygiene.md:23-32 | **doc-hygiene §2** pattern (with the find-then-update fix) | brainstorm and ideate reuse it; one pointer per doc, updated on rerun. |
| Anti-pattern ❌ lists | brainstorm:228-237; ideate:163-172; codemap:137-146; project-overview:148-157 | none; each restates the body | Delete them or reduce to ≤3 non-redundant items (−600 across 4). |
| Doc-family hygiene (keep-block, pointer, commit) | doc-hygiene.md; codemap:99-106; project-overview:100-139; design-spec:101-107 | **doc-hygiene.md** (FIXED) | Move the knowledge-build step into it too. |

#### Search manifest

| Command | Result |
|---|---|
| `grep -n <7 skill names> docs/analysis/2026-09-02-builtin-skills-review.md` | 24 hits |
| `wc -c -l` on all files of the 7 skills | 11 files |
| `grep "pub struct TaskRequest" crates/cas-mcp/src/types.rs` | 1 hit (actions and params incl. `risk`, `labels`, `external_ref`, `epic`, `to_id`, `acceptance_criteria`) |
| `grep "risk is required"` | `types/task.rs:138` plus the test at `lifecycle.rs:2800` |
| `grep validate_task_risk_declaration` | 3 call sites |
| `grep "pub enum TaskStatus"` | `cas-types/src/task.rs:19` (alias `pending_supervisor_review` → AwaitingMerge at :37) |
| `grep status query.rs` | substring filter at :655-658 |
| `grep doc_type` search types | `search.rs:22`; `ops_secondary.rs:58` (entry and rule valid) |
| `grep "struct SpecRequest"` | `types.rs:811` (`mcp__cas__spec` exists) |
| `grep MemoryRequest` | remember/update/title/tags valid |
| `cas knowledge --help`, `build --help`, `read --help`, `search --help` | all used flags exist (`--timeout-secs` default 90, `--max-sources` default 25, `--dry-run`) |
| `cas knowledge search "codemap module layout workspace"` | 10 pages, incl. duplicate `project_cas_codemap(.md)` pointers |
| `cas knowledge status --full` | 2 sources, 2 failed |
| `cas knowledge build --dry-run --max-sources 5` | would distill 1,120 |
| `pipeline.rs max_sources` | truncate at :277-279 |
| `sources.rs` | `.md` anywhere is distillable |
| `cas codemap --help` / `status` | status, pending and clear exist; `Status: stale` format confirmed |
| `cas project-overview --help` / `status` | clear exists; cas-src has no PRODUCT_OVERVIEW.md |
| `grep -n codemap pre_tool.rs` | gate at :461-494 (supervisor, SignificantlyStale only) |
| `codemap.rs` | Missing → high banner (:237-242); git-based evaluation (:340-425) |
| `project_overview.rs check_freshness` | pending file first, then git (:510-527) |
| `grep session_learn cas-cli/src` | `include_str!` at `handlers_session.rs:1678`; the prompt uses `max_turns(1)` |
| `stop_flow.rs:393-491` | floor obs≥5; confidence 0.6 (0.5 for correction); `dedup_hits` non-empty → drop; `store.add` |
| `grep "pub struct SessionLearnDraft"` | `hooks/handlers.rs:207` (required fields) |
| `grep AskUserQuestion pre_tool.rs` | factory deny at :146-160 |
| `grep -c AskUserQuestion` | brainstorm 3 + 2; ideate 2 + 1 |
| `gh issue close --help` | `--duplicate-of`, reason `duplicate` exist |
| `gh issue list`, `gh pr list`, `gh issue create`, `gh repo view --help` | all used flags valid (`--json comments`/`createdAt`/`labels` valid) |
| `cas config get` for issues.* and history.github_repo | `mecha_cassy` deprecation warning shown |
| `grep mecha_cassy cas-cli/src/builtins` | 10 hits (cross-lane) |
| `strings claude-2.1.282 \| grep auto-expire` | 5 hits; `scheduled_tasks.json` 2 hits |
| Twin diff (prefix-normalised) for 7 skills × codex/grok | 0 residual drift |
| grep `GPT-6 Luna` in skills | supervisor lane registry only; ideate:90 is the misuse |
| grep `/plan`, `pending_supervisor_review`, `cas-code-review`, `verified on this machine` in the 7 skills | **0 hits** |
| grep `cas-[0-9a-f]{4}` ticket IDs in the 7 skills | 3 (`session-learn:101`, `cas-github-issues:138-139`; the latter is an intentional bad/good example) |
| grep `Done when` in the 7 skills | **0 hits** |
| grep `allow_implicit_invocation` in cas-cli/src | **0 hits** |
| grep `questionnaire` in cas-brainstorm | **0 hits** |
| `mcp__cas__search query="project_cas_codemap CODEMAP pointer" doc_type=entry` | 4 cas-src duplicate pointers |

### Appendix E — Group E — tooling & method skills (cas-9233 L5 audit)

Repo HEAD 4836e56f7 (v3.31.0). Canonical: `cas-cli/src/builtins/skills/<skill>/` (paths below are relative to that dir unless they start with `cas-cli/`, `crates/` or `~`). Tools verified: codex-cli 0.156.0, claude 2.1.282, cas 3.31.0. `~/.codex/config.toml` default model = `gpt-6-sol`, `model_reasoning_effort = "high"` (read-only).

#### Sizes (bytes / lines)

| Skill | SKILL.md | references / scripts |
|---|---|---|
| mcp-integration | 4032 / 81 | references/diagnosis.md 4260 / 53 |
| cli-routing | 2417 / 46 | references/routing.md 4346 / 96 |
| cas-codex-exec | 2883 / 69 | — |
| cas-viktor | 2498 / 54 | references/gateway.md 3434 / 53 |
| cas-servers | 4734 / 101 | — |
| cas-codebase-design | 6627 / 133 | — (DEEPENING/DESIGN-IT-TWICE inlined) |
| cas-tdd | 3334 / 43 | — (mocking.md/tests.md inlined) |
| cas-wizard | 1296 / 15 | template.sh 1621 / 27 |
| cas-diagnosing-bugs | 2837 / 64 | — |
| cas-resolving-merge-conflicts | 1120 / 20 | — |

Description lengths (chars, always surface): mcp-integration 241, cli-routing 230, cas-codex-exec 192, cas-codebase-design 187, cas-wizard 161, cas-viktor 156, cas-servers 122, cas-tdd 115, cas-diagnosing-bugs 93, cas-resolving-merge-conflicts 63 — all ≤ 250, all lead with "Use when". Every `name` == dir name. No reference file carries `name:`/`description:` frontmatter (0 hits).

#### Per-skill scores (1–5): FM · Desc · PD · Wording · Proc · Accuracy · Parity · Tokens

| Skill | FM | Desc | PD | Word | Proc | Acc | Par | Tok |
|---|---|---|---|---|---|---|---|---|
| mcp-integration | 4 | 4 | 4 | 4 | 4 | 2 | 5 | 4 |
| cli-routing | 4 | 3 | 3 | 4 | 4 | 3 | 4 | 3 |
| cas-codex-exec | 4 | 4 | 5 | 4 | 4 | 3 | 5 | 4 |
| cas-viktor | 4 | 5 | 3 | 4 | 4 | 1 | 5 | 3 |
| cas-servers | 4 | 5 | 5 | 3 | 4 | 5 | 5 | 4 |
| cas-codebase-design | 4 | 4 | 2 | 4 | 3 | 3 | 5 | 3 |
| cas-tdd | 4 | 5 | 5 | 4 | 3 | 4 | 5 | 4 |
| cas-wizard | 4 | 5 | 4 | 4 | 3 | 3 | 5 | 5 |
| cas-diagnosing-bugs | 4 | 4 | 5 | 5 | 4 | 5 | 5 | 5 |
| cas-resolving-merge-conflicts | 4 | 5 | 5 | 4 | 3 | 5 | 5 | 5 |

FM = 4 everywhere only because of top-level `managed_by: cas` (rubric P2, one group-wide row below).

#### Twin parity (Claude → Codex → Grok)

All 10 skills, every file: after normalising `mcp__cas__`→`mcp__cs__` (codex) / `cas__` (grok), residual diff = **0 lines**, no extra/missing files in either twin (script in manifest). The prior `cas-wizard/template.sh` twin drift is FIXED in source. Installed-copy drift observed in `~/.claude/skills` (stale `cas-wizard/template.sh` missing the `set -e`/`if confirm` lines; orphan `cas-tdd/mocking.md` still carrying the NestJS carve-out; orphan `cas-codebase-design/DEEPENING.md`, `DESIGN-IT-TWICE.md`) is an instance of **known cross-lane P0 (a)** (non-SKILL.md files never refresh) — not re-reported. `/home/pippenz/Petrastella/cas-src/.claude/skills/*` copies match source exactly.

#### Group-wide

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P2 | always | all 10 `SKILL.md:4-5` | Top-level `managed_by: cas` (non-standard key) | rubric Axis 1; grep `^managed_by` = 10/10 | Move to `metadata: {managed_by: cas}` (merge into existing `metadata` in codebase-design/diagnosing-bugs/merge-conflicts) | ~0 |
| P2 | per-invoke | 9 of 10 (all but codebase-design, diagnosing-bugs) | No `Done when …` criterion | grep -ci "done when" = 0 in mcp-integration, cli-routing, cas-codex-exec, cas-viktor, cas-servers, cas-tdd, cas-wizard, cas-resolving-merge-conflicts | Add one closing `Done when …` line per skill (proposed text in each skill's rows) | +25 each |

---

#### mcp-integration

Prior-review status: teaches only `claude mcp` (P0 #17) — **FIXED** (`:11-52` uses `cas mcp list/add/import` + `proxy_*`). Stance-before-procedure (ladder at :87) — **FIXED** (step 1 at `:11`). Body/diagnosis duplication — **FIXED** (`diagnosis.md:3-5` declares split). Viktor scopes/run handles in diagnosis.md — **FIXED** (0 viktor hits). `--show-secrets` mention — **FIXED** (`:13`).

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P0 | per-invoke | `SKILL.md:39-57` (+ whole skill) | Never mentions the proxy **allowlist**. `cas mcp add` and `proxy_add` do not write it; it is fail-closed and a project `.cas/proxy.toml` list *replaces* the user list. Step 5's "one cheap read-only call through `mcp_execute`" is therefore denied by policy after every fresh add, and creating a project `proxy.toml` silently removes any user-allowlisted route. | `crates/cas-mcp-proxy/src/config.rs:53-59` ("An empty list is intentionally fail-closed"), `:421-425` (project allowlist authoritative); grep `allowlist` in `cas-cli/src/cli/mcp_cmd.rs` and `agent_search_system/system.rs` = 0; live `.cas/proxy.toml` carries `allowlist = ["mecha-cassy.mecha_read", …]` | Add step 4b: "Admit each route you need as `"<server>.<tool>"` in `allowlist` of the same config file; an empty list denies every call, and a project list replaces the user list. For factory workers set `[servers.<name>] worker_access = "read-only"` when appropriate." | +90 |
| P0 | per-invoke | `SKILL.md:24-28` | Claude's `-s local` semantics applied to `cas mcp`: "A local registration is keyed by its directory, so it does not follow a Cassy worktree. Choose `user` for workers…". For `cas mcp add`, `local` (default) and `project` both write `<cas_root>/proxy.toml`, and `cas_root` in a worktree resolves to the main `.cas` — so local *is* fleet-visible; `user` needlessly publishes a project server to every project on the machine (`~/.config/code-mode-mcp/config.toml`). | `cas-cli/src/cli/mcp_cmd.rs:54-55` (default "local"), `:118-124` (`_ => cas_root.join("proxy.toml")`); `cas-cli/src/store/detect.rs:50-70` (CAS_ROOT/worktree → main store); this worktree: `CAS_ROOT=/home/pippenz/Petrastella/cas-src/.cas` | Replace with: "`local`/`project` write `.cas/proxy.toml` (git-ignored, shared by every Cassy worktree of the repo); `user` writes `~/.config/code-mode-mcp/config.toml` for all projects. Prefer the project file." | −10 |
| P2 | per-invoke | `SKILL.md:24,50` | Creating `.cas/proxy.toml` also stops the managed Viktor default refresh; not mentioned here (only in cas-viktor `gateway.md:20-21`) | `cas-cli/src/mcp/server/runtime.rs:319-332` | One sentence: "A project `.cas/proxy.toml` opts out of managed defaults (Viktor, MechaCassy); configure them explicitly — see cas-viktor / mecha-cassy." | +35 |
| P3 | per-invoke | `SKILL.md:72-77` | "When MCP is unavailable" mixes a harness-degradation rule with a design opinion (script vs MCP) | — | Keep the degradation rule; drop the script-vs-MCP stance (belongs in codebase-design) | −40 |
| P3 | per-invoke | end of SKILL.md | No completion line | — | "Done when `cas mcp list --json` shows the server connected with the expected tool count, the routes are allowlisted, and one read-only `mcp_execute` call returned data." | +40 |

Verified OK: `cas mcp add` flags `-s/-t/-e/-H/--auth` (`cas mcp add --help`); `cas mcp import --from claude|codex --dry-run --force` (`cas mcp import --help`); `cas mcp list --json --show-secrets` redaction (`cas mcp list --help`); `proxy_add` fields `name/transport/url/command/args/env/auth` (`crates/cas-mcp/src/types/ops_secondary.rs:312-350`), dispatch `cas-cli/src/mcp/tools/service/mod.rs:978-984`; "Restart `cas serve`" matches handler text (`agent_search_system/system.rs:707-708`); `${API_TOKEN}` placeholder is expanded (`crates/cas-mcp-proxy/src/lib.rs:1948-1954`); `mcp_search server:<name>` (`mod.rs:1285`).

#### cli-routing

Prior-review status: operator e-mail — **FIXED** (config key `release.claude_account_allowlist`, registered `cas-cli/src/config/meta/seed/release.rs:6`, pinned by `builtin_skill_description_test.rs:215,340-388`). Source-tree-only `../../../../` / dossier links — **FIXED** (0 hits; only sibling `../<skill>/SKILL.md` links remain). "Verified on this machine … 2.1.231" — **FIXED** (0 hits). Two competing codex recipes — **mostly FIXED** (`routing.md:14-16` defers to cas-codex-exec) but see P1 below. Release-note posting in three places — **STILL OPEN (carry-over)**. Description rewrite — **FIXED**.

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P1 | on-demand | `references/routing.md:14-16` | Claims cas-codex-exec owns "closing stdin, and redirecting long output to a file"; the canonical recipe (`cas-codex-exec/SKILL.md:17,34`) has no `< /dev/null`. `codex exec` appends piped stdin to the prompt and waits for EOF. | `codex exec --help`: "If stdin is piped and a prompt is also provided, stdin is appended as a `<stdin>` block". Claude Code's Bash stdin is `/dev/null` (checked), but tmux/Codex/Grok panes are not guaranteed | Fix at the owner: add `< /dev/null` to both cas-codex-exec recipes; keep this sentence | 0 here |
| P1 | on-demand | `references/routing.md:18-21` | For "a narrowly scoped write" recommends `--dangerously-bypass-approvals-and-sandbox`; codex 0.156 offers `-s workspace-write` (+ `--add-dir`) and `--approve-for-me` for exactly this | `codex exec --help` (`-s … workspace-write`, `--add-dir`, `--approve-for-me`; bypass flag "EXTREMELY DANGEROUS … solely for … externally sandboxed") | "A write uses `-s workspace-write` (add `--add-dir <DIR>` for paths outside `-C`); use the bypass flag only inside an external sandbox." Move to cas-codex-exec (see overlap) | +10 |
| P1 | always | `SKILL.md:3` vs `cas-codex-exec/SKILL.md:3` | Both descriptions trigger on "a one-shot `codex exec` subprocess" → double-fire for any Codex one-shot | both descriptions | cli-routing: "Use when a one-shot `codex exec` failed for capacity/auth or a bounded write/structured-output one-shot is needed and a `claude -p` fallback may be required; the Claude account gate in references/routing.md decides." | +5 |
| P2 | per-invoke | `SKILL.md:32-40` + `routing.md:81-96` | Release-note posting restated twice here and again in `release-notes/SKILL.md:24-30` and `mecha-cassy/SKILL.md:9,30` (carry-over) | quotes: "never use Claude.ai Slack or a personal connector" ×4 files | Delete both sections; one line in Do-not-trigger: "Slack posting is release-notes + mecha-cassy, never a CLI one-shot." | −380 |
| P2 | per-invoke | `SKILL.md:20-25` vs `routing.md:52-66` | Account-gate conditions stated in full twice | — | Body keeps "only after the account gate in routing.md passes"; conditions live in the reference | −90 |
| P2 | on-demand | `references/routing.md:40,71` | Example profile `$HOME/.claude-alt` is this operator's directory; models copy example literals | `ls -d ~/.claude-alt` exists on this box | Use `CLAUDE_CONFIG_DIR="<profile dir>"` | 0 |
| P2 | on-demand | `references/routing.md:29-32` | Strict `--output-schema` note omits `additionalProperties: false`, which strict mode also requires | OpenAI strict structured-output rules; flag exists in `codex exec --help` | "…and every object sets `additionalProperties: false`." Move to cas-codex-exec | +12 |
| P2 | per-invoke | Codex twin `codex/skills/cli-routing/SKILL.md:15` | "Try Codex first" is meaningless for a Codex agent recovering from its own capacity loss; twin is prefix-only | parity residual 0 | Add harness note: "If you are Codex, 'Codex first' means a fresh `codex exec`; capacity loss on the same account goes straight to the gate." (or ALLOWED_FLAVOR_ONLY line) | +25 |
| P3 | on-demand | `references/routing.md:5-10` | "We found no reproducible … text" — investigation narration | — | "No reliable quota preflight or exhaustion text exists; treat only a nonzero exit plus captured stderr as capacity evidence." | −40 |
| P3 | per-invoke | end | No Done-when | — | "Done when the one-shot produced its output file with exit 0, or both receipts are reported as blocked." | +25 |

Verified OK: `claude -p/--print`, `--output-format text|json|stream-json` (`claude --help` :143-168); `claude auth status --json` (`claude auth status --help`); JSON keys `loggedIn, authMethod, apiProvider, email, subscriptionType` present (live probe keys list); `-c model_reasoning_effort="low"` is a real config key (`~/.codex/config.toml:2`); `--output-schema <FILE>` (`codex exec --help`); `cas config get/set release.claude_account_allowlist` (`cas-cli/src/config/access/get.rs:138`, `set.rs:578`).

#### cas-codex-exec

Prior-review status: `-m gpt-5.5` pin — **FIXED** (`:24-26` "Omit `-m/--model`"; current default `gpt-6-sol`). "Verified on this machine" — **FIXED** (0 hits). Competing recipe — **FIXED** (declared canonical `:13-14`).

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P1 | per-invoke | `SKILL.md:17,34` | Canonical recipe does not close stdin; a harness with a live stdin pipe blocks/appends | `codex exec --help` PROMPT arg text; `routing.md:15` promises it | `… codex exec -s read-only -C "$PWD" -o "$out" "<prompt>" < /dev/null` | +3 |
| P2 | per-invoke | `SKILL.md:30-35` | Long sweep uses shell `&`, contradicting cas-servers rule 1 (`cas-servers/SKILL.md:85-87` "If you catch yourself typing `&` …") and bypassing the harness's own background runner; 1800 s exceeds Claude Code's 600 s Bash cap so a background mechanism is needed | cas-servers rule text; Bash tool max 600000 ms | "Run it with the harness's background execution (Claude Code `run_in_background`), not `&`; poll `$out`." | +5 |
| P2 | per-invoke | `SKILL.md:17,34` | `/usr/bin/timeout` is GNU coreutils; macOS has no `/usr/bin/timeout` (Homebrew `gtimeout`) — Mac agents fail at exec | macOS base system lacks `timeout` (not verified on a Mac here) | `timeout 600 …` and a failure-mode line "no `timeout` on macOS → `gtimeout` or omit and rely on the harness timeout" | +15 |
| P2 | per-invoke | `SKILL.md:65-69` | Missing failure mode: codex refuses outside a git repo (e.g. `-C ~/.cas/artifacts/...`) | `codex exec --help`: `--skip-git-repo-check  Allow running Codex outside a Git repository` | Add "Outside a git repo add `--skip-git-repo-check`"; optionally recommend `--ephemeral` to avoid session files | +20 |
| P2 | per-invoke | `SKILL.md:65-69` vs `cli-routing/SKILL.md:18-27` | Capacity/auth failure → "do the investigation directly" here, → "preserve evidence, route via account gate" in cli-routing; two policies for one failure | quotes | One line: "On capacity/auth failure keep command, exit, stderr and follow cli-routing." | +15 |
| P3 | always | `SKILL.md:3` | `READ-ONLY` in caps in the description | rubric Axis 2 (no shouting) | "read-only" | 0 |
| P3 | per-invoke | `SKILL.md:24-26` | "never a `-codex`-suffixed slug" — unexplained, unverifiable against help | not in `codex exec --help` | Drop, or give the reason | −10 |
| P3 | per-invoke | end | No Done-when | — | "Done when the `-o` file holds an answer naming what was inspected, or the failure is reported with its exit status." | +25 |

Verified OK: `-s/--sandbox read-only`, `-C/--cd`, `-m/--model`, `-o/--output-last-message`, `--json` (`codex exec --help`).

#### cas-viktor

Prior-review status: never shows the `mcp_execute` call shape — **FIXED in form** (`:23-29`) but the shown args are wrong (new P0). Allowlist and cadence — still verified. Key procedure duplicated body vs `gateway.md` — **STILL OPEN (carry-over)**.

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P0 | per-invoke | `SKILL.md:28` | Example `ask_viktor` args `{"question":…,"cas_task_id":…}`: Viktor's schema requires `message` (no `question`), and `cas_task_id` is not a parameter; the proxy does no mapping. Copying the example fails validation or starts a run without the message. | live `mcp__viktor__ask_viktor` schema: `required: ["message"]`, props `message, metadata, idempotency_key, response_format, speed, timeout_seconds`; grep `cas_task_id`/`"question"` in `crates/cas-mcp-proxy/src` and `viktor_watch.rs` = 0 | `"args":{"message":"<bounded question>","metadata":{"cas_task_id":"<task-id>"},"idempotency_key":"<task-id>-<n>"}` | +10 |
| P1 | per-invoke | `SKILL.md:35-39`, `gateway.md:49-50` | "Never automatically retry an uncertain start" but never uses the provider's `idempotency_key`, which makes a retry safe (same key → same thread/run) | ask_viktor schema `idempotency_key` description | "Always pass `idempotency_key`; a retry with the same key and args returns the original run." | +25 |
| P2 | per-invoke | `SKILL.md:13-21` vs `gateway.md:30-38`; `SKILL.md:35-46` vs `gateway.md:34-38,47-53`; `SKILL.md:48-54` vs `gateway.md:5-11,52-53` | Procedure, watch/notification and credential rules each stated in both files (carry-over) | side-by-side | Body = steps + call shape + boundary; gateway.md = provisioning, opt-out, inbound-thread mechanics only | −300 |
| P2 | on-demand | `gateway.md:40-45` | Daemon internals (32-thread scan, 4 `list_messages`, 4 s budget) are not agent-actionable | `cas-cli/src/mcp/viktor_watch.rs:13-21` (values correct) | Keep "Viktor-originated questions arrive as `origin=viktor` notifications; reply with `send_message` on the supplied thread." | −90 |
| P3 | per-invoke | `SKILL.md:31-32` | "equivalent to the proxy's dot-call form when that route advertises it" — dot-call syntax never shown; adds a second form to choose | `mod.rs:1329` | Drop the sentence; one form | −20 |
| P3 | per-invoke | end | No Done-when | — | "Done when the run is registered (thread/run id noted on the task) or the reply has been received and recorded." | +25 |

Context (code, not skill): the `mcp_search`/`mcp_execute` `code` schema says "TypeScript code to execute…" (`crates/cas-mcp/src/types/ops_secondary.rs:1262`) while the tool descriptions say keyword/`server:name` and JSON dispatch (`mod.rs:1285,1329`) — schema drift the skill has to fight; file against cas-mcp types. Verified OK: allowlist = 9 tools exactly (`crates/cas-mcp-proxy/src/config.rs:13-23` = `gateway.md:17-18`); `https://api.viktor.com/mcp`, `env:VIKTOR_API_KEY` (`config.rs:10-11`); `cas viktor`, `cas viktor key` (`cas viktor --help`); project `.cas/proxy.toml` opt-out (`runtime.rs:319-332`); 30 s cadence (`viktor_watch.rs:13`). Live `mcp_search server:viktor` here → "upstream 'viktor' is absent" (project proxy.toml exists and allowlists only mecha-cassy), consistent with gateway.md:20-28.

#### cas-servers

Prior-review status: "keep, every param verified" — still true. Nit "never background yourself" stated 3× — **STILL OPEN (carry-over, P3)**.

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P2 | per-invoke | `SKILL.md:9,20,85,88` | Bold/absolute emphasis on non-safety rules ("**Never background a server yourself.**", "**Registered servers are the only ones…**", rules 1–2 bold) | rubric Axis 4 | Plain imperatives + reason; state "don't `&`/`nohup`/`setsid`" once (rule 1) | −60 |
| P3 | per-invoke | `SKILL.md:89-90` | "Starting a second one usually just fails to bind" — same `id` is refused explicitly | `cas-cli/src/mcp/tools/service/server_ops.rs:127-140` ("a server named '…' is already running … Stop it first") | "A duplicate `id` is refused; a duplicate port fails to bind." | 0 |
| P3 | per-invoke | end | No Done-when | — | "Done when `server_list task_id=<task>` shows nothing you started still running, or each survivor is named in the handoff." | +30 |

Verified OK: `server_start command/cwd/port/id/shared/task_id` (`crates/cas-mcp/src/types/ops_secondary.rs:796-822,1222-1248`; handler `server_ops.rs:83-140`); `server_stop` accepts only `action,id` (`mod.rs:579`, `server_ops.rs:197-212`); `server_list task_id` filter (`server_ops.rs:279-300`); dispatch `mod.rs:686,713-715,1472-1474`.

#### cas-codebase-design

Prior-review status: tiny references `DEEPENING.md`/`DESIGN-IT-TWICE.md` — **FIXED** (inlined `:64-81`). NestJS leakage `:32-34` — **FIXED** (generalised "framework vocabulary wins"). `cas-domain-modeling` merge — **FIXED** (`:37-50`). Completion criterion — **FIXED** (`:125-133`). Generic `mcp__cas__spec` — **FIXED** (`action=create`).

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P1 | per-invoke | `SKILL.md:106-107` | "Follow the `cas-update-and-doctor-read-like-reports` precedent" — unresolvable name (no skill, rule, doc or reference by that name) | `grep -rl cas-update-and-doctor-read-like-reports` → only the 3 twin copies of this file | Replace with "use cas-cli-craft for CLI/TUI output" | −10 |
| P2 | per-invoke | `SKILL.md:83-123` | "API and DX taste" + a second 3-axis critique rubric duplicates cas-cli-craft (`cas-cli-craft/SKILL.md:17-44`, its `references/critique-rubric.md`) and cas-ui-craft's rubric; three competing public-surface rubrics | description of cas-cli-craft ("Owns … the scored critique") | Keep Names/Errors/Receipts bullets (API contract, `:89-103`); replace `:104-123` with "For printed output, apply cas-cli-craft." | −330 |
| P2 | per-invoke | whole file (133 lines) | Over the ~80-line house budget for methodology skills; no numbered procedure — first imperative is `:39` inside prose | rubric Axis 3/5 | After the cut above (~110 lines), add a 5-step procedure at top: vocabulary check → design it twice → deletion test → record decision → Done-when | −100 net |
| P3 | per-invoke | `SKILL.md:57-58,79-81` | Restates cas-tdd `:15,24` (test through the interface) | text | Point to cas-tdd | −40 |

Verified: `mcp__cas__memory action=remember` with `scope`/`tags` (`crates/cas-mcp/src/types.rs:47,72`); `note_type=decision` (`cas-cli/src/mcp/tools/core/task/notes.rs:74`); `mcp__cas__spec action=create` (`types.rs:814`).

#### cas-tdd

Prior-review status: identity-first description — **FIXED** (`:3`). NestJS carve-out in `:32` + `mocking.md` — **FIXED** (`:36` generalised; `mocking.md` inlined/removed in source — stale copy survives in `~/.claude/skills`, known P0 (a)). `mocking.md`/`tests.md` restating body — **FIXED** (inlined). Worst-line "record red and green run with note_type=progress" — **PARTIAL** (`:18,43` ask for the result but not the red run).

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P1 | per-invoke | `SKILL.md:18,40,43` | Requires running the red and the green test before handoff; a factory worker on a Rust lane is denied every `cargo`/`run-scoped-tests.sh` call and must park without building. No carve-out → worker loops on a denied command or skips the skill | `cas-worker/references/discipline.md:8-26`, `close-gate.md:128`; CLAUDE.md "Factory workers never run Rust builds" | Add: "In a factory Rust lane, write the failing test, commit, and park; the supervisor's `ASSEMBLY_PROOF` is the red→green evidence (cas-worker)." | +35 |
| P2 | per-invoke | `SKILL.md:18` vs `:43` | Same rule twice (scoped command, nonzero count, record in task) | text | Keep loop rule 4; drop `:18` | −30 |
| P3 | per-invoke | `SKILL.md:40-43` | No explicit Done-when / note type | — | "Done when the red run and the green run (command, test count, exit) are recorded with `mcp__cas__task action=notes note_type=progress`." | +20 |

#### cas-wizard

Prior-review status: identity-first description — **FIXED**. `template.sh` twin drift — **FIXED** in source (parity 0). `set -e` + bare `confirm` — **FIXED** (`template.sh:21-22,26` documents and demonstrates `if confirm`).

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P1 | on-demand | `template.sh:9` | `open_url` chains `A && B \|\| C && D \|\| E`; bash evaluates left-to-right, so after a successful `xdg-open` it *also* runs `open "$1"`, and prints "Open manually" when that fails. On this box `/usr/bin/open → xdg-open` (URL opens twice); on Debian `open` is `openvt` | reproduced: `A && B \|\| C && D \|\| E` with A,B true → runs D and E; `readlink -f $(command -v open)` = `/usr/bin/xdg-open` | `open_url() { if command -v xdg-open >/dev/null; then xdg-open "$1"; elif command -v open >/dev/null; then open "$1"; else printf 'Open manually: %s\n' "$1"; fi; }` | +10 |
| P2 | per-invoke | `SKILL.md:11-15` | Procedure is three prose paragraphs; no numbered steps or Done-when; "task's approved output area" (`:13`) undefined | rubric Axis 5 | Steps: 1 enumerate manual steps (URL, destination, secret?) 2 confirm stage plan with user 3 copy template, edit after STAGES 4 `bash -n` + static trace 5 record path in task note. "Done when `bash -n` passes and the user approved the stage list." Name the location ("the task's artifact dir or a path the user names") | +30 |
| P3 | on-demand | `template.sh:16` | `write_env` persists secrets to `.env` in plaintext without checking it is git-ignored | code | Add `git check-ignore -q "$ENV_FILE" \|\| say "WARNING: $ENV_FILE is not ignored"` | +15 |

Note: `bash -n template.sh` passes.

#### cas-diagnosing-bugs

Prior-review status: names no Cassy tool / "task note" → `note_type=discovery` — **FIXED** (`:42,64`).

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P2 | per-invoke | `SKILL.md:21,54-57` | Preferred loop is "a failing scoped test" and Phase 5 requires making the regression fail/pass — same factory-Rust-worker conflict as cas-tdd | `cas-worker/references/discipline.md:8-26` | One line: "In a factory Rust lane, use a non-cargo loop (CLI fixture, log replay) and park the regression test for assembly." | +25 |
| P3 | per-invoke | `SKILL.md:18,33,39,46,52,59` | "Phase 1…6" headings trip the retired "Phase 1/2" vocabulary lint although they are procedure, not ticket narration | rubric Axis 6 list | Rename to "Step 1 — …" or numbered list | 0 |

#### cas-resolving-merge-conflicts

Prior-review status: `note_type=decision` — **FIXED** (`:16`). Provenance line dead context — **FIXED** (moved to `metadata.provenance`).

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P2 | per-invoke | `SKILL.md:14,17` | No concrete commands; "inspect state" / "finish" leave the agent to guess (merge vs rebase continue) | — | `git status`, `git diff --name-only --diff-filter=U`, `git log --merge --oneline`; finish with `git add` + `git merge --continue` / `git rebase --continue` | +40 |
| P3 | per-invoke | `SKILL.md:18` | Factory Rust worker cannot "run the project's affected checks" | as cas-tdd | "(factory Rust lanes: park for assembly)" | +10 |
| P3 | per-invoke | end | No Done-when | — | "Done when `git status` shows no unmerged paths, the merge/rebase is complete, and any trade-off is noted." | +25 |

---

#### Overlap map and ownership

##### cas-codex-exec ↔ cli-routing

| Topic | cas-codex-exec | cli-routing | Owner → action |
|---|---|---|---|
| Trigger ("one-shot `codex exec`") | `SKILL.md:3` | `SKILL.md:3` | cas-codex-exec owns any Codex one-shot; cli-routing's description narrows to capacity fallback + Claude gate (P1 row above) |
| Invocation recipe (sandbox, `-C`, model default, `-o`, stdin) | `:11-35` | `SKILL.md:15-17` (pointer), `routing.md:12-16` (pointer) | cas-codex-exec; add `< /dev/null` there |
| Write-mode flags, `-c model_reasoning_effort`, `--output-schema` strictness | — | `routing.md:18-23,29-32` | Move to cas-codex-exec as a "Write / structured output" subsection (Codex flag knowledge in one place); cli-routing keeps nothing Codex-specific |
| Failure → fallback policy | `:65-69` ("investigate directly") | `SKILL.md:18-27`, `routing.md:3-10,77-79` (evidence, Claude gate) | cli-routing owns capacity/auth routing; cas-codex-exec failure section points to it |
| Evidence capture (output file + exit) | `:27,34,69` | `SKILL.md:18-19`, `routing.md:22-23` | cas-codex-exec (mechanics); cli-routing references "the receipt" |
| Release-note posting | — | `SKILL.md:32-40`, `routing.md:81-96` | Neither — release-notes (`SKILL.md:24-30`) + mecha-cassy (`SKILL.md:9,30`) own it; delete from cli-routing |
| Claude one-shot + account gate | — | `SKILL.md:20-27`, `routing.md:34-79` | cli-routing (sole owner) |

Net: cli-routing shrinks to ~30-line body + ~55-line reference (≈ −500 tokens across both), cas-codex-exec grows ≈ +80.

##### mcp-integration ↔ cas-viktor

| Topic | mcp-integration | cas-viktor | Owner → action |
|---|---|---|---|
| Discover + execute through proxy | `SKILL.md:53-57` | `SKILL.md:13-29`, `gateway.md:32-34` | mcp-integration owns the generic ladder; cas-viktor keeps only the Viktor call shape (fixed args) |
| Credential boundary | `SKILL.md:59-62` | `SKILL.md:48-54`, `gateway.md:5-11,52-53` | mcp-integration owns the generic rule; cas-viktor keeps `cas viktor key` provisioning only |
| Retry/idempotency of side-effecting calls | `SKILL.md:68-70`, `diagnosis.md:38-40` | `SKILL.md:35-46`, `gateway.md:49-50` | mcp-integration owns the generic classification; cas-viktor states the Viktor specifics (`idempotency_key`, daemon watch) |
| Allowlist / project `proxy.toml` opt-out | missing (P0) | `SKILL.md:17,33,50`, `gateway.md:15-21` | mcp-integration must own allowlist + opt-out semantics; cas-viktor lists its 9 routes and points there |
| Scopes, `cas mcp add/import/list`, `proxy_*` | `SKILL.md:11-52` | — | mcp-integration |
| Viktor watch / inbound threads / cost | — | `SKILL.md:35-46`, `gateway.md:30-45` | cas-viktor (trim internals) |

##### Other overlaps

- cas-codebase-design `:83-123` ↔ cas-cli-craft `SKILL.md:17-44` (+ `references/critique-rubric.md`) and cas-ui-craft rubric → cli-craft owns printed-output critique; codebase-design keeps API contract bullets.
- cas-codebase-design `:57-58,79-81` ↔ cas-tdd `:15,24,28` → cas-tdd owns "test through the public interface"; codebase-design points.
- cas-codebase-design `:45-50` ↔ cas-memory-management (`SKILL.md:20-47`) → acceptable (one concrete call); no action.
- cas-tdd `:18,40-43`, cas-diagnosing-bugs `:21,54-57`, cas-resolving-merge-conflicts `:18` ↔ cas-worker `references/discipline.md:8-26` → cas-worker owns the Rust no-build rule; each method skill needs a one-line carve-out pointer.
- cas-codex-exec `:30-35` (`&`) ↔ cas-servers `:9,85-87` (never `&`) → cas-servers owns long-lived processes; codex one-shots use the harness background runner.
- cli-routing `:32-40` ↔ release-notes `:24-30` ↔ mecha-cassy `:9,30` → covered above.

#### Search manifest

| Command | Hits |
|---|---|
| `grep -n` group skill names / codex / viktor / NestJS / template.sh / note_type in `docs/analysis/2026-09-02-builtin-skills-review.md` | 49 lines |
| `grep -rnE "pippenz\|@gmail\|\.\./\.\./\.\./\|cas-1c67\|SLACK_POSTING\|NestJS\|\.service\.ts\|verified on this machine\|2\.1\.[0-9]{3}\|gpt-5\|gpt-6"` over the 10 skills + codex/grok twins of cli-routing, cas-codex-exec, cas-tdd, cas-codebase-design | **0** |
| `grep -c '^name:'` in heads of all `references/*.md` in group | **0** |
| `grep -ciE "done when"` per SKILL.md | 0 in 8 skills; codebase-design 1 ("Done when"), diagnosing-bugs 0 ("Before claiming done") |
| `grep -cE "\b(NEVER\|MUST\|ALWAYS\|CRITICAL\|IMPORTANT\|ONLY\|READ-ONLY)\b"` per SKILL.md | 1 (cas-codex-exec, description) |
| prefix-normalised `diff` canonical vs codex/grok, all files, 10 skills | residual 0 / 0; extra files 0 |
| `diff -r` source vs `/home/pippenz/Petrastella/cas-src/.claude/skills/<s>` | 0 diff in all 10 |
| `diff -r` source vs `~/.claude/skills/<s>` | 4 orphan files (tdd ×2, codebase-design ×2), template.sh −3 lines (known P0 a) |
| `grep -n allowlist cas-cli/src/cli/mcp_cmd.rs cas-cli/src/mcp/tools/service/agent_search_system/system.rs` | **0** |
| `grep -rn allowlist skills/mcp-integration` | **0** |
| `grep -rn "cas_task_id\|\"question\""` in `crates/cas-mcp-proxy/src`, `cas-cli/src/mcp/viktor_watch.rs` | **0** (only unrelated `valid_cas_task_id`) |
| `grep -rl cas-update-and-doctor-read-like-reports` repo | 3 (this skill + 2 twins only) |
| `grep -rn '"server_start"\|"server_stop"\|"server_list"' cas-cli/src crates` | 11 (dispatch `mod.rs:579,686,713-715,876-878,1472-1474`) |
| `grep -rn "claude_account_allowlist" --include=*.rs` | 10+ (config get/set/seed, tests) |
| `codex exec --help`; `claude --help`; `claude auth status --help`; `cas mcp --help`, `cas mcp add/import/list --help`; `cas viktor --help`; `cas config set --help` | flags cited above |
| `mcp__cas__mcp_search code="server:viktor"` (live) | error: upstream 'viktor' absent (configured, not connected) |
| `ToolSearch select:mcp__viktor__ask_viktor` (live schema) | `required: [message]`; no `question`/`cas_task_id` |
| bash precedence repro of `template.sh:9` | runs `open` after successful `xdg-open`, then prints fallback |
| `bash -n cas-wizard/template.sh` | syntax ok |
