# ADR: Commander browser-origin and hub security architecture

- **Status:** Accepted and binding for Commander H1, H2, H4, and H7
- **Date:** 2026-08-08
- **Decision owner:** Commander H0 (`cas-c798`)
- **Parent:** Commander control-plane epic (`cas-bec9`)
- **Supersedes:** No prior Commander security decision

This Markdown file is the normative architecture decision. The adjacent HTML file is a faithful presentation copy; if the two ever differ, this file governs.

## Decision

Commander is a browser-only viewer and control surface over one per-machine Cassy hub. It does not own agents, create model calls, or proxy a fleet through a central server.

Each browser profile designates exactly one paired hub as its **controller origin**. Commander is loaded from that origin, and that origin's browser storage contains the profile's machine catalog and device credentials. The browser connects directly from the controller origin to every other paired hub. Target hubs authorize that exact origin during pairing. Visiting a different hub origin creates a separate, empty browser security domain; catalogs and credentials are never silently copied, merged, or synchronized.

Every hub keeps machine identity, authorization state, and audit records in owner-only hub-local state. Browser control uses proof-of-possession device credentials and short-lived, single-use WebSocket tickets. Browser APIs and WebSocket upgrades enforce an exact allowed `Origin`. Plaintext control is restricted to loopback. A hub holds exactly one upstream daemon WebSocket per daemon/session and fans pane-tagged output out to any number of downstream viewers. At most one remote device holds the controller lease for a session.

These decisions are requirements, not implementation suggestions. An implementation may change internal types or storage engines only if every invariant and test in this ADR remains true.

## Context

The current factory daemon WebSocket is an internal loopback interface. It accepts mutating messages without authentication, broadcasts session state to all connected clients, and tags output by pane. The current bridge has a static bearer and optional CORS, but neither interface is a safe browser-facing fleet boundary. Commander adds that boundary without replacing the daemon protocol or the existing CLI/TUI.

The design must withstand:

- a malicious website opened beside Commander;
- an unprivileged local process or another local OS user;
- a hostile LAN or untrusted Wi-Fi;
- copied browser storage, a stolen device, or a compromised paired device;
- replay of pairing codes, HTTP proofs, or WebSocket tickets;
- XSS in Commander and CSRF/cross-site WebSocket hijacking;
- concurrent viewers and competing controllers;
- abrupt daemon death, including a signal such as `SIGILL`.

The host OS account and the controller origin are explicit trust boundaries. A process already executing as the Cassy account, or script execution inside the controller origin, can exercise that account's active authority. Commander reduces persistence, replay, and cross-origin exposure, but does not claim to isolate mutually hostile processes running under one Unix UID or to survive arbitrary same-origin code execution.

## Options considered

| Option | Browser catalog | Fleet traffic | Security and operating result | Decision |
|---|---|---|---|---|
| Per-hub origins with independent catalogs | Fragmented per origin | Browser to each hub | Simple, but users repeatedly pair and cannot get one coherent fleet view | Rejected |
| Hosted static Commander origin | Central browser origin | Browser to each hub | Rejected on 2026-08-08 for a new hosted availability/supply-chain boundary; **superseded 2026-08-10 as an optional, explicitly trusted static deployment** under the binding controls below | Opt-in only |
| Controller hub origin | One explicit hub origin per browser profile | Browser directly to each paired hub | One coherent catalog without a central proxy or new service; exact origin can be bound at pairing | **Selected** |
| Central hub proxy | Server-side fleet catalog | Browser through one hub | Makes one machine a privileged fleet control plane and stores remote session data there | Rejected |

## Consequences and reversal cost

The selected design keeps fleet metadata and credentials local to one browser origin, leaves hub-to-daemon traffic local to each machine, and gives every target hub an exact browser-origin policy. A controller hub outage prevents that browser profile from loading Commander, but does not stop daemons, CLI/TUI access, or already-running work. Users may deliberately choose another controller origin by re-pairing; there is no implicit migration.

Changing the controller origin invalidates its origin-scoped IndexedDB and every target hub's origin binding. Reversal therefore requires explicit device re-pairing and new credentials. That cost is intentional: an origin move is a security-domain move, not a cosmetic URL change.

