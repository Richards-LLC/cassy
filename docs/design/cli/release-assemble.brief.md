# Release assembly

| Field | Contract |
| --- | --- |
| First two lines | State whether the tested integration tip was assembled, then name the gate step or the reason assembly was refused. |
| Scannable | A verdict, a next step, and the immutable commit receipt fit three ASCII lines. |
| Readable | Refusal names missing, stale, or failing integration evidence and directs the supervisor to the epic merge/sweep workflow. |
| Machine output | The existing release train has no `--json` mode; `.cas/merge-sweeps/integration.json` records base, epic inputs, tip, status, detail, and affected epics. |
| Omitted | Merge output and per-test logs stay in the integration receipt and sweep logs; assembly never prints a Git graph. |

The command uses plain ASCII with no colour or TTY control sequences. Success requires a clean detached or release branch checkout, a passing receipt matching the integration ref, and unchanged input refs. It fast-forwards only. Failure never resets the destination.

## Critique

terminal-qa: PASS release-assemble · 11 runs · 0 fail · 0 warn · 0 allowed

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | PASS and the next gate action precede the immutable tip. |
| Fit | 4 | Three lines fit the release train's existing shell workflow. |
| Craft | 4 | Plain ASCII, no wrapping, and no hidden colour meaning. |
| Theme safety | 5 | Four palettes, piped, NO_COLOR, and C-locale captures passed. |
| Machine contract | 4 | The separate JSON receipt contains all integration inputs and status. |

Scored 2026-09-10. Captures and full receipt: task artifact `terminal-qa/report.json`.

```text
PASS release assembly
Next: run release-train.sh with --gate
Tip: <40-character commit>
```
