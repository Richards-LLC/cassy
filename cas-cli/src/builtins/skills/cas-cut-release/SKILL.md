---
name: cas-cut-release
description: Use when cutting a Cassy runtime release from an assembled epic.
metadata:
  managed_by: cas
---

# One cut owner

The train uses its existing PR. Lane checks catch local defects; the queue owns
full proof and archives, publication owns catch-up. Add no train suite receipt,
candidate build, local assembly run or second cut owner.

1. Finish review and batch QA; keep entries in `changelog.d/` and regenerate the codemap.
2. Write the notes at the train's `release-notes.md` artifact path, following
   `docs/release-notes/RUBRIC.md` and `docs/RELEASE_SLACK_RUBRIC.md`; Cassy never authors notes.
   Declare epics, exclusions and no-code proofs in `CAS_RELEASE_INPUTS`;
   bind review/QA receipts to the plan's included task IDs.
3. Read `cas release plan --train <train-id> --notes <draft>`. The default is
   read-only: it lists blockers and fixes before build, using the train tip's linter,
   membership, proofs, review, eligibility, version ownership and queue configuration.
4. Cassy runs the cut when the published train tip is ready. Manual
   `scripts/release.sh --notes <draft>` enters the same lease and detached
   pinned checkout, even when another worktree holds the branch. Competing cuts
   refuse with `CUT_LEASE_BUSY`; `--hotfix` selects a patch, `--next-version` is read-only.
5. A failed cut records its exact `release-cut-blocker:<tip>:<code>` label.
   Only a task carrying that label and `release-repair` can merge through the
   standing freeze; merging consumes that grant. Corrected notes re-arm the cut.
   Never start a parallel release or lift/reapply the freeze for repair.
   Commander lets the operator pause auto-cut or explicitly lift the freeze.
6. The owner prepares versions, changelog and Commander once, persists cut
   tracking, pushes, reuses the PR and checks admission once. Durable SHA/tag
   reminders carry landing and publication; add no CI/PR polling loop.
   A queue ejection gets one repair batch and re-enqueue; a second is an operator P0.
7. Publication adopts and audits the queue archives, launch-smokes both platforms,
   publishes and verifies installation. Follow the post-release notice, using
   `cas-release-notes` for Violet and `cas-release-report` for measured delivery-to-install
   timing. Repair publication in its existing workflow; never replace a release.
   Inspect stranded deliveries before epic close.