## Amendment — 2026-08-10: optional hosted static controller origin (GH #211)

The controller-hub origin remains the default and selected deployment mode. A fleet operator MAY instead choose the HTTPS hosted static controller origin `https://hub.petrastella.io` (serving the reviewed `hub-web/dist/`) for a browser profile. This supersedes the earlier outright rejection because the t3code reference design has demonstrated the same static-SPA/direct-machine shape at `app.t3.codes`; it does **not** remove the hosted origin's availability or supply-chain boundary. Choosing it is an explicit, per-machine trust grant: every target must be paired with `cas hub pair --origin https://hub.petrastella.io`, and changing from a hub origin (or between hosted origins) is a security-domain move requiring revocation/re-pairing, never catalog or credential migration.

The following controls are normative for any hosted deployment:

1. The host serves static files only: no application API, redirect service, server-side session state, or credentials. Deploys are immutable; rollback redeploys a previously pinned artifact.
2. The live files are byte-identical to a reviewed `hub-web/dist/` from one named `cas-src` commit. A deploy record MUST name that commit, immutable artifact digest, deploy owner, and rollback artifact. The two Ghostty WASM files retain the SHA-256 hashes published in `hub-web/README.md`; CI verifies them before publishing. The cloud team consumes a versioned artifact named by commit + digest, not a rebuilt approximation.
3. The hosted response CSP MUST forbid third-party runtime, inline script, eval, forms, framing, and credential egress. It may allow `connect-src 'self' https: wss:` because a static CSP cannot enumerate a browser's dynamic paired-machine catalog; catalog membership is instead the application authority: all authenticated fetches/WebSockets are constructed only from a paired `StoredMachine.baseUrl`. The pairing ceremony may contact only an operator-entered target URL or the reviewed external pairing relay origin defined below. No telemetry, analytics, beacon, credential, DPoP, catalog, session, or pairing-relay request may be sent to the hosted origin after the static application loads.
4. Commander MUST fetch authenticated `GET /v1/machine` before enabling controls, compare `schema_version` and named capabilities, and show a visible version-skew banner for missing/unknown/newer capabilities. Unsupported controls remain disabled; it must never silently assume a capability from its own build version.
5. Exact-Origin hub authorization remains unchanged: pairing records the literal HTTPS origin, API preflight reflects it only when an active paired credential authorizes it, and authenticated API/WS requests bind the DPoP credential and ticket to that same origin. A hosted SPA is not a wildcard or a proxy.
6. Residual risk is accepted and revocable: compromise of the hosted host, DNS, or deploy authority can serve malicious same-origin JavaScript until detected. This is a paired-device compromise; operators revoke affected devices/origin bindings, roll back to a recorded artifact, and re-pair after recovery.

### CSP and origin review finding

Reviewed against the shipped implementation on 2026-08-10. `cas-cli/src/hub/server.rs` supplies restrictive hub-asset CSP headers and gates CORS preflight with `AuthStore::is_paired_origin`; `AuthStore::authorize` requires literal equality of the request Origin and stored `controller_origin`, and the pairing exchange repeats that equality. `hub-web/src/connection.ts` previously did not inspect `/v1/machine`; this amendment adds the required capability check and visible degraded state. No hosted-origin wildcard was found. The broad `https:`/`wss:` connection schemes are necessary for direct, dynamically paired hubs and are not treated as authorization; the catalog-bound construction and exact Origin checks above are the enforcement boundary.

### Amendment — 2026-08-13: page-initiated pairing relay boundary

The reviewed wire-v1 pairing relay is a narrow, external ceremony boundary at
`https://petra-stella-cloud.vercel.app`. The byte-identical Commander bundle
names that HTTPS origin in `hub-web/index.html`; changing it requires source
review and a new pinned `hub-web/dist/` artifact. Both the default embedded
controller-hub mode and optional `https://hub.petrastella.io` static mode send
only unauthenticated create, poll, and acknowledge requests under
`/api/hub/pairing/` to this relay, always with browser credentials omitted.
The relay applies exact-Origin CORS and must bind the declared
`controller_origin` to the request Origin. Missing, malformed, credentialed,
non-root, or non-HTTPS relay metadata disables page-initiated pairing; it must
never fall back to a same-origin route.

