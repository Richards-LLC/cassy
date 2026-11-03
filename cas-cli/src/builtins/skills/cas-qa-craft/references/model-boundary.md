# Model-owned plan boundaries

A plan can reserve meaning for the model while allowing deterministic state,
validation and permissions. Put this optional block in the approved epic
description or its referenced plan; existing plans without it keep their current
verification behavior.

````markdown
```model-boundary
{
  "model_owned_paths": ["src/reader/**", "tests/reader/**"],
  "model_owned_concerns": ["Interpret operator assent and corrections"],
  "model_boundary_exceptions": [
    {
      "paths": ["src/reader/protocol.rs"],
      "kinds": ["literal_collection"],
      "reason": "State schema: validate protocol state names, without interpreting replies"
    }
  ]
}
```
````

Paths are case-sensitive repository-relative `fnmatch` globs; `*` can cross `/`.
When paths are present they limit the scan. Concerns explain the reserved
meaning to the verifier. A concerns-only declaration scans all changed text and
leaves relevance to that reviewer. Empty paths and concerns declare no boundary.
Exceptions need nonempty paths, supported kinds and a reason; they belong to the
approved plan, not an unapproved worker revision. Apply every parent/plan
declaration independently; an exception in one cannot waive another.

After finding the immutable task delivery base and head, run the installed
helper (the path is under the current harness's skills directory):

```bash
python3 <skills>/cas-qa-craft/scripts/model-boundary.py \
  --repo <review checkout> --base <delivery base> --head <bound head> \
  --plan docs/reader-plan.md
```

`--plan` reads the plan from the base snapshot, so deleting its block in the
delivery cannot disable the check. If the approved source is an epic description,
artifact or an authorized later revision, save that source in the task artifacts
and use `--plan-file <file>` instead. That file can contain one fenced block or
the raw boundary JSON. Cite the authorization and source; never export an
unapproved delivery's weaker declaration. Invalid metadata or unreadable input
returns exit 2 with a JSON error.

The helper emits one JSON document with `plan`, `base`, `head`, `boundary`,
`findings` and `allowed`. Each site has `file`, `line` and `kind`; allowed sites
also name the reason. Exit 1 and `status: rejected` flag blocking candidates;
exit 0 means clear or not declared. The supported syntax signals are:

- `literal_collection`: quoted members of a literal array, set or assigned tuple;
  new members of an existing multiline collection also count.
- `regex`: common Rust/Python/JavaScript constructors or JavaScript regex literals.
- `exact_string_assertion`: common equality assertion macros, Python assertions
  and JavaScript equality matchers containing a quoted literal.

Only added lines count. Moves are diffed as delete/add so moving a list into a
reserved path cannot bypass review. The helper uses Git blobs and performs no
source execution. It reports sites without echoing literal contents.

These deliberately broad syntax signals do not decide intent. The verifier reads
each site and the complete diff, distinguishes deterministic state/validation
from reply interpretation, and records the plan evidence for any false positive.
Aliases, mixed/dynamic collections, comments inside lists and unsupported syntax
may evade the heuristic; a clear scan is not evidence of semantic compliance.
New meaning-matching code within the boundary is blocking unless the approved
plan allows it. Name each site, the violated concern and authorization for any
exception. Record `MODEL-BOUNDARY` alongside `PLAN-FIT`; the existing plan-fit
rejection rules still apply. Script fixtures prove detection and scope, not the
independent model's verdict.
