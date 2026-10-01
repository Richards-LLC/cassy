# CI tier contracts

`../test-ci-test-tiers.sh` runs three independent checks:

1. `policy.py check` parses workflow YAML and the branch-protection JSON. The
   registry names the policy consumer, executable source selector and predicate.
   IDs/actions/command anchors identify steps; display labels cannot prove wiring.
   Public job/status names remain explicit, including Fast Validation and macOS
   Check. Source interface pins carry a reason and do not claim runtime proof.
2. `executable-contracts.sh` runs the real classifier, admission/receipt guards,
   watchdogs, publication retry body, cache isolation and fallback scripts with
   isolated Git/HTTP-command/process fixtures. Action and publication bodies come
   from parsed run scalars, independent of YAML indentation or step labels.
3. `test-policy.py` verifies reformatting/renaming compatibility and deliberately
   broken triggers, contexts, wiring, cache/concurrency contracts. It also runs
   the actual rollup, runner-route and Markdown-filter shell bodies.

`prose-pins.json` is separate: each entry names its consumer and reason. Markdown
contracts still check the named document. Historical workflow annotations now
live in this registry; their wording in comments is not executable evidence.

The parser requires `pyyaml==6.0.3` from `requirements.txt`. Missing PyYAML is a
hard failure, with an installation message. The existing publication-guard step
reuses importable YAML; otherwise it creates a venv under `RUNNER_TEMP`, installs
the pinned dependency there and puts that venv first on `PATH` before
`make -C cas-cli test-ci-tiers`. System Python packages are never modified. Its
triggers,
runner selection, gates, jobs and required contexts are unchanged.

## Preservation ledger

The previous script reported 832 checks: 778 `require_*` instances plus 54
standalone checks. The initial extraction classified these as 681 parsed/source,
14 prose and 83 executable-output instances. Twelve of the latter were ruleset
JSON checks; they now belong to parsed policy, giving **693 / 14 / 71**, plus the
54 standalone checks.

All 71 output assertions remain in the shell fixture suite. Of the 54 standalone
checks, 33 remain there; 4 archive, 15 heavy-concurrency and 1 skew-guard checks
move to parsed positive-control/mutation cases; the copied Markdown filter moves
to an actual step-body subprocess case. Thus the shell suite reports **104**.
The parsed/prose phase reports **709** (693 + 14 + 2 dependency checks), and Python
reports **14 test cases**. The combined report is **827 counted checks/cases**;
Python groups the original mutations and adds independent negative/positive
controls, so the counting unit differs from the old individual shell assertions.

Each preserved registry entry carries its original line. The task's durable
832-row before/after matrix records individual instances, including loop arms
and standalone mutations. New policy entries intentionally have no legacy line.

The additional provisioning case executes the real step body with isolated tool
fixtures: importable YAML performs no install; missing YAML selects venv pip and
venv Python; venv/install failures stop before make. Real provisioning on both
Python paths is recorded separately in the task delivery proof.
