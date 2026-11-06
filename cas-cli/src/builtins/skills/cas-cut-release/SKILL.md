---
name: cas-cut-release
description: Use when cutting a Cassy runtime release from an assembled epic.
metadata:
  managed_by: cas
---

# One-command release

The release rides in the canonical open release train PR (the oldest open Cassy epic labelled `release-train`). One command prepares it, the merge queue
proves the exact tree that ships, and landing on `main` publishes it. There is
no release PR, no tag push and no local build. Platforms: Linux runs the suite;
macOS: compile-checked and launch-tested; suite runs on Linux.

Release prep checklist: compare the locked `@playwright/test` version in
`hub-web/package-lock.json` with `npm view @playwright/test version` (run from
the checkout). Flag **more than one minor behind** in the prep notes; a major
gap also needs review. Upgrade and refresh Playwright guidance before the cut,
or record the supervisor's explicit deferral. After an upgrade, regenerate
agents in scratch with `npx playwright init-agents --loop=<harness>` and review
them against `cas-playwright-debug`'s healer guard before adopting any changes.

1. Start from the epic branch only after closeout is complete: one full review,
   one batch fix, fix-diff re-review, one QA round (cas-supervisor Phase 4). CHANGELOG `[Unreleased]` and the
   `changelog.d/` fragments hold the release entries. `scripts/release.sh --next-version` prints the version and
   the draft path. The supervisor writes release notes; workers never draft them, and release prep only merges, writes the changelog and regenerates the codemap.
   Write the Slack draft there per
   `docs/release-notes/RUBRIC.md` and `docs/RELEASE_SLACK_RUBRIC.md`, or keep
   it anywhere and pass `--notes <file>` (`{{VERSION}}` becomes the version).
   Commit the regenerated `.claude/CODEMAP.md` and the previous release's
   report there too; no docs PR follows. Fixture versions use `9.99.x`.
   The train's ready-to-cut notice prompts this preparation after review.
2. Check out the train lane, then run `scripts/release.sh` with an installed
   Cassy that has `release context`, `release track` and `release installed`
   (`CAS_BIN` selects a candidate for the first cut). It validates the canonical
   train and integration branch, merges the base when behind, bumps versions,
   promotes the changelog, builds Commander, commits and pushes. It reuses the
   train PR, requests auto-merge only when needed and checks admission once.
   An auto-merge request awaiting required checks is reported as not admitted;
   a red required-check snapshot exits nonzero without a CI polling loop.
   It returns after admission; `--no-watch` behaves the same way. No PR-CI watch
   or PR polling stage runs: the queue proves the prepared tree once.
   A hotfix is a branch from `main` released with `scripts/release.sh --hotfix`
   (next patch), through the same queue. No train or a different train lane
   keeps the caller's `--base` flow (default `main`); tracking is advisory and
   cannot prevent the cut. Missing/unreachable owners use the shared resolver's
   peer fallback, or print one clear tracking-skipped message.
3. The command automatically arms cross-session `branch_contained_in` for the
   release SHA and integration branch, plus `tag_exists` for the version, even
   with `--no-watch`. A retry reuses these durable occurrences; a version
   re-bump cancels the previous version's landing and tag watches. Do not arm a
   duplicate CI/PR watcher. A missing admission is exit 3; resolve the named
   queue failure in one batch and rerun. If another version landed first,
   rerunning merges it and takes the next version. Exit 4 names a code conflict:
   resolve it on the train, push and rerun. When train children are the cause,
   `cas factory train drop <task>... --reason "<failing job>"` is the repair cycle:
   it reverts them all, returns each to its worker and re-enqueues the train once.
   A second ejection is an operator P0.
4. The merge queue runs required checks and starts Release Prebuild for the
   exact release tree. Commander journey shards run only for web product inputs.
   Landing wakes the supervisor through the persisted SHA reminder; publication
   wakes through the tag reminder. The command never closes the train: core
   rolls it over when later work is created. For a distinct recovery CI run,
   use `remind_event=ci_completed`; workers never poll CI.
5. Landing on `main` starts `release.yml` with no agent: it adopts the queue
   run's archives by tree id, runs the ISA audit and the operator launch smoke
   on the packaged Linux and macOS assets, creates the annotated tag and the
   GitHub release, and dispatches the install-path proof. The host installs
   through the `cassy-install-on-publish` systemd user timer on Linux or the
   `dev.cassy.install-on-publish` LaunchAgent on macOS (set it up once per host
   with `scripts/install-on-publish.sh --install`). While the user session is
   active it checks every minute; after a successful update and version check,
   the notes record the first host's actual install time and PR opened to
   installed timer. A verified publication plus matching installed binary sends
   one owner-addressed post-release action notice. A failed notice retries on
   the next timer tick without reinstalling. A generic binary-skew notice also
   wakes once; it is not publication evidence. Confirm with `cas --version`.
6. Off the clock: fill the draft checksums with
   `release-published-receipt.sh --write-draft`, post through the Violet hub
   (`cas-release-notes`; never a personal Slack route), and publish the report
   (`cas-release-report`). The publication receipt prints first child commit →
   GitHub mergedAt against 30 minutes, mergedAt → publishedAt against 20 minutes,
   and associated queue CI attempts. Missing evidence is unavailable; overruns
   report the measured time without adding a release gate.
7. Recovery: if `release.yml` fails after landing, fix the cause on `main`,
   then `gh workflow run release.yml -f dry_run=false`; it never replaces an
   existing release. Whole-path dry run for any branch:
   `gh workflow run release-prebuild.yml --ref <branch>`, then
   `gh workflow run release.yml --ref <branch>` (a deleted throwaway
   prerelease).
8. Close the epic only after the merge and a stranded-branch inspection;
   `stranded_branch_override` requires supervisor proof.
