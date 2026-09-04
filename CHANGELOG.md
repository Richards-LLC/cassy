# Changelog

All notable changes to CAS are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- Updating no longer ends with a false alarm. After the new version installed
  and its post-install work ran correctly, the update still failed with a
  message saying the wrong version had done the work and telling you to run it
  again. It was reading the build date out of the version line and comparing
  that to the version number. It now reads the version number itself, and still
  stops the moment the work really was done by a different version.

## [3.15.5] - 2026-09-04

### Added
- The release train now publishes from the repository's own script as well:
  after the release pull request lands, one command tags the exact landed
  commit from a clean detached worktree and starts the publisher, refusing up
  front when the landed commit is missing, when the remote has moved past it,
  or when the version recorded in that commit is not the one being released.
  The publisher's log, process id and exit receipt sit beside the gate and
  pipeline receipts for the same run.
- A worker's merge request now reaches an idle supervisor's screen instead of
  waiting in an inbox for the supervisor to go looking. A finished task that was
  parked pending a merge could sit unseen for hours, because the message that
  said so was delivered silently. Only merge requests gain this — ordinary
  status chatter still waits for the supervisor's next turn, so nothing types
  over work in progress.
- A supervisor whose messages have gone unread past their delivery window is now
  told how many are waiting and who sent the oldest, instead of finding out by
  checking. Blockers, verification handoffs and status replies still arrive
  quietly, as before — what changed is that the backlog itself is announced once,
  and again only when a different message becomes the oldest one waiting.

### Changed
- Whether a message may interrupt a supervisor now depends on who actually sent
  it, as recorded when the message was written, rather than on the sender label
  attached to it. Labels can be set by whoever sends the message; the record
  cannot.
- A message that could not interrupt is now reported with which of the two
  reasons applied: it was never the kind of message that interrupts, or it was
  and the moment was wrong. Both used to read the same, so a genuinely stalled
  request looked identical to routine traffic working as intended.

### Fixed
- A message could interrupt a supervisor's screen simply by carrying another
  supervisor's name. The name was checked against the roster, but anyone sending
  a message can choose the name it carries, so spelling a real supervisor's name
  was enough to skip the content check that every other interrupting message has
  to pass. Interruptions are now decided from the sender recorded at send time.
- Messages sent to a worker now appear once instead of twice. A worker that was
  idle received the full message a second time as a separate prompt, moments
  after the first copy, with nothing to mark it as a repeat — so a worker that
  had already acted on it could be led to do the same work again. The second
  copy is now a single line telling the worker a message is waiting and naming
  it, and the message itself is delivered once.

## [3.15.4] - 2026-09-04

### Added
- `worker_status` now names the epic each live supervisor is running, including
  supervisors in other factory sessions that share the same checkout. Knowing
  another supervisor is live was only half of what an operator needs before a
  merge, reset or shutdown; the other half is what that supervisor is in the
  middle of. A supervisor running nothing reads "no epic", and a task store that
  cannot be read this pass says so explicitly rather than being reported as
  "no epic".

### Fixed
- A message from one supervisor to another now reaches the recipient's pane
  instead of waiting in an inbox for a poll. Supervisor-sent messages were
  labelled with the generic sender "supervisor", which the recipient's delivery
  path could not match to any registered supervisor, so the wake was declined
  every time and cross-session coordination depended on the recipient happening
  to check. Messages between supervisors now name the supervisor that sent them,
  which is also the only useful label when two of them share a checkout.
  Messages to workers are unchanged.
- `cas update` now runs its post-install phases with the binary it just
  installed. Updating from an older version used to run the schema-migration,
  all-projects refresh, user-level store and skills-sync phases in the
  pre-update process image, so the first update reported the *old* version's
  behaviour and a second `cas update` was needed to converge. The receipt now
  states `refresh_binary_version`, the version that actually performed the
  refresh, and `--json` emits a single combined document instead of two.
- A release no longer fails because the machine cutting it was busy. `cas init`
  aborts itself after a wall-clock budget so a hang cannot squat a CPU core, and
  that budget was a fixed 300 seconds with an all-or-nothing opt-out. On a loaded
  host a test's child `cas init` reached it and took the release gate's
  archive-mode row down with it, on timing alone — the same tree passed minutes
  later on a quiet box. The budget is now settable with `CAS_INIT_TIMEOUT_SECS`,
  and the release gate raises it to 900 seconds for its own children, naming the
  value in its receipt. An ordinary `cas init` keeps the 300-second watchdog, a
  meaningless override falls back to it rather than disabling it, and
  `CAS_INIT_NO_TIMEOUT=1` still turns the watchdog off outright.
- Concurrent `cas hub start` / `cas hub restart` no longer stall ten seconds
  and report a spurious failure when another command has already brought up
  the hub that was asked for: the waiters resolve as soon as a live hub
  satisfying the request is observed, plain `cas hub stop` is unchanged, and
  each timeout names the wait that expired. The concurrent lock-owner test now
  asserts both commands succeed and exactly one owner exists.
- Release gates run through `scripts/release-train.sh`: each run gets a
  directory keyed by version and worktree with an attributable receipt, the
  gate's PID is recorded, a second gate for the same run is refused by name,
  `--stop` signals only the recorded PID, and no release script locates a
  process by name pattern. `release-train.sh pipeline` opens the PR, waits for
  the pull-request checks to actually pass, enqueues, re-enqueues if the queue
  drops the entry, and records the landed main sha.

### Changed
- **Behaviour change for automation:** if the newly installed binary cannot be
  run for the post-install phases, `cas update` now exits **non-zero** with
  "binary updated to X; refresh did not run — run `cas update` again", and its
  JSON receipt carries `refresh_binary_version: null` and
  `refresh_status: "skipped"`. Previously such a run fell back to refreshing
  with the pre-update image and exited 0, which read as converged when it was
  not. A run whose refresh is performed by a version other than the one just
  installed fails the same way.

## [3.15.3] - 2026-09-04

### Fixed
- `cas doctor` no longer counts a dependency whose endpoint is a quarantined
  task as an "orphaned dependency". Quarantining foreign rows (which doctor
  itself prescribes) hid those tasks from the board but not from the dependency
  table, so every edge touching one read as orphaned with no command that could
  clear it. Quarantined endpoints now fold into the OK row as a stated count;
  only an id genuinely absent from the store warns, naming the offending rows
  and `cas doctor --fix`, which prunes them. A store error during the lookup is
  never mistaken for absence: such rows go to an unresolved bucket, the prune is
  skipped for that run, and the row names the error.
- `cas index code` now reconciles the code-vector queue at the end of every run
  (and on daemon start): queue rows whose symbol no longer exists are dropped,
  failed rows are re-armed (all of them on `--force`, otherwise all but
  provider-rejected input), rows whose content hash drifted are rewritten, and
  eligible symbols that were never queued are enqueued, with the counts printed.
  Doctor's "queue rows name symbols that no longer exist" and "failed" clauses
  now name commands that clear them. Indexer writes take the SQLite write lock
  up front with bounded retry and wait out a concurrent BM25 writer, so a
  `cas doctor` or second `cas serve` running alongside no longer turns into
  "failed to retire deleted source file: database is locked" file failures.
- The cross-project row scan no longer reports a registered root whose database
  has no `tasks` table as a project that "could NOT be read". Such roots (a
  copied CAS root used as a test fixture under the artifacts directory) are
  listed as skipped, never drive a warning, and the registry refuses to
  register roots under the factory artifacts root, `~/.cas/scratch`, or
  Cassy-named temp directories in the first place. New
  `cas known-repos forget <path>` removes a live registry row and its bindings
  with a receipt; it refuses the current project root without `--yes`.
- Closing a task no longer fails the tmpfs proof-receipt gate because the close
  reason merely mentions a temp path in prose. A temp path counts as a cited
  receipt only when it is presented as one (a proof cue word shortly before it,
  or the path opening a line or list item); a reason that cites no durable
  artifact at all is still rejected, and the quoted path no longer carries the
  sentence's trailing period.
