# One staging walk per batch

Batch QA is the built-in default for fresh projects and new machines.
Opt into per-delivery evidence and independent QA with
`cas config set qa.mode per_task`. QA keys resolve per key from project
`.cas/config.toml`, then user `~/.cas/config.toml`, then built-in defaults.
An unreadable or invalid user QA config warns and is ignored; invalid project
config still reports its error. The setting is operator-owned. No background
process is added.

In batch mode, an implementer's pre-merge close still checks added test skip
and focus markers, then parks the delivery with evidence deferred. Cassy does
not automatically dispatch a per-task QA pass. A supervisor can still request
one explicitly with `verification action=qa_request`. A pending or failed explicit
round still binds: re-parking a changed tip opens the next round. A logged
waiver ends that per-task obligation; deferred batch evidence is still owed.

Once per release, at the frozen tip after the fix-diff re-review, the supervisor
builds a candidate, walks the combined batch matrix and records the reviewed
revision.
In qa.mode=batch, Cassy closes merged deliveries covered by the release verdict through ordinary gates; a batch QA receipt alone does not replace that verdict, and the worker does nothing. A deferred delivery reports its remaining QA or release-review gate. Recording QA retries ordinary close and reports any release verdict still owed. Covered QA anchors are not offered again. The supervisor gets one
offer per distinct uncovered delivery set. Advancing main alone stays quiet; a
changed set receives an offer naming the current published tip in its receipt call.

For parked deliveries already on `origin/<target>`, Cassy offers the supervisor
the exact receipt call once per target branch and live parked task set. Unrelated
merges that only move the target tip do not repeat it. A new task set receives
an offer using the current target SHA; the supervisor still records the receipt.

Offers belong to the epic's current verification owner. Ownership transfers
create a new offer for that owner; an owner absent from this machine never
falls back to an unrelated supervisor. Delivery and inbox reads recheck both
ownership and outstanding QA. Old hourly `qa_batch_overdue` notices and offers
already covered by QA are withheld with their stored history retained, even
when a different close gate still holds the shipped task open.

While building, implementers capture before/after evidence at 390 and 1440
pixels for visual changes. A non-implementer walks the frozen-tip candidate (or
deployed staging where one exists), capped per release, and writes one ledger
citing that evidence; uncovered changes are `NOT EXERCISED`. Include only tasks that passed;
failed or unreviewed tasks remain open.

Before the walk, run `cas qa ledger init --task <batch-task-or-epic>
--covers cas-delivery-a,cas-delivery-b` with every intended receipt delivery.
The generated `covers:` header names those IDs; without `--covers`, init uses
its owner task. Before publishing, run `cas qa ledger check <path>
--task <batch-task-or-epic> --covers cas-delivery-a,cas-delivery-b` with the same
set. Missing, malformed or incomplete coverage fails preflight. QA and release
receipts enforce declared coverage even when other tasks appear in the prose.
Historical ledgers without this header retain their existing receipt checks.

Publish the ledger using `artifact action=publish task_id=<batch-task-or-epic>
path=<absolute-ledger-path>`. Cloud storage is optional: a locally published
artifact is sufficient. A live registered supervisor who did not implement
any listed task records the receipt through the verification MCP tool. Prior
lease owners, completion-receipt workers, QA-round implementers, preserved
factory branch owners and exact Git actors on delivery anchor history also
count as implementers after reassignment. Integration and verification alone
do not count as implementation.

QA and closes can run on different machines. Recording a receipt adds a
reserved `QA_BATCH_RECEIPT_PROOF:` line to each listed task, and task sync
copies it into the other machine's receipt store, so either machine's close
gate and `verification show id=<qab-id>` accept it. The ledger file stays on
the machine that recorded it: the peer accepts the SHA-256 and size checked
there, and still byte-checks a ledger that is published locally. The
independent-author check runs against the closing machine's own record of who
implemented each task.

