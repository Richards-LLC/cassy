# Changelog

All notable changes to CAS are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [2.33.0] - 2026-07-28

### Fixed
- **A busy Codex worker is no longer reported as stalled.** `worker_status` resolved a worker's transcript through a Claude-only path, so for Codex it always came back empty — the activity clock froze at the last CAS call and in-flight suppression never engaged. A worker running shell commands continuously read as dead, and the documented response to that is to kill it. `worker_status`, `worker_activity` and `cas factory is-wedged` now share one harness-aware resolution, and a read-only `codex exec` shell-out creating a second rollout in the same directory no longer makes that resolution ambiguous. Codex workers also report a context band again.
- **Messages to workers actually arrive.** The prompt queue could re-select the same undeliverable batch indefinitely — 513 stranded rows, the oldest four months old, re-scanned roughly nine times a second — blocking every later message behind them. Undeliverable rows now become terminal under a bounded retry, one stuck target cannot hold up delivery to a live one, and retry budgets are measured from the first real attempt so a long wait before a worker registers no longer consumes them. Delivery to an idle worker, including urgent interrupts, is verified against the worker actually starting a turn rather than against a transport acknowledgement.
- **Restarting a session no longer discards queued work.** The queue's cleanup pass ran on the daemon's first tick with an empty roster, irreversibly abandoning pending messages for workers that were about to be respawned — most likely to fire on exactly the restart that installs a new build. It now waits for a populated roster and counts registered agents, not only attached panes.
- **Reusing a worker name after shutdown works.** A shutdown left a permanent tombstone on the name, so any later spawn reusing it was built and silently discarded, leaving the operator with a success message and no workers. Cancellation is now scoped to the specific in-flight spawn, logged at warning level, and cleans up the worktree it created.
- **The merge check no longer passes work it cannot see.** It keyed on a branch derived from whoever was assigned, so a branch reused across two groups of work could strand an unrelated one; keying instead on each task's own recorded commit then treated "no record" as "verified", which silently passed anything lacking a receipt. It now falls back to inspecting the live branch when no record exists, records the commit that was actually created rather than whatever HEAD points at afterwards, and invalidates that record when work is reopened.
- **Workers start from the right branch.** An isolated worker for a newly-created group of work branched from trunk instead of that group's branch, silently producing a worktree without the code the task referenced. It now branches correctly and reports loudly when a base mismatch is detected.
- **Skill reference docs reach downstream projects.** Only skill bodies synced; their `references/*.md` never did, leaving projects on whatever reference docs they were first initialized with. A managed skill body now owns its references directory, with local edits preserved and reported rather than overwritten.
- **Lease history records release reasons in their own field** instead of the transfer-attribution column, with existing rows still readable.
- **Codex commit hooks are configured.** Codex supports `PostToolUse` hooks; CAS generated them only for Claude. Hook config is now written for Codex too, surfaced for the review Codex requires rather than bypassing its trust boundary.
- **A family of parallel-run test flakes is gone.** Four separate environment-isolation helpers across two different locks were collapsed into one guard, and real-PTY tests serialize across test binaries via a file lock.

## [2.28.5] - 2026-07-22

### Fixed
- **Code review no longer silently discards reviewer findings.** The deterministic merge dropped any persona finding under its confidence threshold with no trace — a P1 that mattered was lost this way and only recovered by reading raw workflow journals. The merge now returns every rejected finding in a `dropped[]` list with reviewer provenance and the exact reason (schema errors or confidence vs threshold), logs each drop, and counts them in `stats.dropped_findings`. The codex adapter is contractually required to emit schema-complete findings, and parity tests lock the standalone, embedded, and shipped copies of the merge logic together.
- **Supervisors closing their own epics are no longer told the epic was "orphaned".** A healthy owner-closed epic now reports "epic verification: owner-closed; child tasks individually verified" in both the close response and the audit row; the orphan-recovery wording is reserved for actual orphans.
- **Workers no longer fire stale merge requests that cross with supervisor replies.** The close-rejection guidance and worker skills now tell workers to re-read just-delivered supervisor messages before escalating (the previously suggested `queue_poll` cannot see supervisor replies), and every escalation carries the current branch tip SHA plus a freshness qualifier so a stale request is self-identifying on sight.
- **The factory MCP integration tests no longer cascade-fail under parallel `cargo test`.** The env-var test lock is poison-tolerant (one failing test no longer poisons every later test), one test acquired its guard after env-sensitive setup and is fixed, and the worker skill documents the single canonical 8-variable env sanitization recipe for full-suite gates.
- **Codemap freshness no longer counts files committed together with CODEMAP.md as drift.** Staleness detection uses a strict commit-range comparison instead of timestamps, so the status line stops reporting phantom staleness after every codemap update — with positive-path coverage proving real drift is still detected.
- **Releasing a started task returns it to the ready pool.** `task action=release` used to drop only the lease, stranding the task as in-progress with no worker; it now resets status to open, clears the assignee, and records an audit note.
- **The tmpfs guardrail no longer warns on routine test runs.** Transient write-then-delete churn (parallel test temp dirs) tripped the staged-artifact warning three times per full test suite; growth must now persist across two samples before warning, while genuinely staged large artifacts still trigger.
- **Director nudges stopped racing the supervisor.** WorkerIdle "assign work" nudges are suppressed when the supervisor has contacted that worker since it went idle, delivery-time revalidation closes the assignment race, and a queued shutdown request can no longer sit unconsumed behind a slow worker spawn (no more zombie workers after shutdown-all).

## [2.28.4] - 2026-07-22

### Added
- **Large writes to memory-backed mounts now trigger a loud warning.** An agent staged 17GB of audio into a 32GB tmpfs `/tmp` over two weeks — swap saturated to 100%, the operator's apps were OOM-killed for days, and the only copies sat one reboot from loss. A new warning-only PostToolUse guardrail tracks per-session writes and usage growth on every tmpfs/ramfs mount (flocked state, single-shot fills detected on first sample, all memory-backed mounts enumerated) and tells the agent where to stage instead. Gated off the hot path: non-Write/Edit/Bash tool calls pay zero config or mount I/O.
- **Per-host staging convention.** `[staging] large_artifact_dir` in `~/.cas/config.toml` (project config wins; only the staging section is host-scoped — operator-level hooks/telemetry/llm settings can never leak into project config). When set, supervisors and workers get a one-line SessionStart notice and the guardrail names the directory in its warning. Settable via `cas config set staging.large_artifact_dir`.
- **Host-scoped memories.** Global memories tagged `host:<hostname>` now inject into SessionStart context for every project on that machine (query-layer filtered, size-capped under the SessionStart budgets). Machine facts like "this host's /tmp is tmpfs" no longer get trapped in the project where they were learned.

### Fixed
- **Task-close lint findings now name the right file and the right line.** The close-gate structural lint reported global diff indices (so multi-file diffs pointed at the wrong line), merged separate comment blocks across files and hunks into false "commented-out code" violations, and pinned findings to a single commit so follow-up fixes could never clear them. Findings are now file-qualified with per-file line numbers, comment runs reset at file and hunk boundaries, XML block doc-headers pass, and the lint evaluates the branch tip — a fix commit clears the finding.

## [2.28.3] - 2026-07-22

### Fixed
- **Factory agents can no longer wedge themselves with `AskUserQuestion`.** In factory topology the tool has no human UI surface — a supervisor calling it (as the built-in skills actively suggested for human-directed questions) got a permission prompt apparently sent to itself and paused the whole session until a human rejected it. The PreToolUse hook now denies `AskUserQuestion` for factory supervisors and workers with role-tailored guidance: ask the human in plain text and end the turn (the director relays replies); reach teammates via `coordination action=message`. The deny works even when no CAS root resolves.
- **The intercept actually fires now: `AskUserQuestion` was missing from every PreToolUse hook matcher.** Both the default settings matcher and the factory per-role settings matcher omitted the tool, so the previous advisory reminder had been dead code in real sessions. Both matchers now include it via an intercept-only list that deliberately stays out of `permissions.allow`, with regression tests preventing matcher/handler drift. Regenerate harness settings (`cas update`) to activate.
- **Skill guidance no longer steers agents into the trap.** The supervisor hard rules, intake reference, and the brainstorm/ideate skills (which mandated `AskUserQuestion` for blocking questions) now carry the factory-mode plain-text rule across all three harness variants (Claude, Codex, Grok).

## [2.28.2] - 2026-07-22

