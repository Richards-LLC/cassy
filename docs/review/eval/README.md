# Reviewer accuracy replay

This corpus evaluates review behavior. It grants no merge authority. The operator
must approve thresholds after inspecting measured results, and must require a
rerun through the installed 3.39.0 registered shadow protocol before enabling any
policy that relies on these reviewers.

`corpus.json` pins 14 real defects (12 classes), five nominal negatives and four eligible clean
negatives from the 3.37.0/3.38.0 cycles. Every case has authentic base, delivery
and repair SHAs. The `task_context` is a read-only export of the original task's
current stored description and exact criteria, rather than a claimed historical
criteria snapshot. `criteria`, `defect` and `expected_fix` outside `task_context`
are hidden curator annotations. They never enter reviewer prompts.

The original c18 negative revealed an unseeded real CI-inventory defect (cas-045f).
Its case and measured verdicts remain preserved with `negative_contaminated=true`;
it is excluded from the clean-negative denominator. Supplementary c19 pins the
real corrected permitted-check fixture at 9ee399dd7 and was added before its
model runs. This is a recorded corpus revision, not a silently removed failure.

Cases c04–c06 are separate affected self-reexec callers from one regression;
report both instance recall and grouped recall. c08–c13 include real failing
fixtures confirmed by the 3.38.0 assembly, rather than alleged production bugs.
The clean negatives are corrected real slices, not claims that their entire
repositories contain no defects. A previously unseeded finding can be correct
if independently substantiated. Standards-only source coverage is itself under
evaluation; no case-specific standards are added to make the expected answer
available.

## Isolated replay

The approved initial transport uses three separate Codex processes per case,
with separate shallow repositories containing only the authentic delivery and
its parent. They receive a canonical Spec reference, canonical Standards
reference plus the real pinned standards/rules, or the unchanged task-verifier
body plus its simulation adapter. Models use the same configured default;
telemetry records configuration and any model identifier emitted by the CLI.
Spec receives exact original task sources. Standards receives no task criteria
or Spec report. The baseline remains read-only. New axes may commit certain
fixes on their own side branches. Both reports seal before cross-checking; a
requested revert happens on the fix owner's isolated branch and leaves a
receipt. Fixes never touch the factory delivery branch.

These processes measure actual reviewer behavior and fix patches. They simulate
CAS identity, API authority, report sealing and cross-check protocol. They do
not prove the installed runtime's authenticated protocol. No Rust compilation
or tests are allowed in the replay; the supervisor supplies targeted-test
receipts for fix commits. Until then the bad-commit rate is a lower bound based
on cross-axis revert requests, with the untested count reported separately.

Run from a factory worktree, backgrounded as required by worker policy. Output
must stay in the owning task's artifact directory. The source pin approved by
the supervisor is `175f78c5` (resolve it to a full SHA in `sources.json`), with
the real promoted rules 026, 172 and 175 in `promoted-rules.json`.

```bash
python3 scripts/reviewer-eval.py run \
  --reviewer-sha 175f78c5 --rules docs/review/eval/promoted-rules.json \
  --out /home/pippenz/.cas/artifacts/cas-b622/measured --jobs 3 --fix-transport bridge
python3 scripts/reviewer-eval.py cross-check \
  --reviewer-sha 175f78c5 --rules docs/review/eval/promoted-rules.json \
  --out /home/pippenz/.cas/artifacts/cas-b622/measured --jobs 3
python3 scripts/reviewer-eval.py score \
  --out /home/pippenz/.cas/artifacts/cas-b622/measured \
  --labels docs/review/eval/adjudication.json
```

`--cases c01,c02` selects a transport smoke test. Completed `result.json` files
are reused without regenerating a verdict; source drift is rejected. Preserve
failed runs and use a new directory for an intentional rerun. Process errors
and absent results remain recall misses. Logs include raw JSONL, prompts,
schemas, usage, elapsed wall time, exit/timeout, fix SHAs and reverts. Configured
MCP servers are disabled and factory session environment is removed for
isolated processes. Audit raw command events for truth leakage, prohibited
commands and out-of-scope edits before counting a run as valid.

Codex's workspace sandbox protects `.git`; the initial native-commit smoke
therefore left uncommitted Spec fixes. `--fix-transport bridge` preserves that
sandbox: the reviewer submits an explicit finding ID and file list, and the
runner stages exactly those files and creates the real own-axis commit. The
reviewer receives the full SHA and reports it. The bridge rejects paths outside
the checkout and Git/bridge metadata; it supplies no CAS identity or merge
authority. Commit execution is simulated and recorded separately from measured
reviewer reasoning, patches and cross-checks. Supervisor approval #1898214 authorized this transport for the measured fixer
batch. Requests are restricted to each case's explicit scope files. The Python
boundary suite is a precondition for each batch. A baseline-only stage
uses `--axes baseline` and requires no commit bridge.

## Independent grading

Do not score by keyword matching a report to the seed. A curator inspects each
finding against the authentic pre-fix file and repair diff, and records an
evidence-backed decision. Ungraded findings remain pending. Record whether it
matches the seed separately from whether it is a correct unexpected finding.

