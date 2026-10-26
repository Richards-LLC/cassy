# Journey registry v1

`cas qa journeys --check` validates the project's journey registry against Git's
tracked files. It exits 1 for invalid YAML, schema problems, broken references,
unmatched owner/group/surface globs, or untracked specs. It exits 0 when only
curation debt remains. It never runs the journey's spec. Human output says
`DEBT` for a valid registry with debt, lists counts by kind beneath the verdict,
and flags any increase since the previous recorded check. Use `--verbose` for
the individual debt rows. `PASS` means there are neither problems nor debt.

The default file is `docs/qa/journeys.yaml`. Set another project-relative path
with `cas config set qa.registry.path docs/qa/custom.yaml`. Run from the project
checkout; global `--json` returns one `{problems, debt, debt_summary}` object.
Each array contains `{code, subject, message}` rows. `debt_summary` contains
`total`, `by_kind` (counts keyed by debt code), `previous_total`, signed `delta`,
`grew`, and `history_status`. The first recorded run has null comparison fields.
The same loader and validator serve impact
and upkeep consumers. Registry upkeep also runs automatically at task park.

Checks retain their latest versioned receipt in the existing project
`cas.db` metadata table, keyed by canonical checkout and registry paths. The
receipt records current counts, the previous total and the check time. Different
worktrees or registry paths have separate baselines; concurrent checks serialize
their receipt updates. A completed validation records its count even when it
finds problems; a read, YAML parse or Git failure does not replace the baseline.
Checking never initializes a database or adds a state file. A corrupt receipt or
an unsupported receipt version is replaced with the current check as a fresh
baseline; comparisons are null and a one-line notice explains the reset. JSON
includes the optional `debt_summary.history_notice`. Other registry baselines
remain intact. When the existing database cannot be read or written, current validation still runs;
`history_status` is `unavailable`, comparisons are null, and `history_error`
explains why in JSON (or human `--verbose`). An unavailable baseline is not zero.

## Path filters

Paths are relative to the repository root. `*` matches within one path segment,
`**` matches any depth, and a leading `!` excludes a path. Later matching entries
win. Brackets are literal: `pages/[id].vue` matches that exact route filename,
not `pages/i.vue`. Matching is case-sensitive, including dotfiles.

Each declared owner, group or surface glob must match at least one tracked file.
Negated globs must also name existing files; ignored paths need not exist.
Dead journeys retain historical owner globs without orphan errors; shared groups
and declared surfaces must still match files. A journey's `spec` must be tracked
regardless of status. Ignore filters affect impact selection, not schema checks.

## Header

| Field | Contract |
| --- | --- |
| `version` | Required integer `1`. |
| `updated` | Required ISO date, `YYYY-MM-DD`, of the last curated edit. |
| `selection` | Optional `globs` (default) or `legacy_catalog`. The latter preserves the existing Markdown catalog's import and CSS selector coverage; YAML still supplies journey metadata and ownership. |
| `enforce` | Optional `off`, `observe`, `enforce`, or `strict`. The tracked header is authoritative; local `qa.registry.enforce` is fallback only when it has no policy. A delivery uses the stricter policy from base and head. |
| `seeded_from` | Optional list of source descriptions. |
| `traffic_snapshot` | Optional free-form map; counts and sources are not validated. |
| `ignore` | Optional ordered path filters ignored by impact and ownership checks. |
| `surfaces` | Optional files requiring ownership; absent inherits `qa.user_facing_paths` plus `qa.api_paths`. An explicit empty list selects none. |
| `kinds` | Optional allowed journey kinds; absent accepts free text. |
| `owner_groups` | Optional `name: {description, globs: [...]}` shared ownership map. |
| `invariants` | Optional `ID: {statement, check, bug?, pending_checker?, definition?, labeled?, compare?}` map. `check` describes the end-state read-back; `bug` may hold task metadata. |
| `manual_checks` | Optional `ID: {statement, check, reason?, pending_checker?, definition?, labeled?, compare?}` map of criteria with no executable checker. `reason` is `human_judgment` (a person must grade it) or `checker_unwritten` (automatable, so `pending_checker` is required). |
| `fixtures` | Optional `name: {description, provider, labels?}` map. When present, journey fixture references must resolve. `labels` maps a criterion ID to that fixture's expected value. |
| `bug_families` | Optional `key: {title, found?, fixed_by: [], tasks: [], invariants: [], journeys: []}` map. Journey and invariant references must resolve. |
| `safety` | Optional `{mode, forbidden_hosts?, protected_credentials?}` QA policy every journey inherits. `mode` is `fixture-only`, `staging` or `production-read-back`; any other value is a validation problem and renders as fixture-only. `forbidden_hosts` takes hosts, `*.domain` or URLs; `protected_credentials` names environment variables. The QA preflight refuses a round whose environment sets a protected credential or whose `qa.deployed_origins` names a forbidden host. |
| `journeys` | Required non-empty list. |

