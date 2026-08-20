# Fast release publication

How a pushed tag becomes published, verified artifacts in single-digit minutes,
and why the pieces are shaped the way they are.

Tracked by GH #449 (fast release publication) and cas-3b7c0.

## The measured problem

Before this lane existed, every release compiled the shipped artifacts from
cold at tag time. Measured tag push -> published release, from the GitHub API:

| Release | Tag pushed | Published | Total | Linux build | macOS build | Create Release |
| --- | --- | --- | --- | --- | --- | --- |
| v3.1.0 | 2026-08-18 23:06:55Z | 23:20:13Z | 13m18s | 12m30s | 12m54s | 18s |
| v3.0.0 | 2026-08-18 19:50:29Z | 20:05:24Z | 14m55s | 9m36s | 14m24s | 23s |
| v3.2.0 | 2026-08-19 17:04:30Z | 17:24:33Z | 20m03s | 13m19s | 14m05s | 15s |
| v2.72.0 | 2026-08-17 19:48:20Z | 20:14:46Z | 26m26s | 17m32s | 21m03s | 11s |

Three facts decide the design:

1. The critical path is **always** the two cold platform builds. Actually
   creating the release never took more than 23 seconds.
2. **macOS ARM64 is the long pole** at 12-21 minutes, and there is no Mac in
   the fleet, so no amount of self-hosted Linux speedup can fix it.
3. The builds only start **after** the tag, which serialises them behind the
   version-bump PR's merge — even though the tree they build is final the
   moment that PR lands.

So the artifacts are built when the release PR merges, and the tag publishes
bytes that already exist.

## The architecture

```
release PR merges to main
        |
        v
release-prebuild.yml
  pending-release        cheap gate: is this a release tree?   (~20s, every main push)
  prebuild-runner-route  pick Linux runner before assignment
  build                  Linux x86_64  (self-hosted warm, or hosted)
  build-macos            macOS ARM64   (hosted macos-26, signed here)
        |
        |  artifacts: cas-x86_64-unknown-linux-gnu, cas-aarch64-apple-darwin
        v
tag vX.Y.Z pushed
        |
        v
release.yml
  prebuilt-lookup        find the prebuild run for THIS commit
  release-runner-route   pick verify/build runner before assignment
  verify                 release-input gate (tag, versions, CHANGELOG, cargo check)
  build / build-macos    SKIPPED when a prebuild was found; otherwise the old cold path
  release                adopt -> re-audit -> publish  (hosted, only write-scoped job)
```

### Why the gate is where it is

`scripts/detect-pending-release.sh` calls a tree pending exactly when every
release-train crate carries the same semver, `CHANGELOG.md` has that version's
heading, and no `v<version>` tag exists on the remote yet. The last condition
makes it self-disarming: once the tag is pushed, later pushes at that version
cost one ~20s ubuntu job and stop. Ordinary main pushes never open the
expensive lanes.

### Why adoption is fail-safe and publication is fail-closed

`scripts/find-release-prebuild.sh` only matches a **successful** prebuild run
whose `head_sha` is exactly the commit being published, and only when both
artifacts are present and unexpired. Every degraded input — no run, a partial
run, expired artifacts, an API outage — reports `found=false`, which routes the
tag back to the pre-existing cold build path. The prebuild is an accelerator;
it can never be the reason a release cannot ship.

Publication is the opposite. `release` requires exactly one complete supply
path: either the prebuild was adopted **and both platform builds were skipped**,
or no prebuild existed **and both platform builds succeeded**. A failed build, a
half-skipped pair, or a failed lookup publishes nothing.

### Why the codesign gate did not move

The Darwin signing gate (cas-67c1) travels with whichever job produces the
Darwin bytes. `strip` invalidates an ad-hoc Mach-O signature, so both the
prebuild and the fallback re-sign and then `codesign --verify` the exact
executable they package. Nothing downstream can publish an unsigned or invalid
binary, because nothing downstream produces Darwin bytes.

