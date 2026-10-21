# Journey registry v1

`cas qa journeys --check` validates the project's journey registry against Git's
tracked files. It exits 1 for invalid YAML, schema problems, broken references,
unmatched owner/group/surface globs, or untracked specs. It exits 0 when only
curation debt remains. It never runs the journey's spec.

The default file is `docs/qa/journeys.yaml`. Set another project-relative path
with `cas config set qa.registry.path docs/qa/custom.yaml`. Run from the project
checkout; global `--json` returns one `{problems, debt}` object. Each array contains
`{code, subject, message}` rows. The same loader and validator serve future impact
and upkeep consumers. Registry upkeep also runs automatically at task park.

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
| `seeded_from` | Optional list of source descriptions. |
| `traffic_snapshot` | Optional free-form map; counts and sources are not validated. |
| `ignore` | Optional ordered path filters ignored by impact and ownership checks. |
| `surfaces` | Optional files requiring ownership; absent inherits `qa.user_facing_paths` plus `qa.api_paths`. An explicit empty list selects none. |
| `kinds` | Optional allowed journey kinds; absent accepts free text. |
| `owner_groups` | Optional `name: {description, globs: [...]}` shared ownership map. |
| `invariants` | Optional `ID: {statement, check, bug?}` map. `check` describes the end-state read-back; `bug` may hold task metadata. |
| `fixtures` | Optional `name: {description, provider}` map. When present, journey fixture references must resolve. |
| `bug_families` | Optional `key: {title, found?, fixed_by: [], tasks: [], invariants: [], journeys: []}` map. Journey and invariant references must resolve. |
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
| `fixtures` | Optional fixture names; validated when the header declares fixtures. |
| `traffic` | Required for curated live journeys: `{share_pct: number or null, basis, source?}`. Non-empty `basis` names what was counted. |
| `spec` | Optional tracked automated journey file. |
| `known_failure` | Optional open task id explaining a currently failing spec. |
| `finisher` | Optional journey id this entry ends in; must resolve. |
| `evidence` | Optional free-form map of previous ledger/verdict metadata. |
| `seeded` | Optional boolean, default false. A live seed is exempt from steps, invariants and traffic requirements until curated. |

A seeded journey remains responsible for a valid entry, ownership and references.
Seeded journeys are reported as debt. Live journeys with explicit
`traffic.share_pct: null` produce traffic debt, never a validation error. Curate
a seed by adding steps, invariant checks and traffic, then removing `seeded`.

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

## Registry upkeep at task park

G1–G3 run before the merge request, using the committed delivery head and the task's
attributed diff (including deletions and rename sources). They read Git snapshots;
uncommitted changes do not repair a delivery. Missing registries disable these gates.

`qa.registry.enforce` accepts `off`, `observe`, `enforce` (default), or `strict`.
`observe` persists would-be refusals as task notes and passes. `strict` is explicit
opt-in and also checks modified unowned surface files. The stage
is operator-controlled; strict is never selected automatically.

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
summaries can count these rows without executing a check again.