The relay does not receive DPoP credentials, the browser catalog, session or
pane data, hub control requests, or WebSocket traffic. After authorization it
delivers a short-lived one-time hub invitation; the browser exchanges that
invitation directly with the normalized target hub origin. All authenticated
`/v1/*` and WebSocket control remains browser-to-target-hub direct. The
controller hub and hosted static origin expose no relay API and must not proxy
this traffic.

## Normative trust boundaries and topology

1. A machine runs at most one Commander hub instance for its Cassy account. The hub is an authorization and multiplexing boundary; it is not an agent runtime.
2. The controller hub serves the static Commander application. It does not proxy target-hub control traffic, persist the remote machine catalog, or cache remote session data.
3. Page-initiated pairing alone may use the reviewed external relay for create, poll, and acknowledge. The controller hub and hosted static origin do not implement or proxy those routes; hub invitation exchange and control stay direct.
4. The browser profile stores its catalog and credentials only under the chosen controller origin. A newly visited origin starts empty and must be paired explicitly.
5. Each target hub records the exact controller origin as part of the paired device authorization. Scheme, host, and effective port must match; suffix, wildcard, substring, reflected-origin, and `null` matches are forbidden.
6. Optional discovery may provide endpoint hints only. Discovery cannot establish identity, add a machine to the trusted catalog, grant a scope, or bypass pairing.
7. Each hub opens exactly one Commander-owned upstream daemon WebSocket per daemon/session, independent of the number of viewers, windows, or panes. Downstream fan-out is performed inside the hub.
8. Commander must not cause a model request, create a second logical Cassy session for a viewed session, or import the t3 agent-owning server/runtime.

## Hub-local state

Hub security state lives under `~/.cas/hub/`, never in a project database or project checkout.

- `~/.cas/hub/` must be owned by the Cassy user and mode `0700`.
- Machine identity and private key material, paired-device records, token hashes, revocation state, and audit files must be owned by the Cassy user and mode `0600`.
- Creation and replacement must be atomic, must not follow symlinks, and must reject unexpected ownership, file type, or broader permissions. The hub must fail closed rather than repair ambiguous state silently.
- Secrets, raw pairing codes, raw WebSocket tickets, HTTP authorization headers, and DPoP proofs must never appear in logs, crash reports, URLs, project databases, or analytics.
- A different OS user is excluded by filesystem permissions. A malicious process already running as the Cassy user is considered an OS-account compromise and is outside Commander's isolation claim; its actions remain attributable where the OS exposes useful identity.

## Browser credential and storage model

The browser creates a non-extractable WebCrypto P-256 signing key for the controller origin. Pairing exchanges a one-time pairing capability for:

- an opaque device credential bound to the public-key thumbprint;
- a stable random device ID;
- an operator-supplied device label and operator label;
- the exact controller origin;
- an explicit scope set; and
- an issuance, last-use, expiry, and revocation record.

The private key, opaque credential, device metadata, and machine catalog live in IndexedDB under the controller origin. The private key must remain non-extractable. Long-lived credentials must not use `localStorage`, `sessionStorage`, cookies, URL query parameters, URL fragments, service-worker caches, export files, or browser sync. Catalog export, if later added, must omit credentials and trust state.

Device credentials expire after 90 days absolute or 30 days without successful use, whichever comes first. Expiry requires a new pairing ceremony; background renewal is forbidden. Revocation takes effect on the next HTTP request or WebSocket-ticket request and closes existing downstream connections for that credential. A copied IndexedDB credential without the non-extractable key is not sufficient to authenticate. A stolen unlocked browser or same-origin XSS is a compromised paired device and must be handled by revocation and audit, not described as prevented.

Browser API requests use `Authorization: DPoP <opaque-credential>` and a DPoP proof bound to the credential key. The proof must cover the exact method and normalized target URI, credential hash (`ath`), issued-at time, and a cryptographically random `jti`. Hubs accept at most 60 seconds of clock skew and cache accepted `jti` values for five minutes. Reuse, method/URI mismatch, key mismatch, expiry, or revocation fails closed.

## Origin, CORS, CSRF, CSP, and transport rules

### Origin and CORS