### Fixed
- **The full parallel test gate is green again.** Six `supervisor_push` lifecycle tests raced with env-mutating tests in other modules (a module-local mutex can't guard a process-wide env var), poisoning a shared lock and failing every default-parallelism `cargo test` run. All `CAS_FACTORY_SESSION`-mutating tests now serialize on the process-wide poison-tolerant env lock with panic-safe restore — verified with 5 consecutive green parallel runs. Red gates mean real failures again.
- **Supervisor rubric consistency pass.** Every copyable spawn recipe across the Claude/Codex/Grok supervisor rubrics now specifies explicit `cli`/`model`/`effort` per the GPT-5.6 Sol tier matrix, the harness `reference.md` twins are normalized (including live-worker transfer lifecycle guidance), workflow message examples include every required argument, and a guard test keeps these invariants from drifting.

## [2.28.1] - 2026-07-22

### Fixed
- **`message_status` no longer contradicts itself on pre-telemetry messages.** Rows delivered before the lifecycle columns existed reported `legacy_status: Delivered` alongside `stage: enqueued` / `pending_reason: awaiting_delivery`, forcing audits back to raw logs. A one-time migration backfill hydrates `highest_stage`/`transport_delivered_at` from `processed_at` — gated to the column-creation moment only, so live legacy paths (`queue poll`/`ack`) can never be silently promoted to a fabricated "delivered" later.
- **Lease history records the real release reason.** `release_lease_for_task` hardcoded "Task closed" for every release, so a MERGE-REQUIRED rejected close was indistinguishable from a genuine close. The reason is now threaded through the `AgentStore` trait and all call sites (awaiting-merge park, verification timeout, supervisor-review queue, reset, force-transfer, worker shutdown, preassign abort, wedged recovery, actual close).
- **Workers posting task notes are no longer flagged stalled.** `task action=notes` now emits a `TaskNoteAdded` activity event with the caller's session (non-fatal if the event store fails), and the director's stall detector counts it as worker activity — steady note-writers no longer trip false "stalled, consider interrupting" alerts.

## [2.25.0] - 2026-06-30

### Changed
- **Heterogeneous Claude + Codex factories now run mixed-harness workers reliably end to end (cas-3cb7).** A factory with one Codex worker and one Claude worker previously drifted in several places — assignment, status surfaces, director messages, and the verification/close path. These are now consistent across both harnesses (details under Fixed).
- **The Nuxt + Playwright skill no longer auto-pulls workers into browser E2E during normal dev or verification (cas-e0d1).** Its description advertised proactive triggers ("Trigger when editing files under tests/…", "when investigating Playwright test failures…"), so the model invoked it as a matter of course — doubling dev/verification wall-clock. The description is now explicit opt-in: invoke ONLY when the operator explicitly asks for Playwright/E2E help. Playwright stays fully available locally on demand (the MCP server config is unchanged); it's just no longer a default. Both the Claude and Codex skill mirrors are updated byte-identically.

### Fixed
- **Director assignment hints now name the worker, so assigning by the suggested target actually moves the task off the ready list (cas-dbbb).** The director surfaced raw session IDs as assignment targets, but assigning by ID left tasks stuck in Ready — only the worker's display name worked. Hints now use display names.
- **`worker_status` shows worktree, branch, and git detail for Codex workers, matching Claude (cas-4491).** The Clone/git block was printed for Claude workers but silently omitted for Codex workers even when the worktree existed.
- **The director no longer emits stale idle or close guidance after a task is already assigned or closed (cas-6aaf).** Status messages are now state-aware instead of telling a supervisor to reassign work that's in flight or close a task that's already done.
- **Codex workers hitting the verification gate get guidance they can actually run (cas-8aaf, cas-1b80).** The jail message handed Codex workers a `Task(subagent_type=…)` subagent flow that doesn't exist for them; it now points at `mcp__cs__coordination`, matching the Codex tool surface.
- **Codex-supervisor factories resolve the correct verification alias (cas-1544, cas-7998).** `CAS_FACTORY_SUPERVISOR_CLI` is now injected into the Codex supervisor `cs` MCP env, so close/verify guidance suggests `mcp__cs__verification` instead of a `mcp__cas__` alias a Codex supervisor can't call; the remaining hardcoded alias sites in close guidance were swept and free-text close reasons are quote-escaped so they can't break a suggested command.
- **Codex worker recovery docs use the `mcp__cs__` alias (cas-5b4f).** The built-in Codex recovery guide hardcoded `mcp__cas__` instructions that are unreachable for a Codex worker; a guardrail test now keeps the Claude and Codex copies from drifting.
- **`cas update --user` now prunes legacy non-managed `cas-*` skill orphans at the user level (cas-e0d1).** The project-level sync already drops stale `cas-*` skill dirs that lack a `managed_by: cas` marker, but the user-level path (`sync_user_builtins`) only wrote builtins and never pruned — so the retired `cas-playwright-debug` skill lingered in `~/.claude/skills` and `~/.codex/skills` on every host. The user-level sync now mirrors the project-level guard (remove only `cas-*` dirs that are neither a known builtin nor `managed_by: cas`), so the orphan is removed on the next `cas update --user`.

## [2.24.3] - 2026-06-30

### Fixed
- **Pasting multi-line text into a factory pane no longer submits the first line and queues the rest (cas-5702).** The client coalesces a paste into one event (the terminal strips the bracketed-paste markers), but it was forwarding the raw bytes to the pane, so the daemon's input parser walked them one at a time and every embedded newline reached the inner CLI as an Enter key — submitting mid-paste and dropping the remainder into the prompt queue. Paste is now carried as a single control event and re-wrapped as a bracketed paste before injection (mirroring the image-drop path), so the whole block — including any embedded newlines or control bytes — lands as one literal multi-line input.

## [2.24.2] - 2026-06-30

### Fixed
- **Codex factory no longer panics at INIT with "there is no reactor running" (cas-e202).** Starting a factory on the `codex` profile crashed the supervisor before any agent came up: `Pty::spawn` is a synchronous constructor, but its codex-only branch used `tokio::spawn` to drive the startup cursor-position (DSR) keep-alive, which panics when called from the factory daemon's runtime-free spawn thread. The keep-alive now runs on a detached `std::thread` with `blocking_lock`, mirroring the reader loop that already locks the same Mutex off-runtime — zero Tokio-runtime dependency. The Claude path was never affected (it has no `tokio::spawn`).

## [2.24.1] - 2026-06-26

### Fixed
- **`task start` no longer jails on a merge-gated sibling task (cas-6a99).** In a supervisor-deferred-merge workflow, a worker who finished task A and hit the worktree-merge gate on close (work done, awaiting the supervisor's merge) was blocked from `task start`-ing an unrelated/bundled task B — the verification-pending guard treated *awaiting-merge* the same as *actively-verifying*. `check_pending_verification` now skips tasks flagged `pending_worktree_merge` (the worker can't resolve a merge gate); the verification jail (no approved verification) still blocks, covered by a negative control in the new regression test.

## [2.24.0] - 2026-06-26

Factory-reliability sprint (multi-worker EPIC). Director coordinator hardening,
provider ergonomics, factory spec config, and cross-cutting sync/skill fixes.

### Added (this sprint)
- **Provider ergonomics — `cas claude` / `cas codex` shortcuts, `cas default <provider>`, and `--default` (cas-7f2c).** Detailed entries below.
- **`--worker-spec` / `--supervisor-spec` JSON flags + `[[factory.workers]]` / `[factory.supervisor]` TOML cascade (cas-1948).** Per-worker and per-supervisor spec config for factory spawns.

### Fixed (this sprint)
- **Director coordinator no longer fabricates "completed" notices, mis-keys assignees by name, or idle-spams (cas-889d).** Root cause: the session filter compared display names against session-id-keyed assignees, dropping every in-progress task and firing false completion events each tick. Now gates completion on real task state, resolves session ids for nudges, and suppresses nudges for workers that already hold an active task.
- **Supervisor/lead can never be nudged as an idle worker (cas-c790).** Two-layer guard in the event detector and the prompt generator.
- **Epic + worker worktrees base off the configured trunk, not the supervisor's incidental HEAD (cas-dc28).** Warns and surfaces the chosen base SHA when HEAD diverges from trunk.
- **Personal projects are no longer auto-promoted to team scope on push (cas-f8e3).** User-level team auto-pick now requires explicit opt-in; projects with no team link stay personal.
- **cas-core sync emits the `disallowed-tools` block in `generate_skill_md` (cas-e2e2).** Skills with a tool blocklist no longer drop it when synced via cas-core.
- **`filing-cas-bugs` + codex `code-review-queue` registered in BUILTIN_SKILLS (cas-61af).** `cas update` no longer silently skips syncing those referenced skill files.
- **Role-based effort defaults removed from the spawn layer; Effort threaded through PtyConfig (cas-34f7f).**

### Tests (this sprint)
- **MCP server worktree → parent-repo `.cas/` resolution coverage (cas-9db0).**
- **Non-feature-gated verification-jail regression tests — Agent-tool task-verifier bypass + factory-worker exemption (cas-c496).**

### Added

- **`cas claude` / `cas codex` provider shortcuts (cas-7f2c).** Launch a
  factory with a specific supervisor provider without remembering the
  `--supervisor-cli` flag.  `cas claude` is equivalent to
  `cas factory --supervisor-cli=claude`; `cas codex` is symmetric.  All
  existing `cas factory` flags pass through.

- **`cas default <provider>` — persist supervisor harness without launching
  (cas-7f2c).** `cas default codex` writes `[llm.supervisor] harness =
  "codex"` to `~/.cas/config.toml` and prints a one-line confirmation.
  Other config keys are preserved.

- **`--default` flag on shortcut commands (cas-7f2c).** `cas codex --default`
  both launches the factory with Codex as supervisor AND persists that choice
  for future sessions.  `cas claude --default` is symmetric.

### Fixed

- **`--supervisor-cli=claude` (or `cas claude`) no longer silently ignored
  when a codex default is persisted (cas-7f2c).** The old config-override
  block in `factory::execute` used `supervisor_cli == "claude"` as a proxy
  for "not explicitly set by the user", so an explicit `--supervisor-cli=claude`
  was indistinguishable from the default and was overridden by a persisted
  codex config value.  A new `supervisor_cli_explicit` flag on `FactoryArgs`
  fixes the precedence: explicit shortcut/flag > persisted config > built-in
  default.

### Added (continued)

- **Auto-detection of team scope on login — `cas cloud team set` no longer required for most users (EPIC cas-ab88).** CAS now fetches your team membership from `/api/me` (petra-stella-cloud) immediately after `cas login` and caches `teams[]` + `default_team_id` into `~/.cas/cloud.json`. The resolution chain in `active_team_id()` then picks the right team automatically: project-level explicit override → user `default_team_id` → implicit single-team auto-pick → personal scope. Single-team users need only `cas login` + `cas cloud sync`; no manual UUID or slug lookup required.

- **`cas cloud team default <slug-or-uuid>` subcommand (cas-6804).** Sets a user-wide team default in `~/.cas/cloud.json`. Takes a team slug (e.g. `petra-stella`) or UUID; resolves against the cached `teams[]` populated at login. Use `--personal` to revert to personal scope (clears the default). This is the recommended first-time setup step for multi-team users; single-team users typically don't need it.

- **`cas cloud team set` repositioned as advanced / per-project override (cas-6b8b).** The subcommand still works and is the right tool for per-project overrides that should differ from the user-wide default (e.g. a contractor working across multiple teams). It is no longer the primary onboarding path; `cas login` + `cas cloud team default` is.

- **First-run backfill notice on upgrade (cas-8f23).** When a user upgrades into the new auto-scope world and logs in for the first time with `teams[]` populated, CAS prints a one-time notice describing the auto-detected team and inviting them to run `cas cloud team default --personal` to opt out. The gate is `team_backfill_notified: bool` in `~/.cas/cloud.json`; it is set once and never re-fires.

### Changed

- **`teams[]` and `default_team_id` added to user-level `~/.cas/cloud.json` (cas-6462).** New `TeamInfo { id, slug, name, role }` struct. Fields use `#[serde(default)]` + `skip_serializing_if` so existing `cloud.json` files deserialise cleanly without migration.

- **`active_team_id()` resolution chain extended to read user-level config (cas-ea2f5).** Priority order: (0) kill-switch `team_auto_promote = false` → always `None`; (1) project-level `team_id` if set; (2) user `default_team_id`; (3) sole team auto-pick when `teams.len() == 1`; (4) `None` (ambiguous or no membership). The `active_team_id_with_user_config(user_cfg)` testable inner keeps the chain exercisable without disk I/O.

## [2.21.0] - 2026-06-23

Coordinated release: cloud-sync reliability (EPIC cas-f75f) + the team ticket
explorer **client half** (EPIC cas-71f7). The cloud half shipped separately
(petra-stella-cloud EPIC cas-9133).

### Fixed

- **Cloud-sync reliability — slug fragmentation, queue poison-head stall, duplicate-enqueue, silent re-homing (EPIC cas-f75f).** (A) `cas cloud team show` reports the concrete resolved slug and warns on bucket ambiguity instead of silently syncing an empty bucket. (B) A single un-pushable queue item is parked as `failed` with a reason instead of head-of-line-blocking the whole queue. (C) Legacy `NULL` `team_id` queue rows are normalized so one task mutation enqueues one item (no permanent residue). (D) `cas cloud push` no longer re-homes existing cloud entities to a changed slug without the explicit `--rehome` flag, and prints truthful per-type insert/update counts.

### Added

- **Team ticket explorer — CLI client half (EPIC cas-71f7).** Three behaviors that keep the CLI in sync with the web ticket explorer (petra-stella-cloud). See `docs/team-ticket-explorer-client.md`.
  - **Canonical project-id adoption on push (cas-8ca5).** `cas cloud push` (team scope) sends your normalized git remote; when the server's returned `git_remote` matches your local `origin`, the returned `canonical_id` is adopted into `.cas/config.toml`. Stops an unpinned machine from syncing a fragmented per-remote bucket. Equality-gated so a shared machine with a different remote is never silently re-homed.
  - **Web-initiated close reconcile on pull (cas-fc52).** A teammate's close from the web UI (`closed_via = "web"` tombstone) is reconciled as an authoritative local close — applied even if the local copy is newer, with `close_reason` preserved and `assignee` cleared. Merges only the close signal so locally-authored unpushed content is not clobbered. Idempotent; never reconciles the client's own pushed closes.
  - **Read-only mirror of web-authored comments (cas-7d54).** `task show` surfaces comments authored in the web explorer (author, timestamp, body, image/video/link attachments), fetched per task. Best-effort: degrades to nothing when not logged in / offline; never blocks or fails `task show`.

## [2.20.0] - 2026-06-07

### Fixed

- **Isolated factory workers no longer leak commits onto the supervisor's branch (EPIC cas-073f).** Workers spawned with `isolate=true` could commit to the supervisor's shared checkout (`main`/`epic`) instead of their own worktree. Root cause: the worktree-reuse path in `WorkerSpawnPrep::run` checked `path.exists()` but not that the directory was a real git worktree on the expected branch — a stale dir made git walk up to the main checkout's `.git`, so `HEAD` resolved to the supervisor's branch and every commit landed there (deterministic, not the race the report hypothesized). The reuse path now validates the branch and hard-errors on mismatch; `isolate=true` fails loudly instead of silently degrading to the shared checkout, and a post-spawn assertion verifies each worker sits on `factory/<name>`.

### Added

- **Defense-in-depth worker commit guards.** Three layers, all on a `factory/<name>` *allowlist* — a worker may only commit on its own branch; `main`/`master`/`staging`/`epic/*`/any other branch and detached HEAD are denied: a PreToolUse intercept on `git commit`/`merge`, an installed git pre-commit hook (the bulletproof floor for non-tool commits), and a SessionStart cwd/branch assertion.
- **`coordination action=worker_status` git introspection.** Reports per worker: branch, worktree path, HEAD sha, ahead/behind vs base, dirty/clean, last pushed ref, and open PR URL — worker "done" is verifiable without git forensics.
- **`task close` gated on merge reality.** Refuses (or routes to pending-merge) when no commit is reachable from the worker's `factory/<name>` branch and no PR exists, without blocking additive-only / zero-commit closes.
- **Worker-stop git-state event + PreCompact findings flush.** On worker stop the final git state is emitted to the supervisor feed; on context compaction, in-flight findings are extracted from the transcript and written to the worker's active task so they survive the compaction.
- **Truthful worktree status.** `worktree_list` / `worktree_status` report live factory (isolation) worktrees instead of the misleading "experimental and disabled" message.

## [2.16.1] - 2026-05-14

### Fixed

- **Hook emitters reverted to shell-form to silence Claude Code 2.1.139's `/doctor` validator (cas-c17b).** Claude Code 2.1.139 introduced an exec-form hook shape `{ "type": "command", "args": [...] }` that the runtime accepts but the `/doctor` schema validator rejects with `Expected string, but received undefined`. The warning fires *before the agent loads* in every spawned worker pane, forcing manual dismissal on every factory worker spawn — significant friction in factory mode with 4+ workers per EPIC. CAS migrated to exec-form in cas-7ecd for the no-shell-parsing safety property; the property was theoretical (`cas hook <Event>` takes zero user-controlled args, payload flows via stdin), so the revert costs nothing functional. All 14 emitter sites flipped: 12 in `cas-cli/src/cli/hook/config_gen.rs` (every event: SessionStart, SessionEnd, Stop, SubagentStart, SubagentStop, PostToolUse, PreToolUse, UserPromptSubmit, PermissionRequest, Notification, PreCompact, plus the `cas factory check-staleness` SessionStart entry) and 2 in `cas-cli/src/ui/factory/daemon/runtime/teams.rs::factory_hooks_block()` (the factory's `supervisor-settings.json` / `worker-settings.json` PreToolUse + PermissionRequest). `duplicate_check::has_cas_hook_entries()` keeps recognizing both forms so existing pre-cas-c17b user settings are still detected as CAS-installed. Upstream tracker: [anthropics/claude-code#58441](https://github.com/anthropics/claude-code/issues/58441) — once the /doctor validator is fixed in claude-code, we can re-evaluate.

- **Hook-emission tests cover all 11 events + check-staleness (cas-aee5).** Follow-up to cas-c17b. The original revert's tests only iterated over 6 of the 11 emitted events; a partial regression leaving SubagentStart, SubagentStop, PermissionRequest, Notification, or PreCompact in exec-form would have shipped undetected. `hook_entries_emit_shell_form_command_string` and `hook_entries_do_not_emit_exec_form_args` now iterate all 11. A new `session_start_check_staleness_emits_shell_form` test reaches the second SessionStart hook entry (which `first_hook_command` cannot — added a `nth_hook_command` helper) and asserts the `cas factory check-staleness` invocation is also shell-form. `test_exec_form_still_detected_by_has_cas_hook_entries` fixture upgraded to include `matcher` + `timeout` fields so the dual-form detection contract tests against realistic pre-cas-c17b settings shape.

### Changed

- **New installs default to `https://petra-stella-cloud.vercel.app` instead of upstream `https://cas.dev` (cas-9cbd).** The `pippenz/cas` fork operates its own cloud; the hardcoded upstream default was leaking into every new install of this fork. Default flipped at four code sites: `default_endpoint()` in `cas-cli/src/cloud/config.rs`, the `LoginArgs.endpoint` clap default in `cas-cli/src/cli/auth.rs`, the `LoginArgs::default()` impl, and the corresponding test pin. `cas-cli/src/ui/factory/daemon/cloud_client.rs` doc comment updated. Existing users with `https://cas.dev` already in their `~/.cas/cloud.json` are NOT auto-migrated — re-run `cas auth login` (no args) to opt in to the new default. The serde compat test at `cas-cli/src/ui/factory/daemon/runtime/cloud.rs:309` deliberately keeps `cas.dev` in its JSON literal (it pins roundtrip semantics, not the default value).

- **`CAS_CLOUD_ENDPOINT` env var now actually overrides the endpoint (cas-9cbd).** Previously the env var was set by `scripts/provision-hetzner.sh:195/:312` and by user shell-rcs with the apparent intent of overriding the endpoint, but production code never read it — only the e2e test harness in `cas-cli/tests/e2e/team_sync.rs` referenced it, and those assertions passed via a "not configured" fallback OR clause. `default_endpoint()` now reads `CAS_CLOUD_ENDPOINT` first (non-empty, trimmed) before falling back to the hardcoded URL; the clap `LoginArgs.endpoint` arg gets `env = "CAS_CLOUD_ENDPOINT"` mirroring the existing `CAS_CLOUD_TOKEN` pattern at `auth.rs:31`. `LoginArgs::default()` now delegates to `default_endpoint()` so programmatic callers also honor the env var. Hetzner provisioning's existing `export CAS_CLOUD_ENDPOINT=https://petra-stella-cloud.vercel.app` now works without script changes.

### Security

- **URL scheme validation on `CAS_CLOUD_ENDPOINT` and `--endpoint` (cas-9cbd).** New `is_acceptable_endpoint()` helper accepts `https://*` and `http://localhost` / `http://127.0.0.1` / `http://0.0.0.0` only. Rejects `file://`, plain hostnames, arbitrary `http://`, and anything else that could redirect the device-code token exchange to an attacker-controlled server. The localhost carveout is required by `cas-cli/tests/e2e/team_sync.rs` (wiremock on `http://127.0.0.1:<port>`). Validation behavior is asymmetric by surface: `default_endpoint()` soft-fails (invalid value → `tracing::warn!` + fallback to hardcoded default, preserves the infallible `Default` contract), `LoginArgs.endpoint` clap value_parser hard-fails (`Error: must be https:// or http://localhost`). Whitespace-only env values also fall back (via `.trim().is_empty()` filter).

### Tests

- **Test race condition introduced by env-var wiring fixed (cas-9cbd follow-up).** When `default_endpoint()` started reading `CAS_CLOUD_ENDPOINT`, every test that constructs `CloudConfig::default()` (directly or via `..Default::default()`) became a potential race victim against the 3 new env-var tests. `CLOUD_ENV_LOCK` was moved from a tests-module-local mutex to a `#[cfg(test)] pub(crate) static` at module scope and is now acquired by all 11 sibling tests in `cas-cli/src/cloud/config.rs` (`test_default_config`, `test_save_and_load`, `test_logout`, `test_set_and_clear_team`, four `test_active_team_id_*`, etc.) with `unwrap_or_else(|p| p.into_inner())` for poison recovery. Re-exported through `cas-cli/src/cloud/mod.rs` so the 5 new `auth.rs` tests share the same mutex.

### Upgrade notes

- **In-flight factory sessions retain the old exec-form `supervisor-settings.json` / `worker-settings.json` until you run `cas factory --new`.** The CAS daemon writes those files eagerly at spawn time, not at binary-upgrade time. Worker panes in your current session will keep getting the `/doctor` warning until you restart the factory. New sessions get clean shell-form settings automatically.
- **Existing logged-in users are not auto-redirected to the Petra Stella cloud.** If your `~/.cas/cloud.json` has `endpoint: "https://cas.dev"`, you keep talking to upstream's cloud. Run `cas auth login` (no `--endpoint`) to pick up the new default; or pass `--endpoint https://your-server` to override; or set `CAS_CLOUD_ENDPOINT` in your shell.
- **Provisioning scripts that set `CAS_CLOUD_ENDPOINT` now actually take effect.** If you have any inherited setup expecting that variable to be ignored, verify the new endpoint is the one you want — the variable will now route token exchange and cloud sync to whatever you set.

## [2.16.0] - 2026-05-13

### Changed

- **Stock worker LLM default flipped to Claude Sonnet 4.6 + `reasoning_effort=high` (cas-05e3).** Previously, workers spawned without an explicit `[llm.worker]` block in `.cas/config.toml` (the common case for new installs and most upgraders) fell through to the harness builtin — whatever Claude Code happened to pick by default. New behavior: `LlmConfig::model_for_role("worker")` and `reasoning_effort_for_role("worker")` apply a stock floor when both the role-specific override (`[llm.worker.X]`) and the top-level fallback (`[llm.X]`) are unset. Three-step chain becomes: role-override → top-level → stock-worker-default (`claude-sonnet-4-6` for model, `high` for reasoning effort). Supervisor role is deliberately untouched — `supervisor_does_not_receive_worker_stock_default` regression-locks that boundary. Existing users who explicitly set top-level `[llm] model = "X"` expecting all roles to inherit still see workers resolve to `X` (back-compat hinge). Runtime-only fallback: no changes to `cas init` or `cas update` config seeding, which means updating the stock constant in cas-src automatically propagates the new default to every install without requiring users to re-init or hand-edit. To pin a different worker model, add `[llm.worker] model = "..."` to `.cas/config.toml`. Constants `STOCK_WORKER_MODEL` and `STOCK_WORKER_REASONING_EFFORT` are now public from `cas-cli/src/config/settings.rs` for downstream consumers. 6 new tests + 1 split of the existing `reasoning_effort_for_role_no_config_returns_none` cover the full resolution matrix.

## [2.15.3] - 2026-05-13

### Fixed

- **`cas cloud team set <uuid>` now eagerly resolves the project canonical slug (cas-1ced, EPIC cas-ffc4 closes).** Final task in the EPIC opened against the original cloud-team bug doc — closes hypothesis #3, the last UX paper-cut from daniel.l's onboarding. Previously, `cas cloud team set` printed `Slug resolution deferred — see cas cloud team show` and didn't actually resolve the canonical project ID. When the working-directory name didn't match the canonical slug (daniel.l cloned the repo into `~/cas` while the canonical project ID was `cas-src`), his first `cas cloud sync` went out with `project_id=cas` and routed push/pull to a phantom project; the documented workaround was renaming the directory. Fix: after persisting the team_id, the handler now runs an eager resolution chain — `.cas/config.toml [project] canonical_id` → `git -C <root> remote get-url origin` (normalized via a new `normalize_git_remote_url` helper handling HTTPS, HTTP, `ssh://git@host/`, and the `git@host:owner/repo` shorthand, with `.git` suffix stripping) → defer. The deferred case explicitly does NOT fall back to the working-dir basename (that was the bug). When a slug is resolved, it's written to `.cas/config.toml [project]` and surfaces in subsequent `get_project_canonical_id()` calls. Output indicates the source (`from .cas/config.toml` / `derived from git remote` / deferred); JSON mode carries the same info as `canonical_id_source`. New `cas cloud project set <canonical-id>` subcommand for manual override (monorepo / non-git / custom layout). `cas cloud team show` now displays the resolved project slug alongside the team UUID. Config plumbing adds a `[project]` section to the `Config` struct (`ProjectConfig { canonical_id: Option<String> }`) wired through `merge_missing` + `init.rs`; `resolve_canonical_id` precedence becomes `config.toml → folder-name → path-hash`, backward-compatible. 17 new tests (6 integration in `team_set_slug_resolution_test.rs` covering config-preserve / HTTPS derive / SSH derive / no-basename-default negative / project set / team show; 11 unit in `cloud::config::tests` covering URL normalization shape table + config.toml round-trip + section-preserve + resolution precedence).

### Cross-team coordination

- **EPIC cas-ffc4 closes end-to-end.** Original bug doc moved to `docs/requests/completed/`. The three hypotheses surfaced in the cloud team's filing are all addressed: (#1) missing endpoint wire-up shipped in v2.15.1; (#2) cross-project watermark reuse shipped in v2.15.2; (#3) deferred slug resolution shipped here in v2.15.3. New team-member onboarding now lands the correct project slug at `cas cloud team set`, hits the team endpoint on `cas cloud sync`, and keeps `since=` watermarks scoped per-(team, project).

## [2.15.2] - 2026-05-13

### Fixed

- **`cas doctor --fix` no longer fails with `no such table: skills` on bootstrap-pending DBs (cas-bdb9, EPIC cas-9fdb).** Surfaced on the ozer-health project (macOS): `cas doctor --fix` / `cas update --schema-only` on a fresh `.cas/cas.db` that had never had `SqliteSkillStore::new()` / `SqliteAgentStore::new()` (or any other lazy-bootstrap store) constructed in-process exploded with `migration failed: skills_add_summary - database error: no such table: skills`. Root cause: the `skills` (and `agents`) tables are created lazily by `CREATE TABLE IF NOT EXISTS` inside the store constructors, but the migration runner did not invoke those constructors before running ALTER migrations like `m071_skills_add_summary`. Fix promotes the lazy-bootstrap schema constants (`SKILL_SCHEMA`, `AGENT_SCHEMA`, `TASK_SCHEMA`, `ENTITY_SCHEMA`, `VERIFICATION_SCHEMA`, `LOOP_SCHEMA`, renamed `ENTRIES_RULES_SCHEMA`, plus extracted `WORKTREE_SCHEMA` and `CODE_SCHEMA` for symmetry) to `pub` and adds `Subsystem::ensure_base_schema(&conn)` + `ensure_base_schemas(&conn)` in `cas-cli/src/migration/mod.rs`, wired into `run_migrations` between `ensure_migrations_table` and `bootstrap_migrations`. Sentinel-gated per subsystem — if the canonical table already exists the bootstrap skips that subsystem and leaves the migration chain authoritative, preventing legacy partial-state DBs from being touched. Subsystems bootstrapped: Entries / Tasks / Skills / Agents / Entities / Verification / Loops. Subsystems with explicit `m###_*_create_table` migrations (Worktrees / Code / Events / Recording / Recordings) are DELIBERATELY EXCLUDED — pre-installing the post-ALTER shape would break later ALTERs (e.g., m112 expects `worktrees.task_id` which m120 renames to `epic_id`). The exclusion list and its rationale are spelled out in the `WORKTREE_SCHEMA` / `CODE_SCHEMA` doc comments. Includes the `task_leases` dual-definition cleanup: `TASK_SCHEMA` previously defined `task_leases` with `renewed_at TEXT` (nullable, no FK) AND `AGENT_SCHEMA` defined it with `renewed_at TEXT NOT NULL` + `FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE CASCADE` — `Subsystem::Tasks` iterating before `Subsystem::Agents` meant the slim version always won on fresh bootstrap, silently losing the NOT-NULL + FK. `AGENT_SCHEMA` is now the single source of truth (lifecycle owns lease semantics). Plus housekeeping: lifted the `idx_entries_helpful_score` expression index into `ENTRIES_RULES_SCHEMA` (was a best-effort `let _ =` in `store_init`); removed the duplicate sessions DDL from `store_init` now that `ENTRIES_RULES_SCHEMA` covers it.

- **`cas cloud sync` no longer reuses stale watermarks across projects within the same team (cas-53d5, EPIC cas-ffc4).** `CloudSyncer::pull_team` previously keyed its `since=` watermark globally per team (`last_team_pull_at_{team_id}`). A user working on team T across two projects P1 and P2 would full-backfill P1, then switch to P2 and have the second pull silently skip historical T+P2 backfill — surfacing as the same "0 of everything" symptom that v2.15.1's cas-6ec7 fixed at the endpoint-routing level (hypothesis #2 from the cloud team's bug doc, the next failure mode lying in wait). Fix re-keys the watermark to `last_team_pull_at_{team_id}_{project_id}`. Absence of the new key is treated as "first sync into this scope" — no `since=` is sent, triggering a full backfill. Best-effort cleanup retires legacy global-per-team keys on first successful per-scope write. `pull_team` now takes `project_id: &str` as an explicit parameter (rather than internal resolution via the process-wide `get_project_canonical_id()` cache); the cached static would otherwise lock all in-binary tests to a single project_id, making the cross-project regression test impossible. `cas cloud pull --full` now scopes its watermark clear to the current `(team, project)` pair only. 3 callers updated (`execute_team_pull`, the `worktree_verification_team_ops` MCP helper, the `team_memories_e2e_test` fixture). 4 new tests in `cas-cli/tests/team_pull_watermark_scope_test.rs` covering: cross-project full backfill (second project sends no `since=`), same-scope incremental (second pull sends the recorded `since=`), `--full` scope isolation (P1 cleared, P2 intact), and key-format lock.

