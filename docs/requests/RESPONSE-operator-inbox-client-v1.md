# Operator inbox v1: cas-src response to Unit 0

Task: cas-4c78; cloud owner: cas-8a96 / petra-stella-cloud team.
Contract inspected: cloud commit `5fd3f9a50dd4e9c20a126537b457847728a2ea64`,
`docs/specs/2026-10-05-operator-inbox-cloud-contract.md`, wire version 1.
Client source baseline: cas-src `b86ec0c2e`, Phase 2a `0c99c8712`.

This response is for supervisor forwarding to
[cloud issue #125](https://github.com/Richards-LLC/petra-stella-cloud/issues/125).
The worker has not posted to or changed the cloud repository. Supervisor message #3540407 owns consolidated outreach; the routing questions are already filed
in [cloud issue #126](https://github.com/Richards-LLC/petra-stella-cloud/issues/126)
per supervisor #3540416. The fake adapter is not a deployment receipt.

## Accepted decisions

Pure authenticated account permission authorizes new-device enrollment and
history recovery when every older device and machine is offline. The cloud
custodies KEK/envelope-wrapped content keys and can decrypt history; this is
explicitly not strict E2E. No recovery kit or older-device approval requirement.
History and attachments last 90 days; undelivered commands last 24 hours.
Device-specific HPKE transport wraps are compatible with cloud custody.

Machine append, device persistence ACK, device cursor and shared conversation
read remain distinct. Hub DPoP credentials never go to PSC. No account is
inferred from an operator label, readable project name or pairing completion.
Phase 2a snapshots remain explicitly unenrolled; enrolling must not implicitly
upload pre-consent history. Current work adds no live transport or crypto.

## Q1–Q10 answers

| Question | Client answer and evidence |
| --- | --- |
| Q1: event ID | 32 lowercase hexadecimal characters, generated from 128 random bits by `cas-store` in `prompt_queue_store/operator_delivery.rs`. The proposed 22–64 base64url-compatible alphabet accepts this unchanged. Prompt/notification row IDs are local provenance, qualified by hub/project/session, not global event IDs. |
| Q2: routing formats | New hub identities are UUID v4 (`hub/identity.rs:load_or_create`), but existing persisted IDs are only checked nonempty. Session names may include an arbitrary project basename (`ui/factory/session.rs:generate_session_name`); ASCII and 200-byte bounds are not guaranteed. Keep routing opaque: propose a persisted authenticated session routing ID, with readable name inside ciphertext. Do not truncate, normalize or sanitize names into colliding identities. Owner approval and mapping lifetime are pending; adapter accepts the published v1 alphabet and refuses unsupported inputs. Use the cloud-resolved canonical project ID, never a path basename or inferred label; enrollment must establish that mapping first. |
| Q3: issuer, audience, challenge | Agree with a configured trusted PSC issuer, its `/api/operator/jwks`, cache at most one hour, `aud=cas-hub:<hub_id>`, and a hub-issued one-use challenge lasting at most five minutes. Never trust an assertion-supplied arbitrary JWKS URL. Signature, typ, issuer/audience, exact origin, installation possession, challenge, expiry and generation/epoch must be checked before the hub persists enrollment. Fetch/rotation failure refuses enrollment; it does not replace keys from untrusted metadata. Verifier implementation is a later admission gate. |
| Q4: installation thumbprint | Yes: `installation_jkt` identifies the browser's hub DPoP installation key from cas-5e53. It is not the device's separate cloud relay signing key. Cloud echoes it in an assertion; the hub verifies the actual installation possession proof. |
| Q5: approval UX | Both the PSC-origin account-authenticated approval page and CLI approval are compatible with pure account recovery. Approval must require explicit retention/custody/scope consent. Neither route may require an older device to be online or place a raw PSC account API key in browser storage. These UX paths are not implemented in this adapter slice. |
| Q6: opaque envelope | Accept the server treating encrypted event envelopes as opaque, provided machine PoP authorizes routing and receiving clients verify AEAD routing/account/epoch bindings and producer signatures against issuer-authenticated machine bindings. `/principals` supplies signed bindings, but `sig_jkt` alone is not a public verification key. Define how the envelope supplies a signing JWK whose thumbprint matches the binding, and how historical bindings survive machine replacement/revocation for retained history. These are client-envelope interoperability gates, not claims of implemented producer verification. |
| Q7: command operation | Yes, `operator_message` only in v1. Terminal/control remains direct. Cloud storage/reservation never implies execution; machine queue admission and execution receipts remain distinct. No offline command adapter or intake is enabled in this slice. |
| Q8: attachments | Accept one ciphertext object up to 25 MiB in v1; no chunk transport API is required. Browser presigned private PUT needs CORS for exact Commander and explicitly configured development origins and required upload headers, without ambient cookies. Verify the provider before enabling; reject size/digest mismatch at completion. Names and references stay encrypted. |
| Q9: grant expiry | Accept sliding 90-day expiry and explicit account-authenticated re-enrollment after inactivity. Re-enrollment supplies still-retained keys/history without a recovery kit or old device. Device cursors stay installation-specific; shared read never suppresses replay. |
| Q10: Rust HPKE compatibility | **Not verified**. Current client slice adds no HPKE dependency. Existing Rust `p256`/`sha2` versions are not proof that the proposed `hpke` 0.14.1 feature graph compiles or interoperates. Supervisor owns Rust dependency/compile checks and bidirectional fixtures with `@hpke/core` 1.9.0 before crypto admission. Keep the suite and pinned libraries as contract targets, not shipped support. |

## Owner clarifications before production admission

1. **Read bounds after expiry.** §8.4 clamps a read mark to the highest live or
   tombstoned sequence in the same conversation; §9.2 removes project/session
   from tombstones. Owner must preserve qualified conversation maxima or
   sufficient routing metadata, or explicitly change the clamp rule. The fake
   retains routing for this purpose; it does not settle cloud retention policy.
2. **Session identity.** Approve or amend the opaque-ID mapping proposed in Q2.
   A hub/session-name mapping must survive restart and not merge two different
   sessions whose readable names collide. The adapter makes no assumption that
   the cloud owner has accepted this proposal.
3. **Key rotation reconciliation.** §7.2's row-level `epoch_retired` is proof of
   absence only if duplicate/tombstone resolution precedes epoch rejection,
   atomically under the account lock. §6.5 currently calls it HTTP 409, while
   §7.2 returns it inside HTTP 200 rows. Specify one consistent shape. Do not
   reseal after a whole-request ambiguity, timeout or `uploads_frozen`.
4. **Read-mark receipt shape.** §8.4 describes PUT as returning the stored mark
   but does not show its exact JSON shape. The fake currently returns
   `wire_version`, `feed_generation`, qualified conversation, `sequence`, with
   optional `updated_at`/`updated_by_device_id`. Confirm this before a live
   transport is added. GET pagination also needs its exact continuation shape.
5. **Producer verification over retention.** Resolve Q6's public signing-key
   discovery and historical bindings before accepting decrypted content.
   Cloud-issued offline observations need their own issuer principal; a
   stopped machine's signature must never be fabricated. Machine presence is
   owned by cas-1f90 and is outside the current feed grant.
6. **Finite deduplication horizon.** §9.2 tombstones expire 365 days after
   content expiry. A retained local retry older than that cannot rely on cloud
   deduplication. Client retry eligibility/purge needs an explicit terminal
   rule; the frozen local payload must never be re-dated implicitly.

## Adapter boundary and proof ownership

`cas-cli/src/hub/operator_inbox` implements typed append/replay/persistence ACK/
cursor/read requests against a separately injected authenticated transport.
It hashes exact body bytes, caps requests/responses, bounds the whole request
future to ten seconds, checks receipt identity/digest/epoch/generation and
validates complete replay coverage without expanding expiry intervals.
Unknown response keys are additive; outcome enums are closed. Malformed
coverage/receipts never authorize a cursor or local outbox change.

Only the in-process fake implements the transport. It uses account/role from
its test grant, not request content, and models loss of an append response,
retry, device isolation, current revocation and independent read/persistence
facts. This is **not** proof of PSC cryptography, Neon locking, IndexedDB
transactions, live authorization or the two-profile/hub-stopped journey.
Supervisor owns Rust execution; real cloud and browser acceptance remain open.
