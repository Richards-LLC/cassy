# Self-hosted Fast Validation pilot

The push-triggered pilot is an advisory duplicate of the required hosted Fast
Validation archive producer. Separately, the CI workflow may route the required
archive producer, preflight, and doctest lanes for a `merge_group` tree to the
same 32-core `soundwave` host.
The route is explicit and fail-safe: unless repository variable
`CASSY_MERGE_QUEUE_SELF_HOSTED` is exactly `enabled`, those required lanes run
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

## Merge-queue latency follow-up

The first routing change exposed a single-listener queue rather than a slow
individual lane. GitHub may assign the archive, preflight, and doctest jobs in
any order, but one `soundwave-cas-ci` listener can execute only one at a time.

| Queue run | Total | Archive | Preflight | Doctests | Limiting observation |
| --- | ---: | ---: | ---: | ---: | --- |
| [PR #568](https://github.com/Richards-LLC/cassy/actions/runs/32399555279) | 5m32s | 1m20s, `soundwave-cas-ci` | 3m28s, hosted | 2m36s, hosted | Hosted comparison; Linux lanes ran in parallel. |
| [PR #576](https://github.com/Richards-LLC/cassy/actions/runs/32413795966) | 7m07s | 1m28s | 2m26s | 2m12s | All three `soundwave-cas-ci` jobs serialized for 6m08s. |
| [PR #577](https://github.com/Richards-LLC/cassy/actions/runs/32414486769) | 6m03s | 1m30s | 1m20s | 1m09s | Linux rollup finished in 4m57s; macOS finished at 6m02s. |
| [PR #578](https://github.com/Richards-LLC/cassy/actions/runs/32415089032) | 11m19s | 1m25s | 1m18s | 0m51s | Archive was assigned third, delaying hosted shards; macOS independently took 11m04s. |

The hosted-cache option has one demonstrated sub-six-minute run (#568), but it
depends on two hosted compiler caches staying warm and adds main-push compiler
work after the exact tree was already validated. It also does nothing for the
observed macOS variance. The selected option is a second trusted listener with
the same labels. That directly bounds the Linux serialization: two of the
three compiling jobs start together, and the third waits for at most one warm
job instead of two. macOS remains an independent floor and is reported rather
than hidden in the acceptance receipts.

### Second-listener receipts

Two consecutive two-slot queue runs prove that the original one-listener Linux
serialization was removed, but neither meets the whole-queue six-minute
target. The new slot was cold for preflight on the first run and reused its
persistent target directory for the second run. macOS remained the critical
path in both receipts.

| Receipt | Total | Archive | Preflight | Doctests | Fast Validation | macOS Check |
| --- | ---: | --- | --- | --- | ---: | ---: |
| [PR #580](https://github.com/Richards-LLC/cassy/actions/runs/32419668295) | 7m54s | 1m38s, `soundwave-cas-ci` | 3m36s, `soundwave-cas-ci-2` (cold) | 1m21s, `soundwave-cas-ci` | 4m02s | 7m37s |
| [PR #581](https://github.com/Richards-LLC/cassy/actions/runs/32420585590) | 7m39s | 3m00s, `soundwave-cas-ci-2` | 1m31s, `soundwave-cas-ci` | 1m22s, `soundwave-cas-ci` | 6m12s | 7m22s |

The two self-hosted listeners accepted archive and preflight concurrently at
21:29:50Z. Doctests started on slot 1 one second after archive completed, so
no job used a shared Cargo target lock. The remaining measured floor was the
hosted macOS job, not the trusted Linux route. In the second receipt, archive
and doctests started concurrently at 21:40:46Z; preflight started on slot 1 one
second after doctests completed. All three self-hosted jobs completed 3m15s
after run creation, while the hosted shards extended the Linux rollup to 6m12s
and macOS set the 7m39s whole-run total.

## Trust boundary

`Richards-LLC/cassy` is public. GitHub warns that persistent self-hosted runners
should almost never execute public-repository pull request workflows because a
fork can persistently compromise the machine. This runner therefore has four
layered restrictions:

1. `.github/workflows/self-hosted-fast-validation.yml` has only a canonical
   repository `push` trigger for `main`, `epic/**`, and `factory/**`. It has no
   pull-request, pull-request-target, workflow-run, comment, or dispatch event.
2. The `cassy-public-trusted` organization runner group selects only repository
   `Richards-LLC/cassy`; it deliberately has `restricted_to_workflows=false`.
   GitHub matches that policy against the synthetic
   `refs/heads/gh-readonly-queue/...` ref for `merge_group`, not `main`, and
   selected-workflow wildcards are rejected. A `ci.yml@refs/heads/main`
   restriction therefore makes a self-hosted queue job unclaimable.
3. The job repeats the canonical repository, non-fork, push-event, and trusted
   ref conditions in a server-evaluated job `if` before runner assignment. The
   same guard requires repository variable `CASSY_SELF_HOSTED_PILOT=enabled`;
   absent or disabled is a clean skip, so registration or maintenance cannot
   create a red/queued advisory run. These checked-in conditions are defense
   in depth, not an independent boundary against a fork's pull-request head:
   that head can modify its own workflow definition.
4. The organization Actions fork-PR contributor approval policy is
   `approval_policy=all_external_contributors`. Every fork workflow run from a
   user who is not an organization member requires a maintainer with write
   access to approve it before it executes. This is the required compensating
   boundary for `restricted_to_workflows=false`; it must not be relaxed to a
   first-time-contributor policy.

Fork pull requests do not execute any workflow until this approval is given.
Approval is a maintainer's explicit authorization to run the PR head, including
its workflow changes; an approver must treat changes that request self-hosted
labels as security-sensitive. The CI route selects the box only when
`github.event_name` is `merge_group` and the explicit control variable is
enabled; the route job does not check out source, and the archive job repeats
the selected-mode guard. The queue route's no-checkout selector,
`merge_group`-only archive guard, canonical-repository check, non-fork check,
queue-ref check, and explicit readiness variable remain required defense in
depth; do not weaken them independently. Ephemeral/JIT runners remain future
hardening, but the operator chose the enforceable approval gate now rather than
changing runner routing or retiring the merge-queue acceleration.

## Availability and isolation

The pilot job is not present in `docs/branch-protection/main-ruleset.json`. The
ruleset still requires only the `Fast Validation` rollup and `macOS Check`.
For ordinary PRs, and whenever the self-hosted control variable is absent or
disabled, every Fast Validation component runs hosted. A merge-queue archive,
preflight, and doctest lane may use the trusted box only after the runner is
confirmed online; the exhaustive shards remain hosted and parallel.

GitHub cannot reassign a job after it has been scheduled on an offline
self-hosted label. The safe maintenance/offline-fallback sequence is therefore
to disable the route first, verify a queue entry selected `ubuntu-latest`, and
only then stop the listener. An unexpected host failure after an operator has
explicitly enabled the route is a GitHub scheduler limitation, not a condition
the workflow can repair after assignment; treat the variable as a readiness
lease and disable it before any maintenance. This preserves a concrete hosted
fallback instead of wedging planned queue work.

Two listeners run as the non-login `cassy-actions` system account under
`cassy-actions-runner.service` and `cassy-actions-runner-2.service`. Their
checkouts, Cargo target directories, Rust toolchain, and sccache live under
`/var/lib/cassy-actions`, never under `.cas` or the factory worktrees. The
listeners share the read-only toolchain but not mutable build state. Every
self-hosted workflow calls `scripts/setup-cassy-actions-rust.sh`: it takes a
shared `flock` around the exceptional `rustup toolchain install` and otherwise
only verifies the pre-provisioned stable toolchain, without changing the shared
rustup default. This keeps concurrent merge-queue lanes safe even when the
runner image needs a one-time toolchain repair:

| Slot | Runner | Checkout | Cargo target | sccache | Port |
| --- | --- | --- | --- | --- | ---: |
| 1 | `soundwave-cas-ci` | `runner` | `cache/cargo-target` | `cache/sccache` | 4227 |
| 2 | `soundwave-cas-ci-2` | `runner-2` | `cache/cargo-target-2` | `cache/sccache-2` | 4228 |

Both units have no privileges or Docker access and apply
systemd filesystem/device/kernel hardening, use `nice=10` and best-effort
`ionice=7`, cap Cargo at 12 jobs, and reserve 2,048 cgroup task slots. This
does not raise compilation concurrency: it prevents sccache plus 12 parallel
rustc/linker process trees from exhausting the distro's 512-task service
default and returning `EAGAIN` while spawning a compiler. The two 12-way caps
leave eight of the host's 32 logical CPUs outside Cargo's job budget. Dedicated
sccache ports 4227 and 4228 prevent either slot from finding the operator's
default-port cache or the other listener's server. The systemd launch wrapper starts sccache before the
GitHub listener, outside Runner.Worker's per-step process tracking; starting it
inside one workflow step causes the runner to reap it before Cargo's next step.
The launch wrapper then uses GitHub's `bin/runsvc.sh`, which translates systemd
termination into the listener signal that closes the remote session cleanly.
`SCCACHE_IDLE_TIMEOUT=0` is committed in the unit: the server must not
self-terminate after ten minutes of queue inactivity. The incident-only debug
drop-in is not a provisioned dependency; inspect the journal or reproduce with
the committed unit instead. The unit also sets `CARGO_CACHE_RUSTC_INFO=0`.
Cargo's otherwise persistent `CARGO_TARGET_DIR/.rustc_info.json` had retained
a failed `sccache rustc -vV` response and replayed it in later jobs without a
new request reaching the healthy server. Disabling only that version-probe
cache retains the shared target artifacts while making each job re-check the
live compiler/cache path.
The pilot workflow explicitly clears `RUSTC_WRAPPER` for its first measured
receipt. The private cache is a later optimization, never a lane prerequisite.
It also points `TMPDIR` at GitHub Runner's disk-backed temporary directory;
nextest's default archive extraction used the unit's private tmpfs-backed
`/tmp` and consumed about 2.5 GB during the shard evaluation.

### Shockwave cache mount and enforced periodic budgets

`/var/lib/cassy-actions/cache` must be the bind mount whose exact filesystem
root is `/home/.cassy-actions-cache`. `/home` is itself backed by the
`/mnt/shockwave` ext4 volume. Both runner units require `/home` and the cache
mount, then run `check-cache-mount.sh` before starting; the guard compares the
cache's exact `findmnt` `FSROOT` and device with Shockwave. A missing mount or a
different directory on the same device fails closed instead of silently
growing a new cache on the root filesystem. The durable host entry is:

```fstab
/home/.cassy-actions-cache /var/lib/cassy-actions/cache none bind,nofail,x-systemd.requires-mounts-for=/home 0 0
```

Each slot has a decimal 60,000,000,000-byte enforced periodic budget. This is
not a filesystem quota or an instantaneous hard ceiling: a running build can
grow past it, and the idle-time pruner restores the bound before a later job.
Cargo target data is
configured at no more than 50,000,000,000 bytes. `SCCACHE_CACHE_SIZE=8G` uses
sccache's binary suffix and therefore means 8,589,934,592 bytes; the configured
sum is 58,589,934,592 bytes. `CARGO_INCREMENTAL=0` is set in the runner units
and CI workflows so incremental sessions cannot regrow.

`cassy-actions-cache-prune.timer` runs daily. GitHub Runner's job-started hook
acquires a shared lock before checkout and a job-completed hook releases it;
the holder clears `RUNNER_TRACKING_ID` so per-job orphan cleanup cannot reap it.
It stays in the runner service cgroup, so stopping the unit still reaps it.
The pruner takes the same lock exclusively and nonblocking, then rechecks the
exact Shockwave mount and both service cgroups for `Runner.Worker`, not merely
`cargo` or `rustc`. This barrier prevents a job from starting across destructive
pruning and prevents pruning throughout a job that happens not to run a Rust
compiler. An active worker skips the scheduled run. Missing cgroups, unreadable
PID state, a bad mount, or a forced run while busy fail closed. When idle, the
pruner removes stale incremental sessions and `deps` files first, then whole
known-rebuildable Cargo profiles if
needed. Every deletion target is canonicalized, must remain on the cache device,
and is rejected if any path component is a symlink. The pruner accounts for the
slot's actual sccache bytes when enforcing the total; unknown over-budget target
data is retained and reported as a failure.

The current migration keeps runner credentials, listeners, workspaces, and
toolchains under `/var/lib/cassy-actions`; only the persistent cache is on
Shockwave. Any retained pre-cutover or staged cache copy remains rollback
evidence until an operator separately verifies post-cutover CI and authorizes
cleanup. This repository installs guards and policy but does not perform the
migration.

For an authoritative-layout rollback rehearsal, keep all self-hosted routes
disabled, prove both organization runners have `busy=false` and no
`Runner.Worker`, then stop both services. Back up `/etc/fstab`, unmount the
cache bind, copy `/home/.cassy-actions-cache/` back into the root-backed
`/var/lib/cassy-actions/cache/` with `rsync -aHAX --numeric-ids`, and require an
empty checksum dry-run before removing the bind entry. Reload systemd and
restart both services only after ownership and byte counts match. Restore the
backed-up fstab and bind mount instead if any pre-start proof fails. This is an
operator procedure; the repository installer never performs a migration or
deletes either copy.

Audit without running the destructive pruner:

```bash
findmnt -T /mnt/shockwave
findmnt -T /home
findmnt -T /var/lib/cassy-actions/cache -o TARGET,SOURCE,FSTYPE,FSROOT,MAJ:MIN
sudo /usr/local/sbin/cassy-actions-cache-prune --check-idle
systemctl status cassy-actions-cache-prune.timer
```

## Provision and audit

Create `cassy-public-trusted` before registration with selected repository
`Richards-LLC/cassy`, `allows_public_repositories=true`, and
`restricted_to_workflows=false`. This is intentional: GitHub cannot match a
selected workflow pinned to `main` against merge-queue refs and rejects a
queue-ref wildcard. The checked-in CI route is additional defense in depth.
The required compensating boundary is the organization-wide
`approval_policy=all_external_contributors` fork-PR approval control; the
checked-in route is defense in depth. Record the live setting before and after
any change:

```bash
gh api orgs/Richards-LLC/actions/permissions/fork-pr-contributor-approval
gh api repos/Richards-LLC/cassy/actions/permissions/fork-pr-contributor-approval
```

After updating an existing group, read it back before enabling the route:

```bash
gh api --method PATCH orgs/Richards-LLC/actions/runner-groups/GROUP_ID \\
  -f restricted_to_workflows=false
gh api orgs/Richards-LLC/actions/runner-groups/GROUP_ID \\
  --jq '{restricted_to_workflows, selected_workflows}'
```

Generate a short-lived organization registration token for each registration,
then run from a trusted checkout. `RUNNER_SLOT` defaults to `1`; provisioning
slot 2 is explicit so rerunning the installer cannot silently change the
existing listener:

```bash
RUNNER_TOKEN=... RUNNER_SLOT=2 SCCACHE_SOURCE="$(command -v sccache)" \
  sudo --preserve-env=RUNNER_TOKEN,RUNNER_SLOT,SCCACHE_SOURCE \
    scripts/install-cassy-actions-runner.sh
```

Verify both runner names report `online` before enabling the job:

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
sudo systemctl stop cassy-actions-runner-2.service
```

Audit without exposing tokens:

```bash
gh api orgs/Richards-LLC/actions/runner-groups
gh api orgs/Richards-LLC/actions/runner-groups/GROUP_ID/repositories
gh api orgs/Richards-LLC/actions/runner-groups/GROUP_ID/runners
systemctl show cassy-actions-runner.service cassy-actions-runner-2.service \
  -p User -p Group -p Nice -p CPUWeight -p IOWeight -p TasksCurrent -p TasksMax \
  -p ReadWritePaths
```

To demonstrate hosted fallback, stop the service, push a Rust-touched commit on
a same-repository factory branch with an open PR, and record that all required
hosted contexts finish while the disabled advisory pilot skips cleanly. Restart
the service only after capturing the receipt and explicitly re-enabling the
opt-in.
