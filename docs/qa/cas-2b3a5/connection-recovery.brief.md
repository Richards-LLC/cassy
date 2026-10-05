# Connection recovery: cause, evidence, action

The operator opens Details beside Reconnecting and learns which layer failed,
what the browser or hub measured, when Commander will retry, and what they can
do. The existing connection log becomes a bounded evidence ledger with a safe
JSON download. The defining move is the same cause flowing from the recovery
state into its explanation and export, followed by a recorded recovery.

Use Commander's existing semantic tokens, dialog, body typography and keyboard
focus behavior. Keep the readable cause and action above the technical ledger;
390px uses wrapped prose and a scrollable dialog. Use the same hierarchy in light
and dark. A ticking retry clock must not repeatedly announce the whole log.

The connection supervisor owns causes and a 64-transition in-memory ring; the
view owns labels and the download. No raw error, URL, label, prompt, credential,
key, pairing code or Tailscale account dump enters the export. Hub counts use at
most 64 route/status/preflight buckets and fixed refusal codes. Correlation IDs
are server UUIDs. Unsupported/unknown permission remains unknown; a denied
granular permission is measured evidence, not proof that it caused the request.

Whole-operation deadlines include signing, fetch and response bodies: reads,
pairing create/poll/ack and credential refresh 10s; catalog and stage deadlines
remain 3–5s; session launch 30s; SSE waits 60s between bytes (hub keepalive is
15s), teardown 1s. Timed-out mutations may have reached the hub. No general
request wrapper automatically resends mutations. Refresh rotation policy belongs
to cas-5e53. Pairing exchange/install persistence also belongs to that task;
this task does not race a partially installed credential against a new timer.

Events continue while one shared catalog request runs; a burst requests one
trailing refresh. Sequence/revision dedupe retains 1024 ids, accepts enriched
revisions, and resets for the hub process epoch. Explicit replay boundaries
prevent a retained-history replay from appearing as a live gap. Retention loss
refreshes current state; it cannot recover expired history. SSE broadcast lag
emits a marker and ends; multiplex lag emits a marker without dropping terminal
channels. Both cause retained replay/catalog resync instead of silent skipping.

SSE authenticates at open today; the heartbeat discovers subsequent refusal.
This change does not claim continuous SSE authorization or device delivery ACK.
Safe hub audit sampling records the first and powers of two of refused/preflight
buckets plus lag markers; raw requests are never recorded by this feature.

Verification: scoped Vitest, TypeScript, built-dist HUB-J12 diagnostics journey
(390/1280 light/dark, export, keyboard, reduced motion/forced colors/contrast).
Rust compilation, bounded-counter and hub SSE/WS execution are supervisor assembly
obligations: workers were explicitly instructed not to run Cargo. Protocol-double
browser receipts prove the built client, not a production hub or actual LNA denial.
