# Commander operator inbox: durable delivery across devices

Task: cas-4c78; epic: cas-7ec4. Status: **Phase 1 proposal, awaiting supervisor approval**.
Inspected 2026-10-05: cas-src b86ec0c2e; cloud local checkout
bd2266fbb34cd6be4cd828cc5e32f562b8f09569 (read-only, not a deployment receipt).

The operator can read a retained reply on every enrolled device after its hub
goes offline. Each device independently downloads and stores it, even when
another device has already read it. A command stored while its machine is
offline says **Pending machine** until that machine durably accepts it.

This proposal adds a ciphertext feed in petra-stella-cloud Neon and an outbox
in each project's existing prompt SQLite database. Direct terminal/control
continues through the hub. Existing shared session history and authenticated
live reply fan-out remain useful delivery paths.

## Evidence and implementation seams

At this source snapshot:

- `crates/cas-store/src/prompt_queue_store.rs:5161` shares conversation history
  across authenticated viewers; `cas-cli/src/hub/server.rs:2703` permits live
  reply fan-out. Neither supplies availability after the hub stops.
- `prompt_queue_store.rs:3429` owns a real IMMEDIATE enqueue transaction, but
  `enqueue_operator_message` at 3383 stamps operator provenance in a subsequent
  write. `cas-cli/src/mcp/tools/service/agent_search_system/message.rs:1324`
  similarly enqueues a reply before recipient/kind/attachment stamps. Adding an
  HTTP call after either write cannot supply atomic delivery.
- `queue_and_events.rs:4789` forwards already-persisted replies to viewers.
  Upload must run independently of viewer presence and that forwarding loop.
- The spike's [delivery model](../reports/cas-f01f/2026-10-05-commander-reliability.md)
  traces attachment lookup, verified transcript mirrors and watchdog notices.
  These operator-visible producers must also enter the atomic recording seam;
  arbitrary worker chatter remains outside this lane.
- Cloud `lib/auth.ts:21` validates account API credentials; its ordinary-user
  cache lasts five minutes. New relay grants require their own current
  revocation checks, rather than inheriting that cache. `drizzle/schema.ts:69`
  provides the existing `users.id` account authority. Transient pairing rows
  and existing device discovery rows are not the retained message store.

Independent read-only design exploration considered three interfaces:

| Proposal | Caller surface | Assessment |
| --- | --- | --- |
| Minimum interface | Atomic record; hub exchange; device exchange | Keeps the transaction and receipt rules together; role-specific exchanges must retain distinct typed receipts. |
| Maximum flexibility | Generic event/key/command operations | Extensible, but makes each caller assemble authority, epochs and ACK choreography. Reject as the public domain interface. |
| Common caller | Commit turn; browser ingest/replay/read; command submission | Makes the ordinary reply and recovery path simple; a browser-only multiplexer cannot repair producer atomicity. |

Choose atomic domain recording plus separate hub/device transport roles and a
browser inbox projection. Enrollment/key management is a separate security
module. Callers supply a turn and a verified audience; they do not manage raw
outbox CRUD. Removing these modules would put transactions, source merging,
retry identity and ACK rules back into the MCP, mirror, watchdog, daemon and
browser callers: they pass the deletion test. SQLite/IndexedDB are real local
test dependencies; daemon logic is in-process; the separately owned cloud is
an owned remote dependency behind a narrow injected transport.

## One account, explicit enrollment

One operator means one authenticated cloud account (`users.id`), with many
installations and hubs. It does not mean one label or one browser profile.
Neither `operator_label`, a caller's `account_id`, nor anonymous pairing
completion establishes membership.

Use an explicit account enrollment ceremony in addition to installation
pairing. Cloud account authentication authorizes a one-use, expiring challenge.
The browser proves possession of separate relay signing and encryption keys,
and explicitly consents to retained content, recovery and the selected machine
grants. An existing enrolled device approves the new key, or the operator
unlocks recovery material. Bootstrap creates the initial content authority on
the first device and pins it during this authenticated ceremony. No secret
account API key is copied into the browser's machine catalog.

Cloud issues a signed, audience-specific enrollment assertion bound to account,
hub, project grants, exact origin, installation signing-key thumbprint, relay
device ID, grant generation, content-authority key, key epoch, challenge and
expiry. The hub verifier checks the configured issuer key/audience, challenge,
signature and possession proof before persisting its server-owned account
binding. Challenges and generation fences prevent replay and substitution.
Changing a project to another account requires explicit detach/enroll; pending
rows keep their original audience. Git remote/project names never choose it.

