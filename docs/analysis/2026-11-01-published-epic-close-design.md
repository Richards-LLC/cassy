# Published releases close finished boards

Portable supporting receipt: `art-6295d21b` (sources, acceptance cases and removal map).

**Proposed design; operator approval pending. Added main → published time: 0 seconds.** Publication schedules bookkeeping; ordinary ownership and close gates decide completion. No implementation task is authorized yet.

## Existing stage, existing observer

Attach to **cas-a677 §10.1 stage 5, Install**, consuming **stage 4, Main push → release workflow**. Preserve §10.2's 14-minute main → installed budget plus 6-minute reserve. Add no CI stage, poller, build, test rerun or green-path agent handoff.

Stage 4 knows successful publication and the exact SHA/tag (`release.yml:263–276`), but not CAS epic identities. Stage 5 already reads the latest release every 60 seconds (`install-on-publish.sh:176–185`). Reuse that response to persist repository/tag/publication identity, including before the same-version early return. Once per new version, resolve the annotated tag to its exact commit. Reject draft, prerelease, dry-run or unknown identities. The owner's existing background mailbox duty joins the observation to known repository epics; GitHub receives no CAS task-write credentials.

## Ordinary close, triggered automatically

1. Select nonterminal boards whose recorded delivery tip is contained in the **published tag commit**, including older finished boards. Today's newer main is insufficient. Missing branches require existing delivery proof; names and version titles prove nothing.
2. Use the same complete child census as `factory epic_status`: include blocked, in-progress, awaiting-merge and quarantine-hidden rows. Require a nonempty, fully known set, all Closed or Cancelled. Otherwise leave open and name remaining rows. Never auto-close children.
3. Run the **normal epic close** under its live registered verification owner. Preserve integration evidence, stranded-branch checks, exact epic verification dispatch and post-release policy. Reuse valid proof; dispatch missing verification once. Existing verdict/check/sync events resume pending attempts. Existing patch-equivalence support may accept a shipped lane; automation supplies no override.
4. Record an idempotent result keyed by repository, published commit, epic and owner generation, citing the observed containment release. Revalidate child/tip/owner snapshots before writing terminal state; changed state invalidates the attempt.

## Ownership and release acceptance

Peer boards remain peer-owned: sync the publication intent through the existing team mailbox; the peer acknowledges once and reconciles locally. Preserve split claims. Unknown/offline owners stay pending; never adopt from heartbeat age. Restart resumes pending work.

Publication arms closure; it cannot waive deployment acceptance. Configured post-release checks require clean or an existing valid operator override. Retain runtime host verification and any owed manual product observation. Failed installation, verification or checks leave the board open. Merge, tag, publish and install never wait for bookkeeping.

## Delete the manual copies

Remove `cas-cut-release` step 8's manual close/stranded-inspection checklist. Remove `cas-supervisor/references/workflow.md:439`'s manual child census and line 448's premature “tip contained in base” completion promise. At line 493 remove only “close the epic”; preserve post-release observations and release-note duties. One pointer to automatic status/exception handling replaces the copies; canonical close gates remain.

## Budget, failure and approval

**0 seconds on main → published:** reconciliation has no dependency back to release creation. Reuse the 60-second observation; target selection/enqueue ≤2 seconds per background pass, with ordinary asynchronous close timeouts unchanged. These are design budgets, not measured implementation results.

API/sync/Git/store/owner/gate failure leaves a resumable reason and one owner notice per changed reason. Observation-write failure warns and retries next tick; it cannot abort installation. No repeated wakes or release rollback.

Sweep `art-65cc5389` supplies controls: stale 3.88/3.89 boards, nonterminal 3.90, peer-owned Accounts, and patch-equivalent stranded work. Add restart/duplicate, dry-run, failed-check and concurrent-state controls. **Record operator approval of this exact design before creating an implementation task.**

Sources: cas-a677 report §10.1–10.2 (`art-d2ccc40d`); canonical `close_ops.rs:50,7715,8488,9619`; `qa_post_release.rs:522–579`; `cloud/epic_split.rs:698–733`, at `ce03d1db75fe`. Full paths and evidence: supporting receipt. Design: `DESIGN.md@b43940e43`.
