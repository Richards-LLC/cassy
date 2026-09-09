# Worked exemplar: exploratory filter matrix

Task `cas-1234` says, “User filters tasks and sees no matches.” The build under
test is commit `8b7f1de`, scope is “filtering plus adjacent task-list status,”
and the 30-minute budget is written in
`/home/pippenz/.cas/artifacts/cas-1234/LEDGER.md` before the run.

```text
id | cell | expected | observed | verdict | label | evidence path | defect task
M01 | submit a task filter | matching rows update | 3 rows remained | PASS | real-build | step-01.png | —
M02 | query with no matches (empty state) | clear no-results copy | message and no rows | PASS | real-build | step-02.png | —
M03 | refresh after the empty state (second visit) | filter and empty state persist | filter reset unexpectedly | FAIL | real-build | step-03.png | cas-defect-1
M04 | simulate a slow request then timeout | user sees retryable error | spinner stayed forever | FAIL | real-build | step-04.png | cas-defect-2
M05 | repeat at phone width | controls remain usable | rows and status fit | PASS | real-build | step-05.png | —
M06 | tab and submit without a pointer | focus and submit remain visible | focus ring and result visible | PASS | real-build | step-06.png | —
M07 | read the adjacent status line after filtering | count agrees with list | “5 results” beside 0 rows | FAIL | real-build | step-07.png | cas-defect-3
```

Rows M02–M06 are conditions the demo never mentioned; M07 is the adjacent
status surface. Only M01 replays the stated happy path. The expected text was
written before each cell; every row has one capture and a two-sentence observed
narrative in the full ledger. The row label is `real-build`, so each PASS is
user-facing evidence rather than source inference or a fixture result.

```markdown
## Telemetry sweep
`[qa] telemetry_sweep = "scripts/qa/telemetry-sweep.sh"` produced the
following valid line before the matrix:

```text
RISING\tcheckout\t42\t17\t2026-09-01..2026-09-07\tcheckout completed
```

The header says `sweep: configured — scripts/qa/telemetry-sweep.sh`, and the
finding is recorded before M01 as:

```text
T01 | telemetry sweep | RISING finding is surfaced | kind=RISING; subject=checkout; count=42; people=17; window=2026-09-01..2026-09-07; sample=checkout completed | PASS | eyewitness/telemetry | telemetry-sweep.stdout | —
```

Known, explained noise is cited rather than filed again with this table:

| subject | explanation | task id | citation |
| --- | --- | --- | --- |
| checkout | expected spike during migration | cas-1234 | task note |

## Constants vs expectation
| `DEBOUNCE_MS=300` | filter.ts:18 | user sees delayed update | no — no progress cue | cas-defect-4 |

## Contradictions
| filtered-empty | list | “No tasks” | status line says “5 results” | cas-defect-3 |

## Honesty
- The timeout was induced by the local network throttle; it is a product-risk observation, not a claim of a production outage.
- The phone-width cell used a real browser viewport, not a fixture.
```

The task note and close reason both cite the durable ledger path. Defects are
filed separately; this QA pass does not patch their implementation.