- Every browser API request, including WebSocket-ticket issuance, must carry an exact allowed `Origin`. Missing, opaque/`null`, malformed, or unpaired origins are rejected before authentication details or session existence are disclosed.
- CORS responses echo only the exact authorized origin and include `Vary: Origin`. `Access-Control-Allow-Origin: *` and `Access-Control-Allow-Credentials: true` are forbidden.
- Preflight permits only the minimum implemented methods and the `Authorization`, `DPoP`, and content-type headers. All mutations use non-simple methods with JSON bodies; GET and HEAD are read-only.
- Control authentication uses no cookies. SameSite settings are therefore not a CSRF control. Exact Origin checks, non-simple requests, DPoP, and one-time WebSocket tickets are the required CSRF and cross-site WebSocket protections.
- A non-browser local administration interface, if one is later required, must be a separately authenticated Unix-domain or loopback-only interface. It must not weaken browser Origin handling.

### Content Security Policy

Commander ships no third-party runtime code and permits neither inline executable script nor `eval`. The application response must enforce at least:

```text
default-src 'none';
script-src 'self';
style-src 'self';
img-src 'self' data:;
font-src 'self';
connect-src 'self' https: wss: http://127.0.0.1:* ws://127.0.0.1:*;
object-src 'none';
base-uri 'none';
frame-ancestors 'none';
form-action 'none';
worker-src 'none';
manifest-src 'self'
```

The broad secure-scheme allowance in `connect-src` is not an authorization grant: the browser client may initiate connections only to catalog entries, and every target hub independently enforces exact Origin, DPoP, and scope. Plaintext IPv6 loopback may be added only with an equivalently literal loopback source expression verified by browser tests; plaintext hostnames, LAN addresses, and wildcard subdomains are not allowed. Responses also send `Referrer-Policy: no-referrer`, `X-Content-Type-Options: nosniff`, and `Strict-Transport-Security: max-age=31536000` on TLS origins. `includeSubDomains` is omitted because a hub must not assert policy for DNS names it does not control. Framing is forbidden by both CSP and any legacy frame header retained for compatibility.

### Transport

Plaintext browser control is allowed only on literal loopback listeners. A hub must refuse to start a non-loopback HTTP or WebSocket control listener. Remote access must use verified TLS (`https`/`wss`) directly or place a loopback hub behind an explicitly supported, TLS-terminating loopback proxy such as `tailscale serve`. TLS 1.3 is the minimum permitted version for a directly terminating hub; a supported proxy must provide an equivalently browser-trusted TLS policy. Forwarded host, scheme, address, and identity headers are ignored unless the immediate peer is a configured loopback proxy; malformed or conflicting forwarded identity fails closed. Certificate errors, hostname mismatches, and user-interface bypasses are never accepted.

## Pairing and WebSocket capabilities

### Pairing

Pairing is a physical or already-authenticated local ceremony. The hub generates 32 random bytes with the operating-system CSPRNG and encodes them as unpadded base64url. The challenge is bound to the target hub identity, exact controller origin, requested maximum scopes, and creation time. It expires after ten minutes and is atomically consumed on the first successful exchange.

Only SHA-256 hashes of live pairing capabilities are stored; comparison is constant-time. A code may be entered manually or placed in a URL fragment, which is not sent in an HTTP request or referrer; the application must remove the fragment with `history.replaceState` before any network request or user navigation. It must never appear in a query string. Pair attempts are limited to five per source per minute and twenty per challenge per hour; a challenge is invalidated after ten failed attempts. The stricter limit wins, responses do not reveal whether a code, origin, or hub identity was the mismatching field, and all failures are audited without recording the code.

### WebSocket tickets

An authenticated DPoP request obtains a WebSocket ticket generated from 32 random bytes. The hub stores only its SHA-256 hash and binds it to the device credential, controller origin, granted scopes, target session, and intended connection endpoint. The ticket expires five minutes after issue, is atomically consumed by exactly one upgrade, and cannot be refreshed or replayed. The upgrade repeats the exact Origin check before ticket consumption. Query strings containing a ticket are redacted from access logs and diagnostics.

The WebSocket inherits the ticket's identity and scope ceiling. Revocation or credential expiry closes it. Reconnection always requires a new DPoP-authenticated ticket.

## Authorization scopes

