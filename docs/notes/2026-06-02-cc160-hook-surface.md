# Claude Code 2.1.160 Hook Surface — Working Notes

EPIC: **cas-2f29** · branch `epic/epic-exploit-claude-code-2-1-152-160-hook-skill-su-cas-2f29` · opened 2026-06-02

Two perspectives tracked per item (release-post convention — user journey first, technical after):
- 👤 **USER** — what changes for the person using Cassy, plain English
- 🛠 **DEV** — technical change, files/gotchas

**Release → Slack mapping:** at release, this becomes the standard #cas-internal **two-thread** post (per `feedback_slack_release_post_template`): Thread 1 = USER summary (parent) + USER details (reply, built from the 👤 lines below); Thread 2 = DEV summary (parent) + DEV details (reply, built from the 🛠 lines + Release mechanics: Cargo.toml bump, tag, commit SHAs). No `cas-xxxx` IDs in the Slack posts.

---

## Status

| Task | Title | Worker | State |
|------|-------|--------|-------|
| cas-f9ad | SessionStart `reloadSkills` | sturdy-fox-40 | ✅ closed — dcb046d, 9 tests |
| cas-ae09 | SessionStart `sessionTitle` | sturdy-fox-40 | ✅ closed — 131a351, 10 tests |
| cas-5be8 | `disallowed-tools` frontmatter | noble-jay-2 | ✅ closed — bdf93e4→65c6368, 7 tests, m083/m086 |
| cas-b39a | `MessageDisplay` hook (opt-in) | fair-octopus-14 | 🔄 in progress |
| cas-f97d | acceptEdits 160 spike | golden-pelican-12 | ✅ closed — NOT IMPACTED |

Branches: `factory/sturdy-fox-40` (cas-f9ad+cas-ae09), `factory/noble-jay-2` (cas-5be8). Full lib suite on sturdy-fox-40: 2004/0.

**#6 proof spike (separate):** cas-2efa — golden-pelican-12 — 🔄 in progress (Workflow-vs-cas-code-review measurement).

