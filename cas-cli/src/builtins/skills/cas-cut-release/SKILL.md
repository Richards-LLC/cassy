---
name: cas-cut-release
description: Use when cutting a Cassy runtime release from an assembled epic; follow this fail-closed train.
managed_by: cas
---

# Cassy release train

This is the only supervisor release procedure. Every numbered step is a gate:
stop at its first failure, record the blocking step in the operator timeline,
and do not claim success without its receipt.

1. Read references/failure-log.md in full. If a new failure is not logged,
   learn it before retrying:
   scripts/release-gate.sh --learn "<symptom>" "<cause>" "<check-id>"
   Add the executable check and scripts/test-release-gate.sh fixture in the
   same commit. Then record the same text with
   mcp__cas__memory action=remember entry_type=learning tags=release before retry.
   Cite added entries in the epic close note.
2. Assemble the exact epic tip: git worktree add .cas/epic-<id>-merge <epic-tip>.
   Reconcile sibling lanes on this tree before the prep commit: run the full
   suite on the assembled tree, inspect guardrail/marker and counted-field
   tests (grep conflicts_resolved in tests and assert directional fields),
   and make a reconciliation commit for any cross-lane trim or move.
3. Resync root projections after builtin changes by copying each changed
   embedded agent file from cas-cli/src/builtins/**/agents onto its root
   projection (.claude/agents, .codex/agents). Never run cas update --sync
   for this: it refreshes every registered project on the host. Run the
   root_managed_projections drift test. Regenerate the
   reference ledger with scripts/gen-builtin-reference-history.sh and require
   git diff --quiet -- cas-cli/src/builtins/reference-history.json.
4. Prepare one source commit: run scripts/bump-release-version.sh <version>,
   run cargo update --workspace --offline, update CHANGELOG.md and the
   runtime-release Slack draft, and carry the previous release's POSTED receipt.
   Then run scripts/release-gate.sh <version>; all PASS rows are required.
5. For archive mode, put suite.tar.zst and TMPDIR under a scratch base with
   NO .cas ancestor directory (the queue runner has none; anything under
   $HOME walks up into ~/.cas), on a large disk, never a 32-GB /tmp tmpfs:
   export CAS_RELEASE_GATE_HOME_DIR=/var/tmp/cas-release-gate on this host. Run cargo nextest archive -p cas --archive-file
   <home-disk>/suite.tar.zst. Build a remap containing a copy of root
   Cargo.toml and an empty directory for every package path from
   git ls-files '*/Cargo.toml'. From outside the checkout, use a bin directory
   of symlinks to cargo/rustc/git/sh/bash/jq/python3 plus /usr/bin:/bin, with
   no rg, and run the archive with --workspace-remap <remap>. Exclude
   component_output_test because its source-tree snapshots are expected to
   need the checkout; do not "fix" those snapshots.
6. Push the source branch and open exactly one protected-main PR:
   git push -u origin <source-branch>
   gh pr create --base main --head <source-branch> --fill
   Queue it with gh pr merge <pr> --merge. Facts: PR Fast Validation and
   macOS Check are admission stubs; the merge_group synthetic tree is the
   real host-independent validation. Watch the run by headBranch pr-<n>;
   re-enter with a fix commit when it fails, and record elapsed minutes.
7. After the merge lands, fast-forward main:
   git fetch origin main && git merge --ff-only origin/main
   git worktree add .cas/release-v<version> <landed>
   cp -al .context/zig .cas/release-v<version>/.context/zig
   Run scripts/check-release-preflight.sh --local v<version>. A bare
   ./scripts/release.sh                 # local audit only is the sole allowed
   pre-warm; export ZIG in a fresh tag worktree. release.sh removes stale
   blake3-* build/.fingerprint outputs before its audit.
8. Publish PID-safely from the tag worktree:
   nohup ./scripts/release.sh --publish-tag > release-publish.log 2>&1 & echo $!
   Save the PID; watch kill -0 <pid> and a done marker, and kill by PID only.
   Do not use pkill -f. Watch the Release workflow without foreground polling.
9. Verify published assets with scripts/release-published-receipt.sh
   --write-draft and scripts/release-latency-receipt.sh. Post the configured
   supervisor session's Slack route (never assume an email/profile): User
   top-level then one reply, then Dev top-level then one reply. Write POSTED
   only with ts and permalink for all four messages. Tell the operator when
   publishing occurs and report a timeline naming the blocking step.
10. Run cas update on the host and verify cas --version and cas hub. Carry the
    POSTED receipt into the next prep commit. Close the epic only after the
    merge commit receipt and stranded-branch inspection; if sibling lanes
    rewrote delivered files, use stranded_branch_override with the narrative
    and proof on main, never no-code.

## What went wrong before

- Archive extraction filled /tmp and then lacked the remap package layout — v3.12.0.
- Root managed projections were stale after builtin edits — v3.12.0.
- Cross-lane guardrails and routing markers drifted after trimming — v3.12.0.
- conflicts_resolved changed meaning without directional assertions — v3.12.0.
- A test hardcoded the new crate version — v3.11.0.
- Doctor snapshots varied with host git wording and temp-path width — v3.11.0.
- release.sh found duplicate BLAKE3 build inputs after mismatched prewarm — v3.11.0.
- A pkill pattern matched the publishing caller and killed it — v3.12.0.
- An epic close stranded rewritten lane files without an inspection override — v3.12.0.
- PR checks were mistaken for merge_group validation — v3.12.0.
- The documented Slack account/profile was not the configured route — v3.12.0.
- Manual gate fiddling hid the green-to-tag timeline — v3.12.0.
- Host cas was not updated and verified from the published receipt — v3.12.0.