## Journeys

| Field | Contract |
| --- | --- |
| `id` | Required, unique, matching `^[A-Za-z0-9][A-Za-z0-9._-]*$`; stable across tasks and ledgers. |
| `name` | Optional title; displays may fall back to the id. |
| `status` | Required: `live`, `dark` or `dead`. |
| `status_reason` | Required non-empty reason for dark/dead journeys. |
| `env_status` | Optional map with only `staging` and `prod` keys, each `{status, note?}`; status uses the same three values. |
| `kind` | Optional label constrained by header `kinds` when declared. |
| `entry` | Required for live journeys: `{url, trigger}` with non-empty strings. |
| `steps` | Required non-empty ordered list for curated live journeys. |
| `owners` | Required for live journeys unless a referenced group supplies globs. Accepts a glob list or `{backend: [...], frontend: [...]}` map; map sides flatten in key order. |
| `owner_groups` | Optional list of header group names; groups resolve to shared globs. |
| `invariants` | Required non-empty list for curated live journeys; each id must resolve. |
| `manual_checks` | Optional list of header `manual_checks` ids; each id must resolve. |
| `fixtures` | Optional fixture names; validated when the header declares fixtures. |
| `traffic` | Required for curated live journeys when the header has a `traffic_snapshot`: `{share_pct: number or null, basis, source?}`. A numeric share needs a non-empty `basis` naming what was counted; a null share needs none. With no snapshot, an omitted `traffic` is unallocated-share debt. |
| `spec` | Optional tracked automated journey file. |
| `spec_status` | Optional `planned`: the `spec` path is a later task's deliverable, so an untracked path is `planned_spec` debt instead of an `untracked_spec` failure. Remove it once the spec is tracked: a marker left on a tracked spec, or on a journey without `spec`, is `stale_planned_spec` debt. `cas qa journeys --status` lists planned specs not yet written, and `cas qa impact` reports them as planned rather than as a spec to open. |
| `known_failure` | Optional open task id explaining a currently failing spec. |
| `finisher` | Optional journey id this entry ends in; must resolve. |
| `evidence` | Optional free-form map of previous ledger/verdict metadata. |
| `seeded` | Optional boolean, default false. A live seed is exempt from steps, invariants and traffic requirements until curated. |

A seeded journey remains responsible for a valid entry, ownership and references.
Seeded journeys are reported as debt. Live journeys with explicit
`traffic.share_pct: null` produce traffic debt, never a validation error. Curate
a seed by adding steps, invariant checks and traffic, then removing `seeded`.

### Shared pipelines and multiple consumers

Ownership is nonexclusive. With `selection: globs` (the default), a changed file
selects **every** journey whose owners match it. Matching one journey does not
consume the file or stop matching. Repeating the same glob in several journeys'
`owners` works, including under `owners.backend`; those map keys organize globs,
not journey IDs.

For a shared pipeline, prefer one header group and reference it from every
consuming journey. For example, define:

```yaml
owner_groups:
  audit_v2:
    description: Shared audit prompts consumed by every post shape
    globs: ["apps/backend/src/tools/audit-v2/*.prompt.ts"]
```

Add `owner_groups: [audit_v2]` to **each** of `spoken_video`, `text_led_video`,
`carousel_sequence`, `single_image`, `montage`, `screen_tutorial`, and
`general_hybrid`, retaining their shape-specific `owners` and `invariants`.
Defining the group alone selects nothing; referencing it from only one journey
selects only that consumer. The registry does not infer pipeline dependencies
from imports. Keep consumer membership current when the pipeline gains a caller.

The [complete seven-shape example](examples/gabber-shared-pipeline.yaml) uses
illustrative routes and output checks. Copy its group and memberships into the
project's registry, adapting routes and checks to the real application. For a
diff changing only `apps/backend/src/tools/audit-v2/shared.prompt.ts`, run:

```bash
cas qa journeys --check
cas qa impact <before-prompt-edit>..<after-prompt-edit> --json
```

The report lists all seven IDs, each with a match whose `via` is
`owner_groups.audit_v2`, and each journey's own invariant statements and read-back
checks. For example, the carousel journey can require comparing its
slides-with-text ratio against the prior prompt on the same fixture. Shared
ownership does not merge or replace those checks, and impact reports do not run
them. Ratios, thresholds and fixture expectations remain application contracts.