Scopes are additive and least-privilege. Pairing displays and grants an explicit subset of this fixed vocabulary:

| Scope | Authority |
|---|---|
| `machine:read` | Read non-secret machine capability and readiness metadata after authentication |
| `session:read` | List authorized sessions and read lifecycle state |
| `pane:read` | Receive pane metadata, scrollback, and live pane output |
| `pane:input` | Send terminal input, focus, and resize messages while holding the controller lease |
| `message:send` | Send a semantic prompt/message while holding the controller lease |
| `pane:interrupt` | Send a targeted pane/worker interrupt while holding the controller lease |
| `factory:manage` | Spawn or shut down workers and perform other factory mutations while holding the target session lease |
| `hub:admin` | Pair/revoke devices, inspect security audit metadata, and force controller takeover |

The colon-form names above are the canonical authorization-policy vocabulary.
Commander JSON wire surfaces, including the hub API and relay wire-v1, encode
the same enum values in kebab form (`machine-read`, `session-read`, and so on).
That spelling boundary is a one-to-one serialization translation only; it does
not create aliases with different authority. Cassy accepts colon spellings at
operator-facing CLI input for compatibility, converts immediately to `Scope`,
and emits the kebab wire spelling. Commander displays the received wire spelling
without cosmetic rewriting.

The default pairing grant is read-only: `machine:read`, `session:read`, and `pane:read`. `pane:input`, `message:send`, `pane:interrupt`, `factory:manage`, and `hub:admin` each require explicit operator selection during pairing or a later local re-authorization. A scope never implies another scope. Legacy session-wide interrupt remains available for compatibility but is not exposed as `pane:interrupt`; targeted interrupt is an additive, separately named daemon protocol operation.

The UI must hide or disable unauthorized controls, but the hub is the enforcement point. Every session mutation requires both its action scope and the active session controller lease. Device administration is machine-scoped and does not require a session lease.

## Attribution and audit

Every accepted or denied mutation produces a durable hub-local audit record containing:

- UTC timestamp, hub machine ID, request/event ID, and outcome;
- device ID, credential ID, device label, operator label, and controller origin;
- verified proxy/tailnet or source metadata when available;
- required scope, action, and target machine/session/pane/worker identifiers; and
- sanitized byte count or digest when useful for correlation.

Audit records never contain raw terminal input, prompt text, output, credentials, proofs, tickets, or pairing codes. Device and operator attribution is preserved when a semantic message enters the prompt queue; remote input must not be recorded or rendered as though it came from the supervisor. Audit writes are ordered with authorization decisions and must make failed, denied, expired, replayed, takeover, disconnect, and revocation events distinguishable. Abrupt process death may truncate only the final in-flight record, not silently erase earlier events.

## Pre-authentication behavior

Before device authentication, a hub exposes only:

1. the immutable Commander application assets on its own origin;
2. a minimal health response containing protocol version and a boolean readiness value; and
3. the pairing exchange for an unexpired challenge already bound to the caller's exact controller origin.

Pre-authentication responses must not disclose hostnames, usernames, filesystem paths, machine labels, session IDs or names, pane or worker data, session counts, recent activity, existence of a requested session, paired devices, or scope grants. They cannot mutate factory/session state. Error shapes and timing must not intentionally distinguish unknown resources from unauthorized resources. No session WebSocket, event stream, scrollback, or state snapshot is available before a device credential and scoped ticket are validated.

## Viewer fan-out and controller arbitration

Any number of authenticated devices with read scopes may observe a session. Observation never acquires control. A session has at most one **remote controller lease** across every device and browser window:

- acquisition is explicit and compare-and-set;
- the lease lasts 30 seconds and is renewed by a heartbeat every 10 seconds;
- disconnect or missed renewal lets it expire; reconnect does not restore it automatically;
- voluntary release or transfer names the next controller explicitly;
- only `hub:admin` may force takeover before expiry; and
- acquire, renew failure, release, expiry, transfer, and forced takeover are audited and broadcast to viewers.

A browser device may hold multiple session leases, but a session cannot have multiple remote controllers. All remote session mutations, including input, resize, semantic message, interrupt, and factory management, require the active lease. Non-holders receive a typed denial that identifies the controller device label but does not expose secret identity data.