A release verdict uses the same action with `verification_type=release`: one
review of the assembled tip, whose published report names every delivery. It rides
task sync on a reserved `RELEASE_VERDICT_PROOF:` line. For Tier 1 deliveries
owned by release review, its exact-anchor coverage also satisfies deferred
batch debt. It does not replace Tier 2 probes or a dedicated review. A verdict recorded after an expired dispatch supersedes
that old wait; live or newer dispatches and rejected verdicts retain authority.

Name who walked the ledger in `ledger_author` (agent id or name). For every
listed tier 1 or tier 2 task, that author must be outside the same
implementation lineage: a QA reviewer, a verifier, or a worker on this or
another machine that did not deliver it. An unregistered author is recorded
with role `external`. The receipt stores the author's id and role, and close
rechecks them, so an author who later joins the lineage no longer satisfies
it. A worker's own tests and captures remain its proof but never count as
independent QA; a passed QA round whose reviewer joins the lineage is refused
the same way. Tier 0 deliveries (docs and tests only) are not gated. Only the
operator overrides, from their own terminal, with
`cas qa author-override <task> --note "<why>"`. The command refuses agent
sessions, the PreToolUse hook refuses agent shells that run it, and the record
lives in `.cas/operator/qa-author-overrides.toml`, which agents may not write.
A task note never overrides, because the delivering worker can write notes on
its own task. The receipt keeps the operator's words and the task gets an audit
note.

