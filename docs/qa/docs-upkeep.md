# Documentation upkeep

Deliveries do not edit the shared generated documents. Each delivery writes its
own fragment files, which never conflict with another delivery's, and release
prep folds them in once (cas-4b33).

| Shared document | A delivery adds | At release prep |
| --- | --- | --- |
| `docs/qa/journeys.yaml` bug families | `docs/qa/bug-families/<slug>.yaml` | `scripts/release.sh` compiles each file into the inline `bug_families` map and deletes it |
| `docs/PRODUCT_OVERVIEW.md` | `docs/product-overview.d/<task>.md` | `scripts/release.sh` appends each `## <Section>` block to that section and deletes the fragment; the supervisor may refresh the overview with the project-overview skill |
| `.claude/CODEMAP.md` | nothing | the supervisor regenerates it with `/codemap` on the train before the cut; `scripts/release.sh` refuses to prepare while a file added or removed since the map's last commit is unmapped (fragments and release notes excepted) |

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

The `journey-catalog` fast row runs two checks:

- `python3 scripts/doc-fragments.py check` when a lane changes the registry, a
  bug family, an overview fragment or the overview. It refuses a registry that
  does not parse, repeated journey IDs, duplicate or malformed bug families,
  family references that do not resolve, and overview fragments that name a
  missing section.
- Cassy's registry validator on every lane diff that touches the registry, a
  bug family or a registry `surfaces` path (any path when the registry
  declares no surfaces): `cas qa journeys --check` plus the former park gates
  G1 (registry problems the diff caused) and G2 (new surface files no journey
  owns; strict also checks modified files), through
  `cas qa journeys --check --base <lane base>` or, on a release before that
  flag, the same facts from `cas qa impact <lane base> --json`. The row uses
  `CAS_BIN`, else installs the latest release with `scripts/cas-install.sh`
  (SHA-256 checked) into a temporary directory, so lane CI needs no workflow
  step. With no validator available the row fails.

Each problem prints its code and a `FIX:` line. The former bug-case gate (G3)
needs the task type, which a lane does not have; the close checks list it as
advice for bug tasks.

Task park no longer runs the registry upkeep gates (G1–G3) or the CODEMAP and
PRODUCT_OVERVIEW drift gates (G5/G6), and supervisors are not stopped from
creating tasks while CODEMAP is stale. `cas qa impact` still lists unowned
surface files and registry problems under "Required upkeep (registry)" before
the lane runs. The `upkeep.*` settings remain readable by
`cas qa journeys --status` but no longer gate anything; `qa.registry.enforce`
is the lane lint's fallback when the registry header sets no policy.