### ⚠️ Pre-merge checklist (before epic→main)
- [x] Migration-number collision: **CLEARED** — sturdy-fox-40's `skill_sync_sentinel` is a flat file (no migration); sessionTitle reads existing TaskStore. No collision with noble's m083/m086.
- [x] Merges: all 3 branches integrated **by SHA** (branch refs were recycled/unreliable) into epic. **All auto-merged clean — zero conflicts.** Order: ff 131a351 (sturdy) → no-ff 65c6368 (noble) → no-ff a883e58 (fair). Epic HEAD = **1607e37**. All 5 task commits confirmed ancestors.
- [x] cargo test on integrated epic — compiles clean. First full parallel run: 2021 pass, 1 fail = the `test_configure_merges_existing` flake (race on real ~/.claude/settings.json).
- [x] **Flake decision (user: fold in):** cherry-picked cas-1888 (176383d) into epic → flake gone.
- [x] **2nd integration failure surfaced + fixed:** `doctor_snapshot` insta test drifted — cas-5be8's m083/m086 migration added the `disallowed_tools` column (443→444 cols, 157→158 rows). Benign schema-count change; snapshot updated + committed (40d0c19). Classic per-worker-green / integration-drift catch.
- [x] **Failure triage (3 distinct, all fixed):** (1) `test_configure_merges_existing` flake → cas-1888 cherry-pick; (2) `doctor_snapshot` cc160 schema drift → snapshot updated; (3) `fresh_teammate_pull_applies_team_memories_to_local_store` — PRE-EXISTING (red on main; epic diff touches zero cloud-sync code; workers ran `--lib` and missed it). Single-threaded run confirmed #3 was the ONLY deterministic failure.
- [x] **#3 fixed (user: "we don't leave failures"):** root cause = **case A, stale test mock**. `entity_matches_project` (cas-6479) requires every pulled row to echo `project_id`; the mock built a Project-scoped entry with none. Patched the mock to echo `project_id` like real cloud → 3/3 green. Folded into the release. **cas-0e16 closed.**
- [x] **Fail-fast was masking failures.** `cargo test` stops at the first failing *binary*, so each run showed only one failure; fixing each unmasked the next. Definitive **single-threaded `--no-fail-fast`** run = the complete deterministic truth.
- [x] **Complete deterministic failure set (post the 3 fixes above):** exactly **3 tests, all in `cas-cli/tests/team_pull_wiring_test.rs`** — `team_pull_hits_endpoint_and_lands_rows` + `execute_sync_hits_each_pull_endpoint` (stale project_id mocks, same class as cas-0e16) and `team_pull_no_op_when_no_team_configured` (expects 0 requests, gets 1 — diagnose real-bug vs global-state leak). **All pre-existing** (file untouched by cc160 → byte-identical on main); from cas-6479 hardening vs cas-ffc4-era mocks. cc160 surface fully green.
- [x] **cas-6ddc** (sturdy-fox-40): DONE — 6f7cc08, test-only (+60/-3), 7/7 green single-threaded AND parallel. (1)+(2) = project_id mocks fixed with dynamic `get_project_canonical_id()`; (3) = Case B global-state leak (machine's `~/.cas/cloud.json` default_team_id bled into a no-team project via `active_team_id()`) → isolated with a CAS_USER_CLOUD_JSON scoped guard. Cherry-picked onto epic → **HEAD 425343b**. (Latent product Q noted: should a no-team project inherit the user default team for sync? Out of scope.)
- [x] **Definitive ship gate**: full parallel `--no-fail-fast` on epic 425343b = **4139 passed, 0 failed, 93 suites**.
- [x] **Migration-leak caught + recovered.** First merge attempt revealed main had moved e99ac5d→6a4add9 during the session: the #6 spike/validation/Phase-A commits (7464045, 6280d02, 5e0082b, **6a4add9**) had been committed **directly to main** (worker-isolation leak). 6a4add9 = Phase A test-first = *failing* JS tests — would've shipped in 2.18.0. Reverted my first merge (`--keep`, WIP safe), fast-forwarded **cas-b667** to 6a4add9 (all migration work preserved on its own epic), reset main to clean **e99ac5d**, re-merged cc160 only → **main e4a4b55** (verified: cc160 present, zero migration commits, WIP intact). cargo gate's tree is identical to the verified 425343b.
- [x] Bump 2.17.5 → **2.18.0** (cas-cli/Cargo.toml:3). Build running (bg bda3a3d9g) to confirm + update Cargo.lock.
- [x] **SHIPPED 2.18.0** → `origin/main` = **f19c706**, tag **v2.18.0** → f19c706, in sync. Push initially rejected (origin had advanced by `db03685` = legit /doctor shell-form fix, NOT the leak); integrated it, resolved the hook_tests conflict to shell-form, **converted cc160's MessageDisplay registration exec-form→shell-form** (else /doctor on CC 2.1.159 would silently disable it), bumped the `every_emitted_hook_object` count 12→13. Final gate **4141 passed, 0 failed**.
- [x] cc160 workers (sturdy-fox-40, noble-jay-2, fair-octopus-14) shut down — work shipped.
- [ ] **OPEN:** golden-pelican-12 paused-safe (holding, not committing). Migration Phase A preserved on **cas-b667 @ a8e5144**. Needs clean re-isolation to resume (worker was committing to main — isolation leak). User to decide: respawn isolated on cas-b667 vs pause migration.
- [ ] **OPEN:** Slack two-thread release post (draft + ask before sending) per convention.
- [ ] cosmetic: cas-b667 carries my reverted merge (2944a5e) in ancestry — harmless, optional cleanup.

**Epic integration commits (on epic branch, HEAD 40d0c19):** bdf93e4→65c6368 (cas-5be8), dcb046d→131a351 (cas-f9ad+ae09), a883e58 (cas-b39a), 2× merge commits, 53e8961 (cas-1888 cherry-pick), 40d0c19 (doctor_snapshot fix).

---

## Items

### cas-f9ad — SessionStart reloadSkills
- 👤 USER: Run `cas update --sync` while a session is open and skill edits now take effect on the next session — no manual copy-into-worktree dance, no restart. Skills just refresh.
- 🛠 DEV: SessionStart hook emits `reloadSkills:true` on detected skill drift; sync sentinel written in `sync_claude_files()` (update.rs:144); new field on SessionStart output struct. Handler at handlers_session.rs:3.

### cas-ae09 — SessionStart sessionTitle
- 👤 USER: The `claude agents` dashboard / tmux panes now show which worker owns which task at a glance — `[worker] cas-1234 · <task>` instead of anonymous sessions. The factory becomes legible without asking.
- 🛠 DEV: `hookSpecificOutput.sessionTitle` built from role + active-task lease (inject_role_guidance, coordination.rs:218); factory-gated; non-factory sessions unaffected.

### cas-5be8 — disallowed-tools frontmatter
- 👤 USER: Workers physically can't reach for TodoWrite/EnterPlanMode anymore (the CLAUDE.md ban becomes real, not prose), and brainstorm/ideate can't accidentally edit files. Fewer "the agent did the wrong thing" moments.
- 🛠 DEV: new `disallowed_tools` field on Skill type (skill.rs:171, migration mirroring m079) + serialize in generate_skill_md (skills.rs:~98-103) + `disallowed-tools` lines in builtin SKILL.md (claude + codex mirrors).

### cas-b39a — MessageDisplay hook (opt-in, default OFF)
- 👤 USER: An opt-in guard that stops the Claude Code pane from crashing on Box-in-Text markdown, and scrubs secrets out of assistant replies before they render. Off unless you turn it on.
- 🛠 DEV: new MessageDisplay handler + register event in get_cas_hooks_config (config_gen.rs ~:159); config flag default-off; default path is byte-identical passthrough (test-proven); reuses PreToolUse secret-redaction helper.

### cas-f97d — acceptEdits 160 regression spike ✅ NOT IMPACTED
- 👤 USER: Confirmed the new 2.1.160 "confirm before writing sensitive config" prompt does NOT stall factory workers — they run in bypass-permissions mode so the check never fires. Nothing to fix. (Heads-up: a *non-factory* user in acceptEdits mode editing `.claude/`, `.mcp.json`, shell rc files, or git config will now see a confirmation prompt — that's intended Claude Code hardening, not a Cassy bug.)
- 🛠 DEV: characterization-first verdict — factory workers spawn with `--dangerously-skip-permissions` → `bypassPermissions`, which short-circuits the 160 `ORA`/`NX$` sensitive-file check (only fires in acceptEdits/default). Trigger set: shell init, git config, `.npmrc`/`.yarnrc`/`bunfig.toml`, `.bazelrc`/wrappers, `.pre-commit-config.yaml`/`lefthook.yml`/`.mcp.json`/`.devcontainer.json`, and sensitive dirs (`.git/`, `.claude/` w/ exceptions, `.cargo/`, etc.). NOT triggered: `Cargo.toml`, `package.json`, `.cas/config.toml`, `pyproject.toml`. No pre_tool.rs change. Memory 2026-06-02-7.

---

## Strategic thread — #6: native Workflow / Agent Teams vs Cassy factory

- 👤 USER: This is about where Cassy spends its effort. Anthropic now ships its own multi-agent orchestration (Workflow tool, Agent Teams, the agents dashboard). The recommendation: Cassy stops competing on *how to run agents* and doubles down on what it uniquely gives you — memory that survives across sessions, and quality gates that catch bad work. As a bonus, some Cassy features (code review, deep research, ideate) get faster and cheaper by riding the native Workflow engine instead of the heavy factory.
- 🛠 DEV: 3-tier posture —
  - **A (keep in factory):** long-lived, human-supervised, cross-session EPIC work; workers that push back + accumulate context. Still differentiated (verification jail, merge-state SHA guards, multi-CLI incl. Codex, task-as-source-of-truth).
  - **B (migrate to Workflow scripts called from Cassy skills):** deterministic fan-out/verify patterns — `cas-code-review` (first), `deep-research`, `cas-ideate`, `session-learn`, duplicate-detector. Skill becomes a thin wrapper that authors the Workflow and writes results back into Cassy memory/tasks.
  - **C (substrate under native orchestration):** native subagents/teammates pull Cassy context at spawn (SessionStart hook already does this) and write learnings back via MCP. Cassy feeds native orchestration instead of competing with it.
  - Open fork: is the orchestration layer a long-term differentiator, or a soon-commoditized mechanism to cede? Lean: cede mechanism, own knowledge + quality.
  - Proof spike (proposed, after the 5 land): re-implement `cas-code-review` dispatch as a Workflow script; measure token cost / latency / determinism vs the current path.

### #6 SPIKE RESULT — cas-2efa (golden-pelican-12), verdict: **HYBRID**
- 👤 USER: A test of the idea showed cas-code-review can run meaningfully cheaper and more predictably on the native engine, while the Cassy-specific smarts (task tracking, review routing, fix loops) stay in Cassy. Roughly half the cost on the heavy part, with results that are reproducible run-to-run. Estimated, not yet measured live.
- 🛠 DEV: Recommendation **HYBRID** — move dispatch+merge to a Workflow script; keep the skill as a ~50-line Cassy-integration wrapper. Cassy task integration (modes, task notes, fix loop, review→task routing) does NOT fit fire-and-return → stays in wrapper. Generalizes to cas-ideate / deep-research / session-learn / duplicate-detector. **Artifacts (commit 7464045):** `.claude/workflows/cas-code-review-prototype.js` + `docs/ideation/2026-06-02-cas-code-review-workflow-migration-spike.md`.

### #6 LIVE-MEASURED — cas-6a84 (golden-pelican-12), HYBRID confirmed, rationale REVISED
- 👤 USER: We actually ran it. The earlier "half the cost" estimate was wrong — real cold-run cost is about the SAME as today (~16 min either way, ~16% cheaper, not ~54%). The real, big win is different: once a diff has been reviewed, re-reviewing it is **instant and free** ($0, 10ms) instead of paying full price again, and the findings come back in a strict validated shape. So the case for moving isn't "it's cheaper" — it's "it's reproducible, resumable, and the repeated review loop becomes free."
- 🛠 DEV: LIVE numbers (wf_0a04a6b2-2d2, diff cas-e603): cold run **587K subagent tok / 970s / 10 agents / 332 tool uses / 24 findings**; cached re-run **0 tok / 10ms / byte-identical**. Dry estimate was **6× off** (92K→587K) — dominant cost is agents running ~33 tool calls each to verify file:line citations before StructuredOutput (present in BOTH paths). Revised: cold savings **16%** (~$1.76 vs ~$2.09/review, the $0.33 from dropping the Opus orchestrator); **cache/resume + schema enforcement are the actual value**, not cost. Commits 6280d02 + 5e0082b; doc updated with measured numbers. Byproduct: the live review found 2 real bugs (P1 supervisor_guidance ceiling, P2 AskUserQuestion reachability) → triage **cas-7265** (likely stale vs current main; did not reproduce in cc160 test run).

---

## Open decisions
- [ ] **Flake fix:** fold cas-1888 (176383d) into the cc160 release? (recommend yes — robustly green parallel suite). **Awaiting user.**
- [ ] **Ship cc160:** merge epic→main, version bump (2.17.5 → propose 2.18.0, feature epic), `cargo test`, push, Slack two-thread post. **Held for user go.**
- [x] **#6 live validation** (cas-6a84) — DONE. Measured: 16% cold savings (not 54%), but $0/10ms cached re-run + schema enforcement are the real value. HYBRID confirmed.
- [x] **#6 fork (data-informed)** — user greenlit **full migration** → **EPIC cas-b667** created (Phase A cas-e4d4 started on golden-pelican-12, Phase B cas-0f13 / Phase C cas-7c64 queued). Runs parallel to the cc160 release.
- [ ] **cas-6ddc** (sturdy-fox-40) — fix 3 pre-existing team_pull_wiring failures → release blocker per "we don't leave failures".
- [ ] **cas-7265** — triage 2 byproduct review findings (non-blocker).
- [x] Greenlight `cas-code-review → Workflow` proof spike — DONE (cas-2efa). Verdict HYBRID; artifacts at 7464045.

## Log
- 2026-06-02 — EPIC cas-2f29 created (epic branch auto-made); 5 tasks filed; 4 isolated workers spawned + assigned; #6 strategic discussion opened.
- 2026-06-02 — Workers booted idle and self-selected before honoring assignments; noble-jay-2 grabbed cas-5be8. Swapped cas-5be8↔cas-f97d (noble-jay-2 keeps cas-5be8, golden-pelican-12 takes cas-f97d spike) to avoid lock churn. Re-assigned all 4 on the teammate channel. Op note: SendMessage to teammates auto-routes through Cassy coordination — use `mcp__cas__coordination action=message` directly.
- 2026-06-02 — cas-f97d CLOSED, verdict NOT IMPACTED (factory bypassPermissions short-circuits the 160 sensitive-file check). golden-pelican-12 now idle.
- 2026-06-02 — User greenlit the #6 proof spike for the idle worker. Created cas-2efa (standalone) and assigned golden-pelican-12: prototype cas-code-review dispatch as a Workflow script + measure tokens/latency/determinism vs current path. Evidence feeds the #6 fork.
- 2026-06-02 — cas-5be8, cas-f9ad, cas-ae09 all CLOSED verified (≈26 new tests across the three). 4/5 cc160 tasks done; only cas-b39a remains. sturdy-fox-40 + noble-jay-2 idle → standby; integration batched until cas-b39a lands. Slack two-thread convention reconfirmed by user (already in feedback_slack_release_post_template).
- 2026-06-02 — cas-b39a CLOSED (a883e58). All 5 cc160 tasks done. Integrated all 3 branches by SHA into epic (HEAD 1607e37) — zero merge conflicts. Full `cargo test`: 2021 pass, 1 fail = pre-existing `test_configure_merges_existing` flake (green 3/3 single-threaded; parallel-only race on real ~/.claude/settings.json). noble-jay-2 had self-selected + fixed this exact flake as cas-1888 (176383d) while idle.
- 2026-06-02 — #6 spike cas-2efa CLOSED: verdict HYBRID (~−54% est. cost; Opus-in-loop→0; deterministic JS merge; Cassy integration stays in skill wrapper). Artifacts at 7464045. Brought both decisions (flake fold-in + ship cc160; #6 fork direction) to user.
