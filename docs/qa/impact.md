# Journey impact

`cas qa impact` maps a change to journey ids, evidence requirements and unowned
files. It reads local Git and project files without network access. Registry
matching runs in-process; the legacy catalog adapter and an opted-in registry's
`selection: legacy_catalog` invoke the existing
read-only Python selector so import/CSS selection stays compatible.

```bash
cas qa impact
cas qa impact origin/main
cas qa impact BASE..HEAD --json
cas qa impact --files src/pages/signup.vue server/auth/signup.ts
printf '%s\n' src/pages/signup.vue | cas qa impact --stdin --markdown
cas qa impact --task cas-1234 --json
```

The default compares the merge base of the local target/default branch to the
working tree and includes untracked files. An explicit base compares committed
snapshots, defaulting its head to HEAD. Explicit paths use working-tree metadata.
`--task` uses the close gate's task identity, lease window, delivery anchor and
attributed delivery paths; an unreadable attribution fails instead of broadening to
another task's files. Options selecting the input are mutually exclusive.

Exit codes: 0 when impact was computed, 2 for usage errors, 3 for Git, registry,
selector or other read errors. Touching a journey is successful computation.
Schema validation errors are reported in registry metadata and warnings; use
`cas qa journeys --check` for the content-check exit-1 contract.

## JSON v1

Global `--json` emits one object with these stable top-level fields:

| Field | Meaning |
| --- | --- |
| `version` | `1`. |
| `source` | `registry`, `legacy_catalog`, or `codemap_fallback`, in preference order. |
| `registry` | `{path, present, valid, errors_from_diff, debt_count}`; diff errors compare the delivered registry/files to the base when available. |
| `journeys` | Selected journey metadata, traffic, invariant statements/checks, fixtures, specs, matches and generated `evidence_required`. |
| `areas` | CODEMAP-only fallback groups `{prefix, summary, files}`, largest file count first; unmapped files have an empty prefix. |
| `unowned` | `{file, surface, codemap}`; the CODEMAP label is the longest literal containing/equal path, or null. |
| `ignored` | Paths excluded by registry ignore filters. |
| `upkeep` | `codemap`, `product_overview`, and `registry`, each `{required, reasons}`. These are impact signals; this command installs no close gate. |
| `warnings` | Missing product/map context, stale CODEMAP, missing registry, validation problems and seed debt. |

Journeys sort live, dark, dead; within a status, highest traffic first, null
last, then id. Matches contain `{file, glob, via}`; `via` is `owners` or
`owner_groups.<name>`. Ordered exclusion filters are applied before recording
positive matching patterns. Rename impacts include both old and new paths.

Live evidence describes an entry walk on staging with a fresh persona and a
system-of-record read-back of its invariant ids. The environment metadata stays
in JSON so reviewers can select their declared QA environment. Dark evidence
says "prove it stays unreachable"; dead evidence says "none (NOT A ROUTE)".

Legacy selection preserves the existing selector's ids at the delivered head.
The adapter supplies catalog names, entries and specs, with no invented
invariants or traffic. The selector supplies no per-file ownership report:
legacy `matches` are empty and files remain unowned by a YAML registry. A warning
makes that limitation explicit. With no YAML or legacy catalog, CODEMAP labels
still provide an area to inspect; missing maps leave bare unowned paths.

Text and Markdown put context warnings first. JSON preserves complete paths,
metadata, matches and upkeep reasons for automation. See the
[registry schema](journey-registry.md) and
[adaptive QA brief](../design/adaptive-qa.brief.md) for the upkeep contracts.

## Automatic task and QA context

Task creation adds a **predicted** impact note using tracked paths, directories
and globs named in the title, description, design and proof targets. This is a
planning hint; the delivered diff determines the authoritative impact.

Parking a delivery snapshots that impact into the task's execution state under
`qa_impact`, with the JSON v1 report above plus `head_sha` and `base_sha`.
It uses the same task attribution as close, including deleted and renamed paths,
and reads registry metadata at the delivered commit. Repeating the same park
reuses the snapshot. The state supports a bounded 1 MiB impact report separately
from the existing 16 KiB resume fields. Cassy manages `qa_impact`; public task updates cannot replace or clear
it. A new delivery head refreshes the snapshot.

Independent QA dispatch includes a journey table with invariant statements,
read-back checks and generated evidence requirements. An explicit QA request
before park computes the same snapshot. Registry journeys can trigger independent
QA even when their files are outside the configured UI/API path lists. Missing or
malformed impact inputs produce a warning and leave the existing dispatch rules
in force; impact collection itself never refuses a delivery.

When a YAML registry exists and the snapshot lists live journeys, an approved
`qa_record` requires a ledger row for **each** live journey. Use an exact journey
ID cell and a verdict cell (`PASS`, `FAIL`, `NOT A ROUTE`, or
`NOT EXERCISED: <reason>`), or a compact list row such as `- SEND: PASS`.
A bare ID, a similar ID, an empty reason or an example inside a code fence does
not count. The journey ID must be the row's first cell, and only the verdict
column counts: the table's `Verdict` header column, else the fifth cell of an
eight-column ledger row, else the second cell. A verdict word in another
column, such as an observed `FAIL: 502`, does not count.
A `BLOCKED_EXTERNAL` verdict with `blocked: provider=<provider> error=<error>`
is neither PASS nor FAIL: the journey stays owed and approval is refused,
naming the journey, cell and provider. A journey that declares
`independence_key` (see [release gate](release-gate.md#independent-runs-independence_key))
needs a distinct `<key>=<id>` in each PASS or FAIL row; a duplicate or
missing identity is refused by name. Rejections remain recordable without these rows. Dark/dead journeys,
legacy catalog reports and projects without a registry keep advisory context.

```markdown
| Journey | Verdict |
| --- | --- |
| SEND | NOT EXERCISED: the sandbox is unavailable |
```

The explicit disposition is the escape: approval may record an unexercised
journey, while the ledger preserves why it was unexercised.
