# Self-hosted Fast Validation pilot

The push-triggered pilot is an advisory duplicate of the required hosted Fast
Validation archive producer. Separately, the CI workflow may route the required
archive producer for a `merge_group` tree to the same 32-core `soundwave` host.
The route is explicit and fail-safe: unless repository variable
`CASSY_MERGE_QUEUE_SELF_HOSTED` is exactly `enabled`, the required archive runs
on GitHub-hosted Ubuntu. Pull-request work always remains hosted. The first
real measurement kept the three exhaustive partitions in the same local job for
evaluation; shard 1 stalled in three subprocess-spawning integration tests for
more than seven minutes, so required shards deliberately remain parallel on
GitHub-hosted runners even when their archive producer uses the box.

The uncached archive build in self-hosted run
[32255590235](https://github.com/Richards-LLC/cassy/actions/runs/32255590235)
took 207.41 seconds. The hosted steady-state baseline in run
[32146014087](https://github.com/Richards-LLC/cassy/actions/runs/32146014087)
was 338 seconds, so the 32-core box saved 130.59 seconds (38.6%) but did
not reach the pilot's two-minute target. This is an honest partial result;
the private sccache service remains outside the workflow and is not a hidden
prerequisite for the measurement.

## Trust boundary

`Richards-LLC/cassy` is public. GitHub warns that persistent self-hosted runners
should almost never execute public-repository pull request workflows because a
fork can persistently compromise the machine. This runner therefore has three
independent restrictions:

1. `.github/workflows/self-hosted-fast-validation.yml` has only a canonical
   repository `push` trigger for `main`, `epic/**`, and `factory/**`. It has no
   pull-request, pull-request-target, workflow-run, comment, or dispatch event.
2. The `cassy-public-trusted` organization runner group selects only repository
   `Richards-LLC/cassy` and only this workflow at an explicit canonical branch
   ref. The pilot ref is replaced by `refs/heads/main` after landing.
3. The job repeats the canonical repository, non-fork, push-event, and trusted
   ref conditions in a server-evaluated job `if` before runner assignment. The
   same guard requires repository variable `CASSY_SELF_HOSTED_PILOT=enabled`;
   absent or disabled is a clean skip, so registration or maintenance cannot
   create a red/queued advisory run.

Fork pull requests continue to run only the existing GitHub-hosted required
checks. The CI route selects the box only when `github.event_name` is
`merge_group` and the explicit control variable is enabled; the route job does
not check out source, and the archive job repeats the selected-mode guard.
Approval of a fork workflow does not grant it the selected runner group. Do not
weaken any of the three restrictions independently.

## Availability and isolation

The pilot job is not present in `docs/branch-protection/main-ruleset.json`. The
ruleset still requires only the `Fast Validation` rollup and `macOS Check`.
For ordinary PRs, and whenever the self-hosted control variable is absent or
disabled, every Fast Validation component runs hosted. A merge-queue archive
may use the trusted box only after the runner is confirmed online; its shards
remain hosted and parallel.

GitHub cannot reassign a job after it has been scheduled on an offline
self-hosted label. The safe maintenance/offline-fallback sequence is therefore
to disable the route first, verify a queue entry selected `ubuntu-latest`, and
only then stop the listener. An unexpected host failure after an operator has
explicitly enabled the route is a GitHub scheduler limitation, not a condition
the workflow can repair after assignment; treat the variable as a readiness
lease and disable it before any maintenance. This preserves a concrete hosted
fallback instead of wedging planned queue work.

The listener runs as the non-login `cassy-actions` system account under
`cassy-actions-runner.service`. Its checkout, Cargo target directory, Rust
toolchain, and sccache live under `/var/lib/cassy-actions`, never under `.cas`
or the factory worktrees. The unit has no privileges or Docker access, applies
systemd filesystem/device/kernel hardening, uses `nice=10` and best-effort
`ionice=7`, and caps Cargo at 12 jobs. One listener and one workflow concurrency
group enforce host job concurrency 1. The runner uses dedicated sccache port
4227; the default port belongs to the operator's cache server and must not be
shared across Unix users. The systemd launch wrapper starts sccache before the
GitHub listener, outside Runner.Worker's per-step process tracking; starting it
inside one workflow step causes the runner to reap it before Cargo's next step.
The launch wrapper then uses GitHub's `bin/runsvc.sh`, which translates systemd
termination into the listener signal that closes the remote session cleanly.
The pilot workflow explicitly clears `RUSTC_WRAPPER` for its first measured
receipt. The private cache is a later optimization, never a lane prerequisite.
It also points `TMPDIR` at GitHub Runner's disk-backed temporary directory;
nextest's default archive extraction used the unit's private tmpfs-backed
`/tmp` and consumed about 2.5 GB during the shard evaluation.

## Provision and audit

Create `cassy-public-trusted` before registration with selected repository
`Richards-LLC/cassy`, `allows_public_repositories=true`,
`restricted_to_workflows=true`, and exactly one selected workflow:

```text
Richards-LLC/cassy/.github/workflows/self-hosted-fast-validation.yml@refs/heads/main
```

Generate a short-lived organization registration token, then run from a trusted
checkout:

```bash
RUNNER_TOKEN=... SCCACHE_SOURCE="$(command -v sccache)" \
  sudo --preserve-env=RUNNER_TOKEN,SCCACHE_SOURCE scripts/install-cassy-actions-runner.sh
```

Verify the runner reports `online` before enabling the job:

```bash
gh api orgs/Richards-LLC/actions/runners
gh variable set CASSY_SELF_HOSTED_PILOT --repo Richards-LLC/cassy --body enabled
```

Enable required merge-queue archive routing only after that online check:

```bash
gh variable set CASSY_MERGE_QUEUE_SELF_HOSTED --repo Richards-LLC/cassy --body enabled
```

Before planned maintenance or an offline-fallback drill, disable required
merge-queue routing and advisory assignment first, then stop the listener:

```bash
gh variable set CASSY_SELF_HOSTED_PILOT --repo Richards-LLC/cassy --body disabled
gh variable set CASSY_MERGE_QUEUE_SELF_HOSTED --repo Richards-LLC/cassy --body disabled
sudo systemctl stop cassy-actions-runner.service
```

Audit without exposing tokens:

```bash
gh api orgs/Richards-LLC/actions/runner-groups
gh api orgs/Richards-LLC/actions/runner-groups/GROUP_ID/repositories
gh api orgs/Richards-LLC/actions/runner-groups/GROUP_ID/runners
systemctl show cassy-actions-runner.service \
  -p User -p Group -p Nice -p CPUWeight -p IOWeight -p ReadWritePaths
```

To demonstrate hosted fallback, stop the service, push a Rust-touched commit on
a same-repository factory branch with an open PR, and record that all required
hosted contexts finish while the disabled advisory pilot skips cleanly. Restart
the service only after capturing the receipt and explicitly re-enabling the
opt-in.
