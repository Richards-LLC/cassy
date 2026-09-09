# Evidence ledger

Store the file at `~/.cas/artifacts/<task-id>/LEDGER.md`. The header names the
build revision, one-line scope, surface, 30-minute budget, headline counts for
cells/PASS/FAIL/NOT EXERCISED, the evidence-label split, and the telemetry
sweep state (`sweep: configured — <path>` or the exact line
`sweep: not configured`).

Use this exact row grammar so the verifier can consume it:

```text
id | cell | expected | observed | verdict | label | evidence path | defect task
```

Every row uses one label: `source-inferred` (code read only; proves nothing
user-facing), `fixture` (harness, emulator, or mock), `real-build` (the actual
binary or site), `eyewitness` (a human report recorded verbatim), or
`eyewitness/telemetry` (one valid finding emitted by the configured read-only
sweep). A weaker label than the cell requires is `NOT EXERCISED`, never `PASS`;
`partial` is not a verdict. Each run cell has one screenshot or terminal
capture and a short observed result. Telemetry rows use the saved sweep stdout
as their evidence path and include all six parsed fields in `observed`.

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
