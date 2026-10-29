# Touched Rust test stability

Target: Cassy worker evidence and task park. design: DESIGN.md@b43940e43.

| Field | Contract |
| --- | --- |
| First two lines | Name the missing stability evidence and the touched source tests, or say that selection could not be resolved. |
| Scannable | Put the verdict before the cause and the one retry action; successful runs print tests, runs, parallelism and retries next to the bound revision. |
| Readable | The worker's existing test log retains each of the five original trial logs and failures, while park explains which evidence remains missing. |
| Machine output | The internal test-receipt JSON gains optional `stability` with `version`, `base`, `head`, `tests`, `names`, `runs`, `parallelism`, `retries` and `trials`; trial records carry exit code, count, start/end and log path. This adds no public CLI `--json` renderer. |
| Omitted | Park does not print all trial output, execute tests, retry failures, quarantine a case or start a CI stage; those responsibilities remain in the existing worker evidence node and its logs. |

## Verification

Candidate-based terminal capture and critique are pending. The existing task-close
transport owns wrapping and JSON envelopes; this change supplies its refusal text.
Do not infer a capture or passing UI receipt from source inspection.
