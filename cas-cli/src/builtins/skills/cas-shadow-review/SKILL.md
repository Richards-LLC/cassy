---
name: cas-shadow-review
description: Use when a supervisor runs independent Spec and Standards reviews alongside the legacy task-verifier; records advisory verdicts and cross-checked fix commits before explicit opt-in.
license: MIT
metadata:
  managed_by: cas
  author: Matt Pocock
  upstream: https://github.com/mattpocock/skills
  provenance: Adapted from mattpocock/skills engineering/code-review (MIT, © 2026 Matt Pocock).
---

# Shadow review

Run this on a parked delivery with a legacy verification dispatch. Keep the
existing task-verifier running: shadow verdicts have no merge-gate authority.
CLI parity is `cas factory shadow-review --request <JSON-file>` with the same
registered caller identity, request shape and JSON response as the MCP action.

1. Read the exact dispatch id and fixed delivery base commit. Require a clean
   delivery checkout at the dispatch HEAD. Spawn two registered Standard
   SubAgent children of the supervisor, distinct from the implementer and
   legacy verifier. Give each only its axis reference initially:
   [Spec](references/spec.md) or [Standards](references/standards.md).
   Each child has its own context and waits for a round id; it never starts,
   closes, or reassigns the implementation task.
2. Call `verification action=shadow review=<JSON>` with:

   ```json
   {"op":"start","task_id":"cas-example","dispatch_id":"vdispatch-example","base_ref":"<fixed-base-sha>","spec_agent_id":"<registered-child-id>","standards_agent_id":"<different-child-id>"}
   ```

   Send only `round.id` to each child. The server seals their identities and
   reserves `review/<task>/spec` and `review/<task>/standards` in separate
   worktrees. Existing refs fail closed; preserve their evidence before
   manually retiring them for a fresh dispatch. Repeating the same start
   returns the same round while the original delivery proof remains current.
3. Each child calls `{"op":"context","round_id":"<round-id>"}`. The server
   returns that axis's sources, side ref, worktree and fixed diff bounds.
   Review `git diff <base_commit>...<head_commit>` in that worktree. Rank each
   axis independently. For a certain finding, commit a small fix on its own
   side ref with subject `review(spec): f1 <fix>` or
   `review(standards): f1 <fix>`. Report uncertain findings without commits.
   Optional Rust proof uses the capped targeted runner: one package and an
   explicit `-E 'test(module::name)'` filter, after the supervisor seeds that
   review worktree's private target cache. An older runtime denial requires
   supervisor proof; never bypass it with another compiling command.
4. Each child sends a typed report using this shape (see its axis reference):

   ```json
   {"op":"report","round_id":"<round-id>","report":{"axis":"spec","status":"approved","summary":"<observed result>","criteria":[{"criterion":"<exact stored line>","status":"approved","evidence":"<proof>"}],"scope_creep":[],"findings":[{"id":"f1","rank":1,"source":"<exact criterion>","evidence":"<file:line and consequence>","uncertain":false,"judgement":false,"commit":"<full-fix-sha>"}]}}
   ```

   Omit `commit` when there is no fix. `status` is approved, rejected, error
   or skipped. Reports seal every side-ref commit, are immutable, and return
   only that child's report. Preserve the pre-fix finding in `evidence` and
   include any targeted-test receipt there.
5. After both reports are sealed, each child calls
   `{"op":"context","round_id":"<round-id>","cross_check":true}`. Inspect
   the other axis's exact fix commits and independently check their effects.
   For every fix, send:

   ```json
   {"op":"cross_check","round_id":"<round-id>","commit":"<other-axis-fix-sha>","decision":"accept","reason":"<observed evidence>"}
   ```

   Use `decision="revert"` to reject. The server records intent, reverts the
   exact fix on its owner's side ref and labels the revert with the checking
   axis, finding id, reason and registered reviewer id. Never cross-check an
   own-axis commit. A failed/conflicting revert leaves durable intent and
   side-ref evidence; ask the supervisor to recover it before proceeding.
6. As supervisor, call `{"op":"show","round_id":"<round-id>"}`. Retain the
   exact legacy verdict alongside both separate axis reports, all fix SHAs,
   cross-check decisions and revert SHAs. A missing legacy verdict is pending
   evidence, not an agreement. Keep rankings separate, including disagreements.
7. Leave fixes on side refs by default. To opt this task in explicitly, call
   `{"op":"apply","round_id":"<round-id>","opt_in":true}` as the bound
   supervisor. Only accepted, cross-checked fixes are cherry-picked onto the
   original clean delivery branch. Conflicts abort the sequence and preserve
   a failed application record; interrupted operations retain an intent record
   for supervisor recovery. Repeating a completed apply returns its receipt.
   Request a fresh legacy dispatch for the changed delivery and follow the
   existing verification/merge path. Shadow results never replace its proof.
8. Before enabling any policy that consumes these verdicts, collect at least
   three real delivery comparisons plus assembly proof for the shadow API.
   Record source task/dispatch ids and the comparison JSON in the epic evidence.
   Synthetic contract tests prove behavior, not review accuracy.
