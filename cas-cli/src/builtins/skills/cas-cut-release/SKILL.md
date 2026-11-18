---
name: cas-cut-release
description: Use when cutting a Cassy runtime release from an assembled epic.
metadata:
  managed_by: cas
---

# One-command release

The release rides in the canonical open release train PR (the oldest open Cassy epic labelled `release-train`). One command prepares it, the merge queue
proves the exact tree that ships, and landing on `main` publishes it. There is
no release PR, no tag push and no local build. Linux runs the suite; macOS is
compile-checked and launch-tested.

Release prep checklist: compare the locked Playwright version with the latest stable and verify browser binaries plus the chosen Node runtime. Any gap needs an upgrade decision before authoring/cut; the Node 26 tracing incident is a compatibility warning, not a universal engine constraint. Generate `npx playwright init-agents --loop=<harness>` and `npx playwright init-skills --loop=<harness>` in scratch, then review the official diff against [cas-playwright-tests](../cas-playwright-tests/SKILL.md), including its healer guard.

1. Start from the epic branch only after closeout is complete: one full review,
   one batch fix, fix-diff re-review, one QA round (cas-supervisor Phase 4).
   Before `train_freeze`, dispatch `gh workflow run ci.yml --ref <train branch>`
   (no train push CI) and read green for the exact tip SHA. On that tip run
   `cargo tree -i aws-lc-rs --target x86_64-unknown-linux-gnu` (empty graph), capped
   `cargo nextest run -p cas --lib -E 'test(gates::registry_tests)'`, and
   `python3 scripts/check-test-shape.py`.
   CHANGELOG `[Unreleased]` and the
   `changelog.d/` fragments hold the release entries. `scripts/release.sh --next-version` prints the version and
   the draft path. The supervisor writes release notes; workers never draft them, and release prep only merges, writes the changelog and regenerates the codemap.
   Write the Slack draft there per
   `docs/release-notes/RUBRIC.md` and `docs/RELEASE_SLACK_RUBRIC.md`, or keep
   it anywhere and pass `--notes <file>` (`{{VERSION}}` becomes the version).
   Draft, commit and lint it with `python3 scripts/release-train-announce.py --lint <draft>`
   while jobs run, before the suite goes green. While frozen, inspect jobs between short
   reminders until terminal; never wait blind on a long reminder or block on a watch.
   Declare the train's constituent epics in the draft's `CAS_RELEASE_INPUTS`. The cut checks each member's existing merge approval alongside the green frozen-train receipt; missing approvals are named before push. Only the release's included tasks gate the zero-would-close check: held legacy members outside them are listed as nonblocking and never refuse the cut. No separate release-review ledger or recorder is needed for code deliveries. Deliberate light, sampled and Tier 0 exemptions use green lane CI plus the green train run; a missing lane receipt is named separately.
   Commit the regenerated `.claude/CODEMAP.md` (`/codemap`) and the previous
   release's report there too; no docs PR follows. The command refuses while a
   file added or removed since the map's last commit is unmapped. Deliveries never edit
   CODEMAP or PRODUCT_OVERVIEW: the command folds the `docs/qa/bug-families/`
   and `docs/product-overview.d/` fragments into the registry and overview
   (`scripts/doc-fragments.py compile`); refresh the overview with
   project-overview when its fragments call for it. Fixture versions use `9.99.x`.
   The train's ready-to-cut notice prompts this preparation after review.
   Auto-cut: when the exact train tip is green with changelog fragments,
   Cassy freezes the train and runs `scripts/release.sh --notes` itself with
   the draft at the `release-notes.md` path its wake names. Write that draft
   and commit a regenerated CODEMAP;
   Cassy never writes or posts notes. The operator pauses auto-cut in Commander.
   Lift a freeze yourself with `factory action=train_freeze id=<train> decision=lift reason="<why>"`
   only when a Cassy gate refuses the cut on a green frozen tip or the operator asked for new work
   in the release. The lift is audited and sent to the operator, and auto-cut leaves that tip alone.
   A freeze the operator applied in Commander is theirs to lift unless they granted it.
2. Read the train CI receipt and complete log artifact; fix every red first.
   The existing CI suite at the newest train tip replaces a local pre-cut full
   run. A known red holds `train_merge`; any override is a visible decision for
   that observed run and does not authorize a red cut. A queued train run with
   zero jobs is replaced once at the same head after ninety seconds; do not
   re-dispatch it by hand. The command checks a
   green receipt for the starting tip before version preparation, reusing it
   on an unchanged prepared retry. The queue proves the version commit.
   Check out the train lane, then run `scripts/release.sh` with an installed
   Cassy that has `release context`, `release track` and `release installed`
   (`CAS_BIN` selects a candidate for the first cut). It validates the canonical
   train and integration branch, merges the base when behind, bumps versions,
   promotes the changelog, builds Commander, commits, marks the train cut and pushes. It reuses the
   train PR, requests auto-merge only when needed and checks admission once,
   then returns (`--no-watch` is an alias). Do not watch PR CI: the queue
   proves the prepared tree once.
   A hotfix is a branch from `main` released with `scripts/release.sh --hotfix`
   (next patch), through the same queue. No train or a different train lane
   keeps the caller's `--base` flow (default `main`). A matched train must persist
   its cut marker before push/enqueue; failure stops with a retry command. New
   tasks join a fresh train, and unfinished deliveries need retargeting; merges
   into the cut lane stay refused after landing too.
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
   A second ejection is an operator P0. A repair targeting the frozen train
   merges once its lane's no-build rows are green, without waiting for its
   duplicate Rust impact jobs: the frozen tip's full set supplies Rust proof,
   and full-set jobs whose own inputs the repair did not change reuse their
   receipts.
4. The merge queue runs required checks and starts Release Prebuild for the
   exact release tree. Commander journey shards run only for web product inputs.
   Landing wakes the supervisor through the persisted SHA reminder; publication
   wakes through the tag reminder. The command never closes the train: core
   rolls it over when later work is created. For a distinct recovery CI run,
   use `remind_event=ci_completed`; workers never poll CI.
5. Landing on `main` starts `release.yml` with no agent: it adopts the queue
   run's archives by tree id, checks the Linux executable against the digest
   the prebuild's one ISA audit recorded, runs the launch smoke on the Linux and
   macOS assets and the macOS signature check, creates the annotated tag and the
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
