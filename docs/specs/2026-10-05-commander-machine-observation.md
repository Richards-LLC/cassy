# Commander boot recovery and independent machine observations

Status: design proposal for cas-1f90, before implementation. Local receipt work
is in cas-src; the cloud additions below require the cloud owner's contract
amendment. No cloud route or watchdog is implemented by this document.

## Evidence and ownership

The Prowl reboot/update incident is operator-reported, not reproduced. A machine
that stopped cannot send its own outage message. A factory heartbeat is not a
hub readiness check, and a live hub is not evidence that its factory survived.

- Commander base: b86ec0c2e. Service predecessor cas-d8e7:
  208b17080e35de184f552a1d3d51a44ae0741126, already in the supervisor's v35
  candidate. Its HANDOFF records Linux user systemd plus linger, macOS GUI
  LaunchAgent KeepAlive, Serve on by default, explicit host opt-out, owned-route
  repair, and bounded update recovery. This task consumes those behaviors;
  it does not rewrite the unit or plist.
- `hub/runtime.rs:16` has a process record; it has no OS boot identity or cloud
  enrollment. `cli/hub.rs` separately verifies loopback health, owned Serve
  configuration and public reachability. Preserve that separation.
- Cloud default branch inspected read-only at
  8dbc59fc61d3988a9746ab60e2baa0cf428706f3. Agent heartbeat
  `app/api/agents/[agentId]/heartbeat/route.ts:25` updates only an authenticated
  account's agent timestamp/status. `lib/agents.ts:38` marks agents stale after
  five minutes; its archive cron caller runs daily at 04:00 (`vercel.json`).
  Neither emits a machine outage or recovery into the operator inbox.
- Read the 1,482-line operator inbox contract at
  5fd3f9a50dd4e9c20a126537b457847728a2ea64, supplied by the cloud team's local
  commit. Its named epic ref was not published on origin when fetched;
  the exact commit was imported read-only into this task's isolated clone.
  It already defines account-scoped machine enrollment, separate PSC-PoP
  machine keys/grants, encrypted feed, device replay/ACKs and 90-day retention.
  It contains no machine lease, heartbeat capability or independent watchdog.
- cas-4c78 owns machine/device enrollment, relay signing/crypto and browser
  inbox. Cloud epic cas-8a96/#125 owns the remote implementation. cas-dae6
  retains Prowl's actual login/reboot persistence proof. Supervisor owns Rust
  compilation and runtime proof: explicit no-cargo direction.

## Chosen seam

Use a hub-owned `MachineObservation` module, independent of worker/factory
coordination. It gathers a fresh local snapshot and, only after explicit
account enrollment and monitoring consent, runs one bounded reporter. Cloud
owns lease expiry and durable transitions. Existing inbox replay carries the
notice to another enrolled device.

Caller-facing interface, illustrative until the cloud amendment is accepted:

```text
collect_runtime_receipt(runtime_sources) -> RuntimeReceipt
run_machine_observation(enrolled_machine, runtime_sources, relay, shutdown)
set_monitoring(account_authority, machine, enabled, policy) -> DecisionReceipt
```

Callers learn: enrolled machine identity; independent observations and their
age/source; explicit monitoring policy; cancellation state; accepted server
receipt. They do not implement timing, retry, process fencing, signing or
notification recipient discovery.

Hidden: OS boot discovery, local instance identity, bounded probes, sequence
and request persistence, epoch activation/reconciliation, monotonic scheduling,
coalescing/backoff, grant refresh, disable acknowledgement, and observation
freshness. Cloud hides serialized lease/outage transitions and its notification
outbox. Deleting the module puts these rules back into startup, status and
browser callers: the seam earns its implementation.

Three independent read-only proposals were compared under cas-codebase-design:
minimum three-entry interface; flexible typed component snapshots; common hub
and phone caller. Choose the minimum interface with typed observations. Reject
per-agent heartbeat reuse: wrong lifecycle and no timely durable notice. Reject
one lease per factory/session: request/notice multiplication and ambiguous
machine recovery. Reject browser-only polling: another closed browser cannot
observe or emit a durable outage. Do not build a second account enrollment or
notification transport alongside the accepted operator inbox contract.

## Identity and observation contract

