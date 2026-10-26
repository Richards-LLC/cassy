# Source event kind on managed activity deliveries

Status: **Satisfied by [Cloud PR #204](https://github.com/Richards-LLC/petra-stella-cloud/pull/204)**,
merged at `a052e1f53a48635f9ea1f484e4cf4bfbc29adef6` on 2026-10-22.
The Cloud supervisor reports this deployed; Cassy independently inspected the
merged source and claim/recovery fixtures, without a live authenticated delivery.
Cassy task: `cas-c28b6df4a2b653af`, from `Richards-LLC/violet_ps#70`.

## Shipped contract

Cloud supplies optional `delivery.event_kind`, alongside `id`, `occurrence_id`
and `route_kind`. **The field is on `delivery`, not `target`.** This supersedes
the original proposal for `target.event_kind`; targets are unchanged.
Cloud also supplies optional `delivery.author_user_id`; this Cassy change does
not use it for classification.

The delivery metadata is present in claim, recover/admission listings, lookup,
rebind and receipt bodies. Supported event kinds are `message`, `app_mention`
and `message_changed`. Historical journal replays may omit the fields.

Example item shape (other fields omitted):

```json
{
  "delivery": {
    "id": "0000de2a-1111-4111-8111-00000000de2a",
    "occurrence_id": "00000cc2-1111-4111-8111-000000000cc2",
    "event_kind": "app_mention",
    "author_user_id": "U01HUMAN42"
  },
  "target": {
    "external_ref": {
      "provider": "slack",
      "workspace_id": "T07ABCDEF",
      "channel_id": "C08COORD01",
      "root_ts": "1760616000.123456"
    },
    "message_ts": "1760616042.000200"
  }
}
```

Confirmed at immutable Cloud revision `a052e1f53a48635f9ea1f484e4cf4bfbc29adef6`:

- [deliveries.ts:36–40](https://github.com/Richards-LLC/petra-stella-cloud/blob/a052e1f53a48635f9ea1f484e4cf4bfbc29adef6/lib/coordination/deliveries.ts#L36)
  projects the persisted occurrence metadata through shared `deliveryJsonSql`.
  Claim uses it at line 106 and recovery at line 342.
- [contract.ts:234–239](https://github.com/Richards-LLC/petra-stella-cloud/blob/a052e1f53a48635f9ea1f484e4cf4bfbc29adef6/lib/coordination/contract.ts#L234)
  declares these optional delivery fields.
- [Claim fixture](https://github.com/Richards-LLC/petra-stella-cloud/blob/a052e1f53a48635f9ea1f484e4cf4bfbc29adef6/tests/contracts/coordination/v1/violet_activity--claim_v2.json)
  puts `event_kind` on `deliveries[].delivery`.
- [Recovery fixture](https://github.com/Richards-LLC/petra-stella-cloud/blob/a052e1f53a48635f9ea1f484e4cf4bfbc29adef6/tests/contracts/coordination/v1/deliveries--recover.json)
  puts it on `admissions[].delivery`, including dispatch-uncertain entries.

## Cassy compatibility and wake policy

Cassy reads optional `item.delivery.event_kind` into `ClaimedDelivery.event_kind`.
Only the exact string `app_mention` emits `kind="mention" addressed="mention"`
and enqueues at High with explicit interrupt intent. Existing per-delivery
idempotency keys remain unchanged; replay cannot escalate the first wake.
Root and reply timestamps do not affect classification.

Missing, null, malformed and unknown values, `message`, and `message_changed`
retain `kind="delivery" addressed="thread"`, High priority and `urgent=false`.
Legacy v1 formatting and unrelated High-priority callers keep their behavior.
An uncertain prior dispatch stays a non-interrupting recovery notice.
No inference from Slack text, timestamps or author identity is used.

The existing Cassy regression now models the shipped Cloud field location,
through the real reader, body source and SQLite queue. The old requested-target
fixture passed locally but missed the shipped shape; the corrected delivery
fixture failed before the parser fix. Local evidence does not establish live
PTY interruption or the original incident's end-to-end latency.

## Original source boundary

At the pre-fix revision `d619807f456304c6a93eaaf5ee152ed251faad74`,
[occurrences.ts:130–133](https://github.com/Richards-LLC/petra-stella-cloud/blob/d619807f456304c6a93eaaf5ee152ed251faad74/lib/coordination/occurrences.ts#L130)
persisted `event_kind`, while
[claim selection/target serialization:106,133–138](https://github.com/Richards-LLC/petra-stella-cloud/blob/d619807f456304c6a93eaaf5ee152ed251faad74/lib/coordination/deliveries.ts#L106)
and [recovery:341,356–361](https://github.com/Richards-LLC/petra-stella-cloud/blob/d619807f456304c6a93eaaf5ee152ed251faad74/lib/coordination/deliveries.ts#L341)
omitted it. Cloud PR #204 resolves that missing metadata boundary.
