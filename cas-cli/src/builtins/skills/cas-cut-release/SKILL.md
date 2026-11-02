---
name: cas-cut-release
description: Use when cutting a Cassy runtime release from an assembled epic.
metadata:
  managed_by: cas
---

# One-command release

The release rides in the epic PR. One command prepares it, the merge queue
proves the exact tree that ships, and landing on `main` publishes it. There is
no release PR, no tag push and no local build. Platforms: Linux runs the suite;
macOS: compile-checked and launch-tested; suite runs on Linux.

1. Start from the epic branch only after closeout is complete: one full review,
   one batch fix, fix-diff re-review, one QA round (cas-supervisor Phase 4). CHANGELOG `[Unreleased]` holds the
   release entries. `scripts/release.sh --next-version` prints the version and
   the draft path; write the Slack draft there per
   `docs/release-notes/RUBRIC.md` and `docs/RELEASE_SLACK_RUBRIC.md`, or keep
   it anywhere and pass `--notes <file>` (`{{VERSION}}` becomes the version).
   Commit the regenerated `.claude/CODEMAP.md` and the previous release's
   report there too; no docs PR follows. Fixture versions use `9.99.x`.
2. Run `scripts/release.sh` in the background with a log; it stays up until
   the PR merges. It merges `main` in when the branch is behind, bumps the
   release crates and `Cargo.lock`, promotes `[Unreleased]` to the dated
   section, names the draft, commits `release: vX.Y.Z`, pushes, opens the PR
   and enables auto-merge. `--no-watch` stops after auto-merge. A hotfix is a
   branch from `main` released with `scripts/release.sh --hotfix` (next
   patch), through the same queue.
3. If another release lands first, the command merges it, takes the next
   version and re-enables auto-merge. Exit 4 is a code conflict with
   `main`: resolve it on the branch, push, rerun. Exit 3 means the queue
   ejected the PR: one repair cycle (run every remaining failing check in one
   batch, fix, push), then rerun the command. A second ejection goes to the
   operator as a P0.
4. The merge queue runs the required checks; Commander journey shards run
   only for `hub-web/` product inputs. The same run starts the optional
   Release Prebuild (Linux `release-fast`, macOS ARM64 without LTO). Wait with
   `remind_event=ci_completed` and, for the landing,
   `remind_event=branch_contained_in`, both `cross_session=true`; supervisors
   monitor CI and workers never poll CI.
5. Landing on `main` starts `release.yml` with no agent: it adopts the queue
   run's archives by tree id, runs the ISA audit and the operator launch smoke
   on the packaged Linux and macOS assets, creates the annotated tag and the
   GitHub release, and dispatches the install-path proof. The host installs
   through the `cassy-install-on-publish` systemd user timer on Linux or the
   `dev.cassy.install-on-publish` LaunchAgent on macOS (set it up once per host
   with `scripts/install-on-publish.sh --install`). While the user session is
   active it checks every minute; after a successful update and version check,
   the notes record the first host's actual install time and PR opened to
   installed timer. Confirm the local version with `cas --version`.
6. Off the clock: fill the draft checksums with
   `release-published-receipt.sh --write-draft`, post through the Violet hub
   (`cas-release-notes`; never a personal Slack route), and publish the report
   (`cas-release-report`).
7. Recovery: if `release.yml` fails after landing, fix the cause on `main`,
   then `gh workflow run release.yml -f dry_run=false`; it never replaces an
   existing release. Whole-path dry run for any branch:
   `gh workflow run release-prebuild.yml --ref <branch>`, then
   `gh workflow run release.yml --ref <branch>` (a deleted throwaway
   prerelease).
8. Close the epic only after the merge and a stranded-branch inspection;
   `stranded_branch_override` requires supervisor proof.