```json
{
  "findings": {
    "c01/spec/f1": {
      "correct": true,
      "seed_match": true,
      "evidence": "Original file:line, consequence, exact repair SHA/hunk"
    }
  },
  "commits": {
    "<full-fix-sha>": {
      "targeted_test_status": "pass",
      "evidence": "Supervisor-owned test receipt and exact tested SHA"
    }
  }
}
```

Recall is seeded defects found / eligible seeded defects, including missing
runs in the denominator. Grouped recall collapses c04–c06. Precision is correct
findings / all adjudicated findings; report pending and judgement findings so a
partial score cannot imply readiness. A negative is falsely flagged only when
an independently adjudicated false finding is reported on that slice.
Bad-commit rate is unique fix commits reverted by the other axis or failing
targeted tests / all actual fix commits. A read-only baseline has no fix-commit
denominator, so its rate is N/A. Independent-pass and cross-check tokens/time
are separate; do not confuse summed actor time with wall time of a concurrent
batch. Cached input tokens remain a subset of total input tokens.

## Real registered rerun

Once 3.39.0 is installed, the supervisor provisions real replay deliveries,
legacy dispatches, implementers, verifier children and two distinct registered
Standard SubAgent reviewers per case. No identity is minted or impersonated by
the runner. The supervisor provides authenticated actor adapters and a binding
file through `REVIEW_EVAL_BINDINGS`. The bundled bridge orchestrates actual
`verification action=shadow` start/context/report/cross_check/show calls:

```bash
REVIEW_EVAL_BINDINGS=/path/to/supervisor-owned-bindings.json \
python3 scripts/reviewer-eval.py run \
  --reviewer-sha 175f78c5 --rules docs/review/eval/promoted-rules.json \
  --out /home/pippenz/.cas/artifacts/cas-b622/registered \
  --registered-adapter scripts/reviewer-eval-registered.py
# Then the same command with operation cross-check, followed by score.
```

Each binding has `task_id`, `dispatch_id`, `base_sha`, `head_sha`, `agent_ids`
for supervisor/spec/standards/baseline/implementer, and `actors` with argv arrays
for the authenticated `shadow` and `model` adapters. A `shadow` adapter consumes
`{"action":"shadow","review":{...}}` on stdin, invokes the actual MCP action
or `cas factory shadow-review --request <file>` under its already registered
actor, and returns decoded CAS JSON on stdout. The `model` adapter runs that
registered child's independent review and returns `report` plus `telemetry`;
for cross-check it returns `decisions` plus `telemetry`. The baseline adapter
must record its real bound verification and return `verification_receipt`.
Adapters must preserve raw model events, not just aggregated telemetry. Keep
credentials in the actor's existing session; do not put secrets in argv or the
bindings file. Adapter commands and environment are never written to receipts.

The runtime snapshots standards from the replay delivery HEAD, not from a
caller-supplied prompt. To keep the same standards pin on these old deliveries,
the supervisor transplants the identical pinned CODING_STANDARDS.md into BOTH
replay base and delivery trees. The authentic historical delivery diff must
remain byte-for-byte identical; the bridge verifies the complete binary diff,
exact task sources, real Standards snapshot, rules and proof bounds before
accepting a report. This replay necessarily has new base/head SHAs, retained
alongside the original source SHAs. Replaying original SHAs without a standards
transplant would evaluate the historical missing standards instead.

The bridge stores decoded protocol request/response JSONL, immutable report
receipts, round/dispatch IDs, exact fix commits, cross-check receipts and the
real legacy comparison. It never invokes apply. These bindings/adapters are
deployment inputs owned by the supervisor; the current worker's unregistered
isolated processes cannot stand in for them. Passing fake-adapter tests proves
runner wiring only. A real successful registered rerun and runtime assembly
proof remain mandatory before the operator enables merge policy.

## Runner boundary checks

```bash
REVIEW_EVAL_TEST_DIR=/home/pippenz/.cas/artifacts/cas-b622/python-test \
  python3 scripts/test-reviewer-eval.py -v
```

These Python checks verify truth isolation, absence of future repair objects,
exact diff bounds, commit ownership, missing-run denominators and the combined
revert/test-failure bad-commit denominator. They are not review-accuracy proof.

## Measured2026-09-30 delivery

[Report](2026-09-30-reviewer-accuracy.md) · [HTML](2026-09-30-reviewer-accuracy.html) · [Exact metrics](metrics.json).

Corpus revision3 retains20 slices:14 defects,4 eligible negatives (c15/c16/c17/c20),2 independently discovered contaminated negatives (c18/c19). Supplement selection/correlation and live-skill source-boundary violations are disclosed. All60 independent reports and26 bridge commit receipts are committed as results.json/bridge-commits.json. Explicit grading includes unresolved judgments rather than forcing labels.

Generic `score` remains rerunnable. `report-reviewer-eval.py --out <measured-dir>` exports measured evidence and renders this dated narrative from Markdown; it refuses changed headline metrics instead of silently recycling interpretations. Future reviewer changes need fresh output, independent grading and a new dated conclusion. Tokens are pinned in report-tokens.css; HTML has no network/assets dependency.

The headline uses protocol-valid recall. Invalid reports retain content-only detection evidence and all actual fix commits in precision/safety denominators; they cannot supply valid coverage. Quarantined external skill reads must be eliminated in the registered rerun before any enforcement decision. The bad-commit rate is an observed lower bound while targeted tests remain unknown.
