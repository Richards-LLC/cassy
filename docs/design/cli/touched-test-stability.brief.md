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

Supervisor-built candidate `82b1f3f7e862a693f724ce66e58961df26fe157a`
(`cas 3.89.0 (82b1f3f)`) exercised a disposable Rust project with isolated
HOME, XDG config and CAS_ROOT. An untouched-test receipt refused park with the
changed source identity; the real worker node then ran five concurrent copies
with zero retries. Independent timestamps show all five test bodies overlapped.
Park accepted the bound receipt and stored AwaitingMerge with its exact anchor.
The task artifact contains `candidate-fixture/{park-refusal,park-acceptance}.txt`,
worker logs, the immutable receipt and its read-back.

The existing task-close transport owns wrapping and JSON envelopes. This change
adds internal receipt policy and its cause/remedy text; it adds no terminal
renderer. Changes under `ui/factory` are fixture setup in test functions only.
No terminal appearance or interaction is claimed from the protocol evidence.
