# Adaptive QA in Cassy: design brief

Task cas-dddf, epic cas-1c88 (GH #1323). Design only: this brief adds no product code.
Status: proposed, 2026-10-20. Every gate below needs operator sign-off before it is staffed.

## What this is for

On 2026-10-20 a gabber-studio signup change broke the workspace claim on 4 of its 5 entry
paths, which carry about 90% of signups. QA had walked one path. Nothing in the repository
recorded that five paths share the changed files, and nothing checked production afterwards.
gabber's fix (its epic cas-147e) has four parts: a journey registry, a "which journeys does
this diff touch" script, a PR comment with a release-label gate, and a production canary.

This brief turns that into Cassy features any project can use, with one requirement the
operator puts above the rest: **registry upkeep is enforced by Cassy gates, not by
instructions.** Agents skip CODEMAP and PRODUCT_OVERVIEW updates today. A registry kept the
same way would rot the same way.

Operator decisions already made (cas-1c88 notes, 2026-10-20 ~9:05 PM ET):

- Cassy owns the registry: a schema plus `cas qa impact <diff>` come first. The PR comment
  and release-label gate come later, only if they earn it.
- Upkeep is enforced. Examples given: refuse a user-facing delivery that touches unowned
  files or leaves the registry stale; require a bug fix to add its case; compute impact
  automatically at task creation and QA dispatch.
- The post-release check blocks "done" and release notes only. It never holds publication.
  It runs in the background for about 30 minutes.

## Inputs read

| Input | Where | What it contributed |
|---|---|---|
| gabber registry | gabber-studio `docs/qa/journeys.yaml` at `e3cdd5f5f` (22740ddcf→500cc05e1) | The schema: 55 journeys (44 live, 3 dark, 8 dead), 8 owner groups, invariants, bug families, traffic snapshot, ignore globs |
| gabber impact script | `scripts/qa/journeys-for-diff.mjs` (→8cbb69af1), tests 4209b5268→5f30429d3 | Diff → journeys sorted by traffic; CODEMAP area for unowned files; `--check`; release evidence gate |
| gabber CI | `.github/workflows/journey-impact.yml` (62a911395), `repository-integrity.yml:108` | PR comment, `--gate` on PRs labelled `release`, `--check` on every PR |
| gabber canary | `apps/backend/src/canary/prod-canary.evaluator.ts` (b80cadeba→9c0759d85), controller, service | Read-only invariant checks over the newest accounts, every 15 min, behind a kill switch |
| gabber seed artifacts | cas-1926 `ROUTES.md`, `LEDGER.md`, `SUPERVISOR-RULINGS.md` | How the registry was seeded: a route sweep from source, then walked rows with a DB read-back |
| gabber's own verdict | cas-dddf notes, 2026-10-21 00:54 | Enforced today: `--check` (orphaned globs) and the release gate. Gap: **a new user-facing file that matches no glob is never flagged** |
| Prior Cassy epic | cas-4329, cas-0bf6, cas-d95f, cas-5042 | Feature map plus a drift close gate. Analysed in "Why cas-4329 did not hold" |
| Cassy close path | `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs` | `qa_evidence_close_gate_for_paths` runs **before** the MERGE REQUIRED park (~line 8201); `feature_map_drift` runs **after** it (~line 10004, function at 599) |
| Cassy QA predicate | `cas-cli/src/qa_pass.rs` `user_facing_reasons`, `catalog_journeys_for` | The shared "is this delivery user-facing" rule, plus an existing journey hook that shells out to a project script |
| Cassy journey catalog | `docs/qa/journeys.md`, `scripts/journeys-for-diff.py`, `cas-cli/src/qa_journeys.rs` | cas-src already has a Markdown catalog (HUB-J1…) and selector, read at the delivered head |
| CODEMAP upkeep | `hooks/handlers/handlers_events/codemap.rs`, `pre_tool.rs:650` | Freshness = structural changes since CODEMAP's last commit; the only enforcement is a **supervisor-only PreToolUse deny** on task create / spawn at ≥10 changes, repo-wide |
| PRODUCT_OVERVIEW upkeep | `hooks/handlers/handlers_events/project_overview.rs` | `WatchTargets` (`domain_paths` any change, `watch_dirs` structural); warnings only, no gate |
| Publication gate | `hooks/handlers/handlers_events/publication_gate.rs` | Refuses a deliverable `violet_post` while verification is open; operator `PUBLICATION OVERRIDE:` note, 6 h |
| QA verdict | `cas-cli/src/mcp/tools/service/qa_pass_ops.rs:44` `verification_qa_record` | Where a QA ledger is accepted |
| Skill text | cas-d866 at `7d1daeee5` (on epic cas-d99e) | Walk rules in cas-qa-craft; "Post-release check" in cas-supervisor `reporting-and-routing.md` |

## Why cas-4329 did not hold

cas-4329 (shipped in 3.33.0) built a feature map: `docs/qa/features/*.md`, one file per
feature, each with a `## Touches` glob list. It also shipped a generator skill, a static
check script and an opt-in close gate (cas-d95f): refuse a close whose diff touches a
feature's globs without touching its file, unless a decision note says
`map unchanged: <reason>`.

**What survived and is reused here:** the gate's shape. It is diff-tied, harness-neutral
(Rust in task close, not a Claude hook), costs milliseconds and launches no process, and
its escape is one decision note with a non-empty reason. cas-d95f's change request also
holds: a malformed map file logs a warning and is skipped, so one bad file never blocks
unrelated closes.

**What failed.** No repository ever armed the gate. No checkout under `~/Petrastella` has
a `docs/qa/features/` directory, and gabber-studio has none on main or on its cas-147e
branch. The keep-or-drop pilot (cas-5042) was cancelled on 2026-10-02 with no
measurement. The reasons, each fixed below:

| cas-4329 decision | Effect | This design |
|---|---|---|
| Opt-in: only fires once `docs/qa/features/` exists | A gate that waits for a hand-built artifact is never armed | `cas qa journeys --seed` builds a valid registry in one command; CODEMAP and PRODUCT_OVERVIEW gates arm wherever those files exist |
| Building the map is a background chore, "never on the critical path" | Nobody owned building it | Seeding is a staffed child task per project, with a rollout check |
| Nothing read the map automatically | A missing or stale map cost nobody anything | Impact feeds task notes, the QA task body and the QA verdict check, so a gap is visible on every user-facing delivery |
| The gate runs after MERGE REQUIRED | A refusal arrives after the code is already on the target branch, so fixing it costs a second merge round | Every upkeep gate runs at park, next to the QA evidence gate, before the merge request |
| The unit was a feature (a code area) | The incident crossed features: 5 entry paths through shared files | The unit is a journey from entry to end state, with shared `owner_groups` |
| A project-wired static check script | Each project had to add it to CI | `cas qa journeys --check` is a Cassy command; the close gate calls the same validator |

**Current CODEMAP enforcement has the same flaw.** It is a global count checked only when a
supervisor creates tasks (`pre_tool.rs:650`). The worker whose diff made the map stale never
sees it; the next supervisor pays for everyone's debt with a full `/codemap` regeneration.
It is also a harness hook, so harnesses without the hook skip it. PRODUCT_OVERVIEW has no
gate at all. Diff-tied gates at park put the cost on the delivery that caused it, while the
change is still in the worker's head.

## 1. The journey schema (registry v1)

**Location:** `docs/qa/journeys.yaml` by default (`qa.registry.path`). One file per project.
It is YAML because gabber's file is already YAML and it needs nested maps.

**Glob semantics:** GitHub path-filter rules. `*` matches one path segment, `**` any depth,
and a leading `!` negates (later entries win). `[` and `]` are **literal**, because routes
such as `pages/audit/[postId].vue` must match themselves. The Rust implementation must
escape brackets before handing a pattern to `glob::Pattern`.

### Header

| Field | Required | Meaning |
|---|---|---|
| `version` | yes | `1` |
| `updated` | yes | ISO date of the last curated edit |
| `seeded_from` | no | Free-text list of the sources the seed read |
| `traffic_snapshot` | no | Free-form map: `as_of`, `window_days`, `source`, counts. Never validated beyond being a map |
| `ignore` | no | Globs that never make a journey touched and are never "unowned" (tests, fixtures, docs) |
| `surfaces` | no | **New in Cassy.** Globs naming files that must be owned by a journey or ignored, for example route, page and controller directories. Defaults to `qa.user_facing_paths` plus `qa.api_paths` |
| `kinds` | no | Allowed `kind` values. If absent, `kind` is free text |
| `owner_groups` | no | `name → {description, globs[]}`, globs shared by several journeys |
| `invariants` | no | `ID → {statement, check, bug?}`, named end-state checks. `check` says what the read-back proves |
| `fixtures` | no | `name → {description, provider}`. If declared, journey `fixtures` must name entries here |
| `bug_families` | no | `key → {title, found?, fixed_by[], tasks[], invariants[], journeys[]}` |
| `journeys` | yes | Non-empty list |

### Journey

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | Stable, unique, `^[A-Za-z0-9][A-Za-z0-9._-]*$`. The id is the contract: tasks, ledgers and specs cite it |
| `name` | no | Human title. If absent, renders show the id |
| `status` | yes | `live` (a real person can reach it today), `dark` (code exists behind a flag or gate), `dead` (nothing reaches it; kept so a ledger can say NOT A ROUTE) |
| `status_reason` | if not live | Why it is dark or dead |
| `env_status` | no | `{staging, prod}: {status, note}` where environments differ |
| `kind` | no | Free label, or one of `kinds` |
| `entry` | if live | `{url, trigger}`, where the person starts |
| `steps` | if live | Ordered list of user-visible steps |
| `owner_groups` | no | Names from the header |
| `owners` | if live, unless groups cover it | Glob list. A `{side: [globs]}` map is accepted and flattened, which keeps gabber's `{backend, frontend}` file valid unchanged |
| `invariants` | if live | Invariant ids |
| `fixtures` | no | Fixture names |
| `traffic` | if live | `{share_pct: number or null, basis, source?}`. `null` means not measurable; `basis` says what the share counts |
| `spec` | no | Tracked file that walks it automatically |
| `known_failure` | no | Open task id that a red spec is expected to stay red on |
| `finisher` | no | Id of the journey this entry ends in |
| `evidence` | no | Free map: last ledger row and verdict |
| `seeded` | no | `true` on entries the seed wrote and nobody has curated. A seeded live journey is exempt from the `steps`, `invariants` and `traffic` requirements, and `--check` lists it as debt |

The illustrative example below has no vendor content:

```yaml
version: 1
updated: "2026-10-21"
ignore: ["**/*.test.ts", "**/__tests__/**", "**/*.md"]
surfaces: ["src/pages/**", "src/routes/**", "server/controllers/**"]
owner_groups:
  account_core:
    description: "Account creation and the workspace every finisher reads"
    globs: ["server/auth/**", "src/stores/auth*.ts"]
invariants:
  INV-ONE-WORKSPACE:
    statement: "A new account owns exactly one claimed workspace"
    check: "workspaces where owner = account: count 1, claimed_at set"
bug_families:
  shell-claim:
    title: "Email-first signup left the arriving workspace unclaimed"
    tasks: [cas-1234]
    invariants: [INV-ONE-WORKSPACE]
    journeys: [B2-home-signup]
journeys:
  - id: B2-home-signup
    name: "Home page to a claimed workspace"
    status: live
    entry: { url: "/", trigger: "Get started" }
    steps: ["enter email", "verify", "land in workspace"]
    owner_groups: [account_core]
    owners: ["src/pages/index.vue"]
    invariants: [INV-ONE-WORKSPACE]
    traffic: { share_pct: 41.0, basis: "share of new accounts, prod, 30 days" }
```

### Validation (`cas qa journeys --check`)

It exits 1 on any problem and prints one line per problem, naming the journey or group:

- schema: version, required fields by status, id pattern, duplicate ids, status values,
  `env_status` keys;
- references: unknown owner group, invariant, fixture (when declared), finisher, or a
  bug-family journey or invariant;
- files: an owner, group or surface glob that matches no tracked file (`git ls-files`); a
  `spec` that is not tracked;
- debt, reported but never an error: seeded journeys, and live journeys with
  `share_pct: null`.

### Relation to cas-src's Markdown catalog

cas-src already has `docs/qa/journeys.md` and `scripts/journeys-for-diff.py`, read by
`qa_pass::catalog_journeys_for` at the delivered head. Its selector is finer than globs
(static imports, CSS selectors). It stays. `cas qa impact` resolves sources in this order:
(1) the YAML registry; (2) the legacy Markdown catalog through its selector, mapped into the
same output with no invariants or traffic; (3) the CODEMAP fallback. Converting cas-src is a
rollout child; it is not a prerequisite.

## 2. The `cas qa impact` contract

**Usage:**

- `cas qa impact` — diff from the merge base with the target branch to HEAD, plus untracked
  files.
- `cas qa impact <base>[..<head>]`, `--files <path>...`, or `--stdin`.
- `--task <id>` — use the task's attributed delivery diff, the same paths the close gates
  see.
- `--json` (the contract), `--markdown` (PR comment or task note), text by default.

**Exit codes:** 0 when it ran (touching journeys is not an error), 2 for usage, 3 for a git
or registry read failure. `--check` is the only mode that exits 1 on content.

**Algorithm.** It is pure: one parse and one glob pass, no process launches beyond git, no
network.

1. Drop files matching `ignore` into `ignored`.
2. A journey is touched when any of its own or group globs matches a remaining file. Record
   each match as `{file, glob, via}`.
3. Sort touched journeys: live, then dark, then dead; within a status by `share_pct`
   (highest first, null last); then by id.
4. Every remaining file that no journey owns is `unowned`. It is labelled with the longest
   CODEMAP path that equals it or contains it (accepting both ` — ` and ` - ` as the
   separator), and gets `surface: true` when it matches `surfaces`.
5. Compute `upkeep` (sections 3.5 and 3.6) and `warnings`.

**JSON (v1):**

| Key | Shape |
|---|---|
| `version` | `1` |
| `source` | `registry`, `legacy_catalog`, or `codemap_fallback` |
| `registry` | `{path, present, valid, errors_from_diff[], debt_count}` |
| `journeys[]` | `{id, name, status, kind, share_pct, basis, entry, env_status, invariants[{id, statement, check}], fixtures[], spec, known_failure, matches[{file, glob, via}], evidence_required}` |
| `areas[]` | CODEMAP fallback only: `{prefix, summary, files[]}`, ordered by file count |
| `unowned[]` | `{file, surface, codemap: {prefix, summary} or null}` |
| `ignored[]` | paths |
| `upkeep` | `{codemap: {required, reasons[]}, product_overview: {required, reasons[]}, registry: {required, reasons[]}}` |
| `warnings[]` | strings, for example a missing PRODUCT_OVERVIEW or a stale CODEMAP |

`evidence_required` is generated, never hand-written. It reads: "walk `<entry>` on
`<env>` with a fresh persona; read back `<invariant ids>` from the system of record;
fixtures: …". For a dark journey it reads "prove it stays unreachable". For a dead one it
reads "none (NOT A ROUTE)".

**CODEMAP fallback.** With no registry, or for unowned files, impact still answers "what
area is this" from CODEMAP. With no registry, `journeys` is empty and `areas` groups the
changed files by CODEMAP prefix, so QA dispatch can say "walk the user-facing entry of
area X". With CODEMAP also missing, impact lists bare files and warns.

**Loud warnings.** These print at the top of the text and Markdown output and go into
`warnings[]`:

- `PRODUCT_OVERVIEW.md is missing: journeys have no product context. Run the
  project-overview skill.`
- `CODEMAP is stale (N structural changes since <sha>): unowned files may be mislabelled.`
- `No journey registry: impact is by CODEMAP area only. Run cas qa journeys --seed.`
- `Registry has N seeded (unreviewed) journeys.`

## 3. Enforcement

### Principles

Every gate follows these rules, so none of them can brick a project:

1. **Diff-scoped.** A gate judges only what this delivery changed (the attributed delivery
   paths that `task_attribution::qa_paths` already computes, plus name-status for
   adds, deletes and renames). Pre-existing registry or map debt never refuses an unrelated
   task. It is reported as a warning line on the close result.
2. **At park, not after merge.** Every upkeep gate runs in the pre-park slot next to
   `qa_evidence_close_gate_for_paths`, before MERGE REQUIRED. Fixing a refusal costs one
   commit on the worker's own branch. A refusal never arrives once the code is already on
   the target branch.
3. **Static and fast.** File reads, one git diff and glob matching only. Target: under
   50 ms for a 2,000-file diff against a 100-journey registry. No app launch, network or
   subprocess other than git.
4. **Fail open on bad inputs the diff did not cause.** If the registry, CODEMAP or overview
   is unreadable or malformed, and this diff did not touch it, the gate logs a warning and
   passes (cas-d95f's rule). If this diff edited the registry and broke it, the gate
   refuses, because the author can fix it.
5. **One cheap escape per gate.** The escape is a task decision note with a fixed token and
   a non-empty reason, recorded with `task action=notes note_type=decision`. That takes
   one call, then the close is retried. The existing registered-supervisor
   `supervisor_override=true` with a reason waives any gate and is logged. Escapes are
   counted, never silent (see "Escape accounting").
6. **Staged per gate.** Each gate is `off`, `observe` or `enforce`; G2 also has `strict`.
   `observe` writes the would-be refusal as a task note and passes. Only operator-edited
   config changes a stage.
7. **Harness-neutral.** Gates G1 to G6 and G8 live in Rust in task close and in
   `qa_record`, which every harness reaches through MCP. G9 extends the existing Violet
   publication gate and the `cas violet post` CLI check.

### Gate table

"User-facing" means `qa_pass::user_facing_reasons` is non-empty: the predicate every QA
gate already shares.

| Gate | Where | Refuses when (exact condition) | Escape | Default stage |
|---|---|---|---|---|
| **G1 Registry broken by this diff** | Task close, pre-park | A registry exists and either (a) this diff touched the registry file and `--check` now reports a schema or reference error, or (b) this diff deleted or renamed a tracked file and, at the delivered head, an owner, group, surface or `spec` path that matched a file at base matches none | `registry unchanged: <reason>` (for example, the path moved to generated code and the journey is dead) | enforce once a registry exists |
| **G2 Unowned surface file** | Task close, pre-park | The delivery is user-facing or touches `surfaces`, and it **adds** (A, or R into a new path) a file that matches `surfaces` (or `qa.user_facing_paths` and `qa.api_paths` when `surfaces` is unset), and no journey owner, owner group or `ignore` glob at the delivered head matches it. Under `strict`: any **modified** surface file that no journey owns refuses too | `no journey impact: <reason>`, or own the file in the same diff (add a glob, or add it to `ignore`) | enforce (new files) |
| **G3 Bug fix without a case** | Task close, pre-park | `task_type=bug`, and the diff touches a file owned by a live journey, and the registry at the delivered head has no `bug_families` entry, and no invariant with `bug.task`, whose `tasks` list names this task id **and** that was added or changed in this diff | `no case: <reason>` (for example a build-only or copy-only bug with no end state to check) | enforce once a registry exists |
| **G4 Owner glob matching nothing** | `cas qa journeys --check`; task close only through G1(b) | Any glob matches no tracked file. At close this refuses only when this diff caused it (G1). Otherwise it is a warning plus a `--check` failure in the project's own CI | Fix the glob, or mark the journey `dead` with a `status_reason` | CI: the project wires it in; close: via G1 |
| **G5 CODEMAP drift** | Task close, pre-park | The delivered `.claude/CODEMAP.md` is untouched by the diff, and either (a) the diff deletes or renames a path that CODEMAP names literally, or (b) the diff adds a **new directory** that is a direct child of an *enumerated* directory: one whose CODEMAP lists at least one direct child. Added files inside existing directories produce an advisory note only | `codemap unchanged: <reason>` | enforce wherever CODEMAP exists |
| **G6 PRODUCT_OVERVIEW drift** | Task close, pre-park | `docs/PRODUCT_OVERVIEW.md` exists and is untouched by the diff, and either (a) the diff modifies a `project_overview.domain_paths` file, or adds or deletes a path inside `watch_dirs` (the existing `WatchTargets`), or (b) the registry diff adds a `live` journey or changes a journey between `live` and `dead` | `overview unchanged: <reason>` | enforce wherever the overview exists |
| **G7 Impact attached automatically** | Task create, park, QA dispatch | Never refuses. Create: a predicted-impact note from paths named in the title, description, design and proof targets. Park: the authoritative impact from the delivered diff, stored as `state_patch.qa_impact` plus a note. QA dispatch: the journey table and `evidence_required` lines are embedded in the QA task description | — | on |
| **G8 QA verdict without journey rows** | `verification action=qa_record status=approved` | The delivery's park-time impact lists live journeys, and the ledger at `ledger_path` has no line naming a journey id together with a verdict (`PASS`, `FAIL`, `NOT EXERCISED: <reason>`, or `NOT A ROUTE`) | Write a `NOT EXERCISED: <reason>` row: the row is the escape | enforce once a registry exists |
| **G9 Post-release check** | `violet_post` publication gate, `cas violet post`, epic close | See section 5 | Operator note `POST-RELEASE OVERRIDE: <operator's words>` on the epic, valid for 6 hours | enforce wherever `qa.post_release` is configured |

Why each gate refuses what it refuses:

- **G2 is gabber's gap.** `--check` catches globs that lost their file. It cannot see a new
  page that no journey owns, and that is how a sixth entry path appears unnoticed. Only
  additions are refused by default: refusing every edit to a legacy unowned file on
  adoption day would brick active projects, so modified files wait for `strict`.
- **G3 makes "every bug adds its case" checkable** without parsing tests. It needs one
  registry entry that names the task and changed in this diff. The red-first test stays a
  cas-tdd rule, and the QA reviewer sees the entry.
- **G5 refuses only where the map now lies.** That is a deleted or renamed path the map
  names, or a new area in a directory the map enumerates. Refusing every new file would
  make the gate noise in a fast-moving codebase, because CODEMAP is a navigation map, not an
  inventory. Deletions and new directories are exactly the changes that send a worker the
  wrong way.
- **G6 reuses the watch targets** projects already declare, plus the one registry signal
  that is product-level by definition: a new way in, or a way removed.

### Refusal text

All refusals render through `close_ops/gate_text.rs`, like the existing families, with the
caller's own tool prefix. Each one names the files, the escape and the retry:

- **G1** — `⚠️ JOURNEY REGISTRY BROKEN BY THIS DELIVERY` / "`docs/qa/journeys.yaml`: <n>
  problem(s) this diff caused:" / problem lines / "Fix them in this branch, or record
  `task action=notes id=<id> note_type=decision notes="registry unchanged: <reason>"`, then
  retry close." It also lists pre-existing problems this diff did not cause, as "not
  blocking".
- **G2** — `⚠️ UNOWNED USER-FACING FILE` / "<n> new surface file(s) no journey owns:" /
  file lines, each with its CODEMAP area / "Own each file in docs/qa/journeys.yaml (an
  owner glob, an owner group, or `ignore`), or record `no journey impact: <reason>`."
- **G3** — `⚠️ BUG FIX WITHOUT A CASE` / "Task <id> is a bug and touches live journeys <ids>.
  Add or extend a `bug_families` entry whose `tasks` lists <id> (with the invariant that
  would have caught it), or record `no case: <reason>`."
- **G5** — `⚠️ CODEMAP DRIFT` / "This delivery <deleted|renamed> <paths> that
  .claude/CODEMAP.md names, or added directories <dirs> under mapped directories. Update
  CODEMAP in this branch, or record `codemap unchanged: <reason>`."
- **G6** — `⚠️ PRODUCT_OVERVIEW DRIFT` / "This delivery changed <paths or journeys> that
  docs/PRODUCT_OVERVIEW.md tracks. Update it in this branch, or record
  `overview unchanged: <reason>`."
- **G8** — `QA RECORD REJECTED: the ledger has no row for live journeys <ids>.` / "Add one
  row per journey: PASS, FAIL, NOT A ROUTE, or `NOT EXERCISED: <reason>`."

### Escape accounting

Escapes must stay cheap without becoming free. All of these are counts and notes; none of
them adds a refusal:

- Every escape is a `✅ DECISION` note with its token, so it is searchable and the
  task-verifier sees it.
- `cas qa journeys --status` prints, for the last 30 days: escapes by gate,
  observe-mode would-refusals, seeded-journey debt and the share of live journeys with
  traffic.
- Epic close appends one line: "Upkeep escapes in this epic: <n> (<task: token>…)".
- The QA task description lists the delivery's escapes, so the independent reviewer can
  challenge a weak reason.

### What happens to the existing mechanisms

- **The `docs/qa/features` gate (cas-d95f)** is retired in the G5/G6 child. It has no
  adopters, it runs in the post-merge slot, and the registry supersedes it. Deleting
  `feature_map_drift` and its `gate_text` message removes a close-path branch that no
  project exercises.
- **The supervisor-wide CODEMAP deny** (`pre_tool.rs:650`, ≥10 structural changes) stays
  as a backstop for commits that never pass through a task close. Its refusal rate is
  measured once G5 enforces; if G5 keeps it at zero for a release, it is demoted to a
  warning in a later task.

## 4. Bootstrap and staged adoption

### Seed: `cas qa journeys --seed`

It writes `docs/qa/journeys.yaml`, or with `--merge` adds only new entries to an existing
file and never rewrites a curated one. Every entry it writes carries `seeded: true`. Its
sources, in this order:

1. **Routes from source.** Surface files are grouped by route.
   - File-based routers: `pages/**`, `app/**/page.*`, `routes/**`, `src/routes/**`.
   - Controller decorators and router calls found by a regular-expression scan, read-only:
     `@Controller`/`@Get`/`@Post`, `router.get|post`, `app.get|post`, axum `.route(`.
   - Each route becomes one candidate journey: `entry.url` from the path, `owners` from the
     file, `status: live`.
2. **PRODUCT_OVERVIEW.** Sections that name user flows rename and merge candidates (id from
   the heading slug). A flow the overview names but no route matches becomes a `dark`
   journey with `status_reason: "named in PRODUCT_OVERVIEW; no route found"`.
3. **CODEMAP.** Each mapped directory that contains surface files becomes an
   `owner_group`, with the directory glob and the CODEMAP summary as its description.
4. **`surfaces`.** Set from the directories the route scan found. The seed guarantees that
   every current surface file is owned or ignored, so G2 refuses nothing on day one.

The seed output passes `--check`. Curating it (steps, invariants, traffic, removing
`seeded`) is ordinary work that the seeded-journey debt counter tracks. For a project with
a real incident history, the cas-1926 method is the template: a route sweep, then walked
rows with a read-back. That is a staffed task, not part of the seed.

### Staged adoption

A gate's stage comes from config. A gate whose input file does not exist is always off.

| Stage | Registry gates G1–G3, G8 | CODEMAP G5 | PRODUCT_OVERVIEW G6 |
|---|---|---|---|
| 0: no file | Off. Impact uses the CODEMAP fallback and warns "no registry". `cas doctor` shows one line | Off; the existing SessionStart warning stays | Off; impact warns "missing" |
| 1: `observe` | Would-be refusals are written as task notes; nothing refuses | Same | Same |
| 2: `enforce` (default once the file exists) | G1, G2 (additions), G3 and G8 refuse | G5 refuses | G6 refuses |
| 3: `strict` (G2 only) | G2 also refuses modified unowned surface files | — | — |

**Config keys** (new keys need registry meta-seed entries and `cas config` get/set/list
round-trips):

- `qa.registry.path` (default `docs/qa/journeys.yaml`)
- `qa.registry.enforce` = `off|observe|enforce|strict` (default `enforce`)
- `upkeep.codemap` = `off|observe|enforce` (default `enforce`)
- `upkeep.product_overview` = `off|observe|enforce` (default `enforce`)

**Why the default is `enforce`, not `observe`.** The operator's first requirement is
enforcement, and every gate is diff-scoped with a one-call escape. The worst cost an
existing project can pay is one decision note per delivery that creates new upkeep debt.
It can never be blocked by old debt. The release that ships G5 and G6 also ships a
**one-release observe window for existing projects**: on the first `cas update` that
introduces these keys, Cassy writes `observe` for G5 and G6 into each existing project's
config, records a note in the update receipt, and flips to `enforce` on the next release
unless the operator pins `observe`. New projects and projects that run `--seed` start in
`enforce`. cas-src and gabber-studio are set to `enforce` at rollout.

**Operator choice needed:** the one-release observe window for existing projects, or
`enforce` immediately everywhere. This brief recommends the window: it costs one release
and measures each gate's would-refuse rate on real deliveries before anyone is refused.

## 5. The post-release check contract

The project declares it in `.cas/config.toml`:

| Key | Meaning |
|---|---|
| `qa.post_release.command` | Project-relative, **read-only** command (for example a canary dry-run endpoint called by a script). Run from the project root, without a shell, with a sanitized environment plus `qa.post_release.env` allow-list names |
| `qa.post_release.window_mins` | Default 30 |
| `qa.post_release.interval_mins` | Default 15. With the defaults: runs at about +0, +15 and +30 min |
| `qa.post_release.release_branches` | Branches whose merge starts a check (default: the default branch) |
| `qa.post_release.timeout_secs` | Per run, default 120. A timeout counts as a failed run |

**Command output.** It writes one JSON object to stdout and exits 0 when it ran; any other
exit code is a run error.

- `ok` — boolean: every invariant held.
- `alarms[]` — `{id, summary, count}`.
- `checked_at` — ISO time.
- `readback` (optional) — `{journey, subject, invariants[], ok}`. One real journey read
  back end to end from the system of record. `subject` is an opaque, non-secret reference.

**Start.** The operator can run `cas qa post-release start --epic <id> --sha <sha>`
from their own terminal; agents cannot record the SHA. State lives under
`.cas/operator/post-release/`. A `branch_contained_in` reminder on the epic tip,
armed at merge request, starts it automatically against the verified first-parent
merge that introduced the epic on a configured origin release branch. FF, squash
and foreign merges cannot establish that identity. It runs as a background job owned by `cas serve`, so
no pane is blocked. Each run appends an epic note
(`POST-RELEASE <k>/<n> <sha>: ok | ALARM <ids> | ERROR <reason>`) and updates
`<artifacts_root>/<epic>/post-release.json`.

**Verdict:**

- **pending** — the window is still open.
- **clean** — the window has elapsed, every run returned `ok: true`, at least one run
  carried `readback.ok: true`, and no run errored.
- **failed** — any alarm, error or failed read-back.

A failed check is an incident: the supervisor files the bug, and the fix-forward release
starts a new check on its own SHA.

**What it blocks** (and nothing else; merge, deploy, tag and publish are never held):

- **Release notes.** `publication_gate.rs` gains a second family: a `violet_post` marked
  `release_notes: true`, or one posted to the channel named in the project's
  `docs/release-notes/RUBRIC.md`, is refused while the epic's check is pending or failed.
  Refusal: `RELEASE NOTES BLOCKED (post_release_pending|post_release_failed): epic <id>
  check <k>/<n> runs, next at <CT time>; last result <…>.` The same predicate runs in
  `cas violet post` (the CLI draft check), so the CLI route refuses too.
- **"Done".** Epic close refuses with `POST-RELEASE CHECK <pending|failed>` until the
  verdict is clean.

**Escape.** An operator note `POST-RELEASE OVERRIDE: <operator's words>` on the epic,
valid for 6 hours and logged on each use, reusing the `PUBLICATION OVERRIDE` mechanism. With
no `qa.post_release` configured, nothing is gated: the skills keep the manual check from
cas-d866, and `cas qa impact` warns "no post-release check declared" on release epics.
Cassy runtime releases keep `cas-cut-release` host verification instead.

## 6. Child split

The children run in order. Each is a ready-to-create task. Rust proofs use capped,
package-scoped, named-test runs. "Fewest tests" means one test per behaviour, through the
real path, in an existing test file where one fits.

| # | Title | Depends on | Risk | Proof targets | Fewest tests |
|---|---|---|---|---|---|
| C1 | Journey registry v1: loader, validator, `cas qa journeys --check`, schema doc `docs/qa/journey-registry.md` | — | none (new module, no callers) | `cas --lib qa_registry` | (1) a gabber-shaped fixture with an owners map, groups and bug families validates clean; (2) a table of invalid registries yields exactly the named problems: orphan glob, unknown invariant, live journey without invariants, seeded exemption; (3) a bracket-literal glob matches `pages/[id].vue` |
| C2 | `cas qa impact`: engine, CLI, JSON v1, CODEMAP fallback, legacy-catalog adapter, loud warnings | C1 | none | `cas --lib qa_impact`, `cas --lib qa_pass` | (1) changed files map to journeys in status/share/id order with `ignore` applied; (2) an unowned file is labelled with the longest CODEMAP prefix, and with no registry `areas` plus a warning come back; (3) the legacy catalog path returns the same ids `catalog_journeys_for` does |
| C3 | Impact attached automatically: predicted note at create, `state_patch.qa_impact` at park, journey table in the QA task, G8 in `qa_record` | C2 | blast-radius (task create, park, QA dispatch, qa_record) | `cas --lib qa_dispatch`, `cas --lib close_ops`, `cas --lib qa_pass_ops` | (1) parking a delivery that touches an owned file records the impact and the QA task description names the journey and its invariants; (2) `qa_record approved` is refused without a journey row and accepted with `NOT EXERCISED: <reason>` |
| C4 | Registry upkeep gates G1–G3 at park, `qa.registry.enforce` stages, observe notes, gate_text, escape accounting | C1, C2 | blast-radius (close path) | `cas --lib close_ops`, `cas --lib gate_text`, `cas --lib qa_registry` | (1) an added unowned surface file refuses, naming the file; the `no journey impact:` note passes; (2) a rename that orphans a glob refuses, while a pre-existing orphan elsewhere passes with a warning; (3) a bug task touching an owned file refuses without a bug-family entry naming it and passes with one; (4) `observe` writes the note and passes; (5) a malformed registry the diff did not touch passes |
| C5 | Docs upkeep gates G5 and G6 at park, `upkeep.*` stages and the one-release observe write; retire the `docs/qa/features` gate | C4 (same file: run in series) | blast-radius (close path, update migration) | `cas --lib close_ops`, `cas --lib gate_text`, `cas --lib config` | (1) a new directory under an enumerated CODEMAP directory refuses, a new file in a mapped directory passes, and `codemap unchanged:` passes; (2) deleting a path CODEMAP names refuses; (3) a `domain_paths` change, or a new live journey, without an overview edit refuses, and `overview unchanged:` passes. Delete the five feature-map tests with the gate |
| C6 | Bootstrap: `cas qa journeys --seed [--merge]`, `--status`, `cas doctor` line | C1 | none | `cas --lib qa_seed` | (1) seeding a fixture repo with file routes, a controller and a CODEMAP yields a registry that passes `--check` and owns every surface file; (2) `--merge` adds a new route without rewriting a curated entry |
| C7 | Post-release check: config, `cas qa post-release start` and `status`, background runner, receipt, publication-gate family, epic-close refusal, override | — (parallel from the start) | blast-radius (publication gate, epic close), concurrency (background job) | `cas --lib qa_post_release`, `cas --lib publication_gate`, `cas --lib close_ops` | (1) the verdict function returns pending, clean or failed over a table of run sequences (alarm, error, missing read-back); (2) a release-notes post is refused while pending and passes with a recent override; (3) epic close is refused while pending |
| C8 | Skills and rollout: cas-qa-craft (impact table, escape vocabulary, journey rows), cas-worker and cas-supervisor (gate refusals and escapes), cas-release-notes (start the check, wait for clean), codemap and project-overview skills (G5 and G6); set gabber-studio and cas-src to `enforce`; validate gabber's file with `cas qa journeys --check`; decide on converting cas-src's catalog | C3–C7 | none | skill lint and budget tests (`integration_contracts`, `builtin_flavor_drift_test`) | No new code tests. Acceptance: `cas qa journeys --check` passes on gabber-studio's checkout, and markdownlint is clean |

**Parallelism.** C1, then C2. C6 can run alongside C2 or C3. C3 and C4 can run in parallel:
they touch different close-path regions, but both edit `close_ops.rs`, so give them one
worker or merge them in series. C5 follows C4. C7 runs from the start. C8 is last. That is
five workers at peak.

**Out of scope until it is earned** (operator decision 1): the PR comment, the
release-label evidence gate, and any GitHub Action Cassy installs. The `--markdown` output
from C2 is all a project needs to wire its own comment, as gabber already does.

## Open questions for the operator

1. **Default stage for existing projects:** the one-release observe window (recommended),
   or `enforce` immediately?
2. **G2 `strict`** (refuse edits to unowned legacy surface files): offer it as opt-in only
   (recommended), or make it the target stage once seeded-journey debt reaches zero?
3. **Retire the `docs/qa/features` gate in C5** (recommended: no adopters), or keep it?
