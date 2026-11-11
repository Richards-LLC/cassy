# Release inventory and proof contract

`qa_batch::resolve_release_inventory(root, repo, &ReleaseScope)` is the shared,
read-only resolver. `cas release inventory --scope scope.json` prints the same
JSON. It does not publish, validate QA/review approval, or close tasks. Publishing
and promotion adapters can consume this contract without another subsystem.

The existing tracked `ReleaseContext` adds optional `release_scope` and
`release_inventory` fields; older cut receipts deserialize with both absent.
The scope is portable; the repository argument is the local checkout to inspect.

| Scope field | Meaning |
| --- | --- |
| `project_id` | Canonical project ID, exactly matching the project store. |
| `repository` | GitHub origin slug, case insensitive, belonging to the store's repository. |
| `release_id` | Stable version or promotion identity. |
| `reviewed_sha` | Full immutable candidate commit. It need not equal the release commit. |
| `final_release_sha` | Full immutable shipped commit. Moving main/epic refs never decide membership. |
| `promotion_pr`, `release_tag` | Optional PR number and tag. A supplied local tag must resolve to the final SHA. Publication/PR authenticity belongs to the trusted adapter. |
| `constituent_epic_ids` | Declared epics whose descendants are searched for shipped deliveries, including terminal rows. |
| `included_task_ids` | Explicit expected deliveries, including terminal rows outside the declared epics. |
| `no_code_members` | Map of explicitly included no-code task IDs to portable proof references. |
| `proof_refs` | Release-level QA/review references for the subsequent proof consumer. They do not imply per-task coverage. |

Each `ReleaseProofRef` has `kind` (`qa`, `review`, `report`), `artifact_id`,
`sha256`, `size_bytes`, `reviewed_sha`, and optional `receipt_id`. References are
identities to validate, not an attestation created by this resolver. Existing
batch QA and release review receipts are collected only when `task_heads` names
the exact original anchor; published-tag receipts never become QA/review proof.
Per-round local ledger paths cannot substitute for portable artifact references.

Declare report proof explicitly in the draft's `CAS_RELEASE_INPUTS` JSON under
`no_code_members`. The audit adds those task IDs to `included_task_ids` and
carries the typed proof lists through `cas release track --inputs` to publication.
Tracking refuses missing or mismatched report bindings before marking the train
cut. Artifact owner/project, hash, size and revocation are checked; the proof head need not equal the release review commit;
a report reference does not supply QA or review approval. Existing receipts without
this optional map remain readable. Promotion accepts the same audited JSON with
`cas release promoted --pr <N> --inputs <audit.json>`; its `checked_sha` must name
the promotion head or merge commit. Recovery keeps frozen membership and commit
identity; an unbound legacy report remains a named residual.

The result includes the original `scope`, sorted `deliveries` and sorted
`exceptions`. Each delivery has `task_id`, `generation`, `anchor`, `base_sha`,
`shipped_commit`, `membership`, and `proof_refs`. `generation` is the captured
`Task.updated_at` RFC3339 snapshot token, not a task lease counter. Consumers must
reload and compare it before acting; any intervening mutation requires fresh
evaluation. It deliberately errs on the side of invalidating stale work.