Mach-O signatures cannot be verified from Linux, so the publish job records the
published digests and the existing `Verify published macOS signature`
`workflow_dispatch` remains the operator's on-Mac receipt.

The publish job additionally re-runs the x86_64 ISA audit on the exact
executable it is about to upload. That is seconds of work, and it closes the
gap that a prebuilt artifact was audited on a different machine at an earlier
time.

## Self-hosted routing and the trust posture

The cas-6981 posture is unchanged: only same-repository, non-fork **push**
events can reach the persistent box, and a public fork cannot emit those.

Routing is a two-key system, matching ci.yml's merge-queue route:

- **Before assignment**, a `*-runner-route` job picks the label set. GitHub
  cannot fall back after assigning an offline self-hosted runner — the job
  simply stays queued — so the choice has to happen first. The repository
  variable `CASSY_RELEASE_SELF_HOSTED` is opt-in, and its absent/disabled state
  selects `ubuntu-latest`, which is the historical release path unchanged.
- **After assignment**, `scripts/check-release-runner-trust.sh` re-asserts the
  contract on the machine itself: push event, canonical repository, routing
  explicitly enabled, ref in `{refs/heads/main, refs/tags/v*}`, Cargo and
  sccache directories isolated under `/var/lib/cassy-actions/`, the runner's
  private sccache port, and not running as root. A future edit to a workflow
  expression therefore cannot silently hand the box untrusted input.

**The publishing job is deliberately never routed to the box.** It holds the
only `contents: write` token in the release and stays on `ubuntu-latest`.

Enabling and disabling the routing is a repository-variable change:

```
gh variable set CASSY_RELEASE_SELF_HOSTED --body enabled    # opt in
gh variable set CASSY_RELEASE_SELF_HOSTED --body disabled   # before box maintenance
```

Disable it before any maintenance on the runner. Every lane then runs hosted,
exactly as it did before this work.

One listener serves the `cas-ci-32core` label, so a routed release lane runs
one job at a time and can queue behind merge-queue validation. Two consequences
worth knowing before enabling it:

- In the fallback path, `verify` and `build` are designed to run in parallel;
  routed to the box they serialise. Warm, that is still a few minutes against
  13-26 minutes cold, but it is not free parallelism.
- A release cut during heavy merge-queue traffic waits for the box. The hosted
  fail-safe is one variable away if that ever matters more than the warm cache.

## Cutting a release on the fast path

1. Merge the version-bump PR to `main`. `Release Prebuild` starts on that push.
2. Wait for its `Release prebuild summary` job. It states plainly whether a tag
   will adopt the artifacts or fall back to building them inline.
3. Push the tag (`scripts/release.sh --publish-tag`). The script reports the
   same thing one last time before it pushes, so a tag is never pushed blind
   into a 15-minute path by accident.
4. Run both receipts once the release is published.

Pushing the tag before the prebuild finishes is safe — it just costs the old
tag-time build. Nothing breaks; the release is simply slow.

## Measuring a real release

Two receipts, both gates rather than reports:

```
scripts/release-published-receipt.sh vX.Y.Z    # what shipped: digests, both assets
scripts/release-latency-receipt.sh   vX.Y.Z    # how fast: tag push -> published
```

`release-latency-receipt.sh` measures from the **first** Release workflow run
created for the tag, so a rerun can never flatter the number, and it exits
non-zero when the latency exceeds its budget (600s by default). A release that
cannot produce a passing latency receipt has not demonstrated fast publication.

## Contract

`scripts/test-ci-test-tiers.sh` pins the properties above — trigger surface,
the pending-release gate, audit and codesign parity between the prebuild and
fallback paths, fail-safe lookup, fail-closed publication, both routing keys,
and the publish job's hosted-only placement. `make -C cas-cli test-ci-tiers`
runs it along with each new guard's own self-test, on the required Fast
Validation lane.
