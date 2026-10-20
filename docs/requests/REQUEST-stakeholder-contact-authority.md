# Shared stakeholder contact authority (Cassy GH #1318)

Owner requested: Richards-LLC/petra-stella-cloud, with Violet as trusted effect
gateway. Status: proposed contract; **not a deployed capability**. Cassy task
cas-8882 implements the client and an explicitly local warning fallback.
Filed: [petra-stella-cloud#198](https://github.com/Richards-LLC/petra-stella-cloud/issues/198) (2026-10-20).

## Problem and boundary

Two supervisors on different machines can each obey an hourly reminder and
still notify the same person twice. Thread claims do not serialize new roots.
Cloud must transact on one key per authorized team, canonical project, Slack
workspace and stakeholder user. Channels share that key. Local timestamps and
presence heartbeats cannot grant authority. No new Cassy store is requested.

Cloud owns the effective policy, server clock, reservations and confirmed
last-contact record. Violet owns actual Slack dispatch, receipt verification and
settlement. Cassy reuses its authenticated Cloud coordination transport and
request journal. A local observation is labelled `local_only` and never imported
as authoritative server evidence.

## Proposed wire contract: `cassy.stakeholder-contacts/v1`

All paths start `/api/coordination/stakeholder-contacts`. Existing bearer and
linked-team-project authorization apply; labels supplied by a client confer no
authority. Error envelopes retain structured evidence, with no message contents.

### `GET /capabilities?project_id=...`

Advertise only after both Cloud transactions and trusted Violet settlement are
deployed and compatible:

```json
{"schema":"cassy.stakeholder-contacts/v1","atomic_reservations":true,"violet_settlement":true}
```

Absent, incompatible or unreachable capability means Cassy warns explicitly
that shared exclusion is unavailable and shows the latest local receipt (or
`unknown`). It permits the post. This is intentional partial delivery, not a
shared guarantee. Once capability succeeds, admission failure never silently
downgrades that attempt to a warning.

### `POST /reserve`

```json
{
  "schema":"cassy.stakeholder-contacts/v1",
  "project_id":"canonical-project",
  "registration_id":"authenticated-registration",
  "request_id":"10000000-0000-4000-8000-000000000001",
  "channel":"configured-channel-name-or-id",
  "payload_sha256":"sha256-of-canonical-violet-arguments-without-contact_authorization",
  "policies":[{"workspace_id":"T123","user_id":"U123","minimum_interval_secs":3600,"mode":"refuse"}]
}
```

`policies` identifies explicit user mentions in the final root text or file
comment. Cloud resolves the verified workspace/channel binding, normalizes keys,
and atomically reserves **every** applicable person or none. Violet recomputes
mentions and digest and must also enforce effective project policy for direct
clients that omit Cassy's fields. Upload `begin` and thread replies do not count;
external file `complete`, inline files and thread roots do. Empty `reply_to`
must not disguise a root. Block and attachment notification text must be checked
by Violet; Cassy's adapter currently handles the documented text/comment routes.

Cloud retains policy revision. Desired local configuration cannot create,
shorten, disable or change a shared policy without authorized operator policy
administration. A disagreement is `policy_conflict`. Implement an operator-only
policy upsert with revision CAS; its UI/API is owned by Cloud. Until a policy is
configured, admission returns `policy_conflict`, never an unprotected grant.

Grant:

```json
{"schema":"cassy.stakeholder-contacts/v1","code":"acquired","authorization":{"operation_id":"opaque-operation","token":"opaque-request-bound-grant"}}
```

Cassy passes `authorization` unchanged as `contact_authorization` on
`violet_post`, along with `request_id`. Tokens must be scoped, expiring and
bound to actor, project, channel, digest, people and effective policy revision.
Violet rejects an invalid/stale grant before Slack. An authorization is not a
publication receipt and never advances the contact timestamp.

Refusal (also required in warn-mode grant's `warnings`):

```json
{
  "schema":"cassy.stakeholder-contacts/v1",
  "code":"contact_too_recent",
  "message":"U123 was contacted at 2026-10-20T16:23:00Z by proud-cheetah-76; next allowed 2026-10-20T17:23:00Z; https://example.slack.com/archives/C123/p1792513395872369",
  "detail":{
    "last_contact_at":"2026-10-20T16:23:00Z",
    "last_poster":"proud-cheetah-76",
    "permalink":"https://example.slack.com/archives/C123/p1792513395872369",
    "next_allowed_at":"2026-10-20T17:23:00Z"
  }
}
```

Other stable codes: `contact_in_flight`, `outcome_unknown`, `policy_conflict`,
`idempotency_conflict`, `coordination_scope_required`, `authority_unavailable`.
Same ID and same inputs replay the same operation; changed inputs conflict.
Request IDs are UUIDs. A thread without explicit `request_id` derives a UUIDv8:
take the first 16 bytes of SHA-256 of compact JSON
`[schema,canonical_project_id,registration_id,idempotency_key]`, set the RFC variant
and version 8 bits. Other posts generate a UUIDv4. Cassy journals immutable
request inputs and returns the ID with unconfirmed-dispatch errors. Thread
ownership reconciliation never consumes these contact journal rows.

Unknown outcomes reconcile through existing `GET /api/coordination/operations/{request_id}`
with registration ID. A caller must reuse its ID and must not mint a new one
to bypass a pending operation. Cassy generates an ID when absent; explicit
`request_id` or thread `idempotency_key` is needed for deliberate replay.
An already settled reserve replay returns `code: "completed"` and its original
Violet `post_receipt` (`ok: true`); Cassy returns it without a second dispatch.

### Trusted Violet dispatch and settlement

Violet must durably register dispatch intent before Slack and settle against
Cloud once from a real Slack root receipt. Cloud stores actual provider time,
poster registration/machine, verified permalink and thread identity. A root
posted before a later reply fails still counts. No client-supplied success flag
or wall clock is sufficient. A definite pre-dispatch failure releases the
reservation without advancing contact time. An uncertain outcome stays pending
until reconciled; elapsed TTL or one missing Slack read is not proof of no send.
Warn mode permits a known interval violation, returns evidence and stamps the
new receipt; it never bypasses an unresolved dispatch reservation.

The trusted relay endpoints and auth must be specified and deployed by Cloud
and Violet together before advertising capability. Cassy does not emulate them
or write a last-contact timestamp to Cloud.

### `GET /?project_id=...`

```json
{"schema":"cassy.stakeholder-contacts/v1","code":"completed","observed_at":"2026-10-20T17:00:00Z","contacts":[{"workspace_id":"T123","user_id":"U123","last_contact_at":"2026-10-20T16:23:00Z","last_poster":"proud-cheetah-76","permalink":"https://example.slack.com/archives/C123/p1792513395872369","next_allowed_at":"2026-10-20T17:23:00Z","owner":{"resource_id":"existing-thread-resource","registration_id":"current-owner","generation":2},"pending":null}]}
```

`owner` is the existing verified thread claim for the last contact, distinct
from its historical poster and a pending reservation holder. Existing claim,
release, renew, offer-transfer and accept verbs retain ownership semantics.
Transfers never reset cooldown or rewrite poster. Do not introduce a second
stakeholder-owner workflow. With no existing claim, owner is null.

## Required upstream proof and rollout

Cloud must prove concurrent different-machine reservations against its actual
transactional store, all-or-none multi-person reservation, exact boundary,
receipt-only stamp, replay/conflict, strict/warn, failure/unknown recovery, and
transfer preserving cooldown. Violet must prove all notification-bearing root
routes, direct calls without Cassy metadata, root-success/reply-failure and
no stamp on upload staging. Supply Cloud-authored fixtures and deployed
capability evidence; Cassy's local doubles do not establish production safety.

Cassy's peers detail renders shared time/poster/permalink/next-allowed/owner;
failed reads remain `unavailable`, never "never contacted". Before capability,
it renders `local_warning` with machine-local evidence and explicit unknowns.

## Cassy configuration

```sh
cas config set slack.stakeholder_contacts '[{"workspace_id":"T123","user_id":"U123","minimum_interval_secs":3600,"mode":"refuse"}]'
cas violet post --channel dev --text '<@U123> review ready'
cas factory peers --detail
```

`mode` is `refuse` (default) or `warn`. An empty list disables this local
adapter; it must not disable Cloud's authoritative policy or Violet's direct
dispatch enforcement. Desired configuration is project-local; shared policy
administration remains operator-authorized in Cloud. Cassy includes degraded
or granted warn-mode evidence as `dispatch_warning` in JSON receipts and human
output. Local fallback evidence is derived from provider timestamps, never the
machine clock; other machines and uncertain sends remain unknown.

The current Violet schema documents `request_id` as managed-reply-only. Cassy's
CLI therefore requires `--reply-to` with `--request-id` and forwards it only
with a managed reply intent. Root reservation IDs in this proposed contract
are future capability-gated adapter metadata, not a supported top-level CLI
flag. Before rollout, use the existing thread `idempotency_key` for thread
reconciliation and the reservation receipt's ID only on the shared capability.
