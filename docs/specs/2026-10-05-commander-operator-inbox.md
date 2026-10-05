# Commander operator inbox: durable delivery across devices

Task: cas-4c78; epic: cas-7ec4. Status: **Phase 2a local recording/outbox
implemented, Rust proof pending supervisor assembly; Phase 2b contract adapter
and fake server authorized, production enrollment/transport not enabled**.
Supervisor decisions: #3540158, #3540204, #3540272 and #3540338.
Operator decisions: pure account-permission recovery, 90-day history and
attachments, 24-hour undelivered commands. Cloud owner: the
[petra-stella-cloud team, issue #125](https://github.com/Richards-LLC/petra-stella-cloud/issues/125).

Cloud wire version 1 is defined by
`docs/specs/2026-10-05-operator-inbox-cloud-contract.md` in that repository,
Unit 0 commit `5fd3f9a50dd4e9c20a126537b457847728a2ea64` on
`epic/durable-operator-inbox-for-commander-cloud-half-gh-cas-8a96`.
This is a contract receipt, not a deployed API receipt. Inspection of the
original implementation seams used cas-src `b86ec0c2e`; the cloud contract
inspected server source `8dbc59f`.

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
grants. Authenticated account permission authorizes enrollment even when every
older
device and hub is offline. Cloud custody supplies every retained epoch key to
the newly enrolled device through an authenticated device-specific wrap. No
recovery kit or approval from an older device is required. No secret
account API key is copied into the browser's machine catalog.

Cloud issues a signed, audience-specific enrollment assertion bound to account,
hub, project grants, exact origin, installation signing-key thumbprint, relay
device ID, grant generation, cloud issuer key ID, key epoch, challenge and
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
method/path, SHA-256 binding of the exact uncompressed request body bytes,
expiry and replay protection. Check
current grant generation/revocation on every page/mutation and renewed poll;
never accept a requested account over the authenticated account. Prefer finite
HTTPS replay polling compatible with Vercel; live hints are optional. Cloud ACKs
and local receipts are validated against the authenticated account, event ID
and envelope digest. Origin allowlists remain exact. Direct hub credentials
and hub DPoP proofs never go to cloud.

## Encryption, recovery and revocation

The accepted security model is **cloud custody, not strict end-to-end
encryption**. The cloud holds account epoch private keys under a KEK/KMS-style
envelope and can decrypt history. Account authentication plus explicit device
enrollment grants access; the operator accepted this trust boundary. There is
no operator-held recovery secret, recovery kit, device-only content authority
or requirement that an older enrolled device be online.

The cloud generates each P-256 HPKE epoch key pair, encrypts its private key
under its custody KEK, and signs the public epoch manifest and machine key
bindings with its issuer. Machines receive only the epoch public key. Enrolled
devices receive retained epoch private keys encrypted to their separate device
encryption keys (`/api/operator/keys/wraps`). These device-specific transport
wraps do not remove cloud custody. Account keys are not copied into the hub's
DPoP credential, machine upload grant or browser machine catalog.

The cloud contract pins HPKE DHKEM(P-256, HKDF-SHA256), HKDF-SHA256 and
AES-256-GCM (`0x0010/0x0001/0x0002`), `@hpke/core` 1.9.0 and Rust `hpke`
0.14.1 (`alloc,getrandom,nistp,aes`, default features off). Dependency
compatibility and Rust/browser interop remain supervisor proof gates before
adding or enabling crypto. Do not implement ad hoc ECDH or reuse ECDSA signing
keys for encryption. [RFC 9180](https://www.rfc-editor.org/rfc/rfc9180.html)
defines the profile; no forward-secrecy claim is made.

The encrypted event envelope binds version, account, hub/project/session,
immutable event identity, epoch and attachment identity. Names, bodies,
summaries, filenames and artifact references belong inside ciphertext. Cloud
sees routing IDs, epochs, sequences, timestamps, sizes and retention metadata.
Clients verify the producer signature against a cloud-signed machine binding;
the cloud treats the encrypted envelope as opaque and authorizes its routing
through the machine PoP grant. Relay sequence is assigned after sealing.
Device key wraps bind account, feed generation, epoch and device ID, with HPKE
info `psc-op-epoch-wrap-v1`. Substitution tests must cover each binding.

With every older device and machine offline, the operator authenticates the
account, approves enrollment of a new device and receives the retained keys
and history. Losing device keys requires re-enrollment, not a destructive feed
reset. An explicit account reset is a separate destructive operation, creates
a new feed generation and requires account management permission. Key custody
or issuer outage reports a safe refusal; it never invents an epoch or grants
access from a label.

Revocation denies subsequent relay operations through fresh grant-generation
checks and serializes epoch rotation with append admission. Cloud generates
the new epoch; it does not depend on an older device creating it. If custody
cannot rotate, uploads freeze with `503 uploads_frozen`; local recording stays
available. A retained duplicate returns its original receipt even under a
retired epoch; a novel retired-epoch event is rejected. Re-sealing requires
authoritative retained/absent reconciliation before changing bytes under the
same identity. Downloaded keys and plaintext cannot be remotely erased.
Hosted JS, cloud custody and unlocked devices remain trusted surfaces.

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
Contract budgets: 100 events/1 MiB per request, 64 KiB message envelope,
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
policy), account-device/machine grants, cloud-held encrypted epoch keys and device transport wraps; `operator_events`;
device cursors; conversation read
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

Retain history and attachments for **90 days**, declared at enrollment;
undelivered commands expire after **24 hours** unless reserved by their machine.
These are accepted operator decisions and fixed server policy. Server policy
bounds expiry;
a producer cannot request unlimited retention or re-date a delayed upload.
Retention deletes event
ciphertext, attachment ciphertext and obsolete wraps under bounded deletion
jobs. Object deletion needs retryable durable jobs; do not report completion
before blob deletion finishes. Shared blobs require account-scoped reference
accounting. Preserve a minimal ID/digest/sequence tombstone for **365 days after
expiry**, per cloud contract §9.2, so delayed retry during
that window cannot resurrect an expired event. Tombstones contain no body.
The finite tombstone horizon must be respected by client retry/purge policy;
it is not a lifetime deduplication promise.

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
authorized object storage with random per-object keys and authenticated envelope
metadata; v1 uses one ciphertext object up to 25 MiB. Encrypt references and
filenames, enforce sizes and hash completion,
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
| petra-stella-cloud team / cas-8a96 / #125 | Enrollment account ceremony/issuer/grant verifier; Neon schema/counter/replay/read/commands; ciphertext attachments/cloud custody/device wrap storage; revocation/retention jobs and SQL race/authorization tests. |
| Supervisor | Protocol/crypto review, full Rust assembly and independent real-build QA. |

The cloud owner and v1 contract are now named. The current authorization is
for a contract-shaped client adapter behind an injected transport and an
in-process fake server, with no live endpoint calls or automatic enrollment.
Production enabling still requires implemented cloud routes, issuer/grant
verification, custody/crypto interoperability, persisted authenticated audience
bindings and stable sealed outbox envelopes. Existing Phase 2a unenrolled rows
must not become account history through implicit backfill. Supervisor owns
Rust execution, assembly and the eventual real-build acceptance proof.

Verification matrix for the eventual delivery:

| Requirement | Proof through real interfaces |
| --- | --- |
| Atomic event | SQLite fault/crash before and after prompt+outbox commit, including stamps, mirror/notices/idempotent producers; exactly one committed event. |
| ACK loss/idempotence/order | Cloud transaction tests for duplicate/conflict, reverse commit timing, epoch cutoff with unACKed event and command replay. |
| Account/revocation | Cross-account machine/device uploads/replay/receipts/commands denied; current grant checks and origin/key substitution refusal. |
| Encryption/recovery | Rust/browser vectors, metadata/wrap tamper refusal, no plaintext in DB/object/request/log snapshots, account-only new-device recovery with all old devices offline, custody outage, epoch rotation. |
| Offline hub | Real changed hub uploads reply; stop hub after relay-storage ACK; two independent real browser profiles replay/store/render one bubble each. Protocol doubles alone do not meet this row. |
| Cursor/read | Storage-failure crash leaves cursor unchanged; shared read on desktop does not suppress phone replay; concurrent tabs/pages and direct/live/cloud duplicates converge. |
| Command | Offline command remains pending; real machine durable admission and receipt after return; replay/crash has one queue row and one observed dispatch, plus an explicit ambiguous-effect case. |
| Outage/expiry/attachments | Cloud outage preserves local rows/outbox; drain resumes; expired intervals explicit; ciphertext and blobs deleted with receipts; attachments read after hub stop. |
| Regression | Existing shared history, live fan-out, installation rotation, cause/deadline and direct command journeys remain green. |

## Phase 2a implementation boundary

The local implementation lives in
`crates/cas-store/src/prompt_queue_store/operator_delivery.rs` with migration
263. `record_operator_turn` commits the complete row, explicit reply ACK and
an immutable, frozen snapshot together. Paired-message wrappers, generic
operator enqueue compatibility, MCP replies, mirror suppression and watchdog
notices use that recording helper. Identity is a random 128-bit store-owned
event ID, with a unique prompt-to-event mapping. No old rows are backfilled.

Claims limit event count, total snapshot bytes and lease duration; token and
expiry checks fence stale completions/retries. A lost storage receipt schedules
bounded backoff; expired leases can be reclaimed after reopening. Snapshot
immutability is enforced by SQLite. Automatic prompt retention preserves
pending local events; an explicit clear atomically purges both local records.

Every Phase 2a event is explicitly **unenrolled**, including events with
verified operator display/provenance. Snapshots contain local plaintext and
attachment references, like existing prompt history. No account verifier,
encryption, ciphertext attachment copy, daemon drain scheduling, remote adapter
or user-visible cloud delivery state is enabled. The transport is exercised
only by an in-process fake. A future remote adapter must enforce authenticated
enrollment, sealing, whole-request deadlines and authenticated receipt binding.
Enrollment cannot silently repurpose pre-consent events as account history.
Terminal/control and the existing live/history wire protocol are unchanged.

The written SQLite tests cover failed insertion and failed completion rollback,
reopen after commit/before drain, lease expiry, stale ACK/retry, ACK loss,
duplicate drain, independent-connection claims, byte/count limits, frozen
metadata, retention and adapter dedupe. Existing MCP and watchdog regressions
also inspect their real handler's frozen event. Migration tests exercise schema
creation/detection/reconciliation; the migration bootstrap invariant includes
the new table. These are proof obligations until the supervisor executes them;
source inspection and syntax parsing do not establish runtime correctness.

Worker evidence uses scoped script/npm checks on clean commits; supervisor
owns Rust execution and assembly. Do not substitute source inspection for the
real two-profile/hub-stopped acceptance gate. Record exact source/build/cloud
revisions, measurements and uncertainties with each proof.

## Phase 2b contract adapter boundary

`cas-cli/src/hub/operator_inbox` adds role-specific `MachineRelay` and
`DeviceInbox` operations over an injected authenticated transport. It does not
supply a network implementation, enrollment, sealer, browser projection or
scheduler. Requests bind exact body bytes for PSC-PoP, carry no account IDs or
hub credentials, and have a ten-second whole-future deadline. Machine append
accepts only already-sealed stable envelopes and validates storage receipts.
Retained and expired acknowledgements remain different typed outcomes.

Device replay validates the feed generation and every covered sequence as an
event or an explicit non-overlapping interval before returning a page. Decimal
strings preserve values beyond JavaScript's safe integer range. Persistence
ACK, cursor write and human read write are separate methods; the caller must
commit its projection before acknowledging. No method silently moves a local
cursor or interprets an HTTP refusal as a successful empty feed. Routing uses
opaque IDs within the published format; human names are never sanitized into
identities. See the [client response](../requests/RESPONSE-operator-inbox-client-v1.md)
for unresolved cloud wire details and admission gates.

The in-process fake models account/role-scoped storage, lost storage responses,
idempotent retry, revocation, independent device persistence/cursors and shared
read state. Its bytes are opaque fixtures, not encrypted messages. Rust tests
are written but unexecuted under the supervisor's no-cargo instruction. The
existing Phase 2a `OperatorDeliveryTransport` cannot yet carry verified sealed
audiences or distinguish expired cloud receipts; no bridge maps these outcomes
onto its local fake storage receipt. That bridge requires a separately
approved durable binding/envelope migration, and cannot upload an unenrolled
snapshot or manufacture a retained receipt for expired history.