Contract confirmed with proud-raven-98 (#3540134): preserve
`AccountEnrollment={state:'unenrolled'}` in
`cas-cli/src/hub/auth/installation.rs` and the browser
`installation-access.ts` until this verifier and persistence exist. Add an
enrolled variant only then. Credential/inventory responses expose safe status,
account ID, grant generation and epoch; no private keys. Direct credential
rotation can preserve a verified binding for the same installation identity.
A lost/replaced key requires explicit account re-enrollment; re-key changes the
epoch explicitly. No legacy device is merged or revoked by its label.

Cloud credentials are distinct from hub DPoP credentials:

| Principal | Bound resource and allowed operations |
| --- | --- |
| Machine | One account/hub and explicit project grants; upload events, inspect epoch policy, retrieve authorized commands, report machine receipts. No general account-feed read. |
| Enrolled account device | One account/origin/installation and current generation; replay, device-persisted receipts, shared read updates; separately granted machine/session command submission. |
| Account enrollment authority | Explicit add/revoke/recovery/key-policy consent. Neither a read grant nor a machine upload grant can enroll devices. |

Relay requests use short-lived proof-of-possession grants, exact relay audience,
method/path, canonical request binding, expiry and replay protection. Check
current grant generation/revocation on every page/mutation and renewed poll;
never accept a requested account over the authenticated account. Prefer finite
HTTPS replay polling compatible with Vercel; live hints are optional. Cloud ACKs
and local receipts are validated against the authenticated account, event ID
and envelope digest. Origin allowlists remain exact. Direct hub credentials
and hub DPoP proofs never go to cloud.

## Encryption, recovery and revocation

Proposed cryptographic profile: random per-event AES-256-GCM content key;
HPKE wraps using DHKEM(P-256, HKDF-SHA256), HKDF-SHA256 and AES-256-GCM
(`0x0010/0x0001/0x0002`). Use a maintained RFC 9180 implementation in Rust and
the browser, with interop vectors before choosing/pinning dependencies. Do not
implement an ad hoc ECDH envelope or reuse ECDSA installation keys as encryption
keys. HPKE base mode supplies encryption; separately sign producer envelopes
and content-authority manifests. [RFC 9180](https://www.rfc-editor.org/rfc/rfc9180.html)
defines these suites and explains recipient-key compromise/forward-secrecy
limits; this design makes no forward-secrecy claim.

Each epoch has a feed encryption key pair. Producers receive its signed public
key, so a machine can encrypt replies without receiving the account's history
decryption key. Enrolled devices receive the epoch private key wrapped to their
separate encryption public keys. A content-authority key signs epoch manifests,
device-key approvals and machine producer/command-key bindings; devices pin it
at enrollment and verify manifest generations. A read grant alone cannot
replace those keys. Management-capable devices hold explicitly delegated
authority, scoped to enrollment/key policy. Private material is protected by
device storage; export only inside authenticated wraps.

AEAD authenticates version, account, hub/project/session, immutable event ID,
epoch, content type and attachment identity. Producer signatures cover the
complete canonical sealed envelope, including wraps. Relay sequence is assigned
after sealing and is not fabricated as producer-authenticated metadata. Names,
message bodies, summaries, artifact references/URLs and filenames are inside
ciphertext. Cloud sees routing IDs, epoch, sequence, timestamps, sizes and
retention metadata. The relay never receives content keys or plaintext bodies.
Epoch policy signatures reduce key substitution; cloud authorization and the
hosted application remain trusted for initial enrollment and delivery.

**Recovery decision:** offer an explicit operator-held recovery kit during
bootstrap. A random 256-bit recovery secret encrypts the content-authority
recovery material and retained epoch private-key bundle; cloud stores only the
encrypted bundle, version and integrity metadata. The secret is displayed or
exported once with explicit save confirmation, never logged, uploaded or sent
through analytics. No human password-derived key in this first profile.
Authenticate the account again before recovery; the secret alone is not a
relay membership grant. Bundle AEAD binds account, authority fingerprint and
version with a fresh nonce on each update; signed policy versions fence stale
bundle rollback. Test substituted account/bundle/key and interrupted updates.

With all old devices and hubs offline, a new phone can authenticate the account,
unlock the recovery bundle locally, approve its new keys, receive its grant and
replay retained history. Test this with independent empty profiles. Recovery
bundle updates for new epochs must commit before those epochs become active.
If the operator declines recovery, old-device approval is required. If both
approved devices and recovery material are lost, old history is unrecoverable:
show **Recovery required** and allow an explicit destructive new-feed reset.
Account login alone never silently decrypts or resets old history. A recovery
kit holder can recover retained past epochs; disclose that authority.

Revocation immediately denies new relay operations, ends renewed subscriptions,
and atomically activates an epoch excluding the revoked key. If no authorized
device/recovery key can create that epoch, freeze new uploads pending key
rotation; do not pretend revocation alone removes a previously shared key.
Grant version and epoch transitions are serialized with append admission.
Hubs refresh signed policy before publishing. Known duplicates can return their
original storage receipt after rotation; novel old-epoch events are refused.
Already-downloaded keys/plaintext cannot be erased. Hosted JS or an unlocked
device compromise can read displayed content; ciphertext storage does not cure
those threats.

## Atomic local recording and bounded drain

Add an `operator_feed_binding` and `operator_delivery_outbox` to the same SQLite
database as `prompt_queue`. The binding carries verified account/hub/project,
generation and signed epoch policy. Recording an operator-visible turn uses
one IMMEDIATE transaction to write its complete prompt/provenance/recipient/
kind/attachment metadata and an immutable event ID, audience and outbox snapshot.
Signal local viewers only after commit. Generic operator enqueue, idempotent
notices and transcript mirrors must use the same internal helper.

The local outbox stores a frozen local payload snapshot/reference, event ID,
logical turn/revision identity, original audience, binding generation,
sealing state, stable ciphertext envelope/digest once sealed, retry state and
relay receipt. Local SQLite already contains plaintext prompt history; this
proposal does not claim encryption of that existing local database. Freezing
the payload prevents a later mutable prompt stamp from changing an event.
Do cryptography outside the SQLite write lock, then CAS the sealed bytes into
the existing row. Retries send those exact bytes. Missing epoch/key material
leaves a durable `awaiting_key` row, preserving the prompt commit. Network calls
never occur inside the recording transaction. Attachment snapshots must be
durable before their reference commits: use immutable hash-addressed staged
files with fsync/atomic rename, or same-transaction blob storage. Cleanup is
fenced by committed references; a mutable artifact path is insufficient.

Outbox claims use bounded leases so process crashes permit recovery. Drain
validates account/epoch, uploads a bounded batch, and records the matching relay
receipt transactionally. A lost ACK leaves the same event for retry. Never
delete an outbox row because a viewer or the local daemon acknowledged it.
Initial budgets proposed: 100 events/1 MiB per request, 64 KiB message envelope,
10-second whole-request deadline, jittered retry from 1 to 60 seconds. Offline
auth/revocation pauses its lane with a safe reason; unrelated local history
continues. Disk-full commit failure reports a storage failure rather than a
false durable success. No silent eviction of unsent events; expose backlog age
and byte count and allow an explicit operator purge.

After an epoch cutoff, reconcile each unacknowledged old envelope by event ID.
If retained, use its original receipt. If authoritatively absent and old-epoch
admission is already fenced off, reseal its frozen payload under the new epoch,
keeping its event ID. This requires the cloud owner to serialize cutoff/status/
append; otherwise an ACK-loss race could conflict with a resealed upload.
Never mutate retained ciphertext. Disabling upload preserves local outbox;
detaching an account cannot relabel its pending events as a new account's data.

## Neon feed, order and retention

Proposed cloud tables: `operator_accounts` (serialized feed counter and key
policy), account-device/machine grants, epoch manifests/wraps and encrypted
recovery bundles; `operator_events`; device cursors; conversation read
watermarks; command intake/authorization/receipts; encrypted attachment objects
and deletion jobs. Keep these separate from ephemeral pairing tables and
generic project/team entity sync.

`operator_events` holds account, hub, project, session, event ID, relay sequence,
sealed ciphertext/wraps, key epoch, ciphertext digest, created/expiry metadata.
Unique `(account_id,event_id)` returns the original receipt for byte-identical
retry; a conflicting digest returns `event_conflict`. One immutable event ID
per revision; encrypted `turn_id`, revision and causal links merge revisions
into one bubble. Existing local notification IDs remain provenance, qualified
by hub/project/session, and never become global identity.

**Order must follow commits.** Append locks an account counter row, checks
grant/epoch, resolves duplicate identity, increments the counter and inserts
the event in the same transaction. A bare Postgres sequence is insufficient:
A could reserve 10, B commit 11, a device advance to 11, then A commit 10 and be
skipped forever. Notifications follow commit; replay queries are authoritative.
Return sequence fields as decimal strings/BigInt-safe values. Across hubs this
is relay arrival order; causal/session fields govern conversation display.

Propose 30-day retained history and attachments, declared at enrollment;
commands expire after 24 hours unless reserved by their machine. Cloud owner
must approve cost/limits before implementation. Server policy bounds expiry;
a producer cannot request unlimited retention or re-date a delayed upload.
Retention deletes event
ciphertext, attachment ciphertext and obsolete wraps under bounded deletion
jobs. Object deletion needs retryable durable jobs; do not report completion
before blob deletion finishes. Shared blobs require account-scoped reference
accounting. Preserve a minimal ID/digest/sequence tombstone through the feed
generation so delayed retry cannot resurrect an expired event. Tombstones
contain no body and have a separately documented metadata lifetime.

Replay returns feed generation, retained floor/head, ordered page, continuation
and explicit expired intervals. A stale cursor produces **History expired**,
never an empty successful history. Mid-feed deletions need gap markers too.
Signed epoch policies and generation-aware reset prevent adopting a silently
lower cursor. Empty pages do not advance beyond events/deletions proven by the
response. Account deletion revokes grants and runs ciphertext/blob/wrap cleanup;
reset uses a new feed generation and never reuses old sequences/identities.

Browser caches apply expiry and remove owned plaintext/key caches on revoke,
sign-out or expiry when the browser next runs. A suspended browser and exported
plaintext cannot be remotely wiped. Existing published plaintext artifacts
remain governed by their earlier publication; the new feed cannot retroactively
make them encrypted. Copy future inbox attachments into encrypted, account-
authorized object storage with random per-object keys and authenticated chunk
metadata; encrypt references and filenames, enforce sizes and hash completion,
and make their decryption/read independent of the hub.

## Device replay, shared read and direct-path merging

Four facts stay separate:

| Fact | Durable authority | Meaning |
| --- | --- | --- |
| Relay-storage ACK | Neon event transaction | Cloud retained ciphertext; no device or machine execution claim. |
| Device-persisted ACK | Device IndexedDB commit, then relay receipt | This installation stored the event/page. |
| Device replay cursor | Per account/feed generation/device | Highest contiguous stored sequence or explicitly accepted expiry gap. |
| Shared operator read watermark | Account/conversation and sequence | Unread presentation across devices; never a replay filter. |

Use an IndexedDB transaction to merge decrypted turns and revisions, record
source identity/digests and advance the cursor together. Then emit the persisted
ACK. Failure/full storage keeps the cursor and ACK unchanged and shows the
storage error. Each profile has its own device ID and local cursor; clearing
storage/new installation cannot inherit another device's persistence receipt.
Parallel tabs share transactional updates and publish invalidation hints.

New live/history protocol fields carry event ID/turn revision plus qualified
legacy notification identity. Cloud, direct history and live sources ingest
through one projection; arrival before relay-sequence assignment is supported.
Dedup identity is account/event, not text or timestamp. A revision cannot
replace a newer revision; retain causal references across pages. Shared read is
per conversation `(account,hub,project,session)`, monotonic and clamped to
validated visible feed positions. Reading one machine must not mark an unrelated
machine conversation read. Receipt success or painting a bubble is not human
read. Changing shared read never advances any other device's replay cursor.

The account inbox loads independently of `MachineConnection`: cloud-connected
devices can read retained replies while direct hub health is offline. Show the
machine's connection state separately from inbox retention/delivery state.
Unuploaded local turns cannot be discovered while their machine is powered off;
only known local delivery state can say **Awaiting machine upload**. Do not
fabricate an unavailable history inventory from cloud silence.

## Offline-machine commands

Command permission is a separate device grant for an exact account/hub/project/
session and allowlisted operation. Preserve direct terminal/control and its
lease semantics; offline intake accepts structured operator messages only.
Generate a stable `command_id` once in transactional browser storage. Encrypt
the payload to the signed machine command public key and separately wrap it
for operator history. Neither a read grant nor a generic account token grants
command execution. Machine command keys must exist before offline submission.

Cloud intake validates current device/target grants and uniqueness, commits
ciphertext and returns `pending_machine`. Machines poll with their own scoped
principal. Before handoff, cloud atomically reserves a command for its exact
machine after checking expiry/current authorization and issues a stable signed
admission authorization. The browser still says Pending machine. Reserve is
not a machine acceptance receipt and does not allow another machine to execute.

The machine validates/decrypts that authorization and writes `command_id`,
frozen digest, verified operator attribution, one prompt/execution-queue row
and a machine-acceptance receipt outbox in the SAME project transaction.
Duplicate delivery returns that receipt without another queue row; changed
payload under the same ID conflicts. Only the uploaded durable machine receipt
changes the browser to **Accepted by machine**. Execution/start/result receipts
are separate. A crash after local admission but before its receipt upload
replays the existing receipt; a crash before commit admits on retry.

Cancellation/expiry succeeds only before irrevocable machine reservation.
After reservation return **Machine handoff in progress; cancellation not
confirmed**, even if its receipt is delayed. Reservation remains assigned to
the same machine until a terminal receipt; expiring a lease must not permit
cancel/success concurrently with a paused machine's valid admission. Grant
revocation denies future reservations; already-issued authorizations are
in-flight commands, disclosed explicitly. No silent reroute to a new session.

One durable queue admission is enforceable. Exactly-once arbitrary external
effects are not supplied by an inbox. Carry command identity to the daemon's
actual dispatch/receipt seam and test one intended command execution across
replay and crash schedules. Any executor with an ambiguous unacknowledged
effect reports **Execution not confirmed** rather than blindly running again.
Completion evidence must distinguish one queue admission, one observed daemon
dispatch and an external side effect; no cloud receipt implies any of them.

## Migration, ownership and delivery gates

Roll out behind explicit opt-in. Add backward-compatible SQLite schema and
optional live/history identity fields; old direct-only clients retain existing
behavior. Provision new cloud tables/routes/grants first, then verifier/enroll,
atomic producer outbox and drain, device replay, shared read, attachments and
command intake. A read-only inbox slice may land early, but cannot close this
task's command/enrollment acceptance criteria. No automatic historical upload.
Optional backfill needs explicit account/scope/time-range consent and stable
source identities under one atomic backfill marker transaction.

| Owner/repo | Work |
| --- | --- |
| cas-src / cas-4c78 | Shared store recording/migrations and identities; hub enrollment verifier; producer envelopes and bounded drain; command admission/receipt identity; browser inbox/key/cache integration and direct/cloud merge. |
| cas-src / cas-5e53 | Installation identity, staged credential rotation, un-enrolled hook; consume verified extension after agreed contract. |
| cas-src / cas-2b3a5 | Existing request deadlines/cause taxonomy/event diagnostics; reuse its transport conventions without concurrent connection.ts hunk ownership. |
| cas-src / cas-e6d2, when assigned | Atomic browser sends and direct device-persisted receipts; use this event/cursor/read contract. |
| petra-stella-cloud / supervisor-assigned owner | Enrollment account ceremony/issuer/grant verifier; Neon schema/counter/replay/read/commands; ciphertext attachments/recovery/wrap storage; revocation/retention jobs and SQL race/authorization tests. |
| Supervisor | Approve decisions, appoint cloud owner, protocol/crypto review, full Rust assembly and independent real-build QA. |

Phase 2 must have a named cloud owner and an agreed wire contract before any
cloud edits. Library choice, exact account authentication ceremony, issuer-key
rotation, retention cost and recovery consent copy require review before those
parts are enabled. They are implementation admission gates, not claims of
already-shipped capability. This Phase 1 changes documentation only; no cargo,
cloud mutations, hub credential changes or deployment.

Verification matrix for the eventual delivery:

| Requirement | Proof through real interfaces |
| --- | --- |
| Atomic event | SQLite fault/crash before and after prompt+outbox commit, including stamps, mirror/notices/idempotent producers; exactly one committed event. |
| ACK loss/idempotence/order | Cloud transaction tests for duplicate/conflict, reverse commit timing, epoch cutoff with unACKed event and command replay. |
| Account/revocation | Cross-account machine/device uploads/replay/receipts/commands denied; current grant checks and origin/key substitution refusal. |
| Encryption/recovery | Rust/browser vectors, metadata/wrap tamper refusal, no plaintext in DB/object/request/log snapshots, both old devices offline recovery, declined/lost recovery, epoch rotation. |
| Offline hub | Real changed hub uploads reply; stop hub after relay-storage ACK; two independent real browser profiles replay/store/render one bubble each. Protocol doubles alone do not meet this row. |
| Cursor/read | Storage-failure crash leaves cursor unchanged; shared read on desktop does not suppress phone replay; concurrent tabs/pages and direct/live/cloud duplicates converge. |
| Command | Offline command remains pending; real machine durable admission and receipt after return; replay/crash has one queue row and one observed dispatch, plus an explicit ambiguous-effect case. |
| Outage/expiry/attachments | Cloud outage preserves local rows/outbox; drain resumes; expired intervals explicit; ciphertext and blobs deleted with receipts; attachments read after hub stop. |
| Regression | Existing shared history, live fan-out, installation rotation, cause/deadline and direct command journeys remain green. |

Worker evidence uses scoped script/npm checks on clean commits; supervisor
owns Rust execution and assembly. Do not substitute source inspection for the
real two-profile/hub-stopped acceptance gate. Record exact source/build/cloud
revisions, measurements and uncertainties with each proof.
