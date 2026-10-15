# Fast release publication

How a version change becomes a published, installed release with no agent in
between, and why the pieces are shaped the way they are.

Tracked by GH #449 (fast release publication), GH #603 (the prebuild race),
cas-3b7c0, cas-28eb and cas-f95b (the release path rebuilt around the merge
queue).

## The measured problem

Until cas-f95b a release took 3 to 5 hours from code-done to installed
(3.53.0: 289 minutes). It passed an epic PR, then a local 13-stage train, a
41-row local gate, a separate release PR with its own merge-queue cycle, a
local tag push and an agent-driven host update. The release binary was built
up to four times and three agent hand-offs sat on the green path. The design
target is 30 minutes from the epic PR opening to `main` and 20 minutes from
`main` to installed.

Three facts decide the design:

1. The merge queue already proves the exact tree that lands on `main`. Any
   later check of that tree re-checks it.
2. macOS ARM64 is the slowest build (609-1,108 s measured with LTO off), and
   there is no Mac in the fleet. It has to overlap other work, not follow it.
3. Every hand-off to an agent cost minutes to tens of minutes.

So the version bump rides in the epic PR, the release builds run once on the
merge-queue tree, and landing on `main` publishes.

## The architecture

```text
scripts/release.sh on the epic branch
  bump crates + Cargo.lock, CHANGELOG [Unreleased] -> [x.y.z], draft,
  commit, push, open the PR, enable auto-merge, watch (re-bump on a race)
        |
        v
merge queue (merge_group)
  ci.yml                 the required checks
  release-prebuild.yml   pending-release: did this entry change the version?
                         build        Linux x86_64 release-fast   (hosted)
                         build-macos  macOS ARM64, LTO off, signed (hosted)
        |  the queue lands its tested merge commit as main
        v
push to main touching cas-cli/Cargo.toml
        |
        v
release.yml
  detect        version changed against the previous main? tag absent?
  lookup        the queue run's archives, by commit or tree id (waits <= 30 min)
  build         FALLBACK ONLY: calls release-prebuild.yml when nothing to adopt
  audit-linux   ISA audit + operator launch smoke on the packaged archive
  audit-macos   codesign verify + operator launch smoke on the packaged archive
  release       annotated tag, GitHub release, install-path proof dispatch
        |
        v
host timer (scripts/install-on-publish.sh, every 60 s)
  cas update --yes --version x.y.z, then the release timer in the notes
```

### Why the version rides in the epic PR

The branch ruleset requires the merge queue, so a version bump cannot reach
`main` on its own without a second PR and a second queue cycle. Committing it
to the epic branch before the PR opens lands it with the code, in the same
queue run, and that run then builds the tree that ships. The cost is that a
merge must be declared a release when its PR opens; every factory epic already
is one.

Two epics racing for the same version: `scripts/release.sh` watches its PR.
When `main` moves to a new version (or the PR conflicts), it merges `main`,
resolves the release files itself (version lines, `Cargo.lock`, CHANGELOG and
a same-path draft), takes the next version, pushes and re-enables auto-merge.
A conflict in any other file stops it with exit 4, and an ejected PR stops it
with exit 3: those need a repair, not a re-bump.

### Why the builds run on merge_group

The queue builds one entry at a time (`max_entries_to_build: 1`) and lands the
merge commit it tested as `main`, so the queue run's head commit is the
published commit. Starting both builds at enqueue overlaps the macOS build
with the 15-minute queue run. They are not required checks: a failed build
never blocks a merge, and the release workflow then builds the same bytes
itself.

`scripts/detect-pending-release.sh --base-version <v>` gates them: a queue
entry is a release when its version differs from the base's, every release
crate agrees, `CHANGELOG.md` has the heading and no `v<version>` tag exists.
Ordinary entries cost one ~20 s job. On `main`, a changed version that cannot
publish (reused or lowered) fails the release workflow loudly.

### Why adoption is fail-safe and publication is fail-closed

`scripts/find-release-prebuild.sh` matches only a **successful** Release
Prebuild run whose head commit is the commit being published or has exactly
its tree, with both artifacts present and unexpired. The queue may land before
its non-required builds finish, so the lookup waits up to 30 minutes for a
matching run in progress. Every degraded input (no run, a partial run, expired
artifacts, an API outage) reports `found=false` with a loud warning, and the
workflow builds the archives by calling `release-prebuild.yml`. The prebuild
is an accelerator; it can never be the reason a release cannot ship.

Publication is the opposite. The `release` job runs only when both audit jobs
passed on the archives about to be uploaded, from exactly one supply path.
`scripts/release-publish.sh` refuses an existing release object, refuses a tag
that peels to another commit, and requires both platform archives.

### Why the audits run on the packaged assets

The ISA audit (3.47.0 shipped AVX-512 instructions, cas-2a16) and the operator
launch smoke (3.51.0 could not be launched, cas-e719) earn their place only on
the bytes that ship. They run on the downloaded archives: Linux on
`ubuntu-latest`, macOS on `macos-26`, where `codesign --verify` also checks the
signature of the exact executable being published. macOS is compile-checked
and launch-tested; the suite runs on Linux.

### Why install on publish is a timer

A webhook cannot reach the supervisor host, and a watcher inside the `cas`
daemon would be Rust that only a release could ship, stop with the daemon, and
replace the binary it runs. A systemd user timer survives reboots, costs one
authenticated API call a minute, and its journal is the install log. Set it up
once per host:

```bash
scripts/install-on-publish.sh --install
journalctl --user -u cassy-install-on-publish
```

## One timer

`release.yml` ends the release notes with the PR number, its opening time and
the publish time. The first host to install completes the line with the
install time: **PR opened to installed**, the one latency figure per release.
It replaces the tag-to-published latency receipt, the stage budgets and the
request/cut metrics, which measured narrower spans.

## Self-hosted routing

None. Every release lane runs on GitHub-hosted runners; the
`CASSY_RELEASE_SELF_HOSTED` variable and the release routing jobs are gone, and
the publishing job is the only one holding a `contents: write` token.

## Dry runs and recovery

```bash
gh workflow run release-prebuild.yml --ref <branch>   # build that tree
gh workflow run release.yml --ref <branch>            # dry_run=true by default
```

The dry run adopts the dispatched build, runs both audits, publishes a
`dryrun-v<version>-<run>` prerelease with the assets, and deletes it. GitHub's
latest-release endpoint, which `cas update` and the installer read, never
returns a prerelease.

If the release workflow fails after the version lands on `main`, fix the cause
on `main` and run `gh workflow run release.yml -f dry_run=false`. It never
replaces an existing release; see
[Recovering a failed or partial release](../RELEASE_SLACK_RUBRIC.md#recovering-a-failed-or-partial-release).

After publication, `scripts/release-published-receipt.sh vX.Y.Z` derives the
announcement digests from the published bytes, and the advisory
[Install path proof](install-path-proof.md) checks a clean install on macOS ARM
and Linux.