- `cas integrate mecha-cassy` now repairs the project `.cas/proxy.toml` that
  shadows the machine registration, so the fix `cas doctor` prescribes actually
  clears the warning it prints. A project proxy file replaces the machine
  allowlist rather than widening it, so one left naming the retired Slack tool
  routes kept them authoritative through any number of re-runs while the
  command reported "already configured". It now rewrites those routes in place
  — comments, key order and every unrelated server and route survive
  untouched — and refuses to claim "already configured" while a shadowing file
  still drifts. A hub server block in that file is removed only when it is
  identical to the machine registration that supplies it; one that differs is
  an override, such as a project pointed at a staging hub, and is kept and
  reported rather than silently switched. A project file that names no hub route is still left alone
  (widening a policy the project declared is not the command's call) and is
  reported with the exact routes to add. The `mecha-cassy` doctor row now names
  the file the stale entries live in, machine or project.

## [3.15.2] - 2026-09-04

### Fixed
- `cas update` no longer reports a project as refreshed while its schema stays
  behind. A migration whose name had already been recorded under another id
  (a store migrated by a pre-release build that numbered it differently) made
  the ledger write fail with "schema was detected but its ledger row could not
  be recorded" on every run, leaving the project pinned two migrations short.
  The runner now reconciles such rows — the stale entry is logged to the
  reconciliation ledger with both ids and removed, the migration is recorded
  under its current id, and whatever migration now owns the vacated id is
  re-evaluated honestly (detected or applied). Every pending migration gets
  its turn before the first failure is reported, and the ledger-write error
  names both ids involved. Read-only checks still report the wedge; only
  `cas update` repairs it.
- `cas update` finds every local project, not just the ones already in its
  registry. The filesystem scan stopped at the first `.cas` it met — on any
  machine with a user-level store that was `$HOME` itself — so discovery had
  silently collapsed to the registry and dozens of projects never received
  migrations while the summary said "N projects refreshed". The scan now
  descends (skipping `.cas/`, hidden and vendored directories), refreshed
  projects are registered so discovery converges, a `.cas` without a database
  is listed as "not refreshed (unregistered)", the user-level `~/.cas` store
  gets its own refresh phase, every schema line names the store it migrated,
  and `cas update --register <path>` adds a project by hand.
- A `CHANGELOG.md` or project doc saved as UTF-16, or with a UTF-8 byte-order
  mark, is now read instead of skipped. The changelog index no longer fails the
  whole pass on an encoding it can decode, a leading byte-order mark no longer
  swallows the file's first release heading, and a doc that genuinely cannot be
  decoded is reported by name and reason in the pass report rather than
  disappearing from the knowledge wiki with no explanation.
- `scripts/release-gate.sh` picks its own scratch base (`/var/tmp/cas-release-gate`)
  instead of one under `$HOME`. On a machine with a user-level `~/.cas` store the
  old default sat beneath it, so the gate refused its own archive-mode and
  snapshot-portability rows until an environment variable was exported by hand.
  Setting `CAS_RELEASE_GATE_HOME_DIR` still overrides the base, and the receipt
  now names the base it used and where that value came from.

## [3.15.1] - 2026-09-04

### Fixed
- Closing a task no longer wedges when the worker's branch moves after the
  verifier dispatch was minted. The proof now binds the delivered commits: new
  commits, merges or fast-forwards on top keep the dispatch valid as long as
  the delivery is still reachable, a rewritten or dropped delivery is still
  refused naming both tips, and re-running close after a genuine drift mints a
  fresh dispatch instead of replaying the spent id.
- `shutdown_workers` accepts a registered worker by name, agent id or session
  id (and a JSON-array argument), using the same identity lookup as
  `worker_status`; the error distinguishes a target refused by policy from one
  that is genuinely unknown instead of claiming both.
- Recording a verification verdict no longer fails instantly with
  "database is locked" under store contention: the write takes the lock up
  front (`BEGIN IMMEDIATE` with bounded retry) instead of upgrading a deferred
  read snapshot, which SQLite refuses without consulting the busy handler; the
  knowledge reindex reads page bodies before opening its write transaction;
  the WAL is capped at 64 MiB; the give-up error states the wait it attempted.
- Tests that set up a temporary HOME no longer read the machine's own project
  configuration: HOME only redirects user-level lookups, so a `proxy.toml`
  registered in a checkout was still visible to tests running from a worktree
  inside it. The test harness pins the project root by default, the release
  gate names such a file and neutralizes it for the run instead of refusing,
  and the gate self-test defaults its scratch base to a path with no `.cas`
  ancestor.
- Insta pending snapshots (`*.snap.new`) are ignored so a test run can no
  longer sweep one into a commit.

## [3.15.0] - 2026-09-03

### Added
- `cas integrate mecha-cassy` sets up MechaCassy on a machine in one command:
  a machine-level proxy registration every project inherits, Claude Code and
  Codex entries by environment-variable name, and an authenticated tool list as
  the receipt. `cas doctor` gains a mecha-cassy row; a credential script handles
  the one human step; onboarding doc for teammates. The `mecha-cassy` skill is
  now the whole posting contract (the separate mecha-cassy-post skill is
  retired) and a new `user skills` doctor row flags stale or orphaned
  user-level skills per harness.
- Cloud sync consumes per-row push outcomes: rows the cloud kept newer are
  acknowledged, rejections are held with their reason and a remedy, and
  `cas update` / `cas doctor` say which is which. Terminal failures parked by
  an older client are requeued once after upgrade.
- Cloud sync consumes dependency deletion tombstones: a removed edge
  propagates everywhere and cannot be resurrected; the "N edges healed" churn
  on every pull is gone.
- Revision-based conflict resolution: when both sides carry a server revision
  the higher revision wins and clocks are not consulted, so a machine with a
  wrong clock no longer silently wins or loses. Revisions arbitrate the
  "keep newest" strategy (personal pulls and teams configured for it); an
  explicit remote-wins or local-wins team strategy stays authoritative.
- `cas doctor --fix-cloud-rows [--yes]` sets aside task rows a past sync leak
  copied into the wrong project: they leave the ready queue and are never
  pushed, stay readable by id, and `--release-cloud-rows` restores them.
  Doctor reports unattributed and colliding rows with the rekey
  recommendation.
- `cas doctor --verbose` prints each check's duration and a slowest-phases
  table; `--json` carries per-check timings.
- `cas doctor`, worker status and the spawn receipt warn when two live
  supervisor sessions share one clone path (GH #699).
- Factory `gc_report` lists stale disposable roots under the temp directory
  with age and size; isolated Cassy roots default to `~/.cas/scratch/<name>`
  and refuse a RAM-backed location that would hold worktrees or build cache
  (GH #704).

### Fixed
- Tasks whose owner project was never recorded can be started and claimed
  again instead of being refused as an "unassigned legacy row" (GH #690).
- Client project-identity canonicalization matches the cloud's rule exactly,
  consumes the cloud's alias record, and a parity audit reports per-project
  convergence (GH #669).
- Pull attribution reads a row's origin project before the server's scope
  stamp, so another project's rows are no longer ingested as this project's;
  throwaway checkouts no longer mint a cloud identity on push (GH #701).
- `cas doctor` on a large store dropped from over two minutes to about three
  seconds: the history-index check ran a per-commit JSON scan over every task
  (GH #700).
- The embedding drain no longer retries a provider refusal forever: oversized
  text is capped, refusals are told apart from outages, a poisoned batch is
  bisected so only the offending unit is quarantined with the provider's
  message, and doctor reports pending and quarantined separately with
  `cas history embed --retry-quarantined` (GH #695).
- Doctor's code-vector figures come from symbol coverage instead of queue
  rows, so they no longer reset when the queue is re-armed, and a discarded
  vector cache generation is named instead of showing every vector gone
  (GH #696).
- `cas doctor --verbose` no longer inserts thousands separators into task ids,
  UUIDs and timestamps, and the "cannot reach N rows" count matches the rows
  it lists (GH #697).
- The code index decodes UTF-16 and UTF-8-BOM source files instead of failing
  forever; undecodable files are skipped with a named reason and excluded from
  coverage; a UTF-8 BOM no longer corrupts a file's first symbol (GH #698).
- A merge request sent after a new push is delivered instead of being
  suppressed as "already landed": the suppressor now checks the live branch
  tip, not the anchor recorded at the previous merge (GH #703).
- `worktree_merge` fetches and fast-forwards the local target before merging,
  refuses a diverged target before touching it, and reports a rejected
  non-fast-forward push honestly with a working remediation (GH #703).
- probe-comm removes its scratch root on success and failure; test fixtures
  no longer leak hand-named directories into the temp directory (GH #704).
- Migration cursor fixtures derive their expectation from the migration
  registry instead of a hand-pinned list.

## [3.14.0] - 2026-09-03

### Added
- Commander gets a session picker in the header and a back control: every
  session on every paired machine is one tap away, and reopening the app
  restores the last session instead of "No session open".
- Commander on a phone shows a reflowed transcript of each pane at readable
  text size, with the true 80-column terminal one tap away, instead of
  squeezing an 80-column agent interface into a ~46-column grid.
- A phone in landscape is treated as a phone: one shared detection rule for
  the stylesheet and the layout logic, and a landscape arrangement with
  full-height terminal and edge rails.
- Clickable phone-sized mock-ups of three mobile Commander directions (Inbox,
  Deck, Voice) and an Android field report are published under docs/reports.

### Fixed
- Remote Commander viewers can no longer shrink the operator's local console:
  while the local dashboard is attached it owns each pane's PTY size, refused
  viewer resizes are audited, and the viewer renders the authoritative size.
- "Talk to the supervisor" sends on Enter and on the Send button, never renders
  a disabled Send, and states the real reason when a message cannot go out,
  including the exact `cas hub pair --scopes` command for a read-only device.
- A default `cas hub pair` invitation pairs on the first attempt: the link
  declares its scope ceiling, the form requests only what was granted, and
  hub refusals are sentences with a next step instead of "unauthorized".
- Opening a pairing link opens the pairing dialog (also in an already-open
  tab), the optional email field no longer pops the keyboard, and the Pair
  button stays reachable with the keyboard up.
- The phone bottom rail is one bar: shared control treatment, no stray
  focused-pane outline on Pair, no seam, readable machine chip, labelled
  attention count, 44px tap targets.
- Terminal attach degrades honestly on older browsers: AbortSignal.any is
  feature-detected, the connecting timer advances so diagnostics appear, a
  missing API is reported once with the minimum browser versions, and repeated
  failures collapse into one attention entry.
- Commander no longer rebuilds the whole page on every hub update, so the
  message composer keeps focus (and a phone keeps its keyboard) while typing.
- The hub session list reports each session's live worker roster from that
  session's own project, instead of an empty list or the wrong project.

## [3.13.1] - 2026-09-03

### Changed
- The built-in `cas-cut-release` skill (Claude, Codex, and Grok mirrors) now
  gives one exact publish procedure: release credentials come from a
  configurable user-level `release.env` and are proven by name only, release
  log/PID/done receipts live under the durable artifacts root instead of the
  clean tag worktree, `release.sh --publish-tag` alone owns annotated-tag
  creation and local preflight, a PID-safe wrapper always records a numeric
  exit receipt, and publication may only be announced with tag, workflow,
  asset, latency, Slack, and host receipts in hand.
- MechaCassy release posts default to the rubric channel name `cas-internal`
  and fall back to the directly configured Slack transport when a live Cassy
  proxy lacks the registration.

## [3.13.0] - 2026-09-03

### Added
- The MechaCassy Slack transport gives every harness a shared, fail-closed way
  to publish release notes with channel checks, paced threads, receipts, and
  environment-only credentials.

### Changed
- Supervisor identity now survives restarts cleanly: live workers remain
  reachable, old same-name sessions retire safely, and task-free worker deaths
  no longer create misleading supervisor warnings (GH #677, GH #678).
- Task creation now detects meaningful overlap in descriptions and shared
  file, line, function, or slug identifiers instead of relying on title words
  alone (GH #679).
- Worktree merge diagnostics now identify the exact check-runs endpoint and
  source SHA, classify missing or failed checks, and state the advisory merge
  policy (GH #680).
- Assignment briefs are rechecked at delivery time so closed or cancelled work
  is not re-delivered, and each worker receives the tool namespace for its
  harness (GH #682).
- The generated code map is refreshed so repository navigation reflects the
  assembled release tree.

## [3.12.1] - 2026-09-02

### Added
- `cas-cut-release`, the supervisor's single release procedure: a ten-step
  fail-closed train with a "what went wrong before" appendix and a
  self-learning `references/failure-log.md` that any agent must extend
  (`scripts/release-gate.sh --learn`) before retrying a new release failure.
- `scripts/release-gate.sh <version>`: the pre-queue gate that refuses to
  proceed on version literals in tests, workspace check, nextest, doctests,
  an archive-mode run outside the checkout without ripgrep, snapshot host
  independence, stale root projections or ledger, changelog and version
  mismatches, release-script preconditions, procedure guardrails, or a dirty
  tree; every failure-log entry must map to a gate check.

### Fixed
- `scripts/release.sh` removes stale BLAKE3 build outputs before its
  portable-ISA audit, so a prior build in the tag worktree no longer fails
  the "exactly one BLAKE3 build output" check.


## [3.12.0] - 2026-09-02

### Fixed
- The task verifier's verdict templates now record `files_reviewed` (the
  field was silently dropped before) and its test-first check uses a valid
  ripgrep flag instead of erroring on every run; the learning reviewer now
  receives the unreviewed learning IDs it is asked to review; the Stop hook
  and the session-learn skill share one prompt source.
- Removed managed builtins are pruned from every synced skill directory
  (the retired code-review skill no longer appears in session menus), the
  orphaned code-review workflow files are removed, the close-rejection and
  factory-planning messages name the real procedures, and dead guide
  constants are gone.
- Supervisor and worker references no longer teach the retired
  `pending_supervisor_review` status or the `bypass_code_review` flag;
  `supervisor_override` is documented once with its constraints; the two
  contradictory merge procedures collapse to the worktree merge; the raw SQL
  recovery recipe is gone; the Codex factory supervisor definition is
  constraints plus a pointer.
- cas-memory-management is rewritten against the live memory API (all
  request fields, one entry type enum, frontmatter inside content, no
  file-store model); the valid-action lists in cas-task-tracking and
  cas-search are generated from the dispatch table and pinned by a test.

### Changed
- Worker session guidance fits the SessionStart budget (about 6 KB instead
  of 9.9 KB), so ready tasks and memories are populated on every spawn;
  harness-enforced rules are no longer restated as prose.
- Eight skill descriptions lead with their trigger; shipped builtins carry no
  operator e-mail, host paths, or unshipped runbook links; the Claude release
  account gate is a config key; cas-brainstorm and cas-ideate can write their
  own artifacts again and hand off to the supervisor instead of a missing
  `/plan` command.
- mcp-integration teaches `cas mcp add/list/import`, proxy.toml and the
  `mcp__cas__system` proxy actions; release-notes is procedure-only and
  rubric-driven; fallow and cas-nuxt-playwright are an opt-in stack tier
  (synced on stack detection or `[skills] optional`).
- The three documentation skills share one hygiene reference;
  cas-domain-modeling is merged into cas-codebase-design; tiny references
  are inlined; the git-history-analyzer and issue-intelligence-analyst agents
  are retired from the builtin set; cas-writing-for-agents now states steps,
  frontmatter, information hierarchy and completion criteria; the projection
  drift guard covers shell, YAML and JS twins.


## [3.11.0] - 2026-09-02

### Changed
- `cas doctor` renders a grouped report: a header with project and version,
  one line per section (Store, Indexes, Cloud, Config, Integrations) when
  every check passes, non-OK checks on their own row with a short message and
  the remediation on an indented line, thousands-separated counts, and a
  counted summary with elapsed time. `--verbose` prints every check in full.
  Cloud-pull attribution warnings no longer spill onto stderr ahead of the
  report; they are collected into one Cloud row.
- `cas update` prints one row per refreshed project (migration, index,
  skills, membership, cloud) with detail only for projects that warned or
  failed, deduplicates warnings that repeat across projects, drops the second
  per-project summary block, keeps step receipts on one line, and closes with
  a version, project count, and elapsed-time banner. `--verbose` restores the
  full transcript. No Rust debug formatting reaches the terminal.
- `cas cloud push`, `cas cloud pull`, and `cas cloud sync` print one summary
  line: non-zero counts only, "nothing newer" when nothing changed, push
  failures grouped by message with a retry hint, and a spinner that only
  appears on a terminal. `--verbose` prints per-kind and per-error detail.
  JSON output for all three commands is unchanged.


## [3.10.1] - 2026-09-02

### Fixed
- Project-scope cloud pull reconciles entries that exist locally as archived
  rows through the same last-writer-wins path as team pull, instead of
  failing every one of them with "entry already exists" (347 per pull on
  one contaminated store); duplicate-key races resolve the same way and
  unrelated store errors still surface.
- `cas cloud purge-foreign` no longer trusts a backfilled `origin_project`
  alone: rows the doctor's cross-database (id, title) and activity evidence
  attribute to another project are now purgeable, id collisions and
  unattributed replicas are never deleted, task deletes match on (id, title),
  safety lookups fail closed, and the dry-run labels each row with the
  evidence that selected it.
- `cas doctor` reports the doctor's foreign-row evidence count next to the
  purge delete-set count, names every retained row with its reason, and stops
  prescribing `purge-foreign` as complete remediation when the two disagree.
- `cas doctor` names the configuration file that registered an unresolvable
  MCP stdio command (the user-scope `code-mode-mcp/config.toml` or the project
  `proxy.toml`) instead of always saying "repair proxy.toml".

## [3.10.0] - 2026-09-01

### Removed
- The multi-persona code-review pipeline: the `cas-code-review` skill and
  workflow, its persona references, the deprecated code-reviewer agents, the
  worker review-dispatch gate, and the `[code_review]` config section.
- The `code_review_findings` close field and its envelope validation; the
  `pending_supervisor_review` task status (existing rows and incoming legacy
  cloud rows map to `awaiting_merge`).

### Changed
- Task review is now the supervisor's merge-time diff review: the worker's
  branch diff is read against the task spec with scoped test receipts, and the
  supervisor records a verification row as the review receipt; the supervisor
  skill documents this as the canonical procedure.
- `bypass_code_review` is replaced by `supervisor_override` (reason required,
  supervisor-only, logged); the old flag remains a deprecated alias for one
  release.


## [3.9.1] - 2026-09-01

### Fixed
- Moving a task to another project now queues exactly one delete for the old
  project key and one destination-keyed upsert, sent in that order in a single
  push, so the old project no longer keeps a re-created copy; deleting a task
  after a move targets its current owner key.
- The hub launched by `cas hub start`, `cas hub restart` or `cas update` runs
  outside any factory worker containment scope, so it survives the worker that
  launched it; `cas hub` shows who launched it and reports a silent exit.
- `cas hub stop`, stale-hub replacement and failed-launch cleanup now only tear
  down a cgroup that Cassy itself created for the hub; a hub record that names
  an inherited terminal or session scope is refused with a warning instead of
  killing every process in it.

### Added
- `delivery_mode = local_merge` on epics/sessions: the close gate tells workers
  to wait for the supervisor's local merge instead of pushing, the worker skill
  documents both modes, and the worker guard refuses `git push` to origin in
  that mode.

## [3.9.0] - 2026-09-01

### Added
- Task dependency edges (epic→child, blocks, related, duplicate) sync to the
  team cloud as a `task_dependencies` entity: enqueued on add/remove/epic
  create, pushed in both envelopes, applied and deleted on pull, with dangling
  edges parked instead of inserted.
- Every pull/sync self-heals dependency edges: edges the cloud lacks are
  re-pushed, edges the local store lacks are materialized, deletes in the same
  envelope are honored, and a one-line "healed N edge(s)" summary appears only
  when something changed (a second sync heals 0).
- `spawn_workers` provisions the worker worktree inside the task's
  `target_repo` (start point resolved there, worktree under that repo's
  `.cas/worktrees/`) and fails fast with an explicit `cross-repo spawn:` message
  when it cannot.
- `cas update` runs the freshly installed binary's post-update hook after the
  swap, so a stale running hub restarts on the same run even across a version
  boundary.

### Changed
- One canonical project identity: git-remote and case variants of a project
  slug resolve to the pinned slug for registration, push stamping, pull
  ownership, dependency ownership and `purge-foreign`; `cas doctor` reports
  rows still attributed to an alias and `cas cloud project --adopt-aliases`
  rewrites them and re-pushes under the canonical identity.
- Cloud push treats a last-write-wins skip as an acknowledgement, consumes
  per-row outcomes/reasons when the server provides them (parking real
  rejections without poisoning the batch), and requeues terminal
  version-gated failures automatically once the client meets the server
  minimum.
- The PTY-timing test family uses bounded deadline polling instead of fixed
  sleeps, ending the recurring merge-queue ejections.
- Moving a task to another project keys the replacement upsert to the
  destination project, so the old project no longer keeps a stale copy.
- A task that becomes unblocked wakes its assigned worker with a start message
  instead of waiting for the stall detector.
- The worker MCP proxy allowlist uses one documented `<server>.<tool>` /
  `<server>.*` format with alias normalization and live reload, denials name
  the exact entry to add, and a missing stdio executable is reported as
  `executable_missing` (also checked by `cas doctor`).
- The sync queue collapses legacy duplicate rows during migration and upserts
  on conflict, `cas cloud queue --retry --retry-reason <reason>` requeues only
  matching parked rows, and `cas doctor` prints the exact counts blocking
  `purge-foreign` with a runnable retry → push → purge remediation.

## [3.8.0] - 2026-09-01

### Added
- `mcp__cas__task` list/ready/blocked/available accept `include_foreign`; the
  board is scoped to the current project by default and prints how many
  foreign-origin rows were hidden. `show` on a foreign task names its owner.
- Supervisor `task update origin_project=<project>` now moves the task in the
  team cloud: the old project-keyed row is deleted before the row is upserted
  under the new owner (delete failure withholds the upsert), the destination
  must be a registered team project, and a decision note records the move.
- `cas hub authorize` resolves the Commander origin without `--hub-url`
  (explicit flag, then the hub process record, then `[hub].public_url`, then
  the last origin that worked), accepts bare hostnames as HTTPS, and prints the
  requested/granted scopes once.

### Changed
- Team push no longer overwrites an explicit `origin_project` with the pushing
  project; only rows with a missing or blank origin inherit it, and Global
  tasks stay unstamped.
- Team pull keeps foreign-keyed task rows and arbitrates duplicates in favor of
  the owning project; a stale replica can no longer reopen a task its owner
  closed (discarded rows are logged as `owner_wins`).
- `cas cloud purge-foreign` classifies only rows explicitly attributed to
  another project as foreign; tasks with no attribution and every local rule are
  kept, and it refuses (even with `--force`) to remove more than half of the
  tasks or any proven rule.
- Bare `cas hub` shows status (with both the running and installed versions);
  `cas hub start` on a live hub whose version or flags differ restarts it
  instead of failing; `cas update` restarts a running hub left on the old
  binary.
- `cas list` reports the live registered worker count for a factory session
  instead of stale session metadata.

## [3.7.7] - 2026-09-01

### Changed
- Factory delivery monitoring now flags a failed required check while
  auto-merge is armed, or an auto-merge arm that disappears after green checks,
  with duplicate notifications suppressed per pull request and head commit
  and new heads re-armed for notification.
- `branch_contained_in` reminders now refresh the exact `origin/<target_branch>`
  ref before checking containment, keep the reminder pending when that refresh
  fails instead of trusting stale state, and show the compared ref and commit.

## [3.7.6] - 2026-08-31

### Changed
- Claude worker launches preserve the requester's config and secure-storage
  selectors independently, auth checks stop waiting after a bounded timeout,
  and failed checks no longer silently choose the main account.

### Fixed
- Custom Claude profile directories now keep truthful names in account badges
  instead of being mislabeled as the main profile.

## [3.7.5] - 2026-08-31

### Added
- `cas setup` guides a newly installed machine through PATH, cloud login and
  team selection, device pairing, hub service, optional Viktor credentials, and
  first-project initialization with safe reruns and dry-run status reporting.

### Changed
- `cas cloud push` drains the complete personal backlog by default, stops when
  no progress is possible, and reports remaining team-scoped rows explicitly.

## [3.7.4] - 2026-08-31

### Changed
- Curated memories with importance at least 0.9 or positive helpful feedback
  stay in the working tier during decay, and reading a cold or archived memory
  brings it back to working; `[memory.decay]` settings and doctor counters make
  the policy visible.
- Legacy search-index inspection counts metadata without loading every stored
  document, so doctor and daemon checks are fast even with stranded entries.
- Tantivy indexes now use versioned paths; schema mismatches preserve the old
  directory and explicit reindexing migrates or quarantines it safely.

## [3.7.3] - 2026-08-31

### Fixed
- `cas update` now turns over stale `cas serve` processes before refreshing
  each project and repairs legacy Tantivy search-index roots; busy locks warn
  without failing the update.
- The contributor `cas-update` helper turns over running processes before
  refreshing project state.

## [3.7.2] - 2026-08-31

### Added
- SessionStart memory injection is now telemetered with honest retrieval
  outcomes: unresolved cards stay distinct from ignored cards, and reading an
  injected memory counts as use. Ambient rule and skill surfaces are recorded
  for the same impact accounting.
- Added a labeled retrieval-evaluation harness with committed fixtures and
  precision@5/recall@5 baseline gates so ranking changes are measured before
  they ship.
- `cas doctor` now detects memory entries stranded in a legacy daemon index;
  `cas doctor --fix` recovers them into the active index with bounded,
  resumable repair.

### Changed
- Helpful Memories now read from the live, curated, tier-aware corpus while
  excluding raw context blobs, and `cas stats` reports the live corpus count.
- `retrieval_metrics` supports session filtering and rejects unsupported
  filters instead of silently widening the result set.
- Rule review is enabled by default, and ambiently surfaced rules count toward
  the promotion flywheel, so useful rules can progress instead of remaining
  invisible drafts.
- Team sync upserts auto-promoted entries without turning a successful pull
  into an error; Claude worker lanes use canonical model identifiers and report
  workers that die during boot as failed. Project-managed skills are ignored by
  git so generated files no longer become repository noise.

### Fixed
- Background and legacy search indexing now share one repair path, making
  daemon-written memories visible to search and giving busy legacy processes a
  bounded doctor warning with a retry remedy.

## [3.7.1] - 2026-08-30

### Added
- Refreshed the generated `.claude/CODEMAP.md` navigation map and added a
  bounded codemap-latency receipt that proves no content change, freshness,
  local commit/push readiness, and the required protected-PR compute budget.
- Added `cas knowledge build --timeout-secs <seconds>` with a 90-second default
  and a single deadline covering the complete build, including source
  processing, model calls, merges, and post-commit repair.
- Added process-group cleanup for knowledge providers so a timed-out provider
  and its ordinary descendants are terminated and reaped, plus regression
  tests for the timeout, receipt, freshness, and no-write contracts.

### Changed
- Protected pull requests now run only the required Fast Validation and macOS
  Check paths; heavy compile lanes remain on main, scheduled, or explicitly
  dispatched runs. First branch pushes use the protected default branch as a
  trusted comparison base, while merge-queue jobs use their event base SHA.
- Updated the Claude, Codex, and Grok codemap skills to invoke the bounded
  knowledge build as best effort, record its exit status, and continue with
  codemap status proof when knowledge distillation is unavailable or times out.

### Fixed
- Codemap latency validation now labels detached GitHub Actions readiness
  separately from ordinary local readiness and rejects stale or missing
  freshness proofs without touching `CODEMAP.md`.
- Added regression coverage for review fixes that intentionally delete content
  after a task parks, preserving the final-tree merge proof contract.

## [3.7.0] - 2026-08-30

### Added
- `cas cloud unlink [--purge-remote]`: sever a project's cloud link locally
  and, with the flag, remove that project's remote records (entries, tasks,
  knowledge pages) — scoped discovery through the single `CloudSyncer` pull
  builder, dry-run support, fail-closed handling for record types the server
  cannot delete, and the local database left byte-untouched.
- Cassy-managed `.gitignore` blocks: consumer-project skill sync now maintains
  an idempotent managed block covering the distributed builtin files
  (`.claude`/`.codex`/`.grok`), preserving user entries, skipping the cas-src
  authoring tree, and surfacing `git rm --cached` remediation when a managed
  file is already tracked.
- Durable external-condition wake triggers for parked work:
  `branch_contained_in` and `tag_exists` reminders persist across sessions,
  are probed by the daemon with bounded git commands, and deliver a
  supervisor notification once on the false→true transition (GH #624).
- Trace archives: bounded, range-readable archive retention in the daemon,
  with archive readers exposed through the CLI store.
- Versioned rule and skill creates with create-history snapshots (m242), rule
  lifecycle gated on measured outcomes, and surfaced-artifact impact tracking
  (m243).
- `cas-supervisor` epic-driving playbook reference: one-integration-PR
  discipline, target pinning, merge sweep, and spawn conventions in a 1.1KB
  imperative reference mirrored to all three harness flavors.

### Fixed
- Team-sync deserialization: `TaskDeliverables` tolerates the legacy
  JSON-string encoding on pull and canonicalizes on serialization, and the
  cross-DB relocation writers normalize payloads before push — ends the
  "Team pull encountered N error(s)" partial pulls (98 cloud rows were
  repaired server-side alongside this release).
- Release workflow: a tag pushed before its Release Prebuild completes now
  waits (bounded, exact-SHA) instead of silently falling back to cold builds,
  and any cold fallback is loudly reported (GH #603).
- Factory: a child task whose WorkTarget equals the parent epic's default no
  longer silently delivers to trunk — it inherits the live epic lane with a
  durable decision note; explicit non-default targets remain authoritative
  (GH #625).
- Skill validation runs network-isolated with a degraded-validation fallback,
  and skill-id parsing tolerates warning-suffixed ids.
- Sync scoping: personal deletes and legacy team task upserts are
  project-scoped; dangling cloud fixture symlinks are rejected.
- Knowledge loop preserves provenance across sync, including synced skills.

## [3.6.0] - 2026-08-29

### Added
- `cas-image-generate` builtin skill: style-aware asset generation for apps,
  websites, and reports. Harvests a project's design context (palette, motifs,
  typography feel) into style tokens, routes each asset type to Google's Nano
  Banana models with per-type prompt templates, supports reference images for
  style consistency, and degrades with explicit setup guidance when
  `GEMINI_API_KEY` is absent. Shipped in Claude / Codex / Grok flavors.
- SVG & web-assets reference for the image skill: agent-authored SVG as a
  first-class route (decision table, 24px-grid authoring standards, palette
  CSS variables, worked examples), a raster→vector bridge that probes local
  tooling (vtracer/potrace/inkscape), and the favicon/OG/WebP web pipeline.
- Explicit origin-project identity on tasks (`origin_project`, m241): foreign
  tasks replicated by the historical unscoped-sync era no longer rank in
  another project's ready/available surfaces; sync stamps identity on push and
  pull, and supervisors get an audited reassignment path.
- New Cassy logo (generated with the image skill) and refreshed README
  capability documentation.

### Fixed
- Image-generate helper: reference payloads are assembled via files instead of
  argv (`--reference` no longer fails on ARG_MAX for real image sizes), and
  output naming honors the API's returned MIME type instead of writing
  mislabeled bytes.
- Merge-request suppression resolves repository context from the explicit
  local root before the host known-repos registry (a duplicate registry
  selector could silently disable suppression), handles linked git worktrees
  correctly, and an explicitly registered agent role now takes precedence over
  the ambient `CAS_AGENT_ROLE` environment.
- Global-scope tasks queue with no origin project instead of inheriting a
  project identity they should not carry.

## [3.5.0] - 2026-08-29

### Fixed
- Team sync deletes now send the project-scoped identity (`project_id`), unparking
  deletions that older servers' project-aware DELETE contract had permanently
  rejected; parked rejections requeue and flush after upgrade.
- Close-gate tree-effect check honors its "present or explicitly evolved" contract:
  union-merged parallel deliveries pass via token-level per-added-line containment,
  and the rejection message names the supervisor merge-receipt recovery path (GH #597).

### Added
- Skill persistence gated on `validation_script` execution at create/update.
- Measured rule promotion: Draft→Proven driven by outcome evidence instead of a
  single call; real rule impact tracking increments `surface_count` at injection.
- Provenance end-to-end: `Entry.source_ids` populated through `Rule.source_ids`.
- Version history with tombstone deletes for rules and skills (rollback capability).
- Append-only trace archive: events/recordings past 30 days compress instead of
  hard-deleting.
- Structured task execution state: schema'd patchable state blob on tasks.
- SessionStart memory injection carries the full first line for high-importance
  memories; hardened session memory hygiene.

## [3.4.2] - 2026-08-27

### Added
- **Cassy can run workers on a fourth AI backend: OpenCode driving Qwen.** `cli=opencode` spawns workers on Qwen 3.8 Max through a QwenCloud Token Plan subscription (`sk-sp-` key), validated end-to-end by a live conformance receipt — a Qwen-driven worker completed a real Cassy task lifecycle (create, code, commit, push, verified close), survived cancellation, retained permission denials, and kept two account roots isolated. Model selectors carry an explicit route (`qwencloud/…`, `alibaba/…`, `local/…`) with no silent cross-route fallback; routes without a receipt are refused before queue insertion.
- **Model routing is now a checked rulebook, not folklore.** A typed, embedded lane registry defines the worker lanes — light: Haiku 4.5, standard: GPT-5.6 Luna at xhigh, taste: Claude Opus 5 at high, heavy: GPT-5.6 Sol at high, with Terra under standing suspension — and every spawn path (MCP, direct CLI, daemon respawn, doctor) enforces it with rejections that name the violated rule and the available alternatives. Work can be requested by `lane=`; a missing primary backend produces a warned, never-silent substitute, and asking for an exact model is never rewritten. Doctor and preflight report per-backend availability as available / unavailable (with the enable command) / unknown. The docs' routing tables are generated from the same registry the code enforces.
- **The Commander hub installs as a managed service.** `cas hub service install|uninstall|status` (with `--dry-run`) writes and enables a systemd user unit on Linux (linger handled) or a launchd agent on macOS: restart-on-failure, discoverable logs, no secrets in unit files, idempotent re-runs, wired into the installer's next steps.
- **Every release now proves its own installer on real machines.** A `release.published`-triggered workflow runs the actual `curl … | sh` install on a hosted Apple Silicon Mac and a clean Linux container, verifies `cas` works from a fresh login shell, and uploads full transcripts; release copy may only claim "install works" with that green receipt.
- **Installing Cassy now leaves a working `cas` in a new terminal.** The installer detects whether its install directory is on PATH in your *login* shell, offers to add a marker-guarded guard to the right startup file (`.zshenv` on zsh, so the non-interactive shells MCP clients spawn also see it; `.bashrc` or `.profile` on bash), never edits the same file twice, and prints the exact line to add if you decline or there is no terminal to ask on. It then checks the result by running `cas --version` in a fresh login shell and only reports success when that actually works.
- **A brand-new machine gets one friendly line.** Typing `cas` before anything is configured now names the next command instead of printing a factory preflight's list of everything missing.

### Fixed
- **Factory messages arrive when sent, not when someone happens to wake.** Enqueuing a message now nudges the daemon immediately, delivered messages carry their age and a staleness marker, and a spawn assignment for a task the worker has already finished is withdrawn instead of replayed as if new.
- **A task can no longer be waved into supervisor review while its branch is unmerged.** Every close-path transition re-fetches and re-validates the live branch tip's ancestry at decision time, closing the race where a straggler commit slid past a partial merge.
- **The workspace-contract hook stops rejecting writes inside a worker's own worktree.** Containment now uses the registered worktree binding with canonicalized, fail-closed path comparison instead of guessing from the current directory, fixing the case where one subtree was allowed and its sibling was blocked.
- **Workers are never cut from a stale epic branch again.** A behind-only epic base fast-forwards at cut time; a diverged base refuses the cut with the exact branches to reconcile, instead of warning and cutting stale anyway.
- **`cas doctor` no longer calls a dead search index healthy.** The search-index check compares per-type document counts and freshness against the store and fails with the exact reindex command when they diverge.
- **Filing a bug report no longer dies on a missing label or guesses a repository.** A missing `agent-reported` label degrades to a labeless filing with the degradation stated, and an unset `issues.repo` refuses with the exact config command instead of proceeding against an implicit default.
- **Release-note posting has a working, verified Slack route again.** The runbook documents the measured transport inventory and the canonical approved route with a real receipt, and codifies the supervisor handoff for workers without a Slack surface.

## [3.4.1] - 2026-08-20

### Changed
- **A tagged release now publishes in about two minutes instead of fifteen.** The release tree is final the moment the version-bump PR lands, so both platform archives are built then, and tagging adopts those prebuilt artifacts instead of starting a cold cross-platform build on the tag's critical path. The Linux lane builds on the self-hosted runner. A tag with no usable prebuild still builds at tag time, so the slow path remains a working fallback rather than a failure.

### Fixed
- **The 3.4.0 notes below now describe what actually shipped.** Two entries overstated the release: the macOS install entry claimed shell-rc PATH wiring that is not in the installer, and the skills entry read as twelve new skills when eight new skill directories landed. Both are corrected in place below rather than left to mislead anyone reading the release history.

## [3.4.0] - 2026-08-20

### Added
- **Eight new built-in skills ship with every harness.** The Matt Pocock skill collection is imported for Claude, Codex, and Grok with harness-correct tool aliases and MIT provenance recorded: eight arrive as new `cas-` prefixed skill directories (writing-for-agents, diagnosing-bugs, domain-modeling, codebase-design, tdd, wizard, resolving-merge-conflicts, to-questionnaire) and four more are folded into existing builtins as reference material. (Corrected in 3.4.1; this entry originally said twelve new skills.)
- **`cas viktor key` completes Viktor setup with one pasted operator key.** The key is validated and stored machine-only with 0600 permissions — never in project state or environment files.
- **One `cas update` now brings every project fully current.** `cas update --all-projects` discovers every local project and runs the whole chain per project — schema migration, skill sync, cloud team-membership refresh, and cloud sync — with per-project receipts, continuable failures, and dry-run support. The contrib `cas-update` helper delegates to it.
- **Macs on Apple Silicon install with the standard one-liner.** `cas-install.sh` handles Darwin/aarch64 including Gatekeeper quarantine clearing and an Intel-Mac gate with a plain-language stop. When the install directory is not on `PATH` it prints the `export PATH=...` line to add; it does not edit any shell startup file. (Corrected in 3.4.1; this entry originally claimed `.zshenv` PATH wiring, which did not ship.)
- **Ambient recall now reads tool traffic and explains itself.** Trigger terms come from tool results and MCP queries (bounded and redacted), a strong-signal floor overrides the conversational precision gate, and every injection or silence carries a source-attributed decision trace.
- **Dropped work announces itself.** Merge-queue ejections and worker delivery stalls push durable, episode-keyed relays to the supervisor and worker instead of leaving tasks waiting silently.

### Changed
- **A code change reaches main in under five minutes.** PR admission checks collapsed to seconds, merge-queue validation runs the full suite once on fast self-hosted hardware with runtime-path-portable test archives, and stale queue runs are cancelled by behavior-tested watchdogs.
- **Release binaries are code-signed after stripping**, with a verification gate before packaging and a dispatch-only job for inspecting published artifacts' signatures.

### Fixed
- **Closed work stays closed.** Cloud pull can no longer silently resurrect terminal tasks: terminal status changes require an attributed reopen, and unattributed remote reopens park in the conflict journal exactly once.
- **Epic close is fast and unambiguous.** Closing a large epic commits first and responds with a compact receipt in about a second; timeout messages state whether the write landed.
- **Concurrent `cas-update` runs no longer corrupt a shared build.** The helper takes an atomic, holder-visible lock.
- **CI cannot skip Rust validation for markdown compiled into the binary.** Everything under `cas-cli/src/` is rust-affecting, enforced by a mutation-proof guard.

## [3.3.0] - 2026-08-19

### Fixed
- **Cloud sync no longer deadlocks on projects whose team bucket predates the git-remote identity contract.** Team registration now adopts the server-resolved canonical project id from the registration response, verifies it, and pins it so the same sync run pushes and pulls against the real bucket. Previously every sync aborted with a misleading "server-side defect" error (gabber-studio was down for two days; any legacy-slug project on a fresh checkout was affected).
- **`cas cloud project set` is authoritative again.** An explicit `[project] canonical_id` pin is no longer silently rewritten to the remote-derived form, and the later team-push adoption path never overrides an existing pin.
- **Registration failure messages now name the server-resolved canonical id** instead of wrongly blaming the server when identity resolution diverges.

## [3.2.0] - 2026-08-19

### Added
- **Factory workers now surface Viktor-originated questions to a live supervisor.** Incoming conversations are persisted and deduplicated, and remain visible for the next supervisor when no live session is available.
- **Cassy can use an alternate worker account without losing its isolated project context.** Each worker now resolves its own project history and hooks instead of inheriting another checkout's state.

### Changed
- **Merge-queue validation now uses the trusted self-hosted route where appropriate, while the required validation set stays intentionally small and explicit.**
- **Worker guidance is more concise and scannable, with evidence-first progress updates and clearer handoff expectations.**

### Fixed
- **Factory spawning and delivery are more reliable.** Workers retain refreshed local epic bases when publication is unavailable, reject unsafe branch-reference state, and handle Codex account, liveness, and terminal-limit conditions more accurately.
- **Task completion and test evidence are stricter and clearer.** Cassy prevents misleading green test receipts, preserves merge and review gates, and repairs the urgent-stop review path.
- **Viktor restart and archive handling now fail visibly and recover safely, reducing silent loss of pending replies and queued work.**

## [3.1.0] - 2026-08-18

### Added
- **Viktor conversations now work through a managed, two-way Cassy gateway.** When a project has no `.cas/proxy.toml`, `cas serve` refreshes a credential-reference-only Viktor upstream with an exact, fail-closed allowlist of nine conversation tools; an explicit project proxy configuration opts out. Run-starting calls are registered for daemon-owned follow-up, so Cassy delivers completed replies as inbound `origin=viktor` notifications instead of agents polling. `cas init` and `cas update --sync` install the `cas-viktor` skill for Claude, Codex, and Grok, and `cas viktor` reports credential-safe provisioning status.
- **Factory spawns can now select each worker's harness and account independently.** `spawn_workers` resolves per-worker `name`, CLI, model, effort, and `config_dir` overrides, validates the matching Claude or Codex account directory, and carries the resolved account into the spawned worker rather than flattening a mixed fleet to one supervisor profile.
- **Ben's Apple Silicon Mac setup guide is now part of the repository.** The guide covers the supported release-binary install, machine-wide Cloud login, project initialization, Commander service, source-checkout maintenance, recovery, and the current macOS process-restart limitation.

## [3.0.0] - 2026-08-18

### Added
- **`cas init` stops scaffolding your home directory by accident.** Run in `$HOME` (or at the filesystem root) it now names what it would create and asks before writing anything; non-interactive runs refuse outright and point at `--allow-non-project` for automation that means it. Project directories, including non-git ones, are unaffected.
- **The Codex account picker remains useful when there is only one account.** An interactive bare `cas codex` launch now offers that account and a `+ Log in a new account…` row, so adding a named account does not require a hidden command.

### Changed
- **CAS now presents itself as Cassy wherever people see it.** Commander, pairing email, installation and documentation copy, CLI help and banners, and Factory startup now use Cassy; commands, paths, environment variables, and code-level CAS names remain unchanged.
- **Factory runs keep the selected Codex account attached to the decision.** Availability checks and explicit `config_dir` preflight inspect that account's `CODEX_HOME`, and launch or attach output names the account home in use.
- **Factory worker guidance now asks for a concise, shaped response to the user.** The runtime prompt carries the response contract instead of leaving worker handoff prose implicit.
- **Signing in to the cloud is a once-per-machine act.** Credentials live in `~/.cas/cloud.json`, so `cas login` works from any directory and every project on the machine is signed in; `cas logout` signs all of them out.
- **Your team is picked up automatically — no setup command to discover.** When you are logged in and CAS can tell which team you are on, `cas cloud sync` scopes the project to that team and registers it, instead of syncing in personal scope until you happen to run a team command. It says which team it adopted and how to undo it; `cas cloud team auto off` keeps a project personal for good, `cas cloud team set` still pins a specific team, and if you belong to several teams with no default CAS asks you to pick rather than guessing.

### Fixed
- **An unavailable explicit Codex profile offers the next login step.** On an interactive terminal, `cas codex --profile <name>` now offers `cas codex login <name>` instead of leaving the account unusable without recovery guidance.
- **`cas claude --workers 0` no longer errors.** The zero-worker path now shares the normal account-selection flow.
- **A successful cloud sync now means your project really is connected to your team.** `cas cloud sync` confirms the project is registered with the active team before reporting success, registers it when it is missing, and stops with the actual reason — including the exact server exchange that failed — instead of printing green checkmarks over a project the team never received. Previously a machine with nothing queued to send registered nothing, so `cas cloud team-memories` answered "this project hasn't been synced to the team yet" right after a clean sync. That message now names the project, team, and endpoint involved instead of repeating the command that just ran.
- **`cas cloud team show` and `cas cloud team auto` agree on which team you are on.** Both resolve the team slug from your cached memberships, so a team set by UUID no longer displays as `<not resolved>` in one command while the other names it.
- **Browser login no longer produces a broken approval link,** and ordinary polling survives a rate limit instead of ending the login with a server-error message.
- **The Mac setup guide runs top to bottom as written.** Its steps are in an order that works on a clean machine, the commands it names exist, its shell block can be pasted twice without duplicating anything, and it names the cloud domain you sign in to.

## [2.72.0] - 2026-08-17

### Added
- **External expertise now crosses one enforced, receipted gateway.** `cas serve` replaces the proxy's compatibility default with an exact configured `(server, tool)` allowlist at boot and reload, denies every external call when that list is empty, and refuses paid verification routes outside the registered-supervisor gateway. The first production flow reserves a durable budget receipt before `ask_viktor`, resumes timed-out runs by their stored run ID, and returns only the fail-closed external-production-verification verdict; configuration and route/budget defaults are documented in `crates/cas-mcp-proxy/README.md`.

### Changed
- **Cloud synchronization retains an auditable outcome.** Terminal task updates are guarded before they can regress, pull provenance and sync receipts surface the applied result, permanent push rejections are parked with concise errors, and canonical task identifiers remain normalized through the full sync path.
- **Repository and release references now point at `Richards-LLC/cassy`.** Install, update, Homebrew, release, and API links follow the canonical repository home.
- **Commander and CLI guidance better match live behavior.** Hosted Commander health checks admit the supported origin, reachable hubs clearly guide users through re-pairing, and command help exposes cloud operations while keeping internal maintenance tools out of the public surface.
- **Supported Rust is now 1.88.** The declared MSRV and all workspace package requirements advance from 1.85 to 1.88.
- **README documentation now explains the knowledge system.**

### Fixed
- **Factory workers see stale output instructions before acting on them.** Spawn briefs name each task's resolved durable artifact directory and warn when task prose prescribes an absolute or home-relative path outside the worktree or sanctioned artifact root.
- **Release and test operations recover more predictably.** The repository includes an Actions-outage release fallback runbook, and PTY tests tolerate loaded runners without flaking.

## [2.71.0] - 2026-08-16

### Added
- **Groundwork: the complete contract for governing external expertise.** This release lands the Viktor delegation gateway as library surface with its enforcement seams in place — a registered-caller policy hook in the MCP proxy, exact parsed (server, tool) allowlist policy, a delegation receipt store with duplicate-call protection, budget reservations, and timed-out-run resumption (migration 236), and a fail-closed verdict contract under which a verifier that could not answer — timeout, malformed output, insufficient scope, transport failure, or any other enumerated non-answer — records a durable non-pass and never reads as approval. **None of it is enforced yet:** no production path installs the policy or writes receipts in this release (the proxy's default policy remains allow-all), so the supervisor-only provider key remains the operative control until the production wiring ships.

### Changed
- **Message status tells the truth.** Activity that merely suggests a recipient saw a message shows as its own weaker state instead of "confirmed", a message repeatedly blocked by a busy recipient is flagged undelivered instead of silently waiting, and only an explicit acknowledgment of the exact message discharges an urgent halt.
- **One authority decides which branch work belongs on.** Worker spawn bases, merge destinations, and newly created or newly linked epic children all resolve through the same declared-work-target precedence chain; an epic branch that has cleanly fallen behind its parent is fast-forwarded before any worker is cut from it.
- **Finished work is recognized as finished.** Epic close reconciles deliveries that were squash-merged and later improved, measures against live branch state instead of stored counts, and keeps unproven anchors fail-closed.
- **Memory keeps instructions, not chatter.** Machine-to-machine relay turns are no longer captured as durable context, while genuine operator instructions are captured again — discriminated by typed delivery provenance instead of text parsing.

### Fixed
- **Commander is finished work on both surfaces.** A polish pass and a UX pass fixed the unreachable phone message button, empty worker panes, the drawer crushing the terminal, dead connection colours, the keyboard closing mid-word, drafts destroyed by the live refresh, sends without feedback, vanishing confirmations, unexplained disabled controls, duplicate alert cards, stale data posing as live, a dead-end first run, and alert cards missing their ticket.
- **Sharp edges removed.** Ending a session no longer risks a nested-runtime panic; requesting an isolated worktree no longer gets a false refusal with invalid TOML instructions; the supervisor checklist no longer instructs an action that severs its own tools; a leak test no longer scatters orphan processes through CI cleanup; and memory list filters (tags, tier, scope) actually filter, with counts that match the rows.

## [2.70.0] - 2026-08-15

### Added
- **Commander opens a session in kilobytes instead of megabytes.** Attaching sends an authoritative terminal keyframe built from current pane state and then streams live updates, so the first screen arrives in roughly 17 KB where it previously required about 44.7 MB, and history is fetched only when scrolled into view.
- **Alerts are triaged instead of listed.** Attention cards are grouped by session, ranked critical, warning or info, and repeated failures collapse into a single card with a count that can be dismissed as a group.
- **The pane you watch gets the room it deserves.** The supervisor pane is dominant by default, panes can be promoted and reordered, and the chosen layout is remembered.
- **Optional AI enrichment can label session cards and attention events.** It ships default off, applies redaction inside the provider so callers cannot bypass it, and honors a deterministic severity floor that enrichment may raise but never lower.

### Changed
- **A failing connection explains itself.** Commander reports an explicit staged lifecycle with per-stage deadlines, jittered backoff, heartbeat latency and authenticated diagnosis, and distinguishes an expired credential from a revoked one, instead of showing an indefinite "Connecting" state.
- **Reporting a viewport is now part of observing a pane, not controlling it.** A read-only viewer can size its own terminal, while a leased pane follows its controller, so an observer can no longer reflow a controller's screen.
- **Browser pairing can request control when it is needed.** Control is no longer fixed at pairing time, and anything still unavailable states why.

### Fixed
- **Terminals render at the size of the pane showing them.** Panes no longer arrive as mangled, mid-word-wrapped text, and a replayed byte tail can no longer begin mid-escape or omit terminal modes.
- **Stopping a registered server is verified rather than assumed.** A stop no longer reports success while a wrapped child process survives it.
- **Release publication reflects the bytes users actually download.** Local audit archives stay local until publishing is explicit, and announced digests come from the published release.
- **Integration and recovery handle real branch state.** Clean epic branches advance on integration, clean-behind receipt closes are unblocked, numeric stash recovery references are disambiguated, and relays no longer pollute session context.

## [2.69.1] - 2026-08-14

### Fixed
- **Commander page-initiated pairing now finishes in browsers that require `fetch` to retain its `Window` receiver.** Every pairing handoff binds the browser fetch function before relay creation, polling, acknowledgement, or credential exchange.

## [2.69.0] - 2026-08-14

### Added
- **Every CAS harness now receives the MCP integration runbook.** `cas update --sync` distributes the same installation and diagnosis guidance to Claude, Codex, and Grok.

### Fixed
- **Commander page-initiated machine pairing now completes.** The pairing handoff sends the hub's canonical origin, so the relay accepts the invitation instead of rejecting it at delivery.
- **A local pairing precondition failure no longer consumes the one-time code.** CAS checks the local hub before claiming and retains the same-machine nonce for a safe retry.

## [2.68.1] - 2026-08-14

### Fixed
- **The v2.68 delivery and recovery wave is now installable on Linux x86_64.** Release builds use the compiler-specific portable baseline, rebuild native dependencies from source before auditing, and fail immediately when a declared native target cannot be produced on the current host.
- **Factory startup and recall keep using the information that is current and relevant.** Base selection consistently prefers the fresh remote-tracking ref, and mid-session recall prioritizes the current request over an overlong task title.
- **Validation diagnostics remain accurate under edge cases.** Scoped-proof validation recognizes nested integration modules, and the unknown-tool MCP test no longer claims an unproven server-side mechanism when its historical timeout cannot be reproduced from retained evidence.

## [2.68.0] - 2026-08-14

### Added
- **Workers can flag a same-session peer collision without leaving the supervisor blind.** Peer warnings stay scoped to the active factory session and include a supervisor copy, while task notes can now be read directly without loading the full task record.

### Changed
- **Delivery and merge decisions now prove the work's content is on its declared target.** Freshness, merge relays, close receipts, and diff attribution follow the actual destination and distinguish present work from commits that were rebased, resolved away, or superseded.
- **Review and recovery state now reflect the real owner and live task state.** Value-only edits retain normal supervisor review, verification recovery names the available escape hatch, and terminal relay backlogs reconcile automatically.

### Fixed
- **Lifecycle instructions no longer turn stale or uncertain state into a misleading action.** Replayed prompts carry provenance, terminal assignments are withheld only with positive current evidence, declined merge anchors cannot reappear as live requests, and urgent stops expire with their acknowledged exchange.
- **Workers start and operate in the correct context more reliably.** Spawned work uses a fresher non-divergent epic base, workers receive queued supervisor corrections before starting, the target checkout is protected from foreign Git writes, and missing local prerequisites are made explicit.
- **Focused recall and memory saving are more dependable under real workloads.** Search opens a consistent schema under concurrency and overlap scoring measures meaningful content similarity rather than shared note structure.

## [2.67.0] - 2026-08-14

### Added
- **Task artifacts are now searchable with their work attached.** Bounded Markdown, text, and JSON deliverables enter the shared search index at close and during reindex, while oversized or unsupported files remain safely excluded.
- **Factory worktrees now surface branch-local prerequisites before work begins.** New checkouts provision the pinned Zig toolchain when available and give lockfile-aware Node installation guidance without sharing path-sensitive dependencies.

### Changed
- **Factory delivery follows the task's declared target branch end to end.** Epic bases, freshness checks, merge relays, and landing status now resolve against the real destination instead of assuming `main` or counting unrelated commits.
- **Task-focused recall and core maintenance contracts are more precise.** Relevant saved guidance remains competitive across search fallbacks, workspace dependency and lint policy is centralized, CLI backends share one typed interface, and lifecycle gate failures retain structured meaning internally.

### Fixed
- **No-code and supervisor-verified work can finish without close-gate deadlocks.** Portable evidence survives parked states, valid updates are no longer discarded alongside one rejected field, stale anchors can be cleared safely, and missing verification dispatches have a bounded recovery path.
- **Lifecycle notifications now identify and acknowledge the event they actually represent.** Wake relays reach the acknowledgement bridge, supervisor-owned gates do not masquerade as worker events, stale completion prompts are revalidated, and instructions use the receiving harness's live tool namespace.
- **Search, startup, and delivery edge cases fail safely instead of losing context.** Unknown colon-bearing terms search literally, custom-profile and Grok supervisors receive startup context, spawn-time corrections arrive before assigned work starts, and squash-landed or non-main-target work is reconciled by content.
- **Harness and checkout setup reports its real capabilities.** Codex context reset names the supported restart path, supervisor launch parity is guarded across harnesses, and worker binding checks reject stale or sibling checkouts before execution.

## [2.66.0] - 2026-08-13

### Added
- **Commander can now begin machine pairing from the page.** A short-lived code lets the target machine authorize the requesting Commander session, with strict controller, relay, and loopback origin boundaries throughout the exchange.
- **Cross-project work can be proposed and followed without losing ownership.** Proposals carry explicit source and target projects, support auditable acceptance or rejection, and keep local dependent tasks blocked until the external work is resolved.

### Changed
- **Proposal synchronization now converges across retries, pagination, and reopen cycles.** Creation is idempotent, replayed feed rows are deduplicated, provenance remains authoritative, and external dependency state follows resolution transitions without duplicating local work.

### Fixed
- **Factory workers now fail closed before starting in the wrong checkout.** Spawn preparation proves the worker's exact worktree and branch, pre-harness validation rejects drift, commit guards deny sibling branches, and binding diagnostics inspect the assigned checkout.
- **Commander pairing handles cancellation, replacement, and cleanup races safely.** Aborted or failed exchanges roll back only their own state, replacement rotates live credentials, stale cleanup cannot erase a newer pairing, and incomplete browser fragments are scrubbed before startup.
- **Coordination and operator surfaces retain truthful state under edge cases.** Supervisor roles survive registration, stale reminders stay quarantined, long code snippets truncate on UTF-8 boundaries, stale-skill warnings render cleanly, and tag CI handles an all-zero base SHA.

## [2.65.0] - 2026-08-13

### Added
- **Silent coordination failures now become visible, recoverable outcomes.** Aged unread messages return a one-shot notice to their sender, stalled sessions escalate durable attention signals, and status output includes recent progress timestamps.
- **Concurrent planning now warns before work is duplicated.** Session startup surfaces simultaneous planning activity, sibling titles are checked without collapsing meaningful distinctions, and duplicate plans are identified early.

### Changed
- **Startup and launch checks fail earlier with actionable context.** Configuration directories are validated before launch, registration failures preserve the relevant terminal tail and reap abandoned processes, and MCP startup applies pending schema migrations only after arming its parent-death watchdog.
- **Close verification handles real delivery shapes without weakening proof.** No-code work can close with portable evidence, target-branch and squash receipts retain a non-empty lint range, merge receipts receive useful correction hints, and merged delivery facts remain immutable.

### Fixed
- **Dead or stale sessions no longer leave work looking active.** Held work is returned to a recoverable state with an audit trail, and epic status marks stale ownership instead of presenting it as live progress.
- **Claude session progress and interrupts are now observable.** Transcript turn watermarks feed stall detection, explicit interrupts report confirmed delivery or a clear failure, and status reports distinguish recent output from recent file changes.
- **Delivery-stall thresholds and bounce eligibility are fail-safe.** Oversized thresholds return a clean error instead of panicking or wrapping, while broadcasts, synthetic traffic, stale rows, cross-session senders, and prior watchdog notices cannot create false bounces.

## [2.64.0] - 2026-08-12

### Added
- **Supervisors can make decisions with the context that matters.** Creating work now surfaces related prior recall, and explicit decision gates make consequential choices visible before work proceeds.

### Changed
- **Release-only changes validate faster.** Workspace version bumps can take the focused required-check path while preserving the heavier validation tier for product changes.
- **Task handoffs now stay current through delivery.** Merge relays refresh the target tip and fetch remote receipts before reporting an outcome.

### Fixed
- **Expired memory and session reminders now respect their intended boundaries.** Valid context survives recall, expired entries stay out, and reminder lifecycle actions remain scoped to the session that created them.
- **Terminal work states and activity reporting are more trustworthy.** Cancelled and superseded work follows a fail-closed lifecycle, and dirty worktrees still report their real activity floor.
- **Automation recovery is clearer and safer.** Negative-result closures retain their evidence, CI red-run receipts are preserved, socket ownership elects one daemon safely, and session end snapshots the current state.

## [2.63.0] - 2026-08-11

### Added
- **Every Codex install now carries the CAS safety hooks pre-trusted.** Provisioning registers hook trust and project trust through a single locked configuration transaction, verified before any agent launches, so agents start working immediately without interactive trust prompts.

### Changed
- **Session startup is leaner and more complete.** The always-injected skill descriptions shrank by two-thirds, team-spawned sessions now receive the same project-memory bundle as direct launches, and retrieval outcomes feed back into recall scoring.
- **Generated hook configuration is byte-stable.** Regenerating `hooks.json` over an unchanged setup produces a byte-identical file, ending spurious git churn.

### Fixed
- **Messages to idle Codex agents now reliably surface a turn.** Prompt delivery is classified and watched end-to-end; an unsurfaced delivery wakes the agent instead of sitting unread indefinitely.
- **Agent cleanup fully tears down what it removes.** Stale-agent maintenance routes through forced shutdown and waits on the terminal process, ending ghost panes and zombie processes after reaps.
- **Concurrent Codex launches no longer race trust registration.** The pre-launch trust write is a verified happens-before of the agent process start; launch refuses rather than parking on an interactive prompt.
- **CI survives build-cache outages.** Cache-service failures at setup or teardown downgrade to an uncached build with a loud warning instead of failing the run.

## [2.62.0] - 2026-08-11

### Added
- **CAS now includes cross-harness data-visualization guidance.** The built-in skill and its quality checks make it easier to turn repository data into readable, reviewable visual artifacts.

### Changed
- **Pull-request validation is faster while preserving the release gate.** A focused warm suite now protects merge requests, while heavier checks remain available on main, schedules, and manual dispatch.
- **Workers and release flows now give clearer, more reliable handoffs.** The checked-in guidance covers the supported task surfaces, artifact evidence, and protected-main release sequence.

### Fixed
- **Hub restarts now complete with live viewers attached.** Existing viewer connections drain within a bounded window, then CAS safely closes any remaining stale connections before the replacement hub starts.
- **Runtime coordination and cloud sync report actionable truth more consistently.** Stale CI failures no longer trigger misleading alerts, weak ambient matches cannot dominate recall, supervisor memory writes use the intended gate, and rejected cloud records retain itemized reasons.

## [2.61.1] - 2026-08-10

### Fixed
- **Hub upgrades now recover Tailscale Serve mappings created by v2.60.0.** Legacy ownership receipts load without the newer diagnostic executable field, so CAS can tear down its exact stale mapping and republish the upgraded hub instead of leaving HTTPS unavailable.

## [2.61.0] - 2026-08-10

### Added
- **Commander hubs can now persist as managed services.** `cas hub service install`, `status`, and `uninstall` provide launchd and systemd integration for durable fleet control.
- **The hosted static Commander origin is explicitly supported.** `https://hub.petrastella.io` is documented as an opt-in trust boundary, and the controller visibly identifies incompatible hub capabilities.

### Changed
- **Hub restart recovers the public Tailscale Serve endpoint on macOS.** CAS discovers the signed Tailscale app-bundle CLI when it is not on `PATH`, preserving the normal start/restart recovery path.

### Fixed
- **Hub stop receipts now report the final Tailscale Serve outcome truthfully.** A mapping removed by the foreground hub during shutdown is recognized as removed rather than reported as untouched.
- **Scoped CI validation now reads ANSI-coloured test summaries correctly.** Matching test failures continue to be reported instead of being obscured by terminal formatting.

## [2.60.0] - 2026-08-10

### Added
- **Failed factory CI runs now reach the supervisor automatically.** The daemon watches completed runs for main and active factory lanes, relays one actionable failure per branch and commit, and includes the run, failing job, and first failing test when available.
- **Merge receipts now report the source lane's latest CI verdict.** A completed red run is surfaced with its URL before a merge decision; unavailable or still-running CI is called out honestly as unknown without changing merge semantics.
- **Cloud sync rejections now explain which records need attention.** Personal and team push failures retain itemized reasons, including partial server-side rejection, so accepted work can continue while the rejected items remain actionable.

### Changed
- **Factory coordination exposes more reliable liveness and recovery signals.** Preassigned workers retain live holders, background process activity is observed across threads, close-gate rejections explain their cause, and a worker wakes only when both delivery and inactivity evidence permit it.
- **Routine checks are more precise and economical.** Scoped test filters explain regex-like input, hook wire captures are audited, migration registry IDs are guarded, workspace write checks recognize valid targets, and CI/test fixtures are isolated consistently.

### Fixed
- **Destructive worker shutdown and merge handling now stay bound to the intended work.** Shutdown targets are validated safely, task-bound delivery merges retain their correct task identity, and merge-conflict status is limited to the affected contribution.
- **Factory startup, test, and interface behavior now tell a truer story.** Startup pulls do not requeue work, panic isolation runs under the intended test profile, ambient recall filters weak matches, and the factory strip consistently shows the running version.

## [2.59.0] - 2026-08-10

### Added
- **Claude users can select and sign into separate accounts directly from CAS.** Bare launch now offers a profile picker, each profile keeps its credentials isolated, and `cas claude login <profile>` makes switching accounts explicit.
- **Cloud queue recovery now has an explicit retry command.** `cas cloud queue --retry` lets operators re-attempt failed queued work without guessing at its state.

### Changed
- **Factory test gates use faster, more targeted defaults.** Scoped nextest runs and shared compiler cache use reduce routine feedback time while retaining the full release checks.

### Fixed
- **Pending cloud work and workspace checks now report and recover more reliably.** Silent pending work is surfaced, failed rows can be retried, and the workspace guard no longer rejects valid Bash write targets.
- **CI fixtures are isolated consistently.** Test runs no longer inherit machine-specific state that can make a healthy change look broken.

## [2.58.0] - 2026-08-10

### Added
- **CAS now carries its own built-in CLI routing guidance.** Common command-line work can reach the right product guidance without relying on a separate external skill setup.
- **Workers now checkpoint before compaction and can retain a sync-conflict journal.** A constrained turn leaves a usable handoff, while a conflicted sync keeps enough history to explain and reconcile the result.

### Changed
- **Factory work now follows a clearer workspace contract.** Writes are constrained to sanctioned roots, durable task artifacts are collected safely, and close evidence cannot point into transient tmpfs paths.
- **Release inputs now fail fast before artifact builds begin.** The release workflow validates the annotated tag, exact version train, changelog, clean inputs, and locked dependency graph before it spends time building platform artifacts.
- **Legacy session and model-effort context now remain attached to the work that needs them.** Daemonized sessions retain their source session through muxing, and queued work preserves the selected model effort.

### Fixed
- **Memory sync no longer drops a daily entry when remote IDs collide.** Collisions are skipped safely instead of silently replacing local history.
- **Verification storage repairs its required schema at open time.** Existing installations converge before verification state is used.
- **Rejected supervisor reviews return through the sanctioned amendment path.** Review state and epic close reporting now agree on the intended target.
- **Cloud sync now exposes intentionally skipped queue work.** Operators can distinguish a retained diagnostic from an unexplained missing update.
- **The factory test and workspace suite is portable across the supported macOS environment.** PTY aliases, hub fixtures, watcher behavior, history cleanup, migration projections, and reminder references now remain stable without Linux-only assumptions.

## [2.57.0] - 2026-08-09

### Added
- **Memory can now consolidate an overlapping entry in one explicit, safe operation.** The opt-in merge returns the surviving identity and receipt, while concurrent edits are detected rather than silently overwritten.
- **Memory recency is now deterministic and self-describing.** Recent results state their ordering and use a stable tie-break; lifecycle guidance explains when to merge, archive, or expire a durable memory instead of creating parallel records.

### Changed
- **A full cloud sync now deliberately re-reads prior history when requested.** `cas cloud sync --full` resets the pull watermark and empty-result streak so recovery starts from a known clean scope.
- **Cloud sync now explains healthy no-op pulls and the active sync state.** Pull output distinguishes up-to-date, personal-only, and fetched work; status names the active team or personal scope, daemon liveness, queue health, and the last successful pull.
- **Factory halt responses now lead with a bounded, actionable exit brief.** Operators get the essential stop context without spending the remaining turn budget on repeated coordination detail.

### Fixed
- **Network probe latency remains an observation, not a one-sample release gate.** Probe conformance now preserves useful p95 telemetry without treating an isolated sample as a verdict.
- **Cloud pull failures now identify the malformed entity that could not be decoded.** Recovery messages name the affected record instead of leaving a generic parse error.
- **Knowledge and team sync now converge more reliably.** Missing knowledge attribution schema is repaired safely, team pulls retain required skills, and the pull-url guard ignores inline test scaffolding while continuing to detect production callers.
- **Supervisor review and amendment flows retain their correct delivery boundary.** Review dispatch binds atomically to the delivery it approves, legacy verification proves the intended repository state, and an amendment stays pinned to its original work.
- **Memory recall returns cleaner, more useful context.** Lexical fallback no longer lets same-session echoes or low-value ambient matches crowd out relevant context.
- **Factory presence and control surfaces now report a truer state.** Held workers stay quiet, duplicate roster entries reconcile to one identity, and repeated startup no longer creates a second visible worker.
- **Reminder guidance now makes bounded waiting explicit.** Long-running operations preserve a reachable session instead of silently occupying the worker pane.

## [2.56.0] - 2026-08-09

### Added
- **Cloud sync now exposes configurable queue-health warnings before work is stranded.** `cloud.queue_pending_warning` and `cloud.queue_oldest_warning_secs` make factory preflight report a growing or aging sync backlog while preserving safe defaults.

### Changed
- **An active CAS daemon now drains cloud work on its regular cadence even when no new activity arrives.** A queued update therefore continues toward the cloud after the event that created it, rather than waiting for a later local action.

### Fixed
- **Cloud deletions now preserve current local truth and record failed deletes for retry.** A stale tombstone is neutralized when its task or entry exists locally again; successful delete routes, already-absent remote rows, personal/team queues, and skipped upserts now converge consistently instead of silently losing or retaining work.

## [2.55.5] - 2026-08-09

### Fixed
- **Commander pairing now returns the exact authorized CORS origin when a bound cross-machine pairing exchange is refused.** A controller-origin browser can read both the successful credential and a generic refusal for its own pairing capability, while unbound, mismatched-origin, or otherwise invalid exchanges remain fail-closed without exposing an allow-origin header.

## [2.55.4] - 2026-08-09

### Fixed
- **Commander hub restart now waits for an authoritative machine-lock handoff before starting the replacement.** Restart propagates stop failures, waits for both the old process and its lock ownership to disappear, and acquires the machine lock before stale-state cleanup or replacement launch. If the bounded handoff deadline expires, the command fails truthfully without starting a competing hub; concurrent start and restart attempts preserve exactly one owner.

## [2.55.3] - 2026-08-09

### Fixed
- **Commander HTTPS origins now instruct browsers to stay on HTTPS for one year.** Responses reached through CAS's verified Tailscale Serve TLS path emit exactly `Strict-Transport-Security: max-age=31536000`, while the documented plaintext loopback listener cannot opt into HSTS through spoofed proxy or identity headers. The policy is bound to a separate server-owned proxy backend and preserves existing CSP, referrer, content-type, frame, authentication, and CORS behavior across successful and error responses.

## [2.55.2] - 2026-08-09

### Fixed
- **Commander now starts securely on a clean installed machine without a manual initialization step.** The hub creates a missing `~/.cas/hub` hierarchy with owner-only permissions for both ordinary and Tailscale Serve startup, preserves existing safe state, and rejects symlinks, non-directories, unsafe final modes, wrong ownership, and unwritable ancestors without exposing filesystem paths.
- **A real daemon `SIGILL` now reaches Commander as evidence-backed `SIGILL`, not `unknown`.** Spawned daemons record an owner-only exit receipt bound to the exact session, PID, and process-start fingerprint; the live hub consumes only an exact match after disconnect, rejects stale PID epochs, distinguishes a still-live transport loss, and leaves absent or malformed evidence honestly unknown. The replacement guidance therefore identifies portable-release remediation only when the operating system actually reported `SIGILL`.

## [2.55.1] - 2026-08-09

### Fixed
- **Do not install the Linux `2.55.0` artifact; use `2.55.1` instead.** The `2.55.0` workflow checked an intermediate Ghostty archive while the final linked executable still contained runtime-dispatched AVX-512 assembly from AWS-LC and BLAKE3. CAS already selects ring as its process-wide TLS provider, so the unused AWS-LC provider is no longer compiled into Linux releases; this intentionally omits AWS-LC's post-quantum-capable paths, which CAS did not exercise. BLAKE3 keeps its portable, SSE, SSE4.1, and AVX2 paths, while an audited 1.8.6 build override makes its upstream runtime-only `no_avx512` feature omit the inactive AVX-512 archive entirely. Explicit portable Rust, C, C++, and Zig targets cover the remaining final-link contributors. Hashes and stored fingerprints are unchanged; AVX-512-capable hosts may see lower throughput only in BLAKE3-heavy indexing and fingerprinting. The strict ISA scanner remains unchanged; the workflow now also checks the locked release features and audits the exact staged executable before it can be uploaded. The immutable `2.55.0` tag, release, and assets remain unchanged for traceability.

## [2.55.0] - 2026-08-09

### Added
- **Commander provides one phone-friendly view across paired CAS machines.** Each machine can run a durable local hub, expose it through an explicitly managed Tailscale Serve route, and contribute its live sessions and terminal panes to a controller-origin catalog without creating another runtime session or model request.
- **Live terminal viewing and control now have an explicit concurrency model.** Multiple observers share one bounded upstream connection per daemon session, one identified controller holds input at a time, slow viewers are isolated, and the embedded offline client supports pane selection, resize, targeted interrupt, and attributed messages through additive protocol negotiation.
- **Browser control is bound to the paired device, origin, operation, and short-lived proof.** Non-extractable device keys, DPoP request binding, exact Origin/CORS handling, one-use pairing and WebSocket credentials, scoped authorization, revocation, controller leases, and attributed audit all fail closed; non-loopback plaintext service is refused.

## [2.54.1] - 2026-08-09

### Fixed
- **The Linux x86_64 release no longer inherits AVX-512 from the build runner.** Ghostty VT now receives an explicit portable Zig target for every supported native and cross build, unknown targets fail closed instead of falling back to the host CPU, and the release path audits the bundled Ghostty archive for forbidden EVEX/AVX-512 instructions. Anyone who installed `2.54.0` should upgrade to `2.54.1`; the original `2.54.0` tag and artifacts remain unchanged for traceability.

## [2.54.0] - 2026-08-09

### Added
- **Relevant project context now arrives automatically at the start of a turn.** CAS creates one bounded query vector and searches knowledge, code history, and the current source index together, then presents only the best role-relevant matches. The path is on by default for authenticated installs, has explicit latency and corpus limits, falls back safely when semantic search is unavailable, and does not turn prompts into stored memory.
- **The live source tree is now a first-class semantic search corpus.** Code files are reconciled automatically, embedded through their own queue and cache, and retired from every index when deleted. Exact-symbol history queries now prioritize the commit that actually touched the requested symbol instead of merely mentioning the same text.
- **"Is this fixed?" can now be answered against the binaries that actually ran.** CAS records executable epochs for its background processes and separates pre-fix, mixed-version, and clean post-fix evidence. Verdicts always include the observed sample size and say when the post-fix window is too small or has not begun, rather than returning an unsupported bare "fixed".
- **The developer updater is now tracked, installable, and safe around running CAS processes.** `contrib/shell-helpers/install.sh` installs `cas-update`; plain `cas-update` builds, installs, migrates, syncs, and turns over only processes whose executable bytes and process-start fingerprint match the replaced binary. `--no-restart`, `--build-only`, `--sync-only`, and `--dry-run` provide explicit narrower modes.

### Fixed
- **Cloud knowledge sync now preserves ownership and deletion truth.** Personal pushes are incremental and carry their repository identity, team pulls and pushes stay within the active team, foreign pages are rejected at ingest, and tombstones propagate deletions instead of allowing removed pages to return.
- **Migration discovery can no longer skip a lower gap or trust a false ledger row forever.** Detection stops at the first missing migration, safe additive migrations recorded without their actual schema effect are reconciled with an audit trail, and the release path automatically runs component-output snapshots whenever the migration registry changes.
- **History and source indexes no longer publish partially reconciled state.** Watcher, vector, and deletion races are closed; doctor reports missing or stale history tables instead of treating them as an empty repository; lag continues to age honestly; and provenance coverage remains visible even on warning paths.

## [2.53.0] - 2026-08-08

### Added
- **CAS can now search the history of your code, not just its current state.** Every commit in the repository is indexed — subject, body, the files it touched and, where the symbol index has data, the functions and types whose lines it changed — and that index keeps itself current in the background rather than needing a command typed at it. You can ask what a query returns across that history from the command line or through the tool surface, and the same history is now a full-standing channel in the blended search everything else already uses, so asking a question about the codebase can be answered by what was done to it and why, not only by what the files say today. Files that keep changing together are reported alongside a result, which is the fastest way to find the second place a change always has to land.
- **A commit can now say which piece of work and which session produced it.** Resolving that link previously depended on a table that had been empty for its entire existence; it is now populated, and each link records both how it was established and how much confidence that method earns, so a reconstructed association is never presented as an observed one. Coverage is reported honestly rather than assumed.
- **Issues, pull requests, their comments and past release notes are indexed alongside the commits.** The discussion around a change is usually where the reason for it lives, so the searchable corpus now covers the written record as well as the diff.
- **The symbol index actually runs.** The tree-sitter index of functions, types and methods had never produced a single row on any installation: the command the tool told you to run did not exist, and the background job that was supposed to do it was gated on the machine being idle — which a working machine never is. The command exists, the index is built and kept fresh automatically, and it is on by default. Quiet moments are still preferred, but politeness can now only defer the work, never cancel it: once the index has gone five minutes without a refresh it is rebuilt regardless of load, and says that it did.
- **Search vectors are computed automatically instead of waiting for someone to remember.** Embeddings were produced in exactly one place — inside a manual sync command — so whether your knowledge was searchable by meaning depended on whether a human had recently typed something. A backlog of over a hundred pages had duly sat unembedded. A logged-in install now drains its own queue in the background and converges to zero pending, across both the knowledge corpus and the new history index.
- **"Is this bug fixed?" is now answered against the software that was actually running, not the date a fix was tagged.** A fix does not start working when it is released; it starts working when the processes serving it restart, and older processes routinely keep running for a further half hour. Anything observed in that overlap comes from both versions at once and proves nothing about either — reading it as evidence of a fix is a real mistake this project made and had to withdraw. CAS now records, for every background process it starts, which binary it is running and how long it was seen alive, and reconstructs that timeline for processes that ran before this landed. A question about a symptom is answered in three parts: the window before the fix ran, the ambiguous overlap, and the clean window after the last old process finally stopped — with the overlap excluded from the verdict by rule rather than by convention. The answer is never a bare "fixed": when the clean window is too small to support the claim it says so and reports how much evidence it actually has, and when no process has yet been seen running the fixed build it says that instead. Replayed against the incident that motivated it, the boundary it derives from live records matches the one that had to be established by hand.

### Fixed
- **A worker running under a second account now receives its messages.** When a session is started against a configuration directory other than the default, its harness reads mail from a mailbox inside that directory — and every routine delivery was being written to the sending daemon's own directory instead, where nothing reads. Such a worker booted deaf: only a forced interruption could reach it, and everything else sat unread forever. Messages are now written into the recipient's own tree, with the roster it needs to make sense of them, while single-account installs are untouched.
- **A session sitting idle with unread mail is now woken to read it.** Thirty-four of thirty-five wake attempts across an entire fleet were declined, every pass, and the only wake that ever landed was a hand-forced interruption. The cause was not any signal from the session: the search for a session's transcript looked in one hardcoded location, so on any machine using a second account it found nothing for every session, and "no transcript" was being read as "busy, do not disturb". Neighbouring checks had always read the same absence as "not busy", which is why one command cheerfully reported a session as available while another refused to wake it. Transcripts are now resolved across every known configuration directory, and an unknown state is no longer allowed to masquerade as a definite one.
- **A message you were interrupted to read no longer comes back.** The path that breaks into a session with an urgent message recorded the delivery in the sender's ledger but never in the per-recipient one the recipient's own unread check reads, so a message that had been delivered, read and acted upon was still eligible to be served again. Every terminal delivery path now writes the receipt, keyed by the name the recipient actually answers to rather than the pane it happened to be typed into.
- **Embedding a large batch no longer fails silently and permanently.** Requests were sent with every pending item in one call against a server that hard-caps them at thirty-two, so any backlog above that size produced a permanent rejection — and the rejection was logged as a warning and otherwise discarded, making the visible result "nothing was embedded", forever, with no error anywhere. Requests are now split at the real limit, and an oversized one is refused before it is sent rather than after.

## [2.52.0] - 2026-08-08

### Fixed
- **Almost every notification about a piece of work changing hands was being destroyed before it could be sent.** 353 of 361 supervisor relays over four days never reached transport — including 34 of 36 "this is ready for you to merge" and 34 of 36 "this close was rejected" notices. The cause was a freshness check that compared the notification's timestamp against the task's timestamp for exact equality, while the two values were read from the clock at two different moments; the test could never pass, and every one of the 397 discarded notices came from that single line. Measured across the discarded rows: zero exact matches, and one missed by 21.9 microseconds. Freshness is now the question it was always meant to be — is the work still in the state this notice announces — which no clock skew can defeat, while the one thing a timestamp can decide soundly is kept. The worst case is a duplicate courtesy notice when a task re-enters the same state; the previous worst case was total silence. Separately, notices withdrawn because their premise expired are no longer filed under the same label as routine de-duplication: a four-day outage sat hidden inside a bucket that reads as normal housekeeping, so a withdrawal now says a decision was made and records which work moved on. (GH #167)
- **A message you had already read, acted on and replied to no longer comes back.** Whole bursts were being re-served. The suspected cause was a second copy of the message; the live records say the duplicate was in the bookkeeping, not the message. "Read" was determined from a per-recipient receipt ledger that only two of the delivery paths ever wrote to — the path that actually hands a message to a session wrote none — so a message could be reported delivered and be simultaneously unread by the recipient's own check, which then handed it back. Every path that declares a message terminally delivered now records the receipt, which matters most for broadcasts, where that ledger is the only thing that can ever retire one. Delivery still does not count as acknowledgement; nothing about what a real reply proves has changed. (GH #176)
- **A supervisor's mail stopped hiding from the supervisor.** A supervisor answers to two names — its own and the generic role everyone addresses it by — and the two readers of the receipt ledger had drifted apart on which names to resolve. A message sent to the role name was unreachable from the supervisor's own inbox check: 40 of 50 such messages were never receipted, against 15 of 59 for the personal name. A message retired under one name also kept no record under the other, so the reader that missed it surfaced it again on a later turn. Both readers now share one identity resolver and a receipt is written for every name the recipient answers to. (GH #176)
- **Sending a message no longer leaves an unreadable copy accumulating in a second place.** When a session runs against a non-default configuration directory, the underlying tool wrote its own copy of every outgoing message into a mailbox tree nothing ever reads and nothing ever prunes. It grew without bound and then arrived as a stale burst the moment any similarly named session started there. Those strays are now marked inert as they appear, and only in trees that were conjured by that write — a real mailbox is never touched. (GH #176)
- **One stuck message no longer floods the log with tens of thousands of identical lines.** A single message wrote 16,604 lines in thirty flat minutes — 12.5% of everything logged that day — because the announcement was emitted before the check that decides whether to actually retry, so the check correctly declined and the line printed anyway, once per hundred-millisecond poll. The line now rides the retry itself and names the attempt number, so volume tracks the deliberate retry budget rather than an internal polling interval. (GH #166)
- **A recorded change no longer disagrees with the record it was written from.** Saving a task re-read the clock rather than reporting the moment it actually stored, so any downstream record derived from a save was stamped microseconds away from the row it described and could never be matched back to it — the mismatch behind the notification loss above. A save now returns exactly the instant it persisted. The test doubles used by the suite also stopped honouring a caller-supplied timestamp, closing the divergence from real storage that made this whole class of defect untestable.
- **Stored commit identifiers are now complete.** Commit fingerprints were recorded at whatever abbreviated width the version-control tool chose at that moment, which grows with repository size, so the records held a mix of widths and anything reading a fixed number of characters silently skipped a large share of them. Full identifiers are now stored and shortened only for display, making every new record an exact match.
- **A fleet-wide rebase no longer drags workers onto a branch they have nothing to do with.** Refreshing everyone against one line of work rebased every session, including those on unrelated standalone work, grafting unrelated unmerged commits onto them and rewriting commits that had already landed — which then made the finish-line check miscount and refuse correct work. Each session is now refreshed only if the branch it actually integrates into matches the one being refreshed, and a skip says which work and which branches. Naming a session explicitly is still an override; consenting to rebase over uncommitted work is not.
- **The sync report now credits the harness that was actually written to.** Updating built-in files printed its summary and its file list under whichever destination heading happened to print last, so a write to one location was reported under another that had not been touched at all, and two of the three destinations were never reported in readable output. Each destination now reports inline, immediately after its own sync, and every claimed write names the directory it landed in.

### Added
- **A scoped test run can no longer report success while running nothing at all.** Three separate runs exited successfully having executed zero tests — a wrong crate name, a path that resolves differently depending on where it is run from, and a filter matching nothing — and all three were read as green. A wrapper now requires three things together: the command succeeded, a test harness genuinely reported, and the number that passed is above zero. The middle one carries the weight, because a success code is exactly what failed in all three cases. (GH #173)

## [2.51.0] - 2026-08-07

### Fixed
- **A message written to a session's inbox was never actually put in front of that session.** The previous release built the turn-start surfacing path that reads a recipient's unread queue and injects it into the turn that is starting. It had seven passing tests and it had never once run in production. The event it hangs off delivers the submitted text under one key; the code declared a different one, and — the part that made this invisible for a full release — the real key was declared on an unrelated, unread field, so nothing failed, nothing warned, and the handler simply returned before reaching the surfacing block. Independent corroboration that the handler had never got that far: the attribution table it also writes held zero rows across the entire life of the database, so the command that reports who wrote a line had never had data to report. The key is now read where it is actually sent, and surfacing was moved ahead of the early return it was sitting behind, so a blank turn can no longer swallow a turn's mail; either change alone restores delivery. Confirmed against a real waiting message, not a synthetic one. The regression tests parse the raw event as it arrives on the wire — every prior test built the payload by hand, which is exactly why a contract mismatch survived a release with a green suite.
- **Two more features were dead on the same wire, found by capturing real events instead of trusting the documentation.** The audit that followed the above deliberately read live captured payloads rather than inferring the shape from our own types, since that circularity is what hid the first defect. It found the guard that keeps long assistant output from wedging the interface reading a whole-message field that is never sent — the text arrives as streaming fragments — so the feature could never have worked had anyone switched it on. And the signal that says "you are already being resumed by a previous stop request" was sent on every relevant event and read nowhere, while five separate places could block a session from stopping, with no way to know they were inside a loop of their own making. Both are now wired to what the wire actually carries. A companion rule requires payload tests to parse raw captured events, so this class of silent mismatch cannot be reintroduced by a hand-built test object.
- **A worker dying no longer leaves its supervisor uninformed.** Death notices were written to one queue that a supervisor only sees if it happens to look, never to the path that actually reaches it, and they were re-emitted every time the death was re-detected — one incident produced over fourteen hundred copies. A death now writes to both places in one idempotent sequence keyed on the death itself, so re-detection collapses onto a single notice while a genuinely separate later death is still reported. The notice carries a wake signal, so it can rouse an idle supervisor and, if it never lands, shows up in the undelivered report instead of vanishing.
- **The delivery-attempt counter now counts the retries that actually happen.** Across nearly eight thousand messages it had never once incremented — not because the writer was broken, but because it was wired exclusively to rare error branches this system had never taken, while the loop that really re-sends a message counted in memory and lost the count on every restart. Attempts are now recorded durably alongside the reason, in one transaction, so a message cannot be seen with a stated reason and no attempt behind it. Being withheld by policy still deliberately costs nothing — a cooldown is not a failed attempt — and a health check now names the messages burning through attempts before they exhaust their budget, which is the only window in which anyone can act. Historical rows are left at zero rather than back-filled; there is no evidence of what their real counts were.
- **A message the recipient genuinely read no longer records itself as still waiting to be read.** Rows acknowledged by the turn-start path were immediately overstamped as delivered-and-awaiting-acknowledgement, crediting the wrong source. No delivery decision was affected — every one of those already keyed off the acknowledgement itself — but the raw records are what post-incident analysis reads, and that state produced two claims that had to be withdrawn. Such a row now records the acknowledgement it holds and names the path that produced it.
- **A completed lane could be refused at the finish line for evidence a supervisor had already produced.** The guard that requires proof a change actually landed did not recognise a supervisor's merge as that proof, so work that was merged correctly still read as undelivered and had to be re-argued by hand. A merge commit now counts as the evidence it is.
- **A background CAS server no longer keeps a project's database open after the session that started it is gone.** Servers were being left behind — four of them on one machine, still holding write-side handles on the shared project database a day and a half after the tools that launched them had died — and because they sit idle they show up in no status view. The mechanism everyone assumed was responsible turned out to work: measured directly, a server whose launcher is killed does shut itself down when its input stream closes. The leak needs the input stream to stay open with nobody on the other end, which happens whenever that stream is a terminal, or was inherited by some unrelated process that is still running, and no amount of input handling fixes it. The server now watches for the disappearance of the process that started it instead, and shuts down within seconds of confirming it, releasing whatever work it was holding on the way out. Confirmation deliberately takes two independent facts — the launcher has been replaced *and* the replacement is the operating system's adopt-orphans process — so a session that is merely quiet is never mistaken for a dead one, and a server started deliberately in the background is left alone entirely. Each server only ever inspects its own launcher and only ever exits itself, so a cleanup in one project cannot reach a live server in another on the same machine.
- **A command-line tool could talk to a socket that no longer belonged to a running program.** Socket election did not verify that the process behind an existing socket was alive and running the current binary, so a stale or superseded listener could keep answering. Election now requires a live server on the current binary before a socket is adopted.
- **Searching stored knowledge for several words at once stopped requiring every one of them.** A multi-word query silently behaved as "all of these terms", so a search that named four related concepts returned nothing while each concept individually had matches. It now ranks results that match more terms higher instead of discarding everything short of a perfect match.
- **Referencing a built-in skill written before the reference ledger existed no longer reads as a broken link.** Older references were being flagged against a ledger that post-dates them; those are now grandfathered, and any reference genuinely skipped at session start is named in a banner rather than dropped in silence.
- **The test suite no longer writes into the real stores it is running next to.** Tests were reaching live project and global databases and left almost a thousand fixture records behind. Tests are now isolated to their own temporary stores, a tool removes the residue and records exactly what it deleted, and thirteen orphaned schema-migration files with no owner were removed with a guard against their reappearance.
- **A wedged test harness can no longer consume ten minutes of a run doing nothing.** The wait for a response is now bounded, so a hang fails fast and legibly instead of burning the job's budget.
- **Retrieval quality measurement now includes the global store instead of quietly dropping it.** The parity check was scoped to project storage only, so a whole tier of what a session actually retrieves was invisible to the numbers everyone was reading.

### Added
- **Reports now ship as a single self-contained HTML file.** A built-in skill produces a report that opens correctly anywhere, with no accompanying folder of assets to keep together or lose.
- **Workers are taught when a fast check is enough and when only full proof will do.** The distinction between an inner-loop check while iterating and the final evidence a piece of work is done was previously left to judgement, which produced both wasted full runs and claims backed by a scoped one.

### Changed
- **The built-in worker and supervisor guidance is substantially shorter.** Project-specific material that had accumulated in shared guidance was removed and a size cap now keeps it from growing back, so every session pays less to be told what it needs.

## [2.50.0] - 2026-08-07

### Fixed
- **A finished piece of work could be parked behind a supervisor who was never told about it.** One session relayed "this lane is ready to merge" notices normally for a while and then went silent for the rest of the day; four completed lanes produced no notice at all and a person ended up carrying the hand-off by hand. The suspected cause — a notice addressed to a session id captured too early — was not what happened. Every notice existed and every one was written to the supervisor's inbox; each was then re-sent on a one-minute cadence because nothing ever drained that inbox, and each was finally stamped as a withheld duplicate at the exact second its task moved on. That stamp was correct about the payload, which had genuinely expired, and fatal for the fact that a notice had failed to arrive, which nothing anywhere recorded. Three things changed, one per layer. A notice that expires without ever being transported is now recorded as a distinct failure rather than being filed alongside "we withheld a copy nobody needed" — conflating those two is what made this invisible for a full day. That failure is surfaced where people already look: a banner above the worker roster, rendered even when no agents are registered, and a check in the health command that warns rather than reporting health when it cannot read the queue. Because both read columns the queue was already writing, the incident is visible retroactively, not just from now on. And the trigger itself is closed: when a supervisor's session restarts mid-run it re-registers under the same pane name with a new identity, and the tie between the old and new rows was being broken by sorting on a random id — a coin flip that could hand every later notice to the identity the operator had already walked away from. Ties now resolve to the session that exists, so a notice sent after a restart reaches a live recipient. Liveness still outranks recency, so a freshly registered but shut-down row cannot swallow notices.
- **A notice retried forever instead of ever reaching a conclusion.** Exempting undelivered notices from the withheld-duplicate stamp closed one silent path and opened another: the stamp only fires when a task leaves the state it is waiting in, so a lane parked behind a supervisor who never came back had no ending at all and would re-send indefinitely. Retries are now bounded — long enough that a merely busy supervisor always wins the race, short enough that an absent one produces a recorded failure instead of a zombie. Every notice now reaches exactly one of delivered or visibly failed. Only a real send attempt counts against the budget; waiting out a cooldown does not.
- **A log line said "delivered" about a message that was never delivered.** The arm that logged success actually fires for an inbox write, which for a message awaiting a turn boundary is not delivery — the row stays untransported and is rewritten every cadence tick. One message logged "delivered" nearly 56,000 times while its row ended up abandoned, never transported. A log line that contradicts the row it describes is a large part of why this took so long to diagnose; deferred writes now say that is what they are.

### Removed
- **Three superseded storage and search paths are gone.** The distilled knowledge library has fully taken over from the older layered store, the markdown-backed store, and the standalone hybrid search path, so those are removed rather than left as a second way to do the same thing. Behaviour for anyone using CAS is unchanged; what goes away is dead weight and the ambiguity of two code paths claiming the same job. A survey documenting what was retired, what replaced it, and what deliberately stays is included alongside.

## [2.49.0] - 2026-08-07

### Fixed
- **A message could be marked delivered to a session that never saw it.** The queue had a delivery path and no surfacing path: a row was written into the recipient's inbox file, stamped `delivered`, and nothing anywhere read it back and put it in front of the recipient. The two explanations that had been argued over — the wake-up never fired, versus a turn starting without a drain — were both true for different populations, and underneath both sat a third defect nobody had named: no hook handler read the queue at all, and the one handler that could have was scoped to a single role and returned early before it could surface anything. A turn-start handler now drains a recipient's unread rows and injects them into the turn that is starting. Selection and receipt happen in one transaction, so a caller can never end up holding content whose receipt failed to persist — the storm guard and the silent-drop guard are the same invariant. The turn-start event is now installed in the generated hooks block, where a handler wired to it would previously have been dead code for exactly the population being stranded; the other twelve events are deliberately untouched. Polling an inbox remains non-consuming.
- **Whether a session was ever woken is now measured rather than asserted.** "Wake: unobserved" was a hardcoded constant with no backing column, which is why three separate incidents produced no signal at all — and the nudge helper returned the same `Delivered` outcome from its success arm, its deferred arm and its error arm, so the three states it already computed were being discarded. They are now carried and persisted (fired, failed, not attempted), status output reports the attempt, and it names the specific signature of a nudge that fired with nothing surfacing behind it. Urgent delivery records an attempted wake too, so the gated and ungated paths are finally comparable. Migration `m220` adds the receipt-source column.
- **Clearing a session's context did nothing while reporting success.** The request enqueued the four characters `/clear` as an ordinary queued message; under team routing that row goes to an inbox, so the recipient read the *string* "/clear" as a note, acknowledged it, and carried on with its entire conversation still loaded — while the tool answered "queued". Six such calls across four sessions in one sitting all "succeeded" and none reset anything, so the checkpoint-and-clear discipline silently degraded into working to exhaustion. The reset is now a control instruction matched ahead of every message-routing path and typed over the same interrupt-and-inject channel urgent traffic uses, so it can no longer land in an inbox. Its post-condition was measured against a real session before anything was built: a genuine clear starts a new session whose transcript records the command. A reset that cannot be proven returns an error naming exactly what was and was not observed, never a cheerful "queued", and the confirmed new session id is written back so subsequent status and activity lookups read the live transcript instead of the dead pre-reset file. Harnesses where the reset is unsupported are refused before anything is queued, rather than guessed at.
- **A review-ownership setting was accepted, reported as unknown, and then ignored.** The runtime always read `code_review.owner`, but it had no command-line surface, so setting it to defer reviews produced an "unknown config key" response — and then the expensive multi-persona review ran five more times against the stated policy. Three layers were broken and instruction alone had already failed to fix any of them. The key is now real to the CLI (get, set and list, with an absent section reporting the owning default and `set` refusing anything outside the two valid values), so the effective policy can be audited. The refusal now covers the entry points that actually spend the tokens, including the current agent-spawn spelling — which had been in no generated matcher and therefore had no seam at all; it is intercepted, never auto-approved, and verifier spawns stay exempt. And the completion path no longer demands from one party the exact artifact the policy says another party produces; a solo caller with nobody to defer to is deliberately still asked for it. The honest limit is documented alongside: the refusal is a pre-tool hook, so a session running on hand-edited settings is advisory again.
- **A new working area could be cut from a months-stale branch, or from the wrong branch entirely.** Two defects on the same path. Work with provably no parent grouping was indistinguishable from work whose grouping could not be determined — both answered "none", and "none" fell through to whatever the operator had pinned; the three states are now distinct, unparented work bases on the trunk, and because that counts as a divergence the override is announced rather than silent. Separately, base resolution read the local branch without ever consulting the remote's copy of it, so a checkout was cut from a ref 71 commits behind a current one sitting right next to it. Local strictly behind now cuts from the remote commit and names both commit ids and the size of the gap; a genuine divergence keeps the local ref but reports the split; ahead, equal, or no remote is unchanged and silent.
- **A command that writes somewhere other than where you are standing now says so.** Root resolution checks the `CAS_ROOT` environment variable before the working directory, and nothing anywhere said so — so an operator who copied a store to a scratch directory, changed into the copy and ran a rehearsal had the rehearsal write to the live store instead. Every change-into-a-copy workflow inherits that trap. The precedence is deliberately unchanged, because checkouts and worktrees depend on it; what changes is that the losing candidate is named out loud, once per process, at the layer all thirty-nine call sites share, together with the one-line way to opt out. The notice goes to stderr and never stdout, so JSON output, hook payloads and stdio framing stay parseable, and no notice is emitted when both candidates resolve to the same directory by different spellings.
- **Help text no longer advertises a build flag that does not exist.** Session recording was documented as requiring a feature flag that had been removed, sending operators to look for something they cannot pass for a capability they already have; the harness options in the same struct listed a mode name the parser rejects, and whose own error message names the real one. Documentation only — no behaviour changed.

### Added
- **Legacy notes can be moved into the distilled knowledge library, and moved back out.** The migration previews by default and writes only when told to, records every page it creates in a ledger, and reports honestly on what it drained rather than rounding up. Its rollback is driven from that ledger rather than by restoring a database backup — deliberately, because the database also holds tasks, leases, sessions, verification records and queued messages that are being written continuously, so restoring it would discard more work than it recovered. Anything the migration never touched cannot be affected, because it is not in the ledger; a page whose stored path no longer matches what the ledger recorded is reported as diverged and left alone, and divergence is not counted as success. Building the rollback so it could be exercised caught a real defect in the migration itself: restored rows were being routed to whichever database happened to have the table, which put one store's rows into another — payloads now carry their origin and an unstamped payload is a hard error rather than a guessed destination. Rehearsed against copies of real databases, the post-rollback state matched the pre-migration state on every axis measured.
- **A retrieval-parity harness proves search does not regress across a migration.** A fixed query set is captured and replayed through read-only channels and the results diffed, so a cutover can be shown not to have degraded retrieval instead of being assumed not to have. Recapturing the baseline inside the frozen window is now a required step, because entries written after a baseline shift a fixed result window and produce parity "regressions" that are nothing of the kind.
- **Content that belongs to a different project is held back from distilled pages.** A cutover rehearsal put another project's client records at the head of the session briefing. Quarantine matching is proper nouns only, chosen against the real corpus: three obvious-looking generic terms were rejected because they match ordinary prose and type names in this codebase, and sixteen further candidates added nothing beyond the proper nouns. Both directions are pinned by tests.

## [2.48.3] - 2026-08-07

### Changed
- **The factory supervisor can be steered remotely again, and it stays patched.** Every agent the factory launched was started with non-essential network traffic switched off and its updater pinned. That is the right posture for a worker — a worker must not swap its own binary partway through a piece of work — but it also silently removed two things from the one session an operator actually sits with. Remote Control depends on feature-flag evaluation, which the traffic switch disables outright, so `claude doctor` inside a supervisor reported the feature as unavailable and its rollout unverifiable. The same switch bundles the updater kill switch, so a long-running supervisor never picked up a security fix. Both settings are now applied to workers only; the supervisor gets Remote Control and auto-updates, and worker behaviour is byte-for-byte unchanged. A machine that has been running with the traffic switch set for a long time may hold frozen feature-flag evaluations in `~/.claude/statsig` (or the equivalent path for an alternate config directory); deleting that cache clears them. One trade-off worth watching: with the updater live, a supervisor can update the shared CLI binary mid-run, so workers started either side of that update may differ in version.

## [2.48.2] - 2026-08-07

### Internal
- **Nothing users run changed in this release: it corrects a test that was mismeasuring a correct product.** The wiring test for a sync run asserted that each pull endpoint is requested exactly once, and it had been failing — reporting that the personal pull happened twice. It did not. A sync makes two genuinely different pulls that happen to share one URL path and are told apart by their query string: the personal pull, and the knowledge pull that asks for distilled pages. The test recognised requests by path alone, so it counted the knowledge pull as a second copy of the personal one. The failure was therefore a description of two requests the product is supposed to make, not a duplicate to be removed — deleting one, which is what the reported diagnosis called for, would have broken knowledge sync outright to make a test pass. The assertion is now made per endpoint rather than per path: the personal-pull expectation requires the discriminating parameter to be absent, and the knowledge pull is asserted in its own right instead of being silently absorbed. "Each pull endpoint exactly once" now means what it says, and the knowledge tail is covered rather than invisible.

## [2.48.1] - 2026-08-07

### Fixed
- **Syncing team knowledge could ask the cloud for every project's pages, not just this one's.** The knowledge pull built its own request and, whenever it could not work out which project it was running in, simply left the project off the request instead of stopping — so in exactly the situation the rest of sync treats as fatal, this one path quietly asked the server for everything and could import another project's pages into your database. That is the cross-project contamination the previous release was cut to clean up, reopened for knowledge pages. Every pull now goes through a single builder that refuses to make the request at all when the project cannot be determined; there is no longer any code path that can produce an unscoped pull, and a test proves the unresolvable case aborts without building a URL.

## [2.48.0] - 2026-08-07

### Internal
- **Database migration numbering.** The knowledge store's migration was developed as `m218` on a feature branch while `m218_prompt_queue_recipient_transport_create_table` shipped independently in 2.47.0. Because a released id is immutable, the knowledge migration was renumbered to **`m219`** before landing; the released `m218` is untouched and applies exactly as it did in 2.47.0. Upgrading from any published release — 2.47.0 or earlier — is unaffected: those databases have never seen either number in the other meaning, and they apply `m218` then `m219` in order. The only database that could misbehave is one that ran a pre-release build of the feature branch itself and therefore recorded id 218 against the knowledge migration; on upgrade it would treat the released `m218` as already applied and skip it. Such a database is not expected to exist outside a development checkout, and the skipped table is additionally created as a startup side effect, so even that case self-heals.

### Added
- **A project can now explain itself to an assistant without anyone writing the explanation.** Understanding an unfamiliar area meant an assistant reading its way there file by file, every session, from scratch — the same expensive rediscovery repeated on every new conversation, and the same questions asked of you again. `cas knowledge build` reads the project's own documentation, README, agent instructions, key configuration and a summary of every indexed code module, and distills them into a wiki of prose pages. The pages are ordinary markdown on disk under `.cas/knowledge/`, so they stay greppable, hand-editable and reviewable in a pull request like any other file. `cas knowledge status`, `list`, `search` and `read` cover the rest of the surface. Distilling costs model tokens, so nothing runs automatically unless you opt in — a pass over an unchanged project is guaranteed to cost nothing at all, because every source is fingerprinted and skipped when it has not moved.
- **A page you write or edit by hand is never overwritten by the machine.** The obvious failure of any generated-documentation system is that it eventually destroys the thing a human corrected. A page can be locked, and a locked page is untouchable from every direction at once: re-distillation cannot rewrite its text, its index row or its file; a cleanup pass that removes pages whose sources are gone will not remove it; and a teammate's copy arriving over sync cannot overwrite it either. Text you write above the first generated section is treated as hand-written and is never edited, even on an unlocked page.
- **Sessions start knowing what the project knows.** The startup briefing now includes a one-line pointer to every distilled page — id, type, title and a short snippet — and an instruction to pull the full text of the ones that matter. Page bodies deliberately never enter the briefing: an index of fifty pages costs a fraction of what one body would, and the assistant fetches only what the actual question needs. The index is capped, fits inside the existing briefing budget, and is byte-identical between runs on an unchanged project so it does not defeat prompt caching.
- **Distilled knowledge is searchable from both the command line and an assistant.** `cas knowledge search` does full-text search across pages, and a new `knowledge` tool gives assistants search, read, write, list and status directly. Project search also gained knowledge as a source: a query matches page text, and pages connected to what you asked about through the project's entity graph surface too — with connected results always ranked below anything that literally matched, so an indirect link can never outrank a direct hit.
- **The codemap and project-overview skills became views over the knowledge store instead of one-off generators.** They now consult existing pages before regenerating and feed what they produce back in, so the documentation they write and the knowledge an assistant retrieves are the same body of text rather than two that drift apart.
- **Teams can share distilled knowledge, and search gets sharper when the cloud is connected.** With an account connected, pages sync alongside memories, tasks and rules, and get semantic embeddings so search matches on meaning rather than only on shared words. Everything here is strictly additive: logged out, no network call is made and no extra files are created on disk, and the local project remains the source of truth either way. `cas cloud status` reports how many pages exist and how many are still awaiting embeddings.

### Fixed
- **Conceptual searches stopped silently discarding most of their own scoring.** Search blends several ranking signals with a fixed weighting, and one of them — meaning-based matching — had been removed without the weighting being updated. Sixty percent of the weight on every conceptual query was allocated to a signal that could only ever return nothing, so every result was scaled down and the remaining signals were left in the wrong proportion to each other. Ranking signals now declare whether they can actually answer, and a dead one's weight is redistributed across the live ones in proportion, preserving the intended emphasis instead of quietly deleting it. The same check stops the meaning-based channel from claiming it can answer when it is connected but has nothing cached yet.
- **A test that had been failing on `main` since the previous release passes again.** The health-check snapshot was last re-pinned before two new health rows were added, so it had been red on `main` from the moment those rows landed. It also captured a value derived from a randomly-named temporary directory, which would have made any naive re-pin fail intermittently; that value is now excluded before the comparison.
- **The startup knowledge index pointed at a command that did not exist.** The index shipped telling readers to fetch page bodies with an action the tool does not accept, so every fetch it invited returned an error — a perfect-looking index where nothing behind it worked. The instruction now names the real action, and a test drives that instruction through the actual tool router so the text and the thing it describes cannot drift apart again.

## [2.47.0] - 2026-08-07

### Fixed
- **Two different repositories that happen to sit in folders with the same name no longer sync into each other.** A project's cloud bucket was decided by its parent-folder name whenever no explicit id was pinned, so two unrelated checkouts both called `accounting` shared one bucket and merged each other's memories, tasks and rules on every sync — for months, across two different clients' work. The git `origin` remote, which identifies the repository rather than where it happens to sit on disk, is now consulted before the folder name; an explicit `cas cloud project set` pin still wins, and a project with no remote still resolves by folder name exactly as before. Because that changes which bucket an unpinned repository with a remote uses, `cas doctor` now reports which bucket the project resolves to and why, and names the exact command to pin the previous one if that is where the synced data lives.
- **`cas doctor` warns when two local projects claim the same cloud bucket.** Nothing anywhere reported a collision — the only symptom was one project's notes turning up in another. Doctor now checks every known local project and raises a warning naming both directories and the shared id. Second clones and git worktrees of the *same* repository are correctly silent; only genuinely different repositories are reported.
- **`cas cloud purge-foreign` can no longer quietly destroy the work it is meant to protect.** Its `--dry-run` reported only how many rows existed, never which ones, so the one preview available before an irreversible delete told you nothing about what you were about to lose; the dry run now lists the concrete delete set (id + title for every entry, task, rule and skill, plus the dependency-edge count) and, with `--json`, the whole set. A real run now refuses — naming the reason — when the last successful cloud pull is missing, unreadable or older than the threshold (`--stale-days`, default 7), or when local changes are still queued and have never reached the cloud; on a long-idle machine the old behaviour deleted everything local and re-pulled a months-old snapshot over it. `--force` is the explicit override. The pre-purge backup is taken with `VACUUM INTO` instead of copying a live WAL database file, which silently omitted every committed transaction still sitting in the `-wal` sidecar — the backup was unreliable exactly when it mattered. A purge whose queue of pending local changes cannot be read now stops and names the reason: that read used to answer "nothing pending" for corruption, schema drift and undecodable rows alike, which disabled the unpushed-work refusal inside the one command that deletes without asking twice.
- **`cas doctor` reports other projects' rows sitting in this project's database.** A sync leak fixed months ago left every database on a multi-project machine carrying copies of other projects' tasks, and nothing ever reported it — frozen replicas of long-finished work still read as open, so a ready list showed another project's backlog as this project's outstanding work. Doctor now scans for that contamination by default, read-only, and reports what it finds; `--foreign-rows` lists every row rather than summarising. Rows are matched on id **and** title together, never id alone: short ids genuinely collide, so an id-only sweep would delete real work — rows that share an id but not a title are reported in a separate warning section for exactly that reason. A row is only called foreign when this database has no local trace of it and another does; a copy with no trace anywhere is reported as unattributed rather than accused, and a replica that is merely closed is distinguished from one that is live. Nothing is deleted or modified by the report.
- **Starting a task that belongs to another project warns first.** Replicated rows could be claimed, worked and closed from the wrong repository — one contaminated database held working records for two other repositories' tasks, while the real rows never moved. Start now warns when a task carries no work target, no link to any local parent, an assignee nobody on this machine has registered, and cloud sync is on. It stays advisory — the task still starts — and the warning names the project, the risk and both ways out. Deliberately narrow: an ordinary task simply missing one of those signals is not flagged.
- **A merge that only moved a local branch no longer reports plain success.** `worktree_merge` moved the target branch on this machine and said "merged", while the remote stayed at the pre-merge commit — so the work was invisible to every other checkout and to the close-time merge check, and only a manual push closed the gap. The merge now publishes the branch and states the outcome every time: pushed, already current, no remote configured, or not pushed with the reason. A failed or slow push degrades to a loud "not pushed" instead of turning a completed merge into an error or hanging; a remote that has diverged is reported, never overwritten; and a branch that does not exist remotely is deliberately not created as a side effect.
- **An ordinary message wakes an idle worker again.** Only an urgent interrupt could reliably wake one — four overnight incidents, the worst leaving a worker idle for two and a half hours with a task already assigned to it. The cause was treating "the message left our outbox" as proof the recipient had seen it, when the assistant only files it away for later. Delivery is now confirmed by evidence that the recipient actually surfaced the message; the storm protection that motivated the old shortcut is unaffected.
- **A finished worker's checkout can be cleaned up through CAS.** Cleanup was unavailable in the shared-checkout setup, so a worker that finished without cleaning up on the way out left a checkout with no supported removal path — the workaround was a manual git command that bypasses tracking entirely. Cleanup now works there too and can target one specific checkout by id, branch or owner, instead of only sweeping whatever it judged abandoned. It refuses while the owner is still live, and refuses to destroy uncommitted or unmerged work unless explicitly forced.
- **A rejected close stops sending people to fix a push that already happened.** The rejection text asserted that the epic branch was unpublished purely from its name, so workers chased a non-existent problem while the real gap — their branch not being merged — went unnamed. It now checks whether the branch exists remotely and says whichever is true.
- **A green build no longer implies the test suite ran.** One job's name suggested full coverage while it compiled the tests without executing any of them; its green was read as a pass while the job that does run the suite was red on the same commit, turning a completely reproducible failure into a day-long hunt for a flaky test. The job is renamed to say it is a compile and build guard that runs no suite, and the one job that does execute the suite is marked as such. The name in your checks list changes accordingly: "Full Matrix" is now "Release-Profile & Build Guard (compile-only, no test suite)".
- **A close that cannot verify the tree now says so instead of passing silently.** The check for uncommitted work returned the same empty answer for "the working tree is clean" and "the working tree could not be inspected at all", and closing treated both as a pass — so a tree that had drifted from what was reviewed could close without a word. Those two answers are now distinct: closing still refuses outright when there is uncommitted work, and it now additionally records what it could not verify — an inspection that failed, leftover untracked files, or a checkout sitting on a different commit than the one being claimed. This is detection, not a fix for any specific drift: it makes an unverifiable close announce itself rather than look identical to a verified one. The guidance for completing work carries the same check, including for quick tasks that skip the longer list.

### Added
- **A one-command way to run tests in a clean environment.** Tests that read configuration from the environment passed locally and failed only on a fresh machine, because the shells they usually run in export a pile of variables — that is how one recently-shipped failure got through. The new command strips them all, enumerated from the live environment rather than a hardcoded list that had already drifted, and prints what it removed.

## [2.46.0] - 2026-08-06

### Fixed
- **Finished work can close after the supervisor merges it.** A worker whose branch showed only a sync-merge after the supervisor had already merged its work was refused closure and steered toward resetting the branch — the exact state that success looks like. A close carrying a valid receipt for merged work now passes, and the refusal text for genuinely empty closes names the receipt path instead of implying branch surgery.
- **A reply no longer counts as having read a message.** Any message back from a recipient used to mark every outstanding message to them "confirmed", silencing the sender's escalation clock while the recipient worked on from a stale premise. Confirmation now requires that the reply came after delivery and that the message was actually shown; without that, the clock keeps counting. Assignments the recipient already acted on are no longer re-served verbatim, and any true redelivery is labeled as one.
- **"Delivered" now means the recipient can actually find it.** Delivery used to be stamped the moment the daemon wrote a message down, with nothing on the recipient's side to corroborate it — messages could sit invisible for an entire task while their status read delivered, and one acknowledgment shape could erase a never-shown message from the recipient's inbox entirely. Every delivery now leaves a per-recipient record in the same transaction, only an explicit acknowledgment or a real surfacing hides an inbox row, and an urgent interrupt is not considered done until there is evidence it actually woke its target — retried on a throttle, never a storm.
- **Workers spawn from the branch their task belongs to.** New worker checkouts were cut from whichever epic the dashboard happened to be focused on, not the epic of the task being assigned — every spawn started on the wrong code and needed a manual reset. Base resolution now follows the task, and the spawn report names which branch it chose and why.
- **The worker-side message storm is over.** A handful of real messages could be re-injected as hundreds of duplicates, flooding a worker's context until it was forcibly compacted — two workers died to it in one afternoon. Identical parked notifications now collapse to one entry with a count, and redelivery follows the once-per-interval contract on the worker path too.
- **A shared checkout can't be silently commandeered.** A worker running without isolation could park the shared repository on its own branch, sending every subsequent supervisor merge and tag quietly to the wrong place. That state is now loudly flagged in status output, and the commit guard steers away from committing onto it.
- **The review gate holds at every door.** The rule that reviews belong to the supervisor was enforced at one entry path and bypassable through another; every dispatch route now applies the same refusal.
- **Test results stopped depending on which account ran them.** Six delivery tests failed on any machine whose environment pointed at an alternate configuration directory — noise that cost real investigation time on unrelated work and could have masked a genuine regression. The fixtures now pin their own configuration root, and a regression test runs the same path under a non-default directory on purpose.
- **Epic status tells the truth about branches.** A merged-and-closed lane could show phantom unmerged commits after its local branch was cleaned up — inviting surgery on work that was already safe — while a branch with no readable state at all reported a reassuring zero. Rows now name which branch they read, fall back to the remote when the local copy is gone, say so explicitly when neither exists, and a leftover base commit inherited from a stale spawn can no longer block an epic from closing.

### Added
- **Workers are told to never sit foreground-blocked.** The worker guides now mandate backgrounding anything long-running, with concrete recipes for builds, test suites, and CI waits — a foreground-blocked worker is unreachable except by turn-breaking interrupt, which was the leading cause of lost in-flight work.

## [2.45.0] - 2026-08-06

### Fixed
- **A parked notification no longer floods the recipient.** A lifecycle transition waiting for its recipient to wake up was re-sent on every queue poll — ten times a second, for as long as it stayed parked — producing byte-identical walls of the same message across turns, outliving both an explicit acknowledgement and the close of the task it referred to. Each transition is now delivered once immediately and then at most once per re-nudge interval, and an acknowledgement stops redelivery permanently instead of merely pausing it.
- **A worker started from a stale branch says so.** When the branch a worker is cut from has fallen behind the trunk or its own remote, the spawn now reports how far behind it is, which commits it is missing, and how to refresh it — in the spawn record and to the supervisor. Previously the only clue was a number in a status column, and workers could quietly begin dozens of commits in the past. Status views also spell out "STALE BASE: N commit(s) behind" instead of leaving that number to be interpreted.
- **Reviews can't be run by the wrong party anymore.** Under the default setup, where reviews belong to the supervisor, a worker asking to run one is now declined at the point of asking and told what to do instead. The instructions attached to the completion step had been telling workers the opposite of the rule, so the conflict resolved in favour of whichever instruction was closest to the action. Setups that assign reviews to workers are unaffected.
- **A busy machine no longer fails a passing test.** A cleanup test that plants a short-lived process could see it exit before the check ran and report a failure that had nothing to do with the code under test. It now retries the setup without weakening a single assertion, and if it genuinely cannot get a foothold it says the machine was loaded rather than blaming the feature.

### Added
- **A recipe for the "hung" test suite that isn't hung.** The recovery guide now covers left-over test processes that sit idle and block the next run — how to tell them apart from a genuinely running suite, how to clear them safely by process, and why clearing them by name is dangerous. One occurrence of this cost an hour; the suite finished in a fraction of a second once cleared.

## [2.44.0] - 2026-08-06

### Fixed
- **Codex reviewers stopped rejecting finished work over a turn of phrase.** The Codex flavor of the completion reviewer still screened close reasons with a keyword blacklist ("pending", "partial", "remaining items") long after that approach was removed elsewhere for flagging work that was genuinely done but mentioned something another team still owed. It now judges a close reason against the task's own acceptance criteria, matching every other flavor, and its review recipes cover TypeScript and Python instead of assuming Rust.
- **The startup briefing always fits, and always arrives.** On busy projects the session-start briefing could outgrow the chat window's size limit, get shunted to a file, and leave the assistant holding only the first couple of KB. The briefing now assembles under a fixed size budget: core guidance is never what gets cut, and bulky sections collapse to a count plus the command that brings the detail back.
- **Team sessions on a second account find their own team.** Team folders, inboxes, and settings files are created inside whichever account the session is actually running as, instead of always landing in the primary account's folder where that session would never look for them. Single-account setups are unchanged.
- **Hook setup follows the account you are configuring.** Installing and removing hooks now reads and writes the settings file of the active configuration directory rather than assuming the default one, so a session on an alternate account comes up with its hooks in place.
- **The operator guides describe the system that actually shipped.** The supervisor and worker guides had drifted from the code: account selection when spawning workers, the long-lived server registry, the merge and fleet-sync commands that keep factory bookkeeping intact, and the evidence a completed review must carry were missing, incomplete, or documented as something the code no longer does. All corrected against the dispatch sites.

### Added
- **The three assistant flavors can no longer drift apart in silence.** A new test compares every shared builtin guide across all three flavors, normalizing only the differences that are meant to exist, and fails the build on any other divergence — the failure mode that had let one flavor sit four months behind the others.

### Fixed
- **The supervisor now hears about parked closes.** A worker's close rejected with MERGE REQUIRED previously vanished — fleets idled silently until a human checked in; the event now reaches the supervisor as a push signal.
- **Messages stop lying about being seen.** Wake-up nudges no longer trust the registry's "busy" claim (an automated git checkpoint counted as activity); pane and transcript evidence decide, vetoed nudges retry instead of stranding, and acks record whether they were explicit or merely inferred from a reply.
- **Fleet sync can no longer destroy work in progress.** `sync_all_workers` refuses dirty or mid-task worktrees without force, and a failed stash pop notifies both the worker and the supervisor with the stash ref instead of silently stranding the changes.
- **Status surfaces tell the truth.** `worker_status` no longer shows closed tasks as in-progress for the lease duration, the constantly-crying STALLED flag is replaced for turn-based workers by a NOT-WAKING check built on unread-mail evidence, parked-awaiting-merge workers are labeled "WAITING ON YOU" instead of looking idle, and every row shows real unread-inbox depth.
- **Triage lists stopped hiding work.** `ready`/`blocked`/`available` sort by priority by default, print the true total, and name the withheld count — previously a silent cap of 10 plus a newest-first default buried ready P0s behind fresh P2s for hours. Sort parameters that used to be silent no-ops are honoured everywhere.
- **A worker is no longer "behind" its own merged work.** Behindness is measured by content (tree equality, then cherry-pick) instead of commit topology, dissolving the circular-authorization deadlock where the follow-up assignment was blocked by the very merge it needed — including squash-merged lanes.
- **Orphaned build tools get reaped, honestly.** `rustc`/`rustdoc` orphans that wedge subsequent builds are detected and cleaned by gc (with `cargo` deliberately excluded — an adopted cargo is routinely a live build), the build-jobs derate tracks the real fleet size, and the hang-vs-kill diagnosis recipe is documented. The issue's original OOM premise was measured and corrected on the record.
- **An empty review can no longer pass as a clean one.** A review outcome missing any mandatory persona lane — not just personas_run=0 — is rejected at the close gate, with lane presence computed from what the orchestrator dispatched rather than self-reported skips.
- **The review-workflow parity guard now guards.** The rendered workflow copy had silently drifted from the shipped builtin for two days while the only parity test lived in a suite nothing ran; the guard now runs under `cargo test`, names the divergent line, and states the repair direction.

### Added
- **Stacked epics are visible.** Creating an epic on top of an unlanded epic branch surfaces the full ancestry chain (depth, not one level) at creation and in `epic_status`, derived live from git topology so it cannot drift.

### Added
- **The GitHub-issues sweep is now a skill instead of folklore.** `cas-github-issues` ships as a builtin for every harness: dedupe double-filed copies, verify-and-close fixed claims, task new issues into the active github-issues epic (creating a successor epic when none is open — never tasking into a closed one), comment each issue with its task ID, unblock chained tasks when lanes merge, and file defects observed since the last sweep.

### Fixed
- **Codex workers no longer wedge silently in untrusted directories.** The factory pre-trusts worker and supervisor workdirs in `~/.codex/config.toml` before launch (hardened against config corruption), and the register-timeout diagnostic now names the trust-prompt cause instead of a generic timeout.
- **Assigning a task actually wakes Codex workers now.** Assignee changes emit durable wake-ups on the Codex path — previously only Claude workers reacted, and assigned P0 work sat idle until a manual nudge. Director idle notices are stamped with the instant their snapshot was read, so stale "worker is idle" claims are identifiable.
- **`task action=update` honours `blocked_by`.** Previously it silently dropped the field and reported "No changes specified", letting work start on stale inputs; blockers are now pre-validated and gated status re-armed, matching `create` semantics.
- **`spawn_workers` no longer demands a ceremonial epic.** Supplying a concrete open `task_id` permits spawning after an epic closes, instead of forcing a single-child wrapper epic.
- **A new epic no longer strands prior work by branching from a stale `main`.** Epic-branch creation compares the intended base against `HEAD`; when `HEAD` is ahead on an epic branch the divergence is surfaced instead of silently basing dozens of commits behind.

### Investigated
- **Dev-profile `split-debuginfo` measured end to end and rejected.** With mold and `debug = 1` already in place it buys no link time, no cold-build time, and no net disk at measurable scale; `packed` is strictly worse. Full numbers on the issue.

### Added
- **Long-running services get a registry instead of an ambush.** `server_start`/`server_stop`/`server_list` register agent-launched servers with ownership, logs, and a pid-identity fingerprint; registered shared servers live in their own cgroup scope so worker teardown deliberately spares them, and `stop` refuses to signal a reused pid rather than killing a bystander.
- **Worker teardown now takes the whole process tree.** Everything a worker spawns dies with it — by process group everywhere, and by cgroup subtree on delegated cgroup-v2 hosts — so escaped `npm run dev`-style stragglers no longer outlive their worker. `gc_report`/`gc_cleanup` additionally sweep dead-parent processes and stale port squatters.
- **A design-spec skill and a release-notes rubric** ship as builtins for every harness, so projects inherit a DESIGN.md generator and a publication rubric instead of reinventing them.

### Fixed
- **Finished work closes.** The close guard scopes to task-attributed commits instead of the spawn-repo factory anchor, honors `target_repo`/`target_branch`, measures against fetched remote refs, and accepts unambiguous abbreviated commit receipts; `awaiting_merge` gained a sanctioned amendment path (`request_changes`); the additive-only gate no longer counts a task's own WIP against it; zero-diff investigation closes stopped being a two-stage trap.
- **Requested workers arrive.** The spawn daemon's queue consumer survives `shutdown_workers count=0`, invalid cli/model combinations are rejected at the door instead of silently defaulting, pre-assigned tasks actually reach the worker, and spawn receipts report liveness instead of hope.
- **Coordination messages stopped lying.** Drained messages are no longer re-delivered on the idle-nudge path, signals are computed from fresh state at send time, and months-old queue items no longer land on freshly spawned workers.
- **Epic close keeps its hands off your checkout.** Closing out an epic no longer flips the main checkout's HEAD onto the epic branch.
- **The test suite is hermetic against its host.** Close-path test outcomes no longer depend on the ambient `CAS_FACTORY_WORKER_CLI` of whoever runs `cargo test`, `cas doctor` prints its breakdowns in deterministic order, timing-budget assertions tolerate loaded hosts without weakening what they prove, and registry tests neither collide on cgroup scope names nor leak five-minute orphans that stall piped test runs.
- **Choosing a Claude account starts CAS again.** `cas claude <profile>` resolved the account directory and then exec'd Claude Code directly, so the factory never started — selecting a second subscription and running CAS became two separate commands to be combined by hand with an environment variable. The account is now exported into the launching process before any thread or pane exists, and the command delegates to the same factory path as the other provider shortcuts with Claude pinned as the supervisor, so the supervisor and every worker it spawns land on the chosen account. Bare `cas claude` launches the factory on the ambient account, matching its siblings; the account listing moved to `--list-profiles`, and `--bare` keeps the plain Claude Code launcher with argument passthrough. Explicitly selecting an account now also scrubs an inherited `ANTHROPIC_API_KEY` on this path, which could otherwise override subscription OAuth and silently defeat the selection.

## [2.40.0] - 2026-08-04

### Changed
- **The default worker tier is now `gpt-5.6-terra` at high effort.** The previous default is reserved for heavy and frontier work, and the supervisor guidance, model-selection reference and code-review workflow were retiered to match. Current Codex model slugs are documented alongside, so the available options are discoverable rather than folklore.

### Fixed
- **Releases now publish a macOS binary.** The release workflow built only `x86_64-unknown-linux-gnu`, so a tag produced a single asset — while the local release script targets both platforms and the Homebrew formula requests `cas-aarch64-apple-darwin.tar.gz`. Mac users had no download path from a published release. A macOS job now builds and packages that artifact, using the pinned runner and explicit Xcode selection that Zig requires to link against a compatible SDK. The release step depends on both builds, so a macOS failure blocks the release rather than publishing a partial one — silently shipping an incomplete release is the defect, not the mitigation.

## [2.39.0] - 2026-08-04

### Fixed
- **Cross-machine sync actually runs.** The automatic sync path pushed personal changes and then pulled, with no team-queue drain between them, so team-scoped rows were never attempted at all — thousands accumulated over a month showing zero retries and no error, which reads as "nothing to do" rather than "never tried". The drain now runs between push and pull, failures record a retry count and an error per row, and a stalled queue is distinguishable from an idle one.
- **Filesystem locks are released across `fork`.** Guards released by closing their descriptor, but POSIX `flock` releases only when every descriptor sharing an open file description closes — and `fork` hands the child a duplicate. A parent dropping its guard released nothing while any forked child survived, producing worktrees and delivery targets held by operations that had already finished, with no live holder to point at. `FD_CLOEXEC` does not help, because it acts on `exec` rather than `fork`. Four call sites now issue an explicit `LOCK_UN` before close in a non-panicking `Drop`; five others already did so, and the fix converges on the pattern that was already the majority.
- **Concurrent atomic writes no longer delete each other's work.** The temporary filename combined only the target name, the process id and a wall-clock timestamp, so same-process writers could collide when clock resolution is coarse — and the loser's unconditional cleanup removed the winner's file, failing a function whose entire purpose is atomicity. Naming now uses a process-local atomic counter, and cleanup is armed only after `create_new` proves ownership, so a collision degrades to a harmless retry instead of corrupting a peer.
- **Merge targets are resolved from the work, not from ambient state.** Resolution now runs task, then assignee's tasks, then explicit authorization, then refusal. The session's display focus is deliberately absent from that chain, and merges into an already-closed target are refused rather than accepted silently.
- **Completion receipts accept the short commit references every git command prints.** A short SHA was rejected with a message indistinguishable from an unrelated merge-state failure, so the natural response was to go looking in the wrong place. Abbreviations are now resolved against the repository and the full immutable id is what gets stored, so the durable record is stronger than the input. Malformed, ambiguous, and non-commit references are rejected with messages that say which problem occurred.
- **Declining delivered work is a supported action.** Reviewing work and asking for changes previously had no sanctioned path — the only mechanism that functioned was a recovery command meant for abandoned work, which cleared the assignee and recorded the outcome as an orphan recovery. There is now an explicit verdict that returns the task to actionable with its assignee intact, records the reason as a first-class decision, and invalidates the declined receipt so refused work cannot close on it.
- **Queued messages are revalidated against live state immediately before delivery.** Notifications could describe a situation that had already changed while they sat in the queue, so a request to merge something already merged was indistinguishable from a genuinely new one without checking by hand. Stale merge requests are now suppressed and replaced with guidance, and stale lifecycle notifications are dropped. Uncertainty always delivers: only positive proof of staleness suppresses.
- **Bug reports reach the project from any machine.** Filing instructions pointed at a local filesystem path that only resolves when two checkouts share a disk, and had no commit step, so reports written elsewhere were lost by construction. Filing now targets a configured issue tracker, the report is written to disk before anything is sent so a failure cannot lose it, and a local fallback states plainly that it must be committed to be visible.

### Changed
- **The `mcp-server` feature is removed and the server is unconditional.** Building without it compiled out the server while the terminal layer still launched it, so the build produced a binary that advertised orchestration and exposed no tools. A flag that cannot produce a working binary is not a flag. Removing it also un-hid roughly 800 tests that had been silently excluded from every run.
- **Continuous integration runs for the first time.** Workflows had been registered and inactive for four months. Enabling them surfaced a linker misconfigured since April, a toolchain mismatch on macOS, tests that passed only where a particular CLI happened to be installed, and a process-environment race between concurrent tests — two of which had broken on the same April day and stayed invisible for three months. The pipeline now returns in about twenty minutes, with the expensive release-profile gate moved off the per-change path onto merges and the nightly schedule.
- **Disk-space checks share one portable helper.** Two call sites read `statvfs` independently and duplicated the platform-width arithmetic that had already caused one macOS-only build failure. They now share a single helper that exposes available and free space as distinct values, because the two callers were never asking the same question.

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

[Unreleased]: https://github.com/Richards-LLC/cassy/compare/v3.7.5...HEAD
[3.7.5]: https://github.com/Richards-LLC/cassy/compare/v3.7.4...v3.7.5
[3.7.4]: https://github.com/Richards-LLC/cassy/compare/v3.7.3...v3.7.4
[3.7.3]: https://github.com/Richards-LLC/cassy/compare/v3.7.2...v3.7.3
[3.7.2]: https://github.com/Richards-LLC/cassy/compare/v3.7.1...v3.7.2
[3.7.1]: https://github.com/Richards-LLC/cassy/compare/v3.7.0...v3.7.1
[3.7.0]: https://github.com/Richards-LLC/cassy/compare/v3.6.0...v3.7.0
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
