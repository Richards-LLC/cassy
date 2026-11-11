# Documentation upkeep

Deliveries do not edit the shared generated documents. Each delivery writes its
own fragment files, which never conflict with another delivery's, and release
prep folds them in once (cas-4b33).

| Shared document | A delivery adds | At release prep |
| --- | --- | --- |
| `docs/qa/journeys.yaml` bug families | `docs/qa/bug-families/<slug>.yaml` | `scripts/release.sh` compiles each file into the inline `bug_families` map and deletes it |
| `docs/PRODUCT_OVERVIEW.md` | `docs/product-overview.d/<task>.md` | `scripts/release.sh` appends each `## <Section>` block to that section and deletes the fragment; the supervisor may refresh the overview with the project-overview skill |
| `.claude/CODEMAP.md` | nothing | the supervisor regenerates it with `/codemap` on the train before the cut |

The compile is `python3 scripts/doc-fragments.py compile`; it runs
`check` first and verifies that every other registry key is unchanged before
writing.

## Overview fragments

Name the file after the task. Start with one or more `## <Section>` headings
that already exist in `docs/PRODUCT_OVERVIEW.md`, each followed by the text to
append to that section:

```markdown
## Core Concepts

- **Worker task queue** - one sentence on what changed for users.
```

## Lane lint

The `journey-catalog` fast row runs `python3 scripts/doc-fragments.py check`
when a lane changes the registry, a bug family, an overview fragment or the
overview. It refuses a registry that does not parse, repeated journey IDs,
duplicate or malformed bug families, family journey and invariant references
that do not resolve, and overview fragments that name a missing section. Each
problem prints its stable code and a `FIX:` line.

Task park no longer runs the registry upkeep gates (G1–G3) or the CODEMAP and
PRODUCT_OVERVIEW drift gates (G5/G6), and supervisors are not stopped from
creating tasks while CODEMAP is stale. `cas qa impact` still lists unowned
surface files and registry problems under "Required upkeep (registry)". The
`upkeep.*` and `qa.registry.enforce` settings remain readable by
`cas qa journeys --status` but no longer gate a close.
