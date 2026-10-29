# Evidence ledger

Create the task ledger with `cas qa ledger init --task <id>`. It prints the
project-scoped path, includes predicted live journeys, and preserves existing
evidence. No-code deliveries start with route exclusions; live routes start
unexercised. Edit the scaffold as you walk.

Run `cas qa ledger check <path> --task <id>` before close or review. The CLI,
close gate and verifier share one parser for fields, counts, labels, breadth,
defect ownership and readable PASS captures. Repair each line/row diagnostic.
The executable owns the grammar; use its scaffold instead of a copied table.
A passing check proves structure and readability, not the expected user outcome.

Name the build revision, scope, surface and budget. For a served app, record
backend and frontend SHAs at walk start and end, including mixed builds
([map the build](../SKILL.md#walk-rules)). Capture each exercised cell and
describe what happened. Label evidence by how it was obtained; source reading
and fixtures do not prove the real build. Telemetry findings cite saved sweep
stdout and include all six parsed fields in the observed result.

A `real-build` row that drives a compiled binary needs a binary of the build
revision, and the header names it: path, `--version` output and SHA-256. Three
sources qualify:

- an exact-revision build by someone the project's build policy allows to
  compile; in a factory that is the supervisor or operator, who places it at
  `<artifacts_root>/<project-key>/<task-id>/candidate/`. In the Cassy source
  repository, run `cas qa candidate build --task <task-id> --rev <sha>`; it
  prints the binary path, version and SHA-256 and writes `VERSION.txt`,
  `SHA256SUMS` and `RECEIPT.json` beside the binary. Anyone, a worker
  included, gets that binary's path with `cas qa candidate path --task
  <task-id>`, which refuses until the candidate exists;
- a CI release archive whose recorded commit or tree equals the revision;
- the installed release, once the revision is published and `--version` names it.

When the build guard refuses you a compile, do not work around it: ask the
supervisor for a candidate, and leave the binary cells `NOT EXERCISED: <reason>` until it
arrives. An installed release whose version names another revision proves
nothing about this one.

Before invoking a candidate `serve` or any stateful command, create a disposable
fixture project with an empty `.cas` directory outside its source checkout. Give
every candidate subprocess, including the fixture's `cas init`, an explicit
`CAS_ROOT` naming that `.cas`, an isolated `HOME`, and `XDG_CONFIG_HOME` beneath
that home. Initialize the fixture before serving it and record the resolved
store path in the ledger. A scratch worktree shares the main store through Git;
unsetting `CAS_ROOT` alone can migrate that live store or sync it to Cloud. Keep
the QA agent's own authenticated MCP environment separate from these subprocess
overrides.

After the rows, include these sections:

```markdown
## Constants vs expectation
| constant | location | visible contract | predictable? | defect task |

## Contradictions
| terminal state | surface | visible claim | conflicting surface | defect task |

## Honesty
- Automation-caused artifacts, mis-taps, unavailable devices, and cells cut by the time box.
```

Grep the touched feature for `MIN_`, `MAX_`, `_MINUTES`, `_MS`, `_SECS`,
`THRESHOLD`, `GRACE`, `DEBOUNCE`, and `RETRY`; an invisible, unpredictable
threshold is a defect row. For terminal states, dump visible text from every
surface and record claims that cannot both be true. File one task per defect;
never fix code from the QA pass. Put the ledger path in the task note and close
reason.
