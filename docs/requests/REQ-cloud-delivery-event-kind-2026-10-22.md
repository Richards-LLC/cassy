# Expose source event kind on managed activity delivery targets

Date: 2026-10-22. Cassy task: `cas-c28b6df4a2b653af`, from
`Richards-LLC/violet_ps#70`. Requested by the Cassy supervisor in message
542324. This file is the requested contract specification; the supervisor
owns relaying it to the Cloud and Violet teams. No Cloud deployment or issue
filing is claimed here.

## Requested additive response field

Add optional `target.event_kind: string | null` to **both**:

- `POST /api/violet/activity/claim`: `deliveries[].target.event_kind`.
- `GET /api/coordination/activity/deliveries`: each recovery admission's
  `target.event_kind`, for resume and recovery-notice entries alike.

The value must come from the persisted occurrence's `event_kind`, with its
existing spelling. For a direct Violet mention this is `"app_mention"`.
Do not infer it from timestamps, thread ownership, the fetched Slack body,
or the reader slot. If the original kind is unavailable, omit the field or
return `null`; never manufacture a mention.

Example delivery target (the surrounding delivery, lease and receipt fields
remain unchanged):

```json
{
  "target": {
    "resource_id": "6d1f0c3a-2b4e-4f6a-8c9d-0e1f2a3b4c5d",
    "project_id": "github.com/richards-llc/cassy",
    "external_ref": {
      "provider": "slack",
      "workspace_id": "T07ABCDEF",
      "channel_id": "C08COORD01",
      "root_ts": "1760616000.123456"
    },
    "message_ts": "1760616042.000200",
    "permalink": null,
    "event_kind": "app_mention"
  }
}
```

The same field must survive delivery recovery and reader rebind. Return
the source occurrence kind, not delivery status, route kind or consumption.
No Slack text or extra personal data is needed.

## Confirmed source boundary

Cloud source inspected at immutable revision
`d619807f456304c6a93eaaf5ee152ed251faad74`:

- [occurrences.ts:130–133](https://github.com/Richards-LLC/petra-stella-cloud/blob/d619807f456304c6a93eaaf5ee152ed251faad74/lib/coordination/occurrences.ts#L130)
  persists `occ.event_kind` in `activity_occurrences.event_kind`.
- [deliveries.ts:106](https://github.com/Richards-LLC/petra-stella-cloud/blob/d619807f456304c6a93eaaf5ee152ed251faad74/lib/coordination/deliveries.ts#L106)
  selects claim target identity and permalink without `o.event_kind`;
  [133–138](https://github.com/Richards-LLC/petra-stella-cloud/blob/d619807f456304c6a93eaaf5ee152ed251faad74/lib/coordination/deliveries.ts#L133)
  serializes that target without source kind.
- [deliveries.ts:341](https://github.com/Richards-LLC/petra-stella-cloud/blob/d619807f456304c6a93eaaf5ee152ed251faad74/lib/coordination/deliveries.ts#L341)
  and [356–361](https://github.com/Richards-LLC/petra-stella-cloud/blob/d619807f456304c6a93eaaf5ee152ed251faad74/lib/coordination/deliveries.ts#L356)
  omit it from the recovery projection and target serialization too.

Cassy's current vendored `violet_activity--claim_v2.json` agrees with this
shape. `violet_reader.rs::parse_delivery` retains the returned target;
`PromptQueueTyper::type_into` currently labels every managed item
`kind="delivery"`. The formatter consequently emits `addressed="thread"`.
Cloud does not directly supply that addressed label.

## Cassy compatibility and wake policy

Cassy will recognize only the exact string `target.event_kind="app_mention"`
as a managed direct mention: emit `kind="mention" addressed="mention"`,
enqueue at High with explicit interrupt intent, and retain the existing
per-delivery idempotency key. Root and reply timestamps do not affect this
classification. Queue ordering (High) alone does not grant interrupt intent.

When the field is absent, null, malformed or unrecognized, retain the current
`kind="delivery" addressed="thread"`, High priority and `urgent=false`.
Plain `message`/reply activity does likewise. Legacy v1 formatting and
unrelated High-priority queue callers keep their current behavior. A
recovery notice about uncertain prior typing remains a recovery notice and
does not reissue the original mention as a fresh interrupt.

The Cassy fixture test explicitly models this **requested, not yet shipped**
field. Its passing result is not proof of live Cloud support.

## Cloud acceptance and handoff

1. Observe an `app_mention`, then claim it: `target.event_kind` is
   `"app_mention"`. A root and a reply mention both retain that value.
2. Recover/rebind the same admission: the value remains available with the
   existing delivery identity and consumption semantics.
3. A plain message retains its original kind; missing historical information
   stays absent/null. Existing clients that ignore the additive field work.
4. Update claim and recovery contract fixtures together and provide their
   source/deployment revision to Cassy for a live-path verification.

Until Cloud supplies that evidence, production mention classification and
the reported incident's end-to-end latency remain unverified. Cassy's
forward-compatible parsing and atomic urgency/dedupe behavior can be proved
locally without inventing current production metadata.