The local CLI/TUI is a privileged out-of-band controller. The first detected local mutating action preempts any remote lease and emits an audited controller-state event. H1 must make local mutation observable to the hub; the UI must never promise exclusivity while an unobservable local writer exists.

For each daemon/session, the hub maintains one upstream daemon WebSocket and one canonical session-state cache. It tags downstream events with session and pane identity, applies pane subscriptions locally, and never creates an upstream connection per pane or viewer. Each viewer has a bounded output queue. A slow viewer is disconnected or told to resynchronize from the canonical cache; it cannot block daemon reads, drop data for other viewers, or trigger another upstream. Reconnect rehydrates from `Welcome`/scrollback and then resumes ordered live events.

## Abrupt daemon death and honest diagnostics

An upstream close is not reported merely as “not running.” H1 must correlate the socket close with session metadata and process status, then emit a typed termination event containing the best available exit code, terminating signal, core-dump indication, last successful daemon event time, and a concise next diagnostic action. Unknown fields remain explicitly `unknown`; Commander must not invent a cause. `SIGILL`, `SIGSEGV`, operator termination, clean exit, lost transport, and indeterminate death must remain distinguishable when the OS provides evidence.

Diagnostics use existing process/session state only. They must not call a model, start a replacement agent, silently reconnect to a different session, or treat a local pre-release binary as evidence about a published artifact.

## Threat/control matrix

| Threat | Required control | Residual boundary |
|---|---|---|
| Malicious website | Exact Origin on APIs/upgrades, no cookies, non-simple requests, DPoP, single-use WS ticket | A fully compromised controller origin is trusted code execution |
| Local unprivileged user/process | `0700` directory, `0600` files, ownership/type/symlink checks | Same-UID code execution is OS-account compromise |
| LAN attacker | Refuse non-loopback plaintext; verified TLS; trusted loopback proxy only | Endpoint availability and traffic volume remain observable to the network |
| Copied browser storage | Non-extractable key binding, short credential lifetime, revocation | Stolen unlocked browser can use its live authority until revoked |
| Replay | One-time hashed capabilities, DPoP `jti` cache, short TTLs, atomic consume | Clock availability is required within the stated skew |
| XSS | Self-hosted assets, restrictive CSP, no inline/eval/third-party runtime, no secret export | Same-origin XSS is paired-device compromise and can use the non-extractable key in place |
| CSRF / cross-site WS hijacking | No auth cookies, exact Origin, DPoP, scoped single-use upgrade ticket | User-approved browser extensions are outside the web-origin boundary |
| Compromised paired device | Least scopes, expiry, revocation, controller lease, full attribution | Authorized actions before revocation remain valid and auditable |
| Concurrent controllers | One compare-and-set lease per session, heartbeat/expiry, explicit/admin takeover | Local CLI/TUI intentionally preempts remote control |
| Slow or excessive viewers | One upstream, bounded per-viewer queues, isolated resync/disconnect, rate limits | A hub may shed all remote viewers to protect the daemon |
| Daemon crash | Typed close correlation and honest unknowns | The OS may not preserve a precise cause after abrupt or external failure |

## Executable delivery invariants

These IDs are acceptance tests, not optional examples. H7 owns assembled-system execution; the implementation task named in each row owns the smallest automated test that proves its part.

