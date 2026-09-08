# Evidence ledger

Store the file at `~/.cas/artifacts/<task-id>/LEDGER.md`. The header names the
build revision, one-line scope, surface, 30-minute budget, headline counts for
cells/PASS/FAIL/NOT EXERCISED, and the evidence-label split.

Use this exact row grammar so the verifier can consume it:

```text
id | cell | expected | observed | verdict | label | evidence path | defect task
```

Every row uses one label: `source-inferred` (code read only; proves nothing
user-facing), `fixture` (harness, emulator, or mock), `real-build` (the actual
binary or site), or `eyewitness` (a human report recorded verbatim). A weaker
label than the cell requires is `NOT EXERCISED`, never `PASS`; `partial` is not
a verdict. Each run cell has one screenshot or terminal capture and a short
observed result.

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