Code membership is `ancestry` (the original anchor is reachable from the final
SHA) or `squash_tree` (a commit in that exact SHA's history has the original
anchor's complete tree). Squash matching requires a valid recorded start base
preceding or equal to the anchor. Structured merge receipts can identify a delivery
that has no factory anchor. A retired factory anchor never becomes active
membership through a later merge receipt. No SQL `branch` value or commit-subject
task mention is used. The source anchor and full start boundary remain intact;
`shipped_commit` records the mapped commit separately. Unproven cherry-picks or
squashes produce `missing_mapping`; patch/content similarity is not approval.

Beyond the explicit scope, live project deliveries with structured recorded
anchors/merge receipts are searched for proven shipped close debt. Descendant
discovery alone does not declare an unstarted or unshipped task as a release
obligation. `included_task_ids` and `no_code_members` keep missing declared
members visible even when their mapping fails. A shipped historical anchor
can retain a hold for an unresolved earlier cycle without rebinding the current
delivery to that anchor. Terminal rows outside explicit scope are omitted. Foreign project/repository
rows, cancelled tasks and retired anchors are exceptions, never close authority.

A reachable anchor with no start base is still shipped, with a `missing_base`
exception. Membership and close readiness are separate: full delivery-effect
validation still needs that base. Unresolvable anchors, invalid bases and absent
mappings remain explicit. Expected no-code reports enter only as
`explicit_no_code`, with a nonempty portable proof list bound to an immutable approval head; their anchor and shipped commit are null.

`exceptions[].code` is one of `missing_task`, `foreign_scope`, `not_epic`,
`cancelled`, `missing_anchor`, `retired_anchor`, `missing_base`, `invalid_base`,
`missing_mapping`, `missing_no_code_proof`, `quarantined`. A delivery may also have a base
exception: callers must inspect both arrays. The inventory does not grant QA,
review, content-preservation or close admission. The resolver itself performs
no publication or lifecycle writes; adapter integration is described below.

Quarantined rows remain suppressed on the ordinary task board. The resolver
uses the existing by-ID inspection path to preserve their shipped facts and
adds a `quarantined` exception; membership never lifts that independent hold.
Recorded anchors and bases must resolve to their own full SHA, never a symbolic
branch alias.

## Close at the shipped commit

Trusted release adapters call `CasCore::close_release_inventory_outcomes` with
this inventory. Each member closes independently when its approval-bound head
is an ancestor of `final_release_sha` and its captured generation is unchanged.
Approval is an approved verification at exactly that head, a recorded review or
QA receipt naming that task and head, or a Tier 0 delivery. Published-tag shipping
receipts never grant approval. A member without approval remains open as its own
`REVIEW_MISSING` residual; it does not hold other members.

The consumer makes one compare-and-swap task close write. It does not rerun the
ordinary task-close stack, review-to-release diff mapping, ledger download or
author validation already performed when recording the receipt. An intervening
task mutation refuses that member. Already closed tasks count as already closed,
including those with historical inventory exceptions.

Explicit no-code members use the same core. Their portable artifact metadata
must match owner/project, hash, size and revocation, and their proof head must
have approval and be contained in the release. The report head need not equal
`scope.reviewed_sha`. Membership alone does not approve a report.

`release_reconcile::preview_release(root, repo, inventory)` evaluates this same
rule without writing tasks, operation receipts, sync queues or artifact caches.
Its member map returns would-close (`Closed`), would-hold (`Held` with a reason),
`AlreadyClosed` or `NotEligible`; inventory exceptions are included. Publication
uses the same evaluator with writes enabled. Parent acceptance retains the
explicit independent epic coverage described below.

Legacy release transformation blocks remain readable by existing receipt and
parent consumers. They are no longer a whole-release member-close gate.

## GitHub issue outcomes after task reconciliation

This stage consumes the same immutable inventory and the delivery outcome map
returned by `CasCore::close_release_inventory_outcomes`. It runs after local
close decisions; issue-side failures remain separate from those decisions.

`issue_reconcile::reconcile_release_issues(root: &Path, repo: &Path,
 inventory: &ReleaseInventory, close_results: &BTreeMap<String, CassyMergeClose>)
 -> Vec<IssueOutcome>` runs after the delivery close calls. `CassyMergeClose`
comes from `mcp::tools::core::task::lifecycle::merge_close`. Each serializable
`IssueOutcome` has `issue`, `action` (`closed`, `partial`, `failed`, `skipped`),
`comment_or_error`, and sorted `task_ids`. The next release receipt should embed
this vector directly; issue failures do not undo successful local task closes.

The incoming inventory selects candidate issues through `task_issue_refs`.
The function reloads every referencing task, including quarantined rows and
terminal deliveries outside the original scope, and expands that scope through
the same inventory resolver. As with `shipped_open_issues`, cancelled attempts
do not keep an otherwise complete issue open. Every remaining reference must
be closed and have proven shipped membership; a held close or quarantine keeps
the issue partial. Missing/invalid start bases do not erase proven ancestry of
an already closed task. Explicit no-code membership still needs the scope's
portable proof. Referencing task versions are checked again after GitHub reads;
changes require a fresh run.

GitHub reads and writes share the existing six-second deadline and repository
bound. A successful close uses reason `completed` and a comment naming the
release identity, immutable final SHA and every referencing task. Already closed
issues are skipped on replay. Partials and every read/write failure remain named
outcomes; offline does not mean closed. Dry runs never contact GitHub.

The shared release operation invokes this function after closes and keeps the
outcomes in its receipt's `issue_outcomes`; every trigger, including installed
and backfill recovery, reaches it only through that operation. Publication
verification precedes this stage; the issue helper never publishes a release.

## Durable reconciliation and parent acceptance

`release_reconcile::reconcile_release(root, &scope, &inventory, trigger)` is the
shared async entry point. Triggers are `publish {tag}`, `promotion {pr,
merge_sha}`, `installed`, and `backfill`. Publication authenticity belongs to
the trusted adapter; the existing immutable close validator owns proof authority.
Historical as-of-approval gate evaluation remains a separate backfill extension.

`ReleaseOperationKey {project_id, release_id, final_release_sha}` identifies one
operation. Its sole authoritative checkpoint is the SQLite
`release_reconcile_operations` row. An OS lock serializes effects for that key;
each result is committed before another member proceeds. A crash after an
ordinary close but before its checkpoint reuses that exact committed close on
replay. Close intent retains the original anchor, start boundary, assignee,
work target, park reason, and delivery transaction identity. Changed cycles are
held instead of rebound. JSON exports are derived, never another checkpoint.

`release_reconcile::completed_operation(root, &key)` returns the completed
`ReleaseReconcileReceipt`, or `None` for absent/held operations. The receipt
contains `members`, `issue_outcomes`, `parents`, inventory `exceptions` (including
`quarantined`), `residuals {task_id, gate, owner, next_action}`, `sync_pending`, and
stage timestamps. Completion never asserts peer synchronization. Every live
member or parent has a named residual; independent exceptions remain visible.

On retry, obsolete discovery holds for missing anchors, mappings or undeclared
no-code proofs are removed when the fresh inventory excludes the task and no
saved close intent binds its cycle. Their residual and sync-pending references
are retired together. Frozen declarations, current obligations and committed
close intents retain their holds.

The operation persists `notice_intent {key, recipient, fingerprint}` and sends
no notice. The publication/promotion adapter alone enqueues that intent using
its stable key and bound recipient. Retry and supervisor turnover reuse the
same intent rather than double-send. The notice context stores
`closeout_counts` with `closed`, `already_closed`, `held`, `not_eligible`,
`residual` and `complete`, derived from the persisted receipt. The headline and
CLI summary use those outcome counts; an attempted or held member is never
reported as a successful close. A completed lookup is the adapter's no-op
check; partial operations remain resumable.

An independently authored, published release review may additionally carry
exactly one fenced `cassy-release-epics` JSON block:

```json
{
  "project_id": "canonical-project",
  "release_id": "v9.99.0",
  "reviewed_sha": "<full candidate SHA>",
  "final_release_sha": "<full shipped SHA>",
  "epics": [{
    "task_id": "cas-example-epic",
    "generation": "<Task.updated_at RFC3339 snapshot>",
    "child_ids": ["cas-example-delivery"],
    "acceptance": "pass"
  }]
}
```

The actual fence label is `cassy-release-epics`, beside the existing
`cassy-release-transformations` block in the same bound review bytes. Each
parent requires its exact generation and complete child set returned by `TaskStore::get_subtasks` (including nested descendants), and the
reviewer must be independent of the parent and every descendant. The validator
reuses the existing artifact binding, candidate mapping and author checks.
Missing or stale acceptance queues one existing epic proof cycle and records
`pending`; prose and tag existence grant no epic acceptance. Empty legacy
containers remain held for an evidenced operator disposition. Accepted parents
close bottom-up through the ordinary lifecycle gate at the immutable release
SHA, including epics whose old branch is gone. Release authority can adopt
positively dead ownership without impersonating the review author.

## Publish and promotion adapters

Every trigger calls the one release reconciliation,
`release_reconcile::reconcile_release(root, scope, inventory, trigger)`, keyed by
(project, release, final SHA). A trigger whose key already has a completed
operation does not reconcile again. Each release gets one `release_closeout`
supervisor notice, deduplicated by the operation's notice intent. It names every
residual and held or pending parent with owner and next action, inventory
exceptions, and partial or failed GitHub issues.

| Trigger | Adapter | Evidence read |
| --- | --- | --- |
| `Publish{tag}` | `cas release published --receipt <release.json> --repo <slug>`, run by `scripts/install-on-publish.sh` once per newly seen publication, before installing | The timer's existing latest-release read; tag SHA verified on origin |
| `Promotion{pr, merge_sha}` | `cas release promoted --pr <N>`, started in the background by the supervisor's PostToolUse hook after its successful raw PR merge into the default branch of a staging-promotion project (log: `.cas/release-reconcile/promotion.log`). A queued or auto-merge PR records a pending promotion under `.cas/release-reconcile/pending-promotions/` and arms the daemon's existing `branch_contained_in` landing observation of its head; when that fires, the same command runs once and the intent is retired | One `gh pr view`: merged into the default branch, merge commit on origin |
| `Installed`, `Backfill` | `cas release installed`, `cas release backfill` | Recovery only: the same operation with its own trigger, a no-op once complete. `cas release backfill --dry-run` is a read-only preview |

The scope comes from the tracked cut receipt (cas-src) or the merged PR
(promotion). The reviewed candidate is the newest recorded release review the
final commit contains, plus every QA/review receipt at that candidate. Members without their own approval or a receipt naming their head remain
named residuals; publication never stands in for review.
No CI job, poller or test rerun is added, and GitHub is read only as evidence.

`cas qa ledger init --task <id> --release-scope scope.json` scaffolds the release
review ledger with the `cassy-release-transformations` and `cassy-release-epics`
fenced blocks for that scope, so the published RELEASE_VERDICT bytes carry them.
Each epic's `child_ids` is its full recursive subtree, nested epics' descendants
included. Remove any epic whose acceptance was not verified before publishing.
