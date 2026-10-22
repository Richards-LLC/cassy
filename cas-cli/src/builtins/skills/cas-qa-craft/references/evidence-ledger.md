# Evidence ledger

Store the file at `~/.cas/artifacts/<project-key>/<task-id>/LEDGER.md`. The header names the
build revision (for a served app, the backend and frontend SHAs at walk start
and end, and any mixed build: [map the build](../SKILL.md#walk-rules)),
one-line scope, surface, 30-minute budget, headline counts for
cells/PASS/FAIL/NOT EXERCISED, the evidence-label split, and the telemetry
sweep state (`sweep: configured — <path>` or the exact line
`sweep: not configured`). When the release gate snapshots the walked
journeys' `depends_on_flags`, copy its header line verbatim:
`Flags: <env>@<sha> {"flag":value}`. Promotion diffs it against the target
environment, and a different value is `CONFIG_MISMATCH`.

Use this exact row grammar so the verifier can consume it:

```text
| id | cell | expected | observed | verdict | label | evidence path | defect task |
```

Every row uses one label: `source-inferred` (code read only; proves nothing
user-facing), `fixture` (harness, emulator, or mock), `real-build` (the actual
binary or site), `eyewitness` (a human report recorded verbatim), or
`eyewitness/telemetry` (one valid finding emitted by the configured read-only
sweep). A weaker label than the cell requires is `NOT EXERCISED`, never `PASS`;
`partial` is not a verdict. A cell an external provider blocked (5xx, 429,
402) is `BLOCKED_EXTERNAL` with evidence `blocked: provider=<provider>
error=<error>`; it stays owed, never PASS or FAIL, and the gate refuses until
it is re-walked. Each run cell has one screenshot or terminal
capture and a short observed result. Telemetry rows use the saved sweep stdout
as their evidence path and include all six parsed fields in `observed`.

A `real-build` row that drives a compiled binary needs a binary of the build
revision, and the header names it: path, `--version` output and SHA-256. Three
sources qualify:

- an exact-revision build by someone the project's build policy allows to
  compile; in a factory that is the supervisor or operator, who places it at
  `<artifacts_root>/<project-key>/<task-id>/candidate/`. In the Cassy source
  repository, run `cas qa candidate build --task <task-id> --rev <sha>`; it
  prints the binary path, version and SHA-256 and writes `VERSION.txt`,
  `SHA256SUMS` and `RECEIPT.json` beside the binary;
- a CI release archive whose recorded commit or tree equals the revision;
- the installed release, once the revision is published and `--version` names it.

When the build guard refuses you a compile, do not work around it: ask the
supervisor for a candidate, and leave the binary cells `NOT EXERCISED` until it
arrives. An installed release whose version names another revision proves
nothing about this one.

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
