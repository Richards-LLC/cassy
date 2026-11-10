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
| `constituent_epic_ids` | Declared epics; recursively include their descendants, including terminal deliveries. |
| `included_task_ids` | Explicit expected deliveries, including terminal rows outside the declared epics. |
| `no_code_members` | Map of explicitly included no-code task IDs to portable proof references. |
| `proof_refs` | Release-level QA/review references for the subsequent proof consumer. They do not imply per-task coverage. |

Each `ReleaseProofRef` has `kind` (`qa`, `review`, `report`), `artifact_id`,
`sha256`, `size_bytes`, `reviewed_sha`, and optional `receipt_id`. References are
identities to validate, not an attestation created by this resolver. Existing
batch QA and release review receipts are collected only when `task_heads` names
the exact original anchor; published-tag receipts never become QA/review proof.
Per-round local ledger paths cannot substitute for portable artifact references.

The result includes the original `scope`, sorted `deliveries` and sorted
`exceptions`. Each delivery has `task_id`, `generation`, `anchor`, `base_sha`,
`shipped_commit`, `membership`, and `proof_refs`. `generation` is the captured
`Task.updated_at` RFC3339 snapshot token, not a task lease counter. Consumers must
reload and compare it before acting; any intervening mutation requires fresh
evaluation. It deliberately errs on the side of invalidating stale work.

Code membership is `ancestry` (the original anchor is reachable from the final
SHA) or `squash_tree` (a commit in that exact SHA's history has the original
anchor's complete tree). Squash matching requires a valid recorded start base
strictly preceding the anchor. Structured merge receipts can identify a delivery
that has no factory anchor. A retired factory anchor never becomes active
membership through a later merge receipt. No SQL `branch` value or commit-subject
task mention is used. The source anchor and full start boundary remain intact;
`shipped_commit` records the mapped commit separately. Unproven cherry-picks or
squashes produce `missing_mapping`; patch/content similarity is not approval.

Beyond the explicit scope, live project deliveries with structured recorded
anchors/merge receipts are considered, so already-shipped close debt is included.
Terminal rows outside explicit scope are omitted. Foreign project/repository
rows, cancelled tasks and retired anchors are exceptions, never close authority.

A reachable anchor with no start base is still shipped, with a `missing_base`
exception. Membership and close readiness are separate: full delivery-effect
validation still needs that base. Unresolvable anchors, invalid bases and absent
mappings remain explicit. Expected no-code reports enter only as
`explicit_no_code`, with a nonempty portable proof list bound to the reviewed
SHA; their anchor and shipped commit are null.

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
this inventory. The consumer re-resolves membership, compares each delivery
snapshot before acting, checks referenced receipts and runs ordinary close with
`final_release_sha` as its evidence target. Stale or held members remain explicit
exceptions while other deliveries continue. The ordinary optimistic write guard
still rejects concurrent changes. Dedicated review, CI overrides and independent
task holds remain gates. Explicit no-code deliveries retain their own close
path; this code-delivery entry reports that limitation.

The full recorded start-to-anchor effect is measured, including earlier commits
and deletions. Changelog fragments use the existing promotion validator: their
entries must survive in a released section of `CHANGELOG.md`. A maintained view
has no unconditional filename exemption.

A release review may attach transformation evidence once, in its published
Markdown/plain-text ledger, as one fenced `cassy-release-transformations` JSON
block. Its object contains `project_id`, `release_id`, `reviewed_sha`,
`final_release_sha` and `paths`. Each path object contains:

| Field | Required evidence |
| --- | --- |
| `path` | `.claude/CODEMAP.md`, `docs/qa/journeys.yaml` or `docs/PRODUCT_OVERVIEW.md`. |
| `input_blobs` | Exact Git blob IDs of the delivered view versions replaced. |
| `output_blob` | Exact view blob at the final release; deletion is not a view transformation. |
| `producing_commit` | Full reachable commit changing this path and establishing its final blob and mode. A supersession chain names its final producer. |
| `producer` | The corresponding skill: `codemap`, `cas-qa-craft` or `project-overview`. |
| `producer_blob` | Exact skill revision in `cas-cli/src/builtins/skills/<producer>/SKILL.md` or `.claude/skills/<producer>/SKILL.md` at the release. |

The scope's `review` proof reference binds artifact, digest, size, reviewed
commit and recorded release-verdict receipt. Published bytes are checked locally
and the author must be independent of the covered deliveries. The receipt must
cover the original anchor. This represents a reviewed skill-authored view, not
a claimed deterministic generator run. Ordinary source files cannot use this
rule. Built-output regeneration retains its existing generator checks.

The reviewed candidate and final release must have identical trees apart from
explicitly reviewed maintained-view transformations. Adapters bind their review
after release preparation; an unreviewed product/configuration change requires
new affected proof rather than an ancestry-only transfer.

QA walk receipts, independent review receipts and published-tag shipping
verdicts keep their distinct kinds. The inventory close entry requires
independent review; shipping alone never creates a review or a QA PASS.
Missing bytes, bad bindings, changed task versions, unknown producers or lost
source leave the delivery held. Existing receipt callbacks also pin ordinary
close to their reviewed SHA; full inventory adapters additionally supply the
validated release transformation manifest.

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

The current installed/backfill adapter invokes this function after closes and
atomically saves `.cas/releases/<tag>-issues.json` with `scope` and `issues`, then
includes `issue_outcomes=<JSON>` in its returned receipt text. This sidecar also
exists for historical releases that have no tracked cut receipt. Publication
verification precedes this stage; the issue helper never publishes a release.
