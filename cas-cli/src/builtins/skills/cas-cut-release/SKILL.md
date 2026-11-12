---
name: cas-cut-release
description: Use when cutting a Cassy runtime release from an assembled epic.
metadata:
  managed_by: cas
---

# One cut owner

The canonical train carries the release through its existing PR. Lane impact
checks catch local defects; the merge queue is the only full proof and builds
the archives. Publication owns train catch-up. Do not add a train full-suite
receipt, candidate build, local assembly run or second cut owner.

1. Finish the supervisor's review and batch QA. Keep release entries in
   `changelog.d/`, regenerate the codemap and include the previous release's
   report. Fixture versions use `9.99.x`. Compare the locked Playwright version
   with the current release; upgrade a gap over one minor or record the
   supervisor's deferral before cutting.
2. Write the notes at the train's `release-notes.md` artifact path, following
   `docs/release-notes/RUBRIC.md` and `docs/RELEASE_SLACK_RUBRIC.md`. Cassy never
   writes or posts notes. Declare constituent epics, exclusions and no-code
   proof references in the notes' `CAS_RELEASE_INPUTS`; use the plan's included
   task IDs for review/QA receipts.
3. Read `cas release plan --train <train-id> --notes <draft>`. The default is
   read-only: one result lists all known blockers and fixes before any build.
   It uses the train tip's linter, input membership, proof bindings, review,
   member eligibility, version ownership and queue configuration.
4. Cassy runs the cut when the published train tip is ready. Manual
   `scripts/release.sh --notes <draft>` enters the same lease and detached
   pinned checkout. A supervisor holding the branch cannot block the checkout;
   a competing cut refuses with `CUT_LEASE_BUSY`. `--hotfix` selects a patch;
   `--next-version` is read-only. No operator freeze lift is needed for repair.
5. A failed cut records its exact `release-cut-blocker:<tip>:<code>` label.
   Only a task carrying that label and `release-repair` can merge through the
   standing freeze. Its merge consumes the tip-bound grant automatically.
   Corrected notes re-arm the existing cut; do not invoke a parallel manual
   release or lift/reapply the freeze. The operator can pause auto-cut in
   Commander or explicitly lift the freeze.
6. The owner prepares versions, changelog and Commander once, persists cut
   tracking, pushes the prepared tree, reuses the train PR and checks enqueue
   admission once. Durable SHA/tag reminders carry landing and publication;
   never add a CI/PR polling loop. A queue ejection gets one repair batch and
   one re-enqueue. A second ejection is an operator P0.
7. Publication adopts the queue archives, audits and launch-smokes both
   platforms, tags and publishes, then verifies installation. The release
   report uses measured last delivery merge to installed time; missing evidence
   remains unavailable. The post-release notice owns the next action.
8. Off the release clock, fill checksums with
   `release-published-receipt.sh --write-draft`, post through Violet using
   `cas-release-notes` and publish the `cas-release-report`. Never use a personal
   Slack route. A failed publication is repaired through the existing workflow;
   it never replaces an existing release. Close the epic after merged delivery
   and stranded-branch inspection, with proof for any override.