| ID | Owner | Executable invariant |
|---|---|---|
| H1-ORIGIN-01 | H1 `cas-8057` | An unauthenticated or wrong-Origin HTTP/WS request receives no session data and cannot mutate state; loopback health reveals only version/readiness. |
| H1-TLS-02 | H1 | Starting plaintext control on a non-loopback address fails closed; loopback plaintext and a configured trusted TLS proxy path pass. |
| H1-MUX-03 | H1 | Two windows viewing three panes in one session produce exactly one Commander-owned daemon WS; pane-tagged output reaches only subscribed viewers. |
| H1-BP-04 | H1 | Saturating one viewer's queue disconnects/resyncs only that viewer without blocking upstream reads or opening another upstream. |
| H1-DEATH-05 | H1 | Fixture exits by zero, `SIGILL`, another signal, and unknown transport loss produce distinct typed diagnostics with no fabricated fields. |
| H1-ZERO-06 | H1 | Viewing/reconnecting does not change model-call count or logical Cassy session count. |
| H2-PERM-01 | H2 `cas-00b3` | Fresh state has `0700`/`0600`; loose mode, wrong owner/type, or symlink causes startup failure; no credential appears in project DB/logs. |
| H2-PAIR-02 | H2 | A 256-bit challenge succeeds once inside ten minutes only for its bound origin/scope ceiling; replay, expiry, origin mismatch, and rate-limit cases fail generically. |
| H2-DPOP-03 | H2 | Valid proof passes; reused `jti`, wrong key/URI/method/`ath`, expired/revoked credential, and excessive clock skew fail. |
| H2-WS-04 | H2 | A 256-bit WS ticket works once inside five minutes only for its device/origin/session/endpoint; concurrent replay yields exactly one winner. |
| H2-SCOPE-05 | H2 | Every action is denied without its exact scope; session mutations are also denied without the active lease; targeted interrupt never degrades to legacy global interrupt. |
| H2-AUDIT-06 | H2 | Accepted/denied mutations, replay, expiry, revocation, and takeover have device/operator/origin attribution and contain no raw secrets or content. |
| H4-CATALOG-01 | H4 `cas-1e82` | A second origin starts with an empty catalog; choosing it as controller requires re-pairing; discovery hints never become trusted entries automatically. |
| H4-STORAGE-02 | H4 | Long-lived secrets are absent from cookies, Web Storage, URLs, caches, exports, and browser sync; signing key is non-extractable IndexedDB material. |
| H4-CSP-03 | H4 | Browser policy tests reject inline/eval/third-party script, framing, forms, and non-loopback plaintext connection targets. |
| H4-LEASE-04 | H4 | Two devices see the same controller identity; only one has enabled controls; expiry, release, transfer, local preemption, and admin takeover update both views. |
| H4-CONN-05 | H4 | Multiple panes and routes reuse one downstream connection per target hub rather than opening pane-specific connections. |
| H7-ADV-01 | H7 `cas-3d85` | Browser adversarial suite covers malicious Origin, preflight, CSRF, cross-site WS, replay, stolen/copied state, and revoked credentials. |
| H7-FLEET-02 | H7 | Two real machines plus a phone-class browser prove pairing, direct multi-hub view, observer/controller arbitration, TLS-only remote access, and audit attribution. |
| H7-INVARIANT-03 | H7 | End-to-end instrumentation proves one upstream per daemon/session, correct pane fan-out/backpressure, unchanged model-call count, and unchanged logical session count. |
| H7-CRASH-04 | H7 | Abrupt daemon termination, including `SIGILL` where supported, produces honest actionable diagnostics and leaves other sessions/viewers operational. |

H1, H2, H4, and H7 must cite this ADR in their implementation or verification receipt. A test that substitutes a local pre-release build for the artifact under test does not satisfy an invariant about a published artifact.

## Explicit non-goals

- Commander does not schedule agents, own prompts, add model calls, or replace factory task coordination.
- Commander does not provide hostile-process isolation within one Unix account.
- Commander does not silently synchronize credentials or catalogs between browser profiles or origins.
- Commander does not make the controller hub a fleet proxy or a store for remote session history.
- Commander does not weaken or remove the legacy interrupt operation; targeted interrupt is additive.

There is no unresolved P0 security choice in this ADR. Future implementation details may be selected only inside the fixed trust boundaries, lifetimes, algorithms, scopes, topology, and executable invariants above. Any proposed relaxation requires a superseding ADR and review before implementation.

## Evidence and provenance

This decision is grounded in:

- `docs/research/2026-08-07-cas-gui-options.html`, which selects the per-machine hub plus thin web client and requires zero added model calls;
- `docs/research/2026-08-07-t3code-gui-control-plane-review.md`, which documents browser-origin pairing, proof-bound credentials, short-lived WS tickets, and the boundary between a control client and an agent-owning runtime;
- the existing factory daemon protocol and WebSocket implementation under `cas-cli/src/ui/factory/daemon/` and `cas-cli/src/ui/factory/protocol.rs`; and
- the current bridge and machine session-state implementations under `cas-cli/src/bridge/` and `cas-cli/src/ui/factory/session.rs`.
