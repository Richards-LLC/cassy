# Coordination contract fixtures, `cassy.coordination/v1`

These are the request and response fixtures for part (a) of spec-022d
([multi-supervisor coordination](../../../../../docs/specs/2026-10-16-multi-supervisor-coordination.md),
§3–§7, §11 and §12(a)). Part (a) covers one owner per Slack thread and routed
wakes. Cloud, Violet and Cassy implement and test against the same files.

## What is here

Cloud owns 15 files, vendored byte for byte from `Richards-LLC/petra-stella-cloud`
`tests/contracts/coordination/v1/` at `00d0675af` (petra-stella-cloud#161, PR #164; PR #179 added the
`strict_activation_required` claim refusal).
Cloud authored them and deployed the routes to production first, as §11
requires. Per Cloud's recorded decision, Cassy and Violet adopt them as they
are. [MANIFEST.json](MANIFEST.json) pins each file's sha256, and the Cassy test
refuses a local edit. To update them, re-vendor from a newer Cloud ref. A
breaking change means a new `v2/`.

| File | Surface | Route |
| --- | --- | --- |
| `external_work--claim.json` | `external_work.claim` | `POST /api/coordination/external-work/claim` |
| `external_work--renew.json` | `external_work.renew` | `POST /api/coordination/external-work/{id}/renew` |
| `external_work--release.json` | `external_work.release` | `POST /api/coordination/external-work/{id}/release` |
| `external_work--complete.json` | `external_work.complete` | `POST /api/coordination/external-work/{id}/complete` |
| `external_work--handoff_offer.json` | `external_work.handoff_offer` | `POST /api/coordination/external-work/{id}/handoffs` |
| `handoffs--accept.json` | `handoffs.accept` | `POST /api/coordination/handoffs/{id}/accept` |
| `operations--lookup.json` | `operations.lookup` | `GET /api/coordination/operations/{request_id}` |
| `activity--observe.json` | `activity.observe` | `POST /api/coordination/activity/observe` (trusted relay) |
| `violet_activity--claim_v2.json` | `violet_activity.claim_v2` | `POST /api/violet/activity/claim` with `schema` |
| `violet_activity--ack_v2.json` | `violet_activity.ack_v2` | `POST /api/violet/activity/ack` with `schema` |
| `deliveries--rebind.json` | `deliveries.rebind` | `POST /api/coordination/activity/deliveries/{id}/rebind` |
| `deliveries--receipts.json` | `deliveries.receipts` | `POST /api/coordination/activity/deliveries/{id}/receipts` |
| `deliveries--recover.json` | `deliveries.recover` | `GET /api/coordination/activity/deliveries` |
| `external_work--reply_intents.json` | `external_work.reply_intents` | `POST /api/coordination/external-work/{id}/reply-intents` |
| `reply_intents--dispatch.json` | `reply_intents.dispatch` | `POST /api/coordination/reply-intents/{id}/dispatch` (relay only) |

Cassy authored two files for the Violet tool surfaces (violet_ps#50). Cloud owns
neither of them. They use the same envelope, with `method: "MCP"` and the tool as
the `path`:

| File | Surface | Tool |
| --- | --- | --- |
| `violet_read--external_ref.json` | `violet_read.external_ref` | `violet_read` with `external_ref`: verified thread identity and an identity proof |
| `violet_post--managed_reply.json` | `violet_post.managed_reply` | `violet_post` with `coordination_operation_id`, `expected_generation` and `request_id` |

`captured/production-2026-10-16.json` holds read-only production probes, taken
before any fixture was trusted. No event, claim or lock was created or leased.
They show three things:

- The coordination routes answer in `cassy.coordination/v1` with the full
  receipt.
- The v1 activity claim and ack are unchanged: `{events, denied}` and
  `{results: [{id, status}]}`.
- The relay-only observe route refuses a user bearer.

## Envelope

Each file is `{contract, surface, method, path, spec, cases}`. Each case is
`{name, request: {payload, path_params?, query?}, response: {status, payload}}`.
A case may also carry `request_rejected_as: <code>`. A response payload carries
`schema`, `code`, `guidance`, an optional `reason`, and a `receipt` with
`operation_id`, `request_id`, `resource`, `visible_owner`, `generation`,
`expires_at`, `observed_at` and `next_action`.

## Stable codes

| Code | Caller behaviour (§6) |
| --- | --- |
| `acquired`, `completed` | Use the returned grant or receipt, only for its named inputs and generation |
| `held`, `rejected`, `stale_generation` | No authority. Show the visible owner, the reason and the remedy |
| `pending`, `outcome_unknown` | No new authority and no second effect. Reconcile the original request id |
| `authority_unavailable` | Fail closed for new work, replies and mutations. Keep local evidence |
| `idempotency_conflict` | The same request id was used with different input. Never retry with a new id |
| `coordination_scope_required` | The project is not a linked team project. Matching repository text grants nothing |
| `strict_activation_required` | A managed binding lacks evidenced strict activation. Lease nothing and type nothing; retry after an operator completes activation |
| `unsupported_version`, `invalid_request`, `not_found` | Fix the request. Authority mutations fail closed |

## Out of scope

Part (b) is not here: target operations, merge fencing, and the ownership view
(`/api/coordination/target-operations*`, `/api/coordination/ownership`). The
Cassy test refuses any fixture that names them.
