# Hosted queue precheck

| Field | Decision |
| --- | --- |
| First two lines | Show the hosted run state and the command to read it again. |
| Scannable | Show one run state, the full commit SHA, then the run and receipt locations. |
| Readable | The completed status compares its elapsed minutes with the October dispatches. |
| Machine output | `--json` returns one object with `sha`, `ref`, `repository`, `run_id`, `run_url`, `status`, `conclusion` and `dispatched_at`; status adds `jobs`, `elapsed_minutes` and `october_dispatch_minutes`. |
| Omitted | Job details and complete paths stay in JSON and the durable receipt. |

The command embeds its dispatcher, pushes the requested commit without changing
HEAD and starts `ci.yml`. Precheck refs select required queue jobs on hosted
runners, without the advisory manual-dispatch builds. The requested commit owns
its workflow definition; historical commits use their historical CI definition.

Design: `DESIGN.md@b43940e43`.

## Critique

terminal-qa: PASS release-precheck · 12 runs · 0 fail · 0 warn · 0 allowed.
Report: task artifact `terminal-qa/report.json`. The captures use the dispatcher
with a declared forge-response fixture, not a completed hosted run.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | State is line one; the status command is line two. |
| Fit | 4 | One run and SHA; job details stay in JSON. |
| Craft | 4 | Lines fit 80 and 120 columns with an explicit JSON escape. |
| Theme safety | 5 | Four palettes, NO_COLOR and C locale carry identical meaning. |
| Machine contract | 5 | JSON is one document and elapsed minutes are numeric. |

Scored by witty-kestrel-57 on 2026-11-01. Candidate and hosted proofs are recorded
in the task ledger separately.
