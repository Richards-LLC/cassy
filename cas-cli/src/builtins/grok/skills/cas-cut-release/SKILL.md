---
name: cas-cut-release
description: Use when cutting a Cassy runtime release from an assembled epic.
managed_by: cas
---

# One-command release train

This is the supervisor's only release procedure. The train fails closed at the
first named blocker and leaves its receipt in a per-run directory keyed by
version and worktree, never a version-keyed path.

1. Read `references/failure-log.md in full`. Learn an absent failure with
   `scripts/release-gate.sh --learn "<symptom>" "<cause>" "<check-id>"`, mirror
   the entry, and store the same text with `cas__memory action=remember
   entry_type=learning tags=release`. The learn command regenerates the builtin
   reference ledger; regenerate it again after the final merge as the last prep
   step.
2. Before merging a release-bound lane, run `scripts/release-train.sh <version>
   <epic-worktree> --check-lane <branch>`. Require the exact branch-tip,
   push-triggered `Scoped Validation` job to be green; missing, skipped, red,
   pending, or malformed evidence refuses the merge. Supervisors monitor CI;
   workers never poll CI.
3. Start a clean detached or `release/` worktree from `origin/main`. Confirm
   the preflight prerequisites: no competing release (open PRs, the
   merge-queue GraphQL query, and remote tags); a writable `scratch-base` with
   space for twice the last archive; readable `CAS_RELEASE_ENV_FILE` (names
   only); resolvable Zig; a dated CHANGELOG heading and draft; and a passing
   integration receipt. Real-project fixtures use
   `cas::test_paths::runtime_fixture_parent()`, and fixture versions use
   `9.99.x`. An intentional doctor row change is a reviewed snapshot update.
4. Run one command:
   `scripts/release-train.sh <version> <release-worktree> --cut`.
   It runs `preflight, assemble, prep, ledger, gate, pr-body, pipeline,
   publish, post-publication, announce, report, receipts, host-update` in that
   order. The ledger is the last prep step. `assemble` invokes stale-base heal
   when required. The gate is a detached process group; inspect its recorded PID
   with `kill -0`, never by process-name search. Every stage writes a SHA
   receipt, and the train preserves pipeline/publisher hand-off epochs. After
   the four announcement writes succeed, `announce` appends the `## POSTED`
   block to the draft. `receipts` commits that draft and the release report
   artifacts on the `release/` branch, writes the commit and branch to the
   run-dir `receipts.commit` receipt, and never opens a docs-only PR. Before
   the next release, `preflight` warns about an unmerged prior receipt commit
   and `prep` carries it forward with a merge before preparing the new draft.
5. If the command stops, answer only the named blocker, then rerun the exact
   printed `--cut --resume` command. Use `scripts/release-train.sh <version>
   <release-worktree> --status` for bounded, read-only state. A targeted
   `--gate --only <row,row>` is diagnostic and never authorizes pipeline.
6. Require the receipt checklist before calling the release published: the
   full exact-SHA gate, queue/pipeline landed SHA, `annotated tag peels`,
   `release.tag-complete.epoch`, `release-published.receipt`, the matching
   workflow and asset proofs, `four Slack POSTED` entries through the
   `MechaCassy` hub (never a personal Slack route), report HTML/PDF evidence,
   `cas --version`, and host JSON with `refresh_binary_version`.
   The report's green-to-published latency is named only from verified receipts.
7. Add one epic note per gate run with tip, failed rows, cause class, and
   blocking step. Close only after merge and stranded-branch inspection;
   `stranded_branch_override` requires supervisor proof. Preserve the draft and
   partial receipts on an uncertain post; never retry an uncertain write.