For a delivery containing only QA records, a supervisor has a narrower release
closeout route (GH #1655). The recorded start-to-anchor Git diff must contain
only `docs/qa/**/LEDGER.md` or Markdown files directly under
`docs/qa/batch-evaluations/`. Product code, journey catalogs, agent instructions,
unreadable diffs and a `no-code` declaration alone do not qualify.

Record `verification action=qa_waive task_id=<id> head_sha=<pushed-delivery-tip>
summary="<reason citing the independent walk>"`, then publish the release report
and record `verification action=batch_receipt verification_type=release
task_ids=["<id>"] staging_sha=<assembled-target-tip> ledger_artifact_id=<artifact>
ledger_path=<published-report-path> ledger_author=<walker>`. The report must name
the delivery, and its anchor must be integrated on its configured target.

For this route only, the release report's walker may be the QA record's author.
The supervisor recording the receipt must still be outside its implementation
lineage. The latest QA round must be a reasoned supervisor waiver bound to the
exact delivery anchor; missing, stale or superseded waivers do not qualify.
The receipt's task note cites the waiver, and close rechecks it and the measured
diff. Recording the release verdict retries ordinary close. This exception
creates no operator author override and does not admit a self-authored batch
QA receipt (`verification_type=task`) or waive review of product implementation.

Use the verification MCP tool:

```json
{
  "action": "batch_receipt",
  "task_ids": ["cas-a123", "cas-b456", "cas-c789"],
  "staging_sha": "<full 40-hex reviewed commit>",
  "ledger_artifact_id": "art-1234abcd",
  "ledger_path": "/absolute/path/STAGING-LEDGER.md",
  "ledger_author": "qa-reviewer-name"
}
```

Every task must belong to this project and have its own recorded delivery
anchor. Each anchor must be an ancestor of `staging_sha`, and that staging
commit must be on the task's configured target branch. Configure staging as
the delivery target when staging is the integration destination. A SHA on an
unrelated branch does not establish containment. Containment uses the merge
gate's local-or-origin rule, so a stale origin does not hide local integration.
Refresh target refs before recording.

After an evidence-only `request_changes` and restart, the active anchor may
be empty while the delivery remains in recorded history. Receipt recording,
status and close use the latest historical anchor in that case. An active
anchor always takes precedence; an invalid active or latest historical anchor
does not fall back to older records or a mutable branch tip. All staging,
target, independent-recorder and published-ledger checks still apply. Close
recovery supplies `commit_receipt` naming that exact historical anchor; a later
integrated correction cannot inherit its approval without a new active parked
anchor and matching QA. This
recovers delivery identity for QA; it does not restore declined merge authority
or clear deferred evidence without its receipt or passed independent round.

If a shipped historical or conflict-recovered task has lost its usable anchor
and implementer, a live registered supervisor can bind its integrated identity
before recording the batch receipt:

```json
{
  "action": "recover_delivery",
  "id": "cas-a123",
  "commit_receipt": "<integrated task commit>",
  "reason": "Inspected the released task and its integrated content"
}
```

This task MCP action requires an attributable commit with a nonempty diff,
preserved content and containment on the configured target. A known source's
tree-equivalent integrated squash can also establish attribution. An unrelated
target commit cannot. Recovery refuses active task leases, replacement of a
resolvable active anchor and identities owned by immutable delivery transactions.
It retains previous anchors, records the supervisor and reason, and binds the
full integrated SHA using a guarded task write. Replaying that binding is a no-op.
Assignment and task status stay unchanged; existing lease, branch and receipt
actors remain in the implementation lineage, joined by the recovered commit's
Git actors. The independent recorder and author checks still apply. Recovery
neither records a QA PASS nor waives deferred batch evidence or release review:
record the independent receipt, then retry ordinary close.

The runtime verifies nonempty UTF-8 ledger bytes against the published
artifact's size and SHA-256. It stores an immutable receipt with project,
recorder, time, staging commit, ledger binding and deduplicated task anchors.
`task_ids` is the supervisor's PASS attestation; the runtime does not infer a
deployed walkthrough from Git ancestry or parse PASS claims from ledger prose.
Supply 1 to 500 entries per receipt; duplicate IDs collapse. Larger batches
can use multiple receipts.

Each listed task can then close without a per-task `qa-bundle:` note or an
automatic independent pass. `verification action=qa_status task_id=<id>`
shows its valid batch receipt, and task notes retain the receipt citation.
Keep the published ledger file available and unchanged for subsequent closes.
When a valid batch receipt or covering passed independent QA round already
reviews the delivery, close uses that review's canonical evidence rather than
rechecking the producer ledger against a newer matrix grammar. Historical
producer evidence stays unchanged. Without valid coverage, existing producer
ledgers still undergo the current format check before verification dispatch.
Changed ledger bytes, a new delivery anchor, a different project, a target
that does not contain reviewed staging, or an omitted task cannot reuse it. Corrections require a new
published ledger and receipt; existing receipts cannot be edited.

An integrated task without a receipt follows the normal evidence gate. A task
that deferred evidence at park must have a valid listed receipt (including a
covering release verdict for release-owned Tier 1 work) or a passed
explicit independent QA round covering its current delivery anchor before it
can close. Pending, failed or older-tip rounds do not clear this debt. The
obligation survives changes to surface classification, gate settings and
`qa.mode`, and also applies to direct `task update status=closed`. A local
target or its origin tracking ref can prove integration; a stale origin cannot
authorize another evidence deferral for code already on the local target.
Skip/focus safeguards remain active for deferred parks and receipts.
supervisor_override=true does not clear batch QA debt. A waived QA round also
does not fulfill it; only a valid listed receipt or a passed independent round
covering the current delivery anchor does. Explicit QA-request authority remains
available.

Batch receipts and deferred obligations are local project records in
`.cas/cas.db` (migration 276). They do not create a cloud synchronization lane.

A successor supervisor's startup recovery list spans all review tiers, rather
than only release-owned Tier 1 deliveries. Each integrated, content-preserved
project delivery without a live review or adverse verdict shows its current
hold and a close action. Tier 0 still passes mechanical gates; dedicated
reviews retain their own verdicts, and Tier 2 still needs an adversarial probe.
CI overrides remain subject to independent QA. Close rechecks these gates; appearing in recovery never approves or closes a delivery.
Unreadable verification metadata skips and reports that row while preserving
the other recovery actions and the inherited supervisor handoff.
