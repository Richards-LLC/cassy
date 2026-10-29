# One staging walk per batch

Batch QA is the built-in default for fresh projects and new machines.
Opt into per-delivery evidence and independent QA with
`cas config set qa.mode per_task`. QA keys resolve per key from project
`.cas/config.toml`, then user `~/.cas/config.toml`, then built-in defaults.
The setting is operator-owned. No background process is added.

In batch mode, an implementer's pre-merge close still checks added test skip
and focus markers, then parks the delivery with evidence deferred. Cassy does
not automatically dispatch a per-task QA pass. A supervisor can still request
one explicitly with `verification action=qa_request`.

Every hour with new deliveries, the supervisor builds a candidate at the exact
epic tip, walks the combined batch matrix and records the reviewed revision.
After the published ledger and `verification action=batch_receipt` cover the
listed delivery anchors, close those deliveries. Until that receipt exists,
a deferred delivery reports "waiting for batch receipt".

While building, implementers capture before/after evidence at 390 and 1440
pixels for visual changes. After merging the batch to its configured staging
target and deploying that exact commit, a non-implementer walks every change
and writes one ledger citing that evidence. Include only tasks that passed;
failed or unreviewed tasks remain open.

Publish the ledger using `artifact action=publish task_id=<batch-task-or-epic>
path=<absolute-ledger-path>`. Cloud storage is optional: a locally published
artifact is sufficient. A live registered supervisor who did not implement
any listed task records the receipt through the verification MCP tool. Prior
lease owners, completion-receipt workers, QA-round implementers, preserved
factory branch owners and exact Git actors on delivery anchor history also
count as implementers after reassignment. Integration and verification alone
do not count as implementation.

Use the verification MCP tool:

```json
{
  "action": "batch_receipt",
  "task_ids": ["cas-a123", "cas-b456", "cas-c789"],
  "staging_sha": "<full 40-hex deployed staging commit>",
  "ledger_artifact_id": "art-1234abcd",
  "ledger_path": "/absolute/path/STAGING-LEDGER.md"
}
```

Every task must belong to this project and have its own recorded delivery
anchor. Each anchor must be an ancestor of `staging_sha`, and that staging
commit must be on the task's configured target branch. Configure staging as
the delivery target when staging is the integration destination. A SHA on an
unrelated branch does not establish containment. Containment uses the merge
gate's local-or-origin rule, so a stale origin does not hide local integration.
Refresh target refs before recording.

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
Changed ledger bytes, a new delivery anchor, a different project, a target
that does not contain reviewed staging, or an omitted task cannot reuse it. Corrections require a new
published ledger and receipt; existing receipts cannot be edited.

An integrated task without a receipt follows the normal evidence gate. A task
that deferred evidence at park must have a valid listed receipt or a passed
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