This needs no new fan-out schema. Explicit `selection: legacy_catalog` instead
delegates journey selection to the catalog selector described below; owner
membership still establishes file ownership but does not override that selection.

### Preserving a semantic catalog

cas-src uses `selection: legacy_catalog` to keep HUB-J1–HUB-J21 selection through
`scripts/journeys-for-diff.py` and `docs/qa/journeys.md`. Both files must be tracked.
The selector, catalog and import graph come from the reviewed head; a diff also
supplies its base for CSS, composition and fixture hunks. Paths-only requests use
the selector's conservative behavior when hunk context is unavailable. No arbitrary
selector command can be configured.

The selector's IDs determine the touched journeys, including an empty selection.
An ID missing from YAML is an error. Names, steps, specs, invariants, fixtures and
traffic come from YAML. Owner globs independently determine file ownership: a
selected journey can have no direct glob match, and selection never makes an
unowned file owned. Ignore filters apply before selection. Ordinary registries
remain a pure glob pass and do not launch the Python selector.

Keep the Markdown narrative, stable IDs and YAML metadata aligned when a journey
changes. Extend YAML owners when source dependencies change; the semantic selector
tracks imports dynamically. cas-src deliberately leaves new, uncatalogued source
files unowned so G2 requires ownership or an explicit decision in that delivery.

### Pending checkers and manual checks

`pending_checker: {owner, due: YYYY-MM-DD}` declares a criterion whose checker
is not written yet. `--check` reports it as `pending_checker` debt naming the
owner and due date, and fails with `overdue_checker` once the due date has
passed. A missing owner or invalid date fails as `pending_checker`. This lets
a docs-only change declare a criterion without a red build, while keeping the
missing checker visible and owned.

Each `manual_checks` entry is its own debt line, coded by reason:
`manual_human_judgment`, `manual_checker_unwritten` (with the owner and due
date; overdue fails like any pending checker), or `manual_unclassified` when
`reason` is absent. An unknown reason fails as `manual_reason`.

```yaml
invariants:
  INV-ONE-RUN:
    statement: One Draft click starts one paid run
    check: Count runs and debits after a double click
    pending_checker: {owner: qa-team, due: "2026-11-15"}
manual_checks:
  MANUAL-CAROUSEL-READS:
    statement: Slide cards read in order at 390px
    check: Compare View Content to the source carousel
    reason: human_judgment
```

### Labeled criteria

Some criteria cannot be graded by one universal rule. "Every slide with text
has an on-screen text block" stalls on a photo carousel whose jersey names and
numbers read as text but are not authored overlays. Mark such a criterion
`labeled: true`, give it a `definition` of what counts, and record the expected
value on each fixture under `labels`. The planned release gate (cas-7789,
golden-set matrix) is the consumer that will grade an observed value against
the fixture's label; today `--check` validates the labels and reports what is
missing.

`--check` fails a labeled criterion with no `definition` (`missing_definition`),
a label keyed by an unknown criterion (`unknown_label`), and a label on a
criterion that is not `labeled: true` (`label_without_labeled`). Each fixture a
journey pairs with a labeled criterion but which has no label, or a `null`
label, for it is one `unlabeled_fixture` debt line however many journeys share
the pair. Unlabeled criteria are unaffected.

```yaml
invariants:
  INV-SLIDE-TEXT:
    statement: Every slide with authored text has onScreenTextBlocks
    check: Compare Stage 1 onScreenTextBlocks to the labeled slides
    definition: Authored overlay text only; text inside the photo does not count
    labeled: true
fixtures:
  jersey-carousel:
    description: Ten-slide photo carousel; jersey names on slides 5 and 9
    provider: frozen
    labels:
      INV-SLIDE-TEXT: {authored_text_slides: [2]}
```

### Numeric comparison

A numeric criterion declares how a measured value is graded against its
baseline. `compare: exact` fails on any difference, so use it for whole-number counts
and presence; fractional measurements need a band.
`compare: {banded: {min_ratio: 0.9}}` passes at or above 90% of the baseline;
`compare: {banded: {max_delta: 20}}` passes within 20 either side. A band names
exactly one bound; `min_ratio` must be in (0, 1] and `max_delta` non-negative,
else `--check` fails with `compare`. A malformed value (`compare: fuzzy`,
`{banded: {min_rato: 0.9}}`) is the same single `compare` problem on that
criterion; the rest of the registry still loads. Graders record the measured value and its
percentage of the baseline, so wording noise (1,862 text characters against a
1,871 baseline is 99.52%) passes a band visibly instead of failing a strict
floor.

### Criterion hygiene

