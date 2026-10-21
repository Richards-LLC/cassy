# Journey impact

`cas qa impact` maps a change to journey ids, evidence requirements and unowned
files. It reads local Git and project files without network access. Registry
matching runs in-process; the legacy catalog adapter invokes the existing
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
attributed QA paths; an unreadable attribution fails instead of broadening to
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