Stable `hub_id` remains the existing protected machine UUID. Cloud `machine_id`
is its enrolled principal UUID, never a hostname/label match. `boot_id` identifies
an OS boot; a fresh `hub_instance_id` identifies each hub process. Unknown OS boot
identity is null/unsupported, never synthesized from process start time.
Linux uses the kernel boot UUID; macOS uses kernel boot-time evidence, scoped to
this hub so raw host boot metadata need not leave the machine.

A snapshot has independent values, each with `observed_at`, age, source and a
stable reason code:

| Dimension | What proves it | What it cannot prove |
| --- | --- | --- |
| Hub readiness | Fresh bounded loopback HTTP health and current process/lock identity | Serve reachability or factory health |
| Serve publication | Current owned-route receipt/configuration plus transport probe result | Another phone can reach the route |
| External reachability | Actual authenticated observer/device probe, if available | Factory work continues |
| Factory health | Registered sessions with fresh supervisor/daemon evidence | Jobs resume or preserve an in-flight turn after reboot |
| Monitoring | Cloud's current accepted lease and last receipt | Every component is healthy |

Healthy, failed, unknown, unsupported and intentionally disabled are distinct.
No metadata file or live PID alone becomes a healthy factory. Observation age
is visible; a stale healthy value becomes unknown. An ongoing machine heartbeat
must not indefinitely preserve old healthy component values.

## Supported boot topology and receipt

Linux: systemd user service enabled at the installed absolute binary, with
linger enabled before advertising reboot persistence. Record manager enabled,
active and linger facts, current hub identity/version/instance, fresh health,
Serve status and factory observations separately. A crash/restart simulation
uses an isolated unit and port; do not change the production unit, Serve routes,
login policy or unrelated processes during validation.

macOS: consume cas-d8e7's `gui/<uid>` LaunchAgent, RunAtLoad and KeepAlive,
with its captured PATH/TAILSCALE and reloaded ProgramArguments. This restores
service after the user's GUI login, not before login. Pre-login operation is
unsupported by that topology. Missing GUI Tailscale access is an explicit
publication failure while loopback may be ready; never claim public recovery
from launchd activity alone. Preserve correct namespace ownership and existing
owned-route refusal rules; no global Serve reset.

`cas hub status --json` should expose a read-only runtime receipt: schema/build,
probe time, OS boot and hub-instance identity, independent hub/publication/factory
facts, service/login prerequisites and remedies. Existing update
`hub_restart.loopback_verified` / `transport_verified` remain historical update
receipts, not perpetual readiness. Receipt creation never enables telemetry,
installs a service, restarts a hub or resumes a factory.

## Proposed cloud amendment: exact required delta

This section is an owner request, not a claim that these routes exist. Reuse
operator contract sections 3–5 account and machine PSC-PoP authorization; never
send hub DPoP credentials or use `/api/agents/.../heartbeat` as a machine lease.

1. Add opted-in machine capability `presence:report` and account-authority
   monitoring consent. Proposed account route:
   `PUT /api/operator/machines/{machine_id}/monitoring` with wire version,
   enabled flag, idempotent decision ID and expected monitoring generation.
   Resolve account/hub from authenticated principal; caller cannot supply an
   account ID. Machine grants lacking explicit capability stay unmonitored.
   Device `account:manage` or PSC account authority can disable while the machine
   is dead. Return accepted generation, policy and server receipt/deadline.
2. Add machine-only `POST /api/operator/machine/presence/activate` and
   `POST /api/operator/machine/presence`. Activation binds boot and instance to
   a server-issued reporter epoch with CAS against the previous epoch and
   idempotent request ID. Heartbeat binds epoch, increasing decimal-string
   sequence, fresh component observations and optional bounded silence intent.
   Exact duplicate returns the original receipt without extending the lease;
   stale epoch, conflicting sequence or disabled generation refuses. Delayed
   offline reports never renew presence or prove recovery. An expired reporter
   must reactivate from a fresh server challenge before recovery.
3. Add account/device `feed:read` snapshot route
   `GET /api/operator/machine-presence`: latest monitoring/observation state,
   server receipt/deadline, independent component values and open outage ID.
   Same account-only authorization as enrolled machine bindings; no public
   health directory, addresses, raw errors, prompts or process command lines.