Invariant and manual-check ids share one namespace (`duplicate_criterion`).
`human_judgment` with a `pending_checker` is a `contradiction`: an automatable
check is `checker_unwritten`. An unknown key within two edits of a known
criterion or fixture field (`lables`, `defintion`) fails as `unknown_field`
rather than silently dropping the rule. Due dates are compared with today's
UTC date.

## Example

This shape preserves gabber-style owner maps, groups and bug families. Adapt its
paths and spec to tracked files in your project before running `--check`.

```yaml
version: 1
updated: "2026-10-21"
ignore: ["**/*.test.ts", "**/*.md"]
surfaces: ["pages/**"]
owner_groups:
  account:
    description: Shared account creation
    globs: ["server/auth/**"]
invariants:
  INV-CLAIM:
    statement: A new account has exactly one claimed workspace
    check: Count owned workspaces and read claimed_at
bug_families:
  claim:
    title: Arriving workspace was unclaimed
    tasks: [cas-1234]
    invariants: [INV-CLAIM]
    journeys: [B2-home]
journeys:
  - id: B2-home
    status: live
    entry: {url: /, trigger: Get started}
    steps: [Enter email, Claim workspace]
    owner_groups: [account]
    owners:
      frontend: ["pages/[id].vue"]
      backend: ["server/auth/**"]
    invariants: [INV-CLAIM]
    traffic: {share_pct: null, basis: New accounts; instrumentation pending}
    spec: tests/signup.spec.ts
```

cas-src's existing Markdown catalog and selector remain available to their
current callers. Registry validation does not convert that catalog. The planned
impact command prefers YAML, then the legacy catalog, then CODEMAP fallback;
see [the adaptive QA brief](../design/adaptive-qa.brief.md).

## Bootstrap and status

Run `cas qa journeys --seed` inside the checkout to create a registry. It refuses
an existing file; use `cas qa journeys --seed --merge` to add newly discovered
routes while preserving the bytes of curated entries. Stage new source files
first: discovery and `--check` use Git's tracked-file index, including additions.
Repeated merge runs add nothing when the routes have not changed.

Discovery scans file routers (`pages`, `routes`, Next `app/**/page.*`), controller
decorators, Express router/app calls and axum `.route(...)` calls. Source routes
are guesses: the command does not execute code or resolve dynamic route values.
PRODUCT_OVERVIEW flow/journey headings rename matching route candidates; unmatched
flows become dark journeys. CODEMAP directory rows provide shared owner groups.
Configured user-facing/API globs and discovered route directories define surfaces;
remaining surface files receive explicit seed ownership. All generated journeys
carry `seeded: true` and need review before they describe verified user behavior.
No source or product context means seeding refuses instead of inventing a journey.

`cas qa journeys --status` reports missing/invalid registries, journey and seed
counts, debt, and unowned surfaces under `Working tree`. Its headline problem
and debt totals use the same validation as `--check`. A separate `Integration
upkeep` section reports drift for CODEMAP, PRODUCT_OVERVIEW, and the journey
registry at the labelled integration ref. Registry upkeep counts validation
problems plus unowned surface files and displays both counts; curation debt is
separate. A notice appears when the working tree differs from that ref:

```bash
cas qa journeys --status
cas qa journeys --status --json
```

`Measured ref: origin/<target>@<sha>` names the exact fetched snapshot. This
report keeps the integration ref even when the checkout is ahead or behind it;
it does not fetch. A long human ref is shortened with `--json` as the full-ref
escape. JSON labels coverage fields with `source: "working_tree"` and adds
`differs_from_measured_ref` (boolean, or null when comparison is unavailable).
The separate `upkeep` object contains `measured_ref` and
`codemap`, `product_overview`, and `registry` objects. Each artifact has `path`,
`state`, nullable `drift_count`, and an explicit `unit`. Missing artifacts and
unavailable refs are not zero drift. The existing individual freshness commands
retain their worker-aware behavior, including local changes ahead of integration.

CODEMAP counts structural change records since its last committed update;
PRODUCT_OVERVIEW counts relevant change records using its declared watch targets.
Registry drift counts validation problems plus unowned surface files in the
integration snapshot; its `debt_count` is separate. These are upkeep inventories,
not predictions that every existing issue will block a delivery. A known positive
count gives the status verdict `DRIFT`; unavailable counts give `UNKNOWN` when no
positive count is known. Local missing/invalid registry verdicts still apply.