### Cross-team coordination

- **EPIC cas-ffc4 remains OPEN — sibling task cas-1ced still pending.** Eager project-slug resolution at `cas cloud team set` (closes hypothesis #3 from the bug doc, fixes the case where the cloned working-dir name doesn't match the canonical project slug and the first sync goes out with the wrong `project_id`) is queued and will ship as a follow-on patch.

## [2.15.1] - 2026-05-13

### Fixed

- **`cas cloud sync` now actually pulls team data for newly-onboarded team members (cas-6ec7, EPIC cas-ffc4).** Filed by the cloud team as P1 (`docs/requests/BUG-cloud-sync-pull-returns-zero-for-new-team-member.md`): a new team member walking through `cas-login` → `cas cloud team set <uuid>` → `cas cloud sync` would see `0 of every entity type` synced despite thousands of team-scoped rows existing for the active project on the cloud side. Push was correctly hitting the team endpoint; pull was hitting only the personal endpoint (`/api/sync/pull`, filtered by `team_id IS NULL`), so a team-only member legitimately got nothing back. Root cause was a missing call site: `CloudSyncer::pull_team` (the team-pull helper at `cas-cli/src/cloud/syncer/pull.rs:688`) was fully built and tested but only invoked by one MCP worker-verification helper and the e2e tests — never from `cas cloud sync` or `cas cloud pull`. Fix wires a new `execute_team_pull` helper into both `execute_pull` (and transitively, `execute_sync`), symmetric to the existing `execute_team_push` (cli/cloud.rs:1313): same isolation contract (errors never propagate), same `report_team_pull_{result,partial,error}` reporter trio, same JSON output shape. `cas cloud pull --full` also clears the per-team `last_team_pull_at_<team_id>` watermark when a team is configured so team backfill happens on `--full` just like personal does. Behavioral wiremock tests in the new `cas-cli/tests/team_pull_wiring_test.rs` (7 tests, `.expect(1)` on both endpoints in the positive case + `.expect(0)` on the team endpoint in the no-team negative case) lock the contract — including the double-call regression guard caught by multi-persona code review.

### Cross-team coordination

- **Companion follow-on tasks remain open under EPIC cas-ffc4.** `cas-53d5` (re-key team-pull watermark to be per-(team_id, project_canonical_id) so cross-project sync from the same team doesn't silently skip historical backfill) and `cas-1ced` (eager project-slug resolution at `cas cloud team set` so a working-dir name that doesn't match the canonical slug stops causing the `project_id=cas` instead of `project_id=cas-src` misroute) are the next two failure modes the bug doc surfaced. Both will ship as separate patches.

## [2.15.0] - 2026-05-12

### Changed

- **`cas cloud pull` now always sends `?project_id=<canonical>` (cas-ed15, EPIC cas-2eb3).** The `cas cloud pull` CLI handler previously built its URL inline via raw `ureq::get` and never appended `project_id=`, bypassing the scoped `CloudSyncer::pull` abstraction that `cas cloud sync` and `cas cloud purge-foreign` already used. The leak returned `team_id IS NULL` rows from all of a user's projects on every pull, contaminating local DBs with foreign-project data. The fix replaces the inline builder with a `CloudSyncer::pull` construction — same scoped abstraction, hard-fails when `get_project_canonical_id()` returns `None`, gates every store import behind `entity_matches_project`. Three regression tests in `cas-cli/tests/pull_scoping_regression_test.rs` (source-level scan + file-level check + wiremock URL assertion) lock the contract. Empirical wire trace from cas-src confirms `GET /api/sync/pull?since=…&project_id=cas-src` post-fix.

- **`CloudSyncer::pull` extended to all 9 entity kinds, properly scoped (cas-bba4, EPIC cas-2eb3).** cas-ed15 fixed the pull leak by routing through `CloudSyncer::pull`, but that abstraction only covered entries / tasks / rules / skills — `cas cloud pull` previously imported specs / events / prompts / file_changes / commit_links from the inline path *unscoped* (the leak). Removing them in cas-ed15 was strictly better than the leak, but `cas cloud pull` returned zero counts for those kinds. This change extends `CloudSyncer::pull` to handle all 9 kinds with the same `entity_matches_project` scoping the original 4 used. Wire trace from cas-src confirms `cas cloud pull --full` now imports the missing entity kinds (9595 events on the test pull) properly scoped. Forward-compatible: `body.specs.unwrap_or_default()` lets older cloud builds (which don't return `specs` yet) deserialize cleanly. Companion cross-team request `docs/requests/FEATURE-cloud-sync-pull-return-specs.md` filed asking cloud to extend the `/api/sync/pull` response.

- **Cloud push client detects and surfaces server-side skipped rows (cas-f645, EPIC cas-2eb3).** `CloudSyncer::push_sub_batch` now parses the response body into a `PushResponse` carrying an optional `skipped: HashMap<String, usize>` per entity type. When the server reports a non-zero skip count for an entity type (the signal Postgres emits when `ON CONFLICT DO UPDATE … WHERE false` silently excludes a cross-project conflict), the client emits a `tracing::warn!` and leaves the entire sub-batch un-marked-synced so items remain retryable in the local queue. Backward-compatible: every field is `#[serde(default)]` so older cloud builds that omit `skipped` deserialize cleanly and fall back to the legacy mark-synced path. Six tests (4 unit + 2 wiremock integration) pin both paths.

- **`cas update --sync` now surfaces silent-skip warnings for stale unmanaged files (cas-4900).** The `sync_builtin` gate previously collapsed two distinct outcomes — "no-op happy path" and "stale source/dest both lack `managed_by: cas`" — into the same `Ok(false)` return. The latter case silently left projects with stale reference files for unknown durations. New `SyncOutcome` enum distinguishes `Created` / `Updated` / `Unchanged` / `SkippedNotManaged`. `SyncResult::skipped_files` is now populated on the silent-skip path, and `cas update --sync` prints a yellow `! <path>` list under the existing "Built-ins" reporting block with a one-line nudge to add the `managed_by: cas` marker. Pre-existing silently-failing class of refresh failures is now loud and debuggable.

### Performance

- **SIMD `memchr` fast-path on the alt-screen scanner (cas-219d).** `Pane::update_alt_screen` previously walked the input byte-by-byte looking for the ESC (`0x1b`) byte that starts a CSI escape sequence. On bulk non-CSI text (the steady state during normal terminal output) that's ~1 cycle per byte. Outer loop now seeks ESC via `memchr::memchr(0x1b, ..)` (SIMD-accelerated to ~16 bytes per cycle on x86_64). Criterion bench in `crates/cas-mux/benches/alt_screen_scan.rs` measures the impact: 64 KiB ESC-free chunk in ~546 ns (~117 GB/s SIMD throughput); sparse-ESC 64 KiB chunk (1 ESC per 200 B) in ~1.76 µs; dense-match 4 KiB chunk in ~1.47 µs. Strict optimization — every loop exit either breaks (memchr None) or strictly advances `i`; observationally identical to the byte-by-byte scan on every input shape (empty, lone ESC, ESC at end, ESC followed by non-`[`, split sequences across feed calls). Regression test `update_alt_screen_esc_free_64k_preserves_state` pins the no-ESC / no-state-change invariant.

### Added

- **`verify-before-claim` pre-close skill (cas-5b2a, EPIC cas-ebea).** New `.claude/skills/verify-before-claim/SKILL.md` (+ Codex mirror) — a four-step agent-discipline protocol that kills the "narrate done before proving it" failure mode. Steps: (1) name the proof command, (2) run it FRESH, (3) capture exit code + tail output, (4) only then call `mcp__cas__task action=close`. CAS already has the mechanical layer (`verification_store` + close-gate's six checks); this skill is the agent-discipline layer on top. Trigger: any time an agent is about to assert tests pass, the build is clean, the script works, the bug is fixed, or the AC is satisfied. Advisory in v1 — required-paste enforcement is a clean follow-up if telemetry shows the advisory form under-performing. Registered in both `BUILTIN_SKILLS` and `CODEX_BUILTIN_SKILLS`; cas-worker SKILL.md wires it into step 6 of the close routine. Five install-path tests cover presence, frontmatter, four-step markers, registration, and cas-worker cross-reference. Confirmed live: SessionStart's available-skills list now picks it up immediately from the destination `.claude/skills/` without a cas-side daemon restart.

- **"Context budgeting" methodology section in `cas-supervisor` + `cas-worker` skills (cas-5787, EPIC cas-ebea).** New section in both skill bodies (Claude + Codex × supervisor + worker = 4 files, plus 4 destination mirrors) naming the three context layers — Immutable Core / Task Context / Ephemeral — citing the 12 KB SessionStart cap enforced by `test_*_guidance_under_12kb`, cross-linking `project_session_start_truncation.md`, and closing with the decision rule "Adding here? Only if every session needs it; else `references/<name>.md`". Regression test `test_skills_document_context_budgeting_cas_5787` asserts seven required markers across all four bundle-relevant files so silent drift via `cas update --sync` becomes a compile failure. `supervisor_guidance()` bundle goes from 11,898 → 12,277 bytes (11-byte headroom under the 12,288 cap) — tight but deliberate, since the new section literally documents the cap that constrains it.

- **`session-learn` skill: 7-signal session classifier (cas-39f5, EPIC cas-ebea, v1 skill-only).** New `.claude/skills/session-learn/SKILL.md` (+ Codex mirror) borrowed from `third-brain-v5-skills` and adapted to the CAS memory schema. Documents the 7-signal taxonomy (concept / entity / correction / pattern / idea / decision / gap) with each signal mapped to a concrete CAS `entry_type` + tags + scope. Available for manual invocation today ("extract this session", "save what we learned"). New `[memory] session_learn_auto = false` opt-in flag in `.cas/config.toml` reserves the auto-trigger contract; the Stop-hook auto-fire implementation is tracked under sibling task `cas-6156`.

### Fixed

- **Factory worker `task.close` no longer hits `VERIFICATION_JAIL_BLOCKED` under owner=supervisor (cas-8edb).** Regression introduced by v2.13.0's `[code_review] owner = "supervisor"` default flip: workers stopped submitting `ReviewOutcome` envelopes at close (because review now runs at supervisor cherry-pick time), but the v2.12.0 self-cert path required that envelope to bypass the jail. Symptom: every factory worker close was rejected with `VERIFICATION_JAIL_BLOCKED: Mutating operation task.close blocked. Task <id> requires verification before any mutations are allowed.`, forcing supervisor close-on-behalf with `bypass_code_review=true` on every task. Fix: two surgical changes gated on `is_factory_worker && code_review.supervisor_owned()` — `cas-cli/src/mcp/server/mod.rs::authorize_agent_action` exempts workers from the jail on `task.close` when owner=supervisor; `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs::cas_task_close` computes `worker_under_supervisor_review` early and skips the verification gate when true. Supervisor-driven paths untouched (`is_factory_worker=false`). Legacy `owner = "worker"` untouched (`supervisor_owned()=false`). Three new regression tests pin the contract: zero-diff worker close self-certs, additive-only worker close self-certs, legacy `owner=worker` still jails clean close without envelope. Post-mortem in `docs/requests/completed/BUG-cas-8edb-verification-jail-regression-on-supervisor-owned-review.md`.

- **`update_alt_screen` correctly handles CSI sub-params + resets `in_alt_screen` on pane exit (cas-e0b9).** Two distinct bugs in the alt-screen state machine, both fixed characterization-first (failing tests committed before the fix so the bugs are pinned in history). (1) CSI sub-params: the parser didn't handle `\x1b[?1049;1h` style colon- or semicolon-separated sub-parameters per ECMA-48 §5.4.2 — split mid-subparam input flipped state unpredictably, and unknown modes inside the sub-param list could spuriously flip `in_alt_screen`. After the first parameter's digit run, the scanner now consumes the full `[0-9;:]` run before checking the final byte; leading mode controls the toggle (xterm semantics), sub-params are read but not interpreted, truncated-mid-subparam skips safely. `trailing_dec_partial` widened to carry partial sub-param sequences across chunk boundaries. (2) Pane exit: when a pane process exited while `in_alt_screen=true`, the flag was never reset, leaving the next process (or terminal redraw) confused about whether the alt-screen was active. `mark_exited` is now a `pub fn` lifecycle API that clears `in_alt_screen` and drops `partial_esc` while preserving the `PtyEvent::Error` path's existing "preserve previously-set exit_code" semantics; `poll` / `drain_output` route through it. Regression coverage adds multi-param chain (`?1049;1;2:3h`), truncated mid-subparam (no panic / no spurious flip), and unknown-mode (`?25;1h` must not flip alt-screen).

- **`test_alt_screen_scroll_is_noop` now asserts the actual scroll contract (cas-a368).** Empty `is_err()` branch was silently passing — the test exercised `Pane::scroll` on an alt-screen pane and asserted nothing meaningful. Empirical probe showed `Pane::scroll` on alt-screen returns `Ok(())` and silently no-ops (not `Err` as the stale docstring claimed — that text was carried over from an earlier ghostty revision). Test now asserts `result.is_ok()` with a helpful failure message, keeps the existing viewport-offset equality check, and rewrites the docstring to match reality plus explain why the UI must forward wheel events to the PTY on alt-screen (host has no scrollback to give). Companion test additions in cas-72c3 pin the daemon's wheel-dispatch decision table and the exact byte shape of `SCROLL_UP_ARROWS` / `SCROLL_DOWN_ARROWS` (previously only length was asserted; a typo in the byte sequence would have silently broken wheel-to-PTY forwarding).

- **`cas-code-review` SKILL.md frontmatter no longer tells workers to autofire pre-close (cas-ec8f).** Under the v2.13.0+ default `[code_review] owner = "supervisor"`, the supervisor invokes `cas-code-review` at cherry-pick + EPIC-merge time — workers must not self-dispatch personas at `task.close`. The stale description framed `autofix` at `task.close` as "the primary path" and called this skill "the pre-close quality gate for CAS factory workers", causing workers to burn ~100K input tokens per close dispatching 4–8 reviewer personas inline. New description leads with supervisor invocation; demotes `mode=autofix` to opt-in for projects pinning `owner = "worker"`. Two regression tests pin the description contract (substring assertions on forbidden phrases + supervisor mention) and lock byte-identity between the `.claude` and `.codex` mirrors. Amendment commit also unsticks `test_cas_worker_skill_documents_code_review_gate`, which had been silently failing on main since commits 8b82273 and 167c57e (cas-8962 / cas-5815 supervisor-default flip) — replaces five stale inline-block markers with the post-flip ownership contract.

- **`FactoryApp::for_test()` documents its ~10 non-obvious fields (cas-11b0).** Expanded the constructor docstring from 3 lines to a structured field-handling note covering `Mux::new` vs `Mux::factory`, the `DirectorEventDetector.initialize` sequence, the `director_stores=None` / `worktree_manager=None` contracts, the `cas_dir`/`project_dir` placeholder warning, and the terminal-cols/rows-Mux-sync pitfall. Adds a canary clause: any new field on `FactoryApp` must also be added here, otherwise the test constructor fails to compile.

### Cross-team coordination

- **Future cloud-side enforcement of `project_id` on `/api/sync/pull` (cas-990b).** Filed `petra-stella-cloud/docs/requests/FEATURE-mandatory-project-id-on-pull.md` asking the cloud team to mirror the existing `MIN_CLIENT_VERSION` + mandatory-`project_canonical_id` gate from `app/api/sync/push/route.ts:29-57` onto both pull endpoints (`/api/sync/pull` and `/api/teams/[teamId]/sync/pull`). With this binary onwards, every `cas cloud pull` call carries `project_id=` on the wire, so the cas-side fix is the prerequisite for the cloud-side enforcement flip. **No breaking change in this binary**: a future cas-cli release will tighten the contract once the cloud-side gate is live and the `MIN_CLIENT_VERSION` constant has rolled forward past unsafe binaries. Users on this binary onwards will not be affected by the flip; users on earlier binaries will receive a clear `400` instead of silent cross-project data leakage. Defense-in-depth complement to `cas-ed15`: cas-side fix prevents *new* contamination on the wire; cloud-side enforcement guarantees that any *future* parallel pull builder regression becomes loud rather than silent.

- **Cloud `/api/sync/pull` should return specs / events / prompts / file_changes / commit_links (cas-bba4 follow-up).** Filed `docs/requests/FEATURE-cloud-sync-pull-return-specs.md` asking cloud to extend the pull response payload to include the entity-kind arrays cas-cli now consumes. cas-side ships forward-compatible (`unwrap_or_default()` on each new field), so this lands independently from the cas-cli rollout.

## [2.14.0] - 2026-05-12

### Added

#### Claude Code 2.1.122–2.1.139 changelog integration (EPIC cas-871f)

Track upstream Claude Code as it ships features and breaking changes that touch CAS surfaces. Six items shipped this release.

- **`CLAUDE_PROJECT_DIR` for `cas serve` MCP stdio project resolution (cas-7cc3, Claude Code 2.1.139).** Claude Code 2.1.139 passes `CLAUDE_PROJECT_DIR` into stdio MCP server environments. `cas-cli/src/mcp/server/runtime.rs::resolve_mcp_serve_root()` now reads it first, falling back to existing `CAS_ROOT` / cwd-walk detection when unset or invalid. Error message names `CLAUDE_PROJECT_DIR` when it points at an uninitialised directory so the user knows which path to `cas init`. Debug-level tracing logs the chosen resolution branch. 4 unit tests cover happy path, fallback on invalid path, fallback when unset, and explicit-error-mentioning-env-var on uninitialised dir; RAII `EnvGuard` ensures panic-safe env restoration. Documented in `cas-cli/docs/ARCHITECTURE.md`.

- **Hook configs converted to exec-form `args` arrays (cas-7ecd, Claude Code 2.1.139).** All 12 CAS-emitted hook entries across 10 hook types (SessionStart, SessionEnd, Stop, SubagentStart, SubagentStop, PostToolUse, PreToolUse, UserPromptSubmit, PermissionRequest, Notification, PreCompact) plus factory check-staleness now emit `"args": ["cas", "hook", "<Event>"]` instead of shell-string `"command": "cas hook <Event>"`. Eliminates path-quoting bugs when the cas binary lives at a path with spaces or shell metacharacters. `has_cas_hook_entries()` + `strip_cas_hooks()` accept BOTH the new exec form AND the legacy command form so existing user `settings.json` continues to be detected and stripped correctly on `cas init` re-run. Fallow gate hook retains shell-form (requires `$CLAUDE_PROJECT_DIR` expansion that exec form doesn't support); HTML comment in `fallow/references/patterns.md` documents the retention. 3 hook-emission test guards added (`hook_entries_emit_exec_form_args_array`, `hook_entries_no_longer_emit_command_string_form`, plus an updated `test_configure_creates_settings` fixture).

### Documentation

#### Two spike brainstorms filed for forward-looking Claude Code architecture decisions

- **`continueOnBlock` for cas-code-review autofix (cas-8655, Claude Code 2.1.139).** Spike concluded: not applicable. CAS PostToolUse hook is `async: true` with `matcher: "Write|Edit|Bash"` — it neither blocks nor matches `mcp__cas__task`. Code review runs entirely inline in the MCP `task.close` handler, so the Claude Code 2.1.139 `continueOnBlock` hook field is architecturally mismatched. Section 7 of the brainstorm flags `continueOnBlock` as potentially useful for the *PreToolUse* hook path (filesystem-write blocks, dangerous Bash) as a separate future investigation. Brainstorm at `docs/brainstorms/2026-05-12-continue-on-block-code-review-spike.md`.

- **OTEL trace propagation post-Claude Code 2.1.128 (cas-8ad7).** Claude Code 2.1.128 stopped subprocesses inheriting `OTEL_*` env vars. Spike concluded: zero impact on CAS. No `opentelemetry` crate in any workspace `Cargo.toml`; `otel.rs::OtelContext` write side fires at SessionStart but the read side is unimplemented in production; CAS emits no spans. Section 6 of the brainstorm documents forward-looking guidance for when CAS does wire OTEL export: read resource attributes from `otel_context.json` via `get_resource_attributes()`, do NOT fall back to `OTEL_RESOURCE_ATTRIBUTES` env var (CC 2.1.128 strip would break that path). Brainstorm at `docs/brainstorms/2026-05-12-otel-propagation-verification.md`.

#### `CLAUDE_CODE_PACKAGE_MANAGER_AUTO_UPDATE` for Homebrew users (cas-03c6, Claude Code 2.1.129)

README Homebrew section now points Homebrew users at Claude Code 2.1.129's `CLAUDE_CODE_PACKAGE_MANAGER_AUTO_UPDATE=1` env var for background Claude Code self-upgrades, with an explicit "this is for Claude Code only — not CAS; CAS updates via `cas update`" disclaimer to prevent the readability hazard.

#### `skillOverrides` escape hatch for CAS builtin skills (cas-2f3f, Claude Code 2.1.129)

README Claude Code Integration section documents Claude Code 2.1.129's `skillOverrides` setting as the way to hide / collapse specific CAS builtin skills without disabling CAS entirely. Three-mode table (`off` / `user-invocable-only` / `name-only`) + JSON example with real CAS skill names.

### Added (also in this release)

#### `cas update --user` — distribute built-ins to user-level (~/.claude, ~/.codex)

`cas update --sync` only writes to the current project's `.claude/.codex`. Worker worktrees that don't ship `.claude/skills/` in tracked git state (the gabber-studio case) fall back to user-level skills, so a stale `~/.claude/skills/cas-worker/SKILL.md` silently kept workers running the old multi-persona pipeline at close even after `cas-update` re-synced every project.

`cas update --user` mirrors `--sync` for built-ins only — calls `sync_all_builtins_for_harness(Claude, ~/.claude)` (and `Codex, ~/.codex` if the dir exists) without touching project-scoped config (settings.json, CLAUDE.md, hooks, db-backed rules/skills). The `cas-update` wrapper now invokes it on every run so user-level skills track binary version.

## [2.13.0] - 2026-05-05

### Changed

#### Default code-review ownership flipped from `worker` to `supervisor` (EPIC cas-cac3 / cas-b51a Stage 2+3)

**The default `[code_review] owner` is now `"supervisor"`.** Projects with no `[code_review]` block in `.cas/config.toml` now use supervisor-owned review by default — no opt-in required.

- **Workers run only the lightweight structural lint at close (<1s).** The multi-persona review pipeline is no longer invoked inline at `task.close` by default. Tasks transition to `pending_supervisor_review` after a clean lint pass; workers are immediately free to pick up the next task.
- **Supervisor runs `/cas-code-review mode=interactive` at cherry-pick time (per-task) and at EPIC→base merge (integration sweep).** See `cas-supervisor/references/workflow.md` Phase 3 step 5 and Phase 4 step 3 for the exact invocation sequence.
- **Pin to legacy behavior** with `[code_review] owner = "worker"` in `.cas/config.toml`. This restores the original inline dispatch (~14 min per close) for teams that want it.
- **`close_ops.rs` absent-section fix (cas-865b):** `.unwrap_or(false)` at the runtime close gate replaced with `.unwrap_or_else(|| CodeReviewConfig::default().supervisor_owned())` so projects with no `[code_review]` block track the config-layer default instead of being hardcoded to worker mode.
- **Skill prose updated:** `cas-worker` workflow (steps re-numbered), `cas-supervisor` workflow (cherry-pick and integration review steps added), `cas-code-review` SKILL.md (ownership table, mode reference, purpose section all reflect new default).

## [2.12.0] - 2026-05-04

### Added

#### Per-worker CLI/model/effort overrides — heterogeneous factory teams (EPIC cas-b3db)

Supervisors can now spawn workers on different AI harnesses within a single factory session. A Claude supervisor can coordinate a Codex worker (or vice versa) without restarting the daemon.

- **`mcp__cas__coordination action=spawn_workers cli=codex`** — new `cli`, `model`, and `effort` fields on the `spawn_workers` coordination action route per-spawn harness overrides through the full stack: MCP → spawn-queue (m201 migration adds `worker_spec` column) → cloud handler → daemon protocol → `finish_worker_spawn`.
- **`cas factory --worker-spec '{"cli":"codex","name":"alice"}'`** — new `--worker-spec` CLI flag resolves and persists per-worker specs at daemon boot; `WorkerSpec::codex_default(name)` constructor added.
- **`MuxConfig.resolved_worker_specs`** — `Mux` struct replaces the three scalar `worker_cli/model/effort` fields with `default_worker_spec: WorkerSpec` + `worker_specs: HashMap<String, WorkerSpec>`. `factory_pane_configs` and `add_worker` use per-worker spec lookup with fallback chain (explicit > map > default).
- **Live re-resolution at spawn time** — `sync_worker_config_from_live_settings()` called at `finish_worker_spawn` and `respawn_worker` re-reads the live `LlmConfig` from disk so `cas config set llm.worker.harness codex` takes effect without daemon restart.
- **Codex effort arg wired** — `PtyConfig::codex` now emits `-c model_reasoning_effort=<level>` when effort is `Some`; previously silently dropped.
- **Heterogeneous spawn smoke test** (`cas-5570`) — `heterogeneous_spawn` integration test in `crates/cas-mux/tests/` confirms Claude-supervisor-spawns-Codex-worker roundtrip. Supervisor skill docs updated with `cli`/`model`/`effort` parameter table and heterogeneous-team example in both `.claude` and `.codex` mirrors.

#### Supervisor-owned code-review pipeline (cas-b51a)

Moves the expensive multi-persona `cas-code-review` skill dispatch from the worker's close path to the supervisor, cutting the per-close latency cost.

- **`[code_review] owner = "worker" | "supervisor"` config knob** — new `CodeReviewConfig` section in `config.toml`. Default is `"worker"` (Stage 1 backwards-compat; Stage 2 flip is a follow-on).
- **`PendingSupervisorReview` task status** — new status value between `InProgress` and `Closed`. When `owner = "supervisor"`, a worker close that passes the lightweight lint gate transitions the task to `PendingSupervisorReview` instead of triggering `CODE_REVIEW_REQUIRED`. Worker is immediately free to pick up the next task.
- **Lightweight structural lint gate** — fast (<1s) pre-close check run by the worker on the raw diff before handing off to the supervisor. Catches `unimplemented!()`, `todo!()`, `dbg!()`, and >5-consecutive-line commented-out blocks. Lint failure returns a structured error naming the violation; the task stays `InProgress`.
- **5 integration tests** in `supervisor_review_flow.rs` covering: supervisor-mode skips `CODE_REVIEW_REQUIRED`, worker-mode unchanged, `PendingSupervisorReview` SQLite round-trip, supervisor verification on pending task, config default is `"worker"`.
- **Supervisor skill docs** (`cas-supervisor.md`, `code-review-queue.md`) updated with queue-management workflow and lint-fail response guidance.

#### Verification jail self-cert (cas-778a / cas-4c64 / cas-164c)

Clean `ReviewOutcome` envelopes now self-certify the worker close path. Workers no longer need to forward to the supervisor when `VERIFICATION_JAIL_BLOCKED` fires on a clean close — the system detects a valid envelope and clears the gate automatically. The old forwarding dance only applies on pre-2.12.0 binaries.

### Changed

#### dbg!() lint tightened + lint-fail integration test (cas-adf0 + cas-b5ac)

- **`contains("dbg!(")` replaces three-part OR** — the lightweight lint's `dbg!` check previously missed `=dbg!(...)` and `let x=dbg!(...)` (no space before `dbg`). Replaced with a single `contains("dbg!(")` that catches all forms regardless of preceding whitespace.
- **4 new unit tests** covering bare, with-space, no-space-after-equals, and embedded forms.
- **Integration test for lint-fail close path** (`test_lint_fail_close_blocked_before_pending_supervisor_review`) — asserts `is_error=true`, error names the offending lint rule, and task remains `InProgress` (no `PendingSupervisorReview` transition on lint failure).

## [2.11.0] - 2026-05-01

### Added

#### Factory close-merge enforcement (EPIC cas-754b)

Closes the silent data-loss vector where `task action=close bypass_code_review=true` could mark tasks Closed without verifying the worker's `factory/<assignee>` branch was merged into the parent epic. Field evidence from gabber-studio cas-6e07 (2026-05-01): 7 stranded tasks, ~21 commits, ~3000 LOC nearly disappeared. Second occurrence in 48h.

- **Per-task close-merge gate (cas-95ce):** `mcp__cas__task action=close` on a non-epic task now rejects when `factory/<assignee>` has commits not on the parent epic. Bypass-immune at the type level (the helper signature does not consume a bypass flag) and at the physical level (gate runs structurally upstream of `bypass_code_review` evaluation). Error names the stranded commit count, factory branch, parent epic branch, and remediation.
- **Epic-close gate (cas-8f8f):** `mcp__cas__task action=close` on an Epic-type task walks every child's factory branch and rejects when any child is stranded. Same bypass-immunity. Caught a P1 critical in autofix: the original `unwrap_or_default()` on a SQLite-backed lookup would have failed open and defeated the entire enforcement. Now propagates as `INTERNAL_ERROR`.
- **`mcp__cas__coordination action=epic_status id=<epic-id>` diagnostic (cas-8f8f):** new callable surface returning a markdown table per child task (assignee | factory branch | unmerged count | last commit | task ID + status). Useful for in-flight audits before attempting epic close.
- **`cas-supervisor-checklist` skill update (cas-8f8f):** "Before Closing an EPIC" section now references `epic_status` as the canonical check and notes that the gate is automatic (defense-in-depth, no longer manual-only).

### Changed

- **`mcp__cas__verification action=add` authz error (cas-a90f3):** the misleading "Supervisors can only verify epics, not individual tasks" rejection has been replaced with a message that names the actual rule (active-assignee-based) and lists the three exemptions (orphaned / inactive assignee / supervisor IS the assignee). Error embeds the offending assignee ID, gives concrete remediation (`mcp__cas__task action=release`), and clarifies that epics remain always supervisor-verifiable. Predicate renamed `assignee_inactive` → `assignee_inactive_or_absent` to make `unwrap_or(true)` semantics self-documenting (logic unchanged).

### Operator guidance

After upgrading, the new gates fire on `task.close` calls. If a worker hits the gate during close, the supervisor must merge `factory/<assignee>` into the parent epic before the close will succeed (this is the desired ordering and matches how the other workflow guidance now reads). For pre-existing stranded factory branches (e.g. gabber-studio cas-6e07), salvage with: `git checkout <epic-branch> && git merge --no-ff factory/<worker>`.

## [2.10.1] - 2026-04-29

### Changed

- **Shared proxy transport (cas-36fd0):** new `cli/integrate/proxy.rs`
  module exposes `ProxyClient` with the proxy lifecycle (`proxy_config_path`,
  `call`, `block_on`, `unwrap_envelope`). Both `ProxyVercelClient` and
  `LiveNeonClient` are now thin wrappers — ~165 LOC of duplicated boilerplate
  retired. Future `Live<X>Client` implementations inherit the wiring.
- Speculative neon parser tolerance shapes (`orgs/data` alias, flat
  `describe_project`) removed until proven against real envelopes; bail
  messages cite cas-36fd0 and request bug filing on real upstream drift.
- `default_database` "neondb" silent fallback → explicit bail with
  provisioning recovery hint.

## [2.10.0] - 2026-04-29

### Added

#### Vercel/Neon/GitHub Auto-Integration (EPIC cas-b65f)
- `cas integrate <vercel|neon|github> [init|refresh|verify]` standalone subcommands.
  - **Vercel**: detects `vercel.json` / `@vercel/*` deps, fuzzy-matches via
    `mcp__vercel__list_projects`, captures team + project + env→branch mapping.
  - **Neon**: detects Prisma + `@neondatabase/*` / `@prisma/adapter-neon`, prompts
    for org when multiple exist, captures `org_id` + `projectId` + `databaseName` +
    branches via `mcp__neon__{list_organizations,list_projects,describe_project,describe_branch}`.
  - **GitHub**: parses `git remote -v` (https + ssh forms), records `owner/repo`.
- `cas init` runs platform detection and prompts Y/N per detected platform,
  delegating to the corresponding `cas integrate <platform> init` in-process.
  Idempotent on re-run: existing populated SKILL.md flips the prompt to
  "Refresh? [y/N]" with default N.
- `--no-integrations`, `--vercel <id>`, `--neon <id>`, `--github <repo>` flags
  for non-interactive `cas init` use.
- Generated SKILL files land in **both** `.claude/skills/<name>/` and
  `.cursor/skills/<name>/` so both harnesses pick them up.
- `<!-- keep <name> -->` … `<!-- /keep <name> -->` named keep blocks preserve
  user-owned IDs across `refresh` regenerations. `--update-ids` opts into
  re-fetching IDs from the platform MCP.
- `<!-- cas:full_name=... -->` identity tag convention for canonical project
  identity inside keep blocks; sanitized to neutralize markdown injection.
- `cas doctor` audits integration freshness via per-platform `verify_report`
  and surfaces stale IDs as warnings (not errors); MCP-down reports as
  `skipped — MCP not configured` rather than failing the doctor run.
- Optional opt-in `[integrations] session_start_warn = true` in
  `.cas/config.toml` emits a low-severity SessionStart banner when integrations
  go stale. Default off — preserves the codemap banner's signal.

#### Codemap Skill (cas-4d84)
- `/codemap` skill ships in `.claude/skills/codemap/`, builtins, and codex
  variant. Generates `.claude/CODEMAP.md` and resets the freshness counter
  via `cas codemap clear` after writing. Closes the long-standing gap where
  hooks referenced a `/codemap` slash command that did not exist.

### Changed

#### Factory Skill Bundles (cas-61af)
- `cas-supervisor.md` split from 44 KB into a 6.8 KB SKILL.md + six
  references (`preflight`, `intake`, `planning`, `workflow`,
  `worker-recovery`, `reference`).
- `cas-worker.md` split from 22 KB into a 5.7 KB SKILL.md + three
  references (`close-gate`, `recovery`, `details`).
- `supervisor_guidance()` and `worker_guidance()` no longer bundle
  `cas-task-tracking`, `cas-memory-management`, or `cas-search` — those are
  autonomous skills the agent invokes via the Skill tool. Bundled payload
  dropped from ~61 KB / ~35 KB to ~10 KB / ~5.5 KB respectively.
- Test ceiling at 12 KB enforces the bundle stays small enough that the
  Claude Code harness does not truncate the SessionStart additionalContext
  to a 2 KB preview.

#### Cross-cutting Hardening (cas-fc38)
- New `cli/integrate/fs.rs` shared module: `atomic_write`,
  `atomic_write_create_dirs`, `read_capped` (4 MiB cap with symlink
  rejection), `is_regular_file`, `locate_repo_root[_from]` (with `git -C`
  discipline that resolves the inner repo on submodule / nested-worktree
  invocations).
- New `cli/integrate/md.rs` shared module: `escape_md_cell`,
  `escape_md_cell_code`, `emit_cas_full_name_tag`, `parse_cas_full_name_tag`.
- `IntegrationStatus` split: `TransportError` distinct from `Stale` so a
  failed MCP call is no longer misreported as a stale ID.
- All three platform handlers consume the shared helpers — atomic-write
  semantics, symlink defense, file-size cap, markdown escaping, and
  identity tag behave uniformly.

#### Team Memories
- `cas cloud team set|show|clear` subcommands to configure the active team
  (UUID input; slug resolution deferred pending cloud-side endpoint).
- `cas memory share <id>|--since <duration>|--all [--dry-run]` for retroactive
  backfill of pre-existing personal memories to the team push queue.
- `cas memory unshare <id>` to mark a memory `share=Private` (blocks future
  team dual-enqueue; does not retract cloud-side copies).
- `share: Option<ShareScope>` (`Private`/`Team`) persisted on Entry, Rule,
  Skill, and Task via SQLite migrations `m037`/`m060`/`m082`/`m121`.
- Automatic dual-enqueue: when a team is configured via
  `cas cloud team set`, `cas memory remember` in any Project-scoped
  non-Preference context queues the entry to both personal and team
  push queues. `cas cloud sync` drains both.
- Coarse kill-switch: `cloud.json.team_auto_promote: false` disables the
  automatic promotion without requiring the team to be cleared.
- Integration test suite: `team_sync_test.rs`, `memory_share_test.rs`,
  `team_memories_e2e_test.rs` cover the full push → pull pipeline.

### Changed

- `mcp-proxy` is now a default Cargo feature so `cas integrate vercel|neon`
  ships out of the box — the wired `ProxyVercelClient` / `LiveNeonClient`
  require it.
- `cas cloud team-memories`'s "no team configured" error now correctly
  directs users to `cas cloud team set <uuid>` (previously referenced a
  non-existent subcommand with `<slug>` argument).
- `cas cloud team set|show|clear` subcommands to configure the active team
  (UUID input; slug resolution deferred pending cloud-side endpoint).
- `cas memory share <id>|--since <duration>|--all [--dry-run]` for retroactive
  backfill of pre-existing personal memories to the team push queue.
- `cas memory unshare <id>` to mark a memory `share=Private` (blocks future
  team dual-enqueue; does not retract cloud-side copies).
- `share: Option<ShareScope>` (`Private`/`Team`) persisted on Entry, Rule,
  Skill, and Task via SQLite migrations `m037`/`m060`/`m082`/`m121`.
- Automatic dual-enqueue: when a team is configured via
  `cas cloud team set`, `cas memory remember` in any Project-scoped
  non-Preference context queues the entry to both personal and team
  push queues. `cas cloud sync` drains both.
- Coarse kill-switch: `cloud.json.team_auto_promote: false` disables the
  automatic promotion without requiring the team to be cleared.
- Integration test suite: `team_sync_test.rs`, `memory_share_test.rs`,
  `team_memories_e2e_test.rs` cover the full push → pull pipeline.

### Changed

- `cas cloud team-memories`'s "no team configured" error now correctly
  directs users to `cas cloud team set <uuid>` (previously referenced a
  non-existent subcommand with `<slug>` argument).

## [2.0.0] - 2026-04-12

### Added

#### Factory System
- Multi-agent factory with supervisor/worker architecture and isolated git worktrees.
- Director event system for task dispatch, worker lifecycle, and epic completion notifications.
- Worker startup confirmation flag to detect crash-on-startup failures.
- Orphaned task reclamation — supervisor can claim tasks from dead workers.
- Coordinator messaging system with priority levels, delivery confirmation, and outbox replay.
- Verification jail exemption for factory workers to prevent universal tool blocking.
- Worker idle/stale notification dedup and suppression.
- Minions theme with ASCII art and themed boot screen for factory workers.

#### Cloud Sync
- Bidirectional cloud sync with Petra Stella Cloud — push/pull tasks, memories, rules.
- Cloud sync queue with shutdown drain, startup push, 10s idle gate, 60s interval.
- Circuit breaker for TLS retry spam with capped event buffer.
- `cas cloud projects` and `cas cloud team-memories` commands.
- `cas cloud purge-foreign` for orphaned dependency cleanup.
- Project-scoped pull requests to prevent cross-project data leaks.

#### MCP Proxy
- `cas-mcp-proxy` crate — proxies upstream MCP servers (Playwright, Neon, GitHub, Vercel, Context7) through CAS. Workers get 2 tools instead of 50+.
- Config-aware hot-reload for proxy server connections.
- Search with keyword matching and server filtering.
- Integration tests, catalog caching, and README.

#### TUI
- Tokyo Night theme variant.
- OSC 52 clipboard copy and auto-inject on image paste.
- `cas open` interactive TUI project picker.
- Tab forwarding to PTY for autocomplete (Ctrl+P for sidecar).
- Clipboard fallback via client-side write with visual feedback.
- Mouse click to focus panes, Ctrl+Arrow pane cycling, Shift+drag text selection.
- Native terminal selection (replaces custom selection implementation).

#### Compound Engineering
- `cas-code-review` skill — multi-persona code review with 7 reviewer personas (correctness, testing, maintainability, project-standards + conditional security, performance, adversarial). Includes bounded autofix loop, confidence gates, fingerprint dedup, and review-to-task routing.
- `cas-brainstorm` and `cas-ideate` skills for structured ideation.
- `git-history-analyzer` and `issue-intelligence-analyst` agent types.
- Multi-persona review merge pipeline with cross-reviewer agreement boost.
- Pre-insert memory overlap detection with configurable threshold actions.
- Implementation Unit Template for EPIC subtask specifications.
- `execution_note` field on tasks: `test-first`, `characterization-first`, `additive-only` postures with enforcement at close.

#### Skills & Agents
- Comprehensive `cas-worker` skill with build failure triage, MCP connectivity guidance, tool selection guide, context exhaustion detection, task reassignment protocol, and section reorder for critical-path-first flow.
- Adversarial supervisor posture with intake gate, scope lock, and rejection authority.
- Partnership posture for supervisor — counter-propose, trajectory gate, situational awareness.
- `cas-supervisor` skill with EPIC sizing heuristics, worker failure recovery, and merge conflict guidance.
- `cas-memory-management` skill with multi-file schema and overlap workflow.
- `cas-search` skill with filter grammar, code symbol search, and module-scoped candidate API.
- CODEMAP system — auto-maintained breadcrumb navigation map with structural change detection hooks.

#### Infrastructure
- Hetzner CCX23 provisioning script for remote CAS server (Ashburn VA).
- Slack bridge: Bolt app scaffolding with per-user daemon architecture, SSE adapter, message formatter, file upload passthrough with security sanitization.
- `cas-install.sh` — portable curl one-liner installer.
- WebSocket endpoint for factory daemon.
- SSE plain-text pane output and tail endpoint.
- Auto-attach prompt with `--attach`/`--new` flags for existing sessions.
- `cas serve` HTTP bridge for Slack integration.

#### Store & Performance
- Sequence table for ID generation (replaces per-insert MAX+LIKE scan).
- SQLite `prepare_cached()` for all statement caching.
- Jitter on SQLite write-retry backoff to break convoy pattern.
- Recursive CTE for dependency cycle-check (replaces iterative BFS).
- Tantivy IndexWriter caching (saves 50MB per write allocation).
- BM25 search index caching and QueryParser reuse.
- Batch code symbol DB inserts in indexing daemon.
- `ImmediateTx` wrapper for atomic store operations.

### Changed

- Bumped version to 2.0.0 with simplified release workflow targeting `pippenz/cas`.
- Config format migrated from YAML to TOML (automatic merge of stale settings).
- `project_canonical_id` derived from folder name instead of git remote URL (required on all cloud pushes).
- Default cloud sync interval reduced from 300s to 60s.
- MCP tool prefix standardized to `mcp__cas__`.
- Worker skill reordered for critical-path-first flow: Task Types and Execution Posture before close procedures.
- Code review section compressed from 65 to 30 lines — pipeline internals moved to `cas-code-review` skill.
- Rules section merged into Rules of Engagement; Valid Actions merged into Schema Cheat Sheet.
- Legacy `code-reviewer` agent deprecated in favor of `cas-code-review` skill.

### Fixed

- **TUI**: Off-by-one in Ghostty VT style run column indices clipping left edge of pane content. Tab click detection using variable-width positions instead of equal-width assumption. Scroll viewport double-compensation when Ghostty preserves viewport position. Task panel flashing empty due to read race between task list and dependency queries. Dark theme contrast — `border_default`, `border_muted`, `hint_description` bumped for readability. Epic state updated before filter in `refresh_data()`.
- **Factory**: Verification jail cascade where one task's pending verification blocked all tools. `CAS_FACTORY_MODE` phantom env var — `pre_tool.rs` required it alongside `CAS_AGENT_ROLE` but no code ever set it. Director dispatching blocked/closed tasks (terminal-status guard added). Supervisor self-verification deadlock. Worktree workers missing MCP access due to gitignored `.mcp.json`/`.claude/` (fixed with symlinks). Duplicate hooks causing PreToolUse errors (`cas hook cleanup` added).
- **Cloud**: WebSocket TLS for `tokio-tungstenite`. HTTP TLS for `ureq` client. Fallback `project_id` for filesystem-root CAS projects. 403/404 error handling with pluralized labels.
- **Store**: N+1 queries in `task_store.rs`. Unbounded `IN` clauses and `LIKE` scans. 8 excessive indexes dropped to reduce write amplification. Lease races and cleanup/prune methods with transaction safety.
- **Close**: Additive-only gate now diffs worker branch commits (not main). Skip close-gate checks for non-isolated tasks. Reject close when worker tree has uncommitted work. Status-update race condition where `status=blocked` overwrites concurrent supervisor close.
- **Other**: `rustls` CryptoProvider installed at startup to prevent daemon crash. Secrets moved from provision script to `~/.config/cas/env` (push protection). GitHub auth token used in self-update to avoid API rate limits.

## [1.0.0] - 2026-03-12

### Added
- Initial open-source release of CAS.
- Factory TUI screenshot in README.
- `.env.worktree.template` for worker environment setup.

### Changed
- Release workflow updated for GitHub Actions with Homebrew auto-update.
- MCP config sync added to `cas update` flow.

### Fixed
- Migration v165 crash when `verifications` table doesn't exist.
- Release workflow secret check moved from job-level to step script.

## [0.6.2] - 2026-02-25

### Added
- Interactive terminal dialog (Ctrl+T) in factory TUI with show/hide/kill.
- MCP proxy catalog caching for SessionStart context injection.
- Billing interval switching buttons (monthly/yearly) with savings display.
- Resume subscription button on cancellation notice.
- `cas changelog` command to show release notes from GitHub releases.

### Changed
- Cloud sync on MCP startup runs in background with 5s timeout (non-blocking).
- Heartbeat uses shorter 5s timeout and spawn_blocking to avoid stalling async loop.
- Refactored cloud routes: org_billing_settings → billing_settings, org_members → members.
- Release bump workflow now requires a matching CHANGELOG.md section.

### Fixed
- Debounced Ctrl+C interrupt to prevent accidental double-sends.
- Update version check now compares versions properly.
- Stripe portal return URL redirects back to billing page instead of settings.
- Removed duplicate type export in types/index.ts.

## [0.5.7] - 2026-02-15

### Fixed
- Avoided macOS factory startup crash by using subprocess daemon mode with attach/socket retries.
- Hardened UTF-8-safe truncation behavior in touched UI/tooling paths to prevent char-boundary panics.

### Changed
- Standardized release-train crate versions to `0.5.7`.

## [0.5.6] - 2026-02-15

### Fixed
- Cleared clippy warnings under `-D warnings` across touched workspace crates.

### Changed
- Standardized release-train crate versions to `0.5.6`.
- Updated local git hook rustfmt invocation to use Rust 2024 edition.

## [0.5.5] - 2026-02-15

### Changed
- Published `0.5.5` release and synchronized release-train crate versions.

## [0.5.4] - 2026-02-15

### Changed
- Improved Supabase auth login UX and callback branding.

## [0.5.3] - 2026-02-15

### Changed
- Initial release carrying Supabase auth login UX and callback branding improvements.

## [0.5.2] - 2026-02-13

### Changed
- Bumped release-train versions to `0.5.2`.

## [0.5.1] - 2026-02-11

### Fixed
- Fixed Sentry transport panic triggered during `cas login`.

## [0.5.0] - 2026-02-11

### Fixed
- Added missing Sentry transport feature to prevent login-time crash.

## [0.4.0] - 2026-01-10

### Added
- Consolidated MCP tool format with unified naming.
- Sort and task type filtering for MCP and CLI.
- ID-based search and CLI/MCP feature parity.
- Git worktree support for task isolation.
- Schema migration system for database upgrades.
- Verification system with task-based exit blocking.
- Statusbar anchoring support.

### Changed
- Extracted `cas-core` and `cas-mcp` crates for better modularity.
- Removed `#[tool_router]` macro from CasCore for compile-time improvement.
- MCP enabled by default in `cas init --yes`.
- Removed legacy MCP mode and added `list_changed` notifications.

### Fixed
- Removed duplicate store implementations from `cas-cli`.
- Fixed scope persistence in crate extraction.
- Task verifier now uses CLI and checks project rules.

## [0.3.0]

### Added
- Initial stable release with core functionality.

[Unreleased]: https://github.com/pippenz/cas/compare/v2.12.0...HEAD
[2.13.0]: https://github.com/pippenz/cas/compare/v2.12.0...v2.13.0
[2.12.0]: https://github.com/pippenz/cas/compare/v2.11.0...v2.12.0
[2.11.0]: https://github.com/pippenz/cas/compare/v2.10.1...v2.11.0
[2.10.1]: https://github.com/pippenz/cas/compare/v2.10.0...v2.10.1
[2.10.0]: https://github.com/pippenz/cas/compare/v2.0.0...v2.10.0
[2.0.0]: https://github.com/pippenz/cas/compare/v1.0...v2.0.0
[1.0.0]: https://github.com/pippenz/cas/compare/v0.6.2...v1.0
[0.6.2]: https://github.com/pippenz/cas/compare/v0.5.7...v0.6.2
[0.5.7]: https://github.com/pippenz/cas/compare/v0.5.6...v0.5.7
[0.5.6]: https://github.com/pippenz/cas/compare/v0.5.5...v0.5.6
[0.5.5]: https://github.com/pippenz/cas/compare/v0.5.4...v0.5.5
[0.5.4]: https://github.com/pippenz/cas/compare/v0.5.3...v0.5.4
[0.5.3]: https://github.com/pippenz/cas/compare/v0.5.2...v0.5.3
[0.5.2]: https://github.com/pippenz/cas/compare/v0.5.1...v0.5.2
[0.5.1]: https://github.com/pippenz/cas/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/pippenz/cas/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/pippenz/cas/compare/v0.3.0...v0.4.0
