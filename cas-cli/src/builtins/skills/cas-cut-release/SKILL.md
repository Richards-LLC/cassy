---
name: cas-cut-release
description: Use when cutting a Cassy runtime release from an assembled epic.
metadata:
  managed_by: cas
---

# One-command release train

This is the supervisor's only release procedure. The train fails closed at the
first named blocker and leaves its receipt in a per-run directory keyed by
version and worktree, never a version-keyed path.

1. Read the `manual:*` entries in `references/failure-log.md`: they are the
   hazards no gate row catches. Every other entry names a `release-gate.sh` row
   that already enforces it, so on a gate failure grep the log for that row id
   instead of reading it all. Learn an absent failure with
   `scripts/release-gate.sh --learn "<symptom>" "<cause>" "<check-id>"`; it
   appends to the failure log and regenerates the builtin reference ledger,
   so commit them with the new check. Store the same text with
   `memory action=remember entry_type=learning tags=release`.
2. Before merging a release-bound lane, run `scripts/release-train.sh <version>
   <epic-worktree> --check-lane <branch>`. Require the exact branch-tip,
   push-triggered `Scoped Validation` job to be green; missing, skipped, red,
   pending, or malformed evidence refuses the merge. Supervisors monitor CI;
   workers never poll CI.
3. Start a clean detached or `release/` worktree from `origin/main`. Copy the
   reviewed dated CHANGELOG section and draft from the epic into this worktree
   and commit them; preflight reads the release checkout. Confirm
   the preflight prerequisites: no competing release (open PRs, the
   merge-queue GraphQL query, and remote tags); a writable `scratch-base` with
   space for twice the last archive; readable `CAS_RELEASE_ENV_FILE` (names
   only); resolvable Zig; a complete host `toolchain` (cargo, cargo-nextest,
   cargo-zigbuild, jq, python3, GNU objdump, an x86_64 Linux C compiler or
   Zig, the `x86_64-unknown-linux-gnu` Rust target); a `report-renderer`
   whose headless Chromium launches (its blocker names the pinned
   `playwright install chromium-headless-shell` command); and a passing
   integration receipt. With no configured scratch base, a checkout on another
   filesystem than the platform default gets `cas-release-gate` beside it. A macOS host needs only
   Homebrew `jq binutils`, those Cargo tools and that target: the train
   supplies the `stat`, `sha256sum`, `setsid` and Cargo-PATH fallbacks itself,
   and defaults its scratch base to `/Users/Shared/cas-release-gate` there
   (`/tmp` and `/var/tmp` are Cassy disposable roots on macOS). Pin the cut date in `run.env` from `started_at` and
   use that date for every draft path, including after midnight. Run the same
   announcement lint during preflight that `announce` will run; that lint
   (`scripts/release-train-announce.py`) is the authority for User-thread
   wording. Real-project fixtures use
   `cas::test_paths::runtime_fixture_parent()`, and fixture versions use
   `9.99.x`. An intentional doctor row change is a reviewed snapshot update.
   When `hub-web/dist` changed since the last tag, commit a passing journey
   evaluation of the assembled bundle before the cut
   (`docs/qa/journey-evaluation.md`: `scripts/journey-eval.sh`, then a
   taste-lane evaluator's report). `prep` stops with `journey-evaluation`
   without one.
4. Run one command:
   `scripts/release-train.sh <version> <release-worktree> --cut`.
   It runs `preflight, assemble, prep, ledger, gate, pr-body, pipeline,
   publish, post-publication, announce, report, receipts, host-update` in that
   order. The ledger is the last prep step. `assemble` invokes stale-base heal
   when required. The gate is a detached process group; inspect its recorded PID
   with `kill -0`, never by process-name search. Every stage writes a SHA
   receipt, and the train preserves pipeline/publisher hand-off epochs.
   `post-publication` fills the draft with
   `release-published-receipt.sh --write-draft`. After the four announcement
   writes succeed, `announce` appends the `## POSTED`
   block to the draft. `receipts` commits that draft and the release report
   artifacts on the `release/` branch, writes the commit and branch to the
   run-dir `receipts.commit` receipt, and never opens a docs-only PR. Before
   the next release, `preflight` warns about an unmerged prior receipt commit.
   These docs-only release commits stay for `prep` to carry forward after assembly; its report files are
   outside the initial metadata rebase allowlist.
   `prep` refreshes `Cargo.lock` with `cargo update --workspace --offline`.
   `assemble` rebases CHANGELOG, release notes and
   `docs/qa/journey-evaluations/*.md` from `main` onto the tested integration
   tip; other paths stop with preservation and fresh-worktree guidance.
   The pipeline waits for `MERGEABLE` plus the required
   status-check rollup before enqueueing, defaulting to 60 attempts at 5 seconds.
5. If the command stops, answer only the named blocker, then rerun the exact
   printed `--cut --resume` command. Use `scripts/release-train.sh <version>
   <release-worktree> --status` for bounded, read-only state. A targeted
   `--gate --only <row,row>` is diagnostic and never authorizes pipeline.
6. Done when the receipt checklist holds; never call the release published before. It needs the
   `receipts release-learning` gate: every blocker in `blockers.log` and every
   supervisor intervention maps to a learned executable gate row (`learn=<row>`)
   or a still-open task (`task=cas-<id>`). On refusal, supply the diagnosed cause
   and executable row to its printed `release-gate.sh --learn` command; its
   `--run-dir` and `--evidence file:line` options annotate the evidence line.
   Preflight warns about release tooling on unreleased epic or parked branches;
   bring those fixes onto main before the next cut. The checklist also needs the
   full exact-SHA gate (the full suite on the assembled tree), queue/pipeline landed SHA, `annotated tag peels`,
   `release.tag-complete.epoch`, `release-published.receipt`, the matching
   workflow and asset proofs, `four Slack POSTED` entries through the
   `Violet` hub (never a personal Slack route), report HTML/PDF evidence,
   `cas --version`, and host JSON with `refresh_binary_version`.
   The report's green-to-published latency is named only from verified receipts.
7. Add one epic note per gate run with tip, failed rows, cause class, and
   blocking step. Close only after merge and stranded-branch inspection;
   `stranded_branch_override` requires supervisor proof. Preserve the draft and
   partial receipts on an uncertain post; never retry an uncertain write.