Changing `upkeep.codemap`, `upkeep.product_overview`, or `qa.registry.enforce`
from `off`/`observe` to `enforce` saves the setting, then warns about positive
drift for that artifact, naming the same measured ref and status command. Zero,
unknown, repeated `enforce`, and `strict` to `enforce` writes do not produce that
warning. Failure to measure never turns a successful save into a failure.
Config `--json` preserves `key`, `old_value`, and `new_value`, and adds `warnings`
as an array in the same document. Tracked registry enforcement remains authoritative.

`cas doctor` includes a journey-registry row; a missing registry points to the seed
command. Neither status nor doctor runs journey specs or installs gates.

## Measured traffic

Commander counts how often each journey's defining act happens, so traffic
shares come from use instead of guesses. `hub-web/src/usage.ts` maps each counted
action to one journey (for example `reply.typed` to HUB-J5), and its test fails
when a live Commander journey has no counted action. Counts stay in the browser
as per-day totals of action names, with no session, project, machine or message
content. They are sent only to the operator's paired machines: each machine gets
every changed day, and its hub keeps the highest count per device, day and action
in `~/.cas/hub/commander-usage.json`, so a repeated post never double counts and
one machine's ledger covers every browser that reached it. Days older than
400 are pruned.

```bash
cas qa journeys --traffic              # shares over the last 30 days
cas qa journeys --traffic --days 14 --json
cas qa journeys --traffic --write      # record them in the registry
```

A journey's share is its count divided by the counted actions of every live
registry journey in the window; ids the registry does not list are reported
separately. `--write` refuses an empty window. Otherwise it sets each live
journey's `traffic` to `{share_pct, basis, source}`, sets `updated`, and writes
`traffic_snapshot: {as_of, window_days, actions, source}`, leaving every other
line unchanged. `cas qa impact` already lists affected journeys highest share
first, and `cas qa post-release start` records the release's affected journeys in
that order; post-release status and its gate message show them as "Watch first".
That list is display only and never changes the verdict.

## Registry upkeep at task park

G1–G3 run before the merge request, using the committed delivery head and the task's
attributed diff (including deletions and rename sources). They read Git snapshots;
uncommitted changes do not repair a delivery. Missing registries disable these gates.

Header `enforce` accepts `off`, `observe`, `enforce`, or `strict`, and takes precedence
over local `qa.registry.enforce`. At each revision the local setting is fallback
only when the header has no policy; its default is `enforce`. A delivery uses the
stricter policy from base and head (`off < observe < enforce < strict`), so a
delivery cannot relax its own gates. A relaxed header takes effect for later
deliveries. A local setting cannot downgrade a tracked policy, and uncommitted
YAML cannot change a delivered policy.
`observe` persists would-be refusals as task notes and passes. `strict` is explicit
opt-in and also checks modified unowned surface files. The stage
is operator-controlled; strict is never selected automatically. Existing
projects get the one-release observe window described in
[documentation upkeep](docs-upkeep.md#stages-and-adoption).

| Gate | Refusal | Decision escape |
| --- | --- | --- |
| G1 | New schema/reference problems when the registry changed; deletion or rename that leaves a previously matching owner/group/surface glob or spec orphaned | `registry unchanged: <reason>` |
| G2 | Added or renamed surface file without a journey owner, owner group or ignore match; strict also checks modified files | `no journey impact: <reason>` |
| G3 | Bug task touches a live journey but has no changed bug-family entry naming the task in `tasks`, or changed invariant naming it in `bug.task` / `bug.tasks` | `no case: <reason>` |

Own a new file in the same delivery to resolve G2. For G3, add or extend the case
with the invariant that would have caught the incident; an unchanged historical
case does not cover a new fix. Pre-existing validation debt and an untouched
malformed registry warn without refusing unrelated delivery.

An escape requires `task action=notes id=<task> note_type=decision notes="<token> <reason>"`.
The token starts the decision body and the reason must be non-empty; quoting it in
progress or warning text does not waive a gate. Retry close after recording it.
Authenticated `supervisor_override=true` with a non-empty close reason also waives
these gates and records the refusal waived.

Accounting is durable and deduplicated across retries: notes start with
`UPKEEP OBSERVE G1|G2|G3:` or `UPKEEP ESCAPE G1|G2|G3:`. The original typed
`✅ DECISION` remains available to the verifier. Status, QA descriptions and epic
summaries can count these rows without executing a check again. Epic close appends
`Upkeep escapes in this epic: <n> (<task: gate/token/reason>…)` across its descendants.
Current warnings and observe summaries also appear in park and close receipts.

Documentation upkeep gates G5/G6 run alongside these gates, including when no
registry exists. See [documentation upkeep](docs-upkeep.md) for CODEMAP and
PRODUCT_OVERVIEW conditions, stages, adoption and decision escapes.