4. Persist `operator_machine_presence` (account/machine PK, monitoring
   generation, reporter epoch, boot/instance, sequence/digest/receipt, lease
   expiry, bounded intent, observations, outage counter/open outage).
   Serialize heartbeat, watchdog, disable and revoke against that row. Add a
   durable transition/outbox uniqueness key `(account,machine,outage_epoch,kind)`.
   Disable/revoke cancels lease and unsent notices; old heartbeats cannot revive
   it. Report remote disable pending while offline; do not claim suppression
   before cloud acknowledgement.
5. Run a dedicated authenticated minute watchdog, not daily agent cleanup or
   hourly inbox retention. Proposed policy for agreement: heartbeat every60s
   with ±10s jitter; lease180s; notification grace60s; evaluator cadence≤60s.
   Advertised detection bound is last accepted report plus lease+grace+cadence
   (up to5min), plus delivery delay, only while evaluator is healthy. Cloud/server
   time owns deadlines; monotonic client scheduling skips missed ticks. A
   watchdog outage is visible as observer unavailable, not silently healthy.
6. In the lease transaction, open one durable machine-unobserved event per
   outage and later one recovery referencing it. A fresh lease may recover
   observation while Serve/factory remain degraded; retain those separate facts.
   Missed lease means unobserved, never proven powered-off or sleeping. Sleep,
   logout, reboot and maintenance are only acknowledged bounded intent; expiry
   resumes ordinary observation, never permanent suppression.
7. Extend the operator feed with a **machine-scoped, cloud-origin observation**
   envelope. Current §7 requires machine producer plus project/session routing;
   a dead producer cannot sign the observer notice. Do not impersonate its key
   or fabricate a project/session. Proposed contract amendment: explicit
   `event_scope: machine` and `producer_kind: cloud_observer`, account-bound
   machine/hub and outage IDs, nullable project/session for this scope only.
   Cloud seals with the active account epoch and signs an observer assertion
   using its existing issuer JWKS, distinct `typ=psc-op-machine-observation+jwt`.
   Client verifies account/machine/event digest/outage transition and issuer,
   then applies normal atomic feed persistence/ACKs. Machine append remains
   machine-only with existing project checks; it cannot choose observer origin.
   Serialize observer append under the existing account counter lock and reuse
   90-day retention, device cursor, ACK and key custody. Amend v1 only before
   both owners freeze/ship it; if v1 consumers already shipped, negotiate v2
   explicitly. No silent nullable routing change in a deployed closed contract.
8. Separate durable inbox storage, device-persisted ACK and external notification
   delivery. This task needs another enrolled device's replay while the source
   machine is absent; background OS push/email is not implied by a feed ACK.
   If background alerts are required, cloud owner must identify its authorized
   push/email delivery adapter and idempotency receipt. No generic Slack post.

Retention proposal: existing90-day notices; one latest observation snapshot per
monitored machine; discard raw heartbeat history rather than retain an activity
trail; minimal fencing/disable tombstone retained through grant expiry. Consent
names the account, observation fields, retention, detection delay and channel.
Disabling telemetry and account deletion remove snapshots/notices under normal
account retention rules; unsent delivery cancellation is fenced. A provider
submission already accepted cannot be recalled.

## Implementation and proof order

Phase A: implement the local read-only receipt/observation collection and
platform prerequisite reporting without cloud mutation; write source tests and
supervisor-run runtime fixtures. Consume d8e7 service behavior, no duplicate
service install implementation. Source may be developed on the Commander base;
supervisor assembles with the predecessor before platform proof.

Phase B: cloud owner approves/amends machine grant, lease/watchdog and observer
feed contract; cas-4c78 exposes its relay-signing/crypto seam. Then wire the hub
reporter and account-device UI to that exact contract. Until then independent
monitoring is unavailable; fake adapters prove client choreography only.

Required proof: isolated Linux manager crash/restart and current identity/health
receipt; actual macOS reboot/login and namespace/Serve/factory observation or
explicit unsupported pre-login condition; clock jumps/jitter/suspend/replay;
stale epochs; duplicate watchdog and heartbeat races; exactly one outage and
matching recovery after producer disappears; grant revoke/disable/account
isolation; other enrolled device persists the signed notice; keyboard/phone
status age and independent component rendering. Rust runtime belongs to the
supervisor under no-cargo. Do not enroll production machines, install services
or restart hosts as validation without operator authorization.
