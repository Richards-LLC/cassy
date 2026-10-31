# CI tier contracts

`../test-ci-test-tiers.sh` runs three independent checks:

1. `policy.py check` parses the workflow YAML and the branch-protection JSON.
   Since cas-a1b0, `policy.json` holds only the required-context contract: each
   context the main ruleset requires (`Fast Validation`, `macOS Check`) is
   required exactly once and is produced by a CI job of that name. It no longer
   pins workflow text, step layout or gate-row parity: those pins (564 `text`
   and 112 `absent` needles) caught no recorded defect and failed 59 of 90
   ci.yml edits (cas-a677 report §4). Removing or moving a CI check now fails
   only if it breaks a required context.
2. `executable-contracts.sh` runs the real classifier, admission/receipt guards,
   watchdogs, publication retry body, cache isolation and fallback scripts with
   isolated Git/HTTP-command/process fixtures.
3. `test-policy.py` executes real workflow step bodies (the required rollup,
   runner route, Markdown filter, parser provisioning) and the shared rustup
   helper, plus the required-context mutations.

`prose-pins.json` is separate: each entry names its consumer and reason. Markdown
contracts still check the named document; annotations are never executable
evidence.

The parser requires `pyyaml==6.0.3` from `requirements.txt`. Missing PyYAML is a
hard failure, with an installation message. The CI tiers job reuses importable
YAML; otherwise it creates a venv under `RUNNER_TEMP`, installs the pinned
dependency there and puts that venv first on `PATH` before
`make -C cas-cli test-ci-tiers`. It runs `npm ci` at the repository root for the
pinned markdownlint-cli2 that `scripts/markdownlint.py` resolves. System Python
packages are never modified.

The shared `.github/actions/run-script-suite-tests` action also provisions the
16 registered script/visual QA suites. PRs and factory pushes select each suite
from its own test and input paths in `ci-script-tests-for-diff.py`, inside the
existing classification job. An unrelated diff installs nothing and runs no
suite. Assembly retains every row in `test-ci-tiers`. Browser suites use the
locked `hub-web` dependencies and Chromium; PDF checks use pinned PyMuPDF in a
venv. The native terminal suite runs against the CLI the full lane already
built. Binary absence is logged explicitly in script-only contexts; native
Codex cases report their missing installed client as explicit unittest skips.

## Platform behavior

The runner cache pruning, mount-guard and job-lifetime lock fixtures exercise
Linux cgroups, `/proc`, `flock` and `findmnt`/mountpoint semantics. On Darwin the
executable suite reports these three cases as explicit skips, separately from
passed cases; it still checks the scripts and fixtures are executable. Linux
runs all three unchanged. Parsed cache policy contracts run on both platforms.
The separate `test-self-hosted-rust-setup.sh` Make cohort fixture also reports
an explicit Darwin skip because its Linux shared-toolchain concurrency contract
requires `flock`. Python policy tests exercise the real setup helper on both
platforms using POSIX file locks with isolated rustup fixtures: two simultaneous
lanes perform one install; incomplete registries, missing standard-library files,
and broken cargo fail clearly without mutating a preprovisioned toolchain.

Both cancellation watchdogs run their full fixtures on macOS and Linux. They
retain GNU `date -u -d` parsing when it succeeds and fall back to Python 3's
UTC-aware ISO-8601 parser for GitHub timestamps when it fails. The fixtures also
force the fallback on Linux, cover UTC offsets/fractions and invalid timestamps,
and verify that successful GNU parsing retains its exact result.

Workflow-body subprocess fixtures retain the parent-selected Bash executable
when isolating fake-tool `PATH`, so Homebrew Bash on macOS is not silently
replaced by the system Bash 3.2. The actual Ubuntu run scalars stay unchanged.
