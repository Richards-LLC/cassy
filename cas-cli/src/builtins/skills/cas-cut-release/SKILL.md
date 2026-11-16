---
name: cas-cut-release
description: Use when cutting a Cassy runtime release from an assembled epic.
metadata:
  managed_by: cas
---

# One cut owner

The train uses its existing PR. Lane checks catch local defects; the frozen
tip's one train suite and the merge queue prove the tree, and publication owns
catch-up. Add no candidate build, local assembly run or second cut owner.

Release prep checklist: compare the locked Playwright version with the latest stable and verify browser binaries plus the chosen Node runtime. Any gap needs an upgrade decision before authoring/cut; the Node 26 tracing incident is a compatibility warning, not a universal engine constraint. Generate `npx playwright init-agents --loop=<harness>` and `npx playwright init-skills --loop=<harness>` in scratch, then review the official diff against [cas-playwright-tests](../cas-playwright-tests/SKILL.md), including its healer guard.

1. Finish review and batch QA; keep entries in `changelog.d/`. Commit the regenerated
   `.claude/CODEMAP.md` (`/codemap`); the cut refuses while a file added or removed
   since the map's last commit is unmapped. Deliveries never edit CODEMAP or
   PRODUCT_OVERVIEW: the cut folds `docs/qa/bug-families/` and `docs/product-overview.d/`
   fragments in (`scripts/doc-fragments.py compile`). Fixture versions use `9.99.x`.
   The supervisor writes release notes; workers never draft them, and release prep only merges, writes the changelog and regenerates the codemap.
2. Write the notes at the train's `release-notes.md` artifact path, following
   `docs/release-notes/RUBRIC.md` and `docs/RELEASE_SLACK_RUBRIC.md`; Cassy never authors notes.
   Declare constituent epics and exclusions in `CAS_RELEASE_INPUTS`. Review comes from
   each member's existing merge approval with the green train receipt; deliberate light,
   sampled and Tier 0 exemptions also need green lane CI. A no-code member's report
   proof is the reference recorded at park.
3. Read `cas release plan --train <train-id> --notes <draft>`. The default is
   read-only: it lists blockers and fixes before build, using the train tip's linter,
   membership, proofs, review, lane CI, eligibility, version ownership and queue
   configuration. Only included members can refuse; legacy holds are listed.
4. Cassy freezes a published train tip with fragments; the freeze starts its one
   train suite, and Cassy runs the cut on that run's green. Manual
   `scripts/release.sh --notes <draft>` checks the starting tip's green train receipt
   before version preparation and enters the same lease and detached pinned checkout,
   even when another worktree holds the branch. Competing cuts refuse with
   `CUT_LEASE_BUSY`; `--hotfix` selects a patch, `--next-version` is read-only.
5. A failed cut records its exact `release-cut-blocker:<tip>:<code>` label.
   Only a `release-repair` task carrying that label, or repairing a train suite red
   at or after the frozen tip, can merge through the standing freeze; merging
   consumes a named grant. Corrected notes re-arm the cut.
   Never start a parallel release or lift/reapply the freeze for repair.
   Commander lets the operator pause auto-cut or explicitly lift the freeze; a live
   supervisor lifts with `factory action=train_freeze id=<train> decision="lift" reason="<reason>"`,
   audited and notified to the operator.
6. The owner prepares versions, changelog and Commander once, persists cut
   tracking, pushes, reuses the PR and checks admission once. Durable SHA/tag
   reminders carry landing and publication; add no CI/PR polling loop.
   A queue ejection gets one repair batch and re-enqueue
   (`cas factory train drop <task>... --reason "<failing job>"`); a second is an operator P0.
7. Publication adopts and audits the queue archives, launch-smokes both platforms,
   publishes and verifies installation. Follow the post-release notice, using
   `cas-release-notes` for Violet and `cas-release-report` for measured delivery-to-install
   timing. Repair publication in its existing workflow; never replace a release.
   Inspect stranded deliveries before epic close.
