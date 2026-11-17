# Evidence ledger

`cas qa ledger init --task <id>` preserves existing evidence and scaffolds schema 2:
no-code exclusions, live journeys unexercised. Keep its eight columns and counts.
In schema 2, graded runtime expectations cite `path:line`; PASS observations begin
`visible:`, `terminal:` or `http:` with captured output. Database read-back only
corroborates. Uncited expectations are spec questions. Legacy schema 1 stays valid.
`cas qa ledger check <path> --task <id>` is the parser close and the verifier
share; a structural PASS never proves the visible outcome.

Name the build revision, scope, surface and budget, and label evidence by how it
was obtained: source reading and fixtures do not prove the real build. Telemetry
findings cite saved sweep stdout and include all six parsed fields in the observed
result.

Independent channel/viewport surfaces run as separate walk-kit jobs, each with its
own ledger, browser profile and test persona. The kit's `record()` stamps
`progress_at` on every result; factory status flags a QA ledger whose progress
stops for `factory.stall_threshold_secs`.

## Binary sources

A `real-build` row that drives a compiled binary needs a binary of the build
revision, and the header names it: path, `--version` output and SHA-256. Three
sources qualify:

- an exact-revision build by someone the project's build policy allows to
  compile; in a factory that is the factory daemon, supervisor or operator. In
  the Cassy source repository a worker or QA walker calls `factory
  action=candidate_request task_id=<own-task> revision=<full-sha>`; the revision
  must be reachable from HEAD of its own checkout, and a newer request replaces
  a queued one. The daemon runs `cas qa candidate build` and messages the path,
  version, revision, SHA-256 and log, or the exact error. Each build lives at
  `<artifacts_root>/<project-key>/<task-id>/candidates/<revision>/` with
  `VERSION.txt`, `SHA256SUMS` and `RECEIPT.json` beside the binary, and a later
  build leaves it alone, so cite that path. `cas qa candidate path --task
  <task-id>` prints the newest build's path;
- a CI release archive whose recorded commit or tree equals the revision;
- the installed release, once the revision is published and `--version` names it.

When the build guard refuses you a compile, request a candidate and leave the
binary cells `NOT EXERCISED: <reason>` until it arrives. An installed release
whose version names another revision proves nothing about this one.

## Candidate fixture isolation

Before invoking a candidate `serve` or any stateful command, create a disposable
fixture project with an empty `.cas` directory outside its source checkout. Give
every candidate subprocess, including the fixture's `cas init`, an explicit
`CAS_ROOT` naming that `.cas`, an isolated `HOME`, and `XDG_CONFIG_HOME` beneath
that home. Initialize the fixture before serving it and record the resolved
store path in the ledger. A scratch worktree shares the main store through Git;
unsetting `CAS_ROOT` alone can migrate that live store or sync it to Cloud. Keep
the QA agent's own authenticated MCP environment separate from these subprocess
overrides.

## Sections after the rows

Fill these from the constants grep and terminal-state dump in the skill's step 6:

```markdown
## Constants vs expectation
| constant | location | visible contract | predictable? | defect task |

## Contradictions
| terminal state | surface | visible claim | conflicting surface | defect task |

## Honesty
- Automation-caused artifacts, mis-taps, unavailable devices, and cells cut by the time box.
```

An invisible, unpredictable threshold is a defect row; so is a pair of visible
claims that cannot both be true.

## Server read-back and harness failures

Grade paid and account routes on server state. Use the UI to trigger the action;
capture one settled frame as evidence, rather than grading success by a toast,
selector or settle timeout. Bind read-back to the exact actor, workspace, item and
operation from this attempt, at the served build. Record expected and observed
values for the applicable invariants: job/operation terminal status, persisted
item/account fields and ownership, credit consumption and grant rows with balance,
and Stripe TEST subscription or charge status. A prior successful attempt is not
evidence for this one. Use read-only access and the journey's safety policy.

When automation fails, adjudicate the product outcome in the same sweep:

- Matching server read-back with no visible product defect: record `PASS` for the
  product, retain the original timeout/selector/settle failure capture and the
  read-back in the row, and link a harness defect task. This harness-only red does
  not block promotion or require a product fix or whole-sweep rerun.
- Read-back mismatch or a visible product defect: record `FAIL` with a product
  defect task, even if a success toast appeared or server state otherwise matches.
  These failures block promotion under the batch fix/re-walk policy.
- Missing, stale or ambiguous read-back: record `NOT EXERCISED: <reason>` and keep
  the evidence owed. An unverified result cannot become a product PASS.

The supervisor records this ruling in the release ledger before the batch receipt.
Preserve native runner exit codes, failed attempts and CI receipts; a product
ruling never rewrites an automated failure into a green run or bypasses a required
mechanical gate.
