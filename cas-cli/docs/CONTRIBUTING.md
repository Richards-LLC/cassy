# Contributing to Cassy

## Managed Slack file replies

`artifact action=post reply_to=...` and `cas violet post --reply-to ... --file ...`
resolve the caller's live reply intent from the project's coordination journal.
Both external-upload steps carry its operation ID, claim generation and one
request UUID. The CLI's `--request-id` reuses that UUID for the same reply.
An expired, released or absent caller intent on a locally known managed thread
fails before contacting Violet. Unknown and unmanaged threads retain Violet's
ordinary posting policy; Violet owns authoritative checks before allocation,
including authority changes on other machines. Never retry an uncertain reply
with a new request ID.

## Memory update receipts

`memory action=update id=<id> title=<title>` saves the title alone or together
with `content`. Omitted fields retain their stored values. A successful edit
returns `structured_content` with `id` and `changes` (the updated field names),
alongside the existing text receipt.

## Native Codex verifier identity

An MCP caller belongs to its connection. Native Codex children share the
parent's connection; choosing a role or receiving a dispatch ID does not create
independent reviewer authority. `coordination whoami` shows the caller's type,
role and parent. A shared-connection child registration fails before mutation
with `rule_id=shared_mcp_child_identity`.

Use the daemon-owned candidate verifier or a separately registered reviewer
with a sealed handoff for the exact dispatch. A native child that observes its
parent's identity returns that evidence without recording a verdict, including
`status=error`. A registered supervisor may perform its own review and record
the verdict directly; its provenance remains `supervisor_direct`.

## Commander message and terminal permissions

`MessageQueued.operator_label` additively carries the paired person's name to
the sender; live/history rows use the same stored operator name. Older peers
without it keep `device_label` as their display fallback. Input leases stop
renewing and release when their conversation or raw-output view is left.
Authenticated departure cleanup is idempotent: releasing an absent or foreign
lease succeeds without removing another device's control or revoking pairing.

A supervisor message requires `message:send`; it does not acquire or require a
pane input lease. A message-only pairing can therefore send while another
paired device controls the terminal. Raw terminal input still requires both
`pane:input` and that device's active session lease. Message permission never
grants terminal input authority.

## Selected-session stores and live phase freshness

Bridge and hub session routes resolve the selected session's `project_dir`
with `find_cas_root_ignoring_env`. A process-wide `CAS_ROOT` can belong to the
project that launched a multi-project hub, so it cannot select another session's
stores. This applies to inbox peek/poll/ack/count, ping, activity, messages,
status and SSE, and artifact URLs. The bridge's explicit `--cas-root` is a
fallback only when session metadata has no resolvable project; without either,
resolution fails. This is a shared session-store policy, not a phase-only override.

Commander publishes completed status reads in request order for each
machine/session: older successes and failures cannot replace a newer settled
read. An available response can still publish while the next read is pending,
so reads slower than the heartbeat do not starve status updates. Native compaction starts expire
after five minutes without a newer lifecycle, including a cancelled compaction
whose completion hook never fires. Expiry drops the phase without claiming
completion or reviving a tool that preceded it. Later native evidence, including
a real completion boundary, remains visible normally.

## Staffing behind a published release

Worker spawn and task assignment receipts include `EPIC_BEHIND_RELEASE` when
the subject epic lacks commits from the latest recorded published release.
The advisory names the tag and missing commit count, and asks the supervisor
to sync the integration branch into the epic first. It does not refuse work or
move Git refs. Publication comes from the target repository's Release ledger;
a tag alone is not publication evidence. Missing receipts or unavailable refs
produce no warning. An origin-only epic tip is usable; when both refs exist, a fresher descendant origin tip is considered before warning.

## Worker contracts across machines

`coordination action=message target=<worker>@<machine>` can carry an advisory
contract between live workers of the same account and repository holding active
claims under a common epic. Send and receive both validate Cloud identity and
claims against synced task ancestry. Missing evidence fails closed; there is no
worker succession redirect or cross-project worker route. Receive records quiet,
idempotent copies for both supervisors before acknowledging the original mail.
Only untyped, ask, answer and receipt intent is allowed; urgency, blocker/merge
escalation and supervisor action kinds remain unavailable. Explicit `wake=false`
stays quiet. Replies bind to a peer message delivered to that worker and recheck
the current scope. This uses the existing MCP coordination dispatch; the factory
message CLI retains its local queue behavior.

## Repaired peer approvals

The authenticated peer receipt path can supersede a stale admitted peer approval
when the same source supervisor and machine supplies a later, independently
approved dispatch for the current repaired delivery. Admission rechecks project,
plan, candidate tree and published Git identity. One store transaction invalidates
only the stale peer dispatch, inserts the new approval and records
`peer_premerge_approval_replaced`; both verdicts and the old proof remain readable.
Local verification cycles cannot be replaced by peer mail. Replaying old approval
with a changed source SHA does not grant authority.

## Automatic supervisor replies

Completed supervisor replies are mirrored to local operator conversation history
by default, including before the operator sends a first message and after the
supervisor becomes idle. This local recording is independent of cloud enrollment.
To opt out, run `cas config set factory.commander_mirror_enabled false`.
Setting it back to `true` starts a new capture window; replies from the disabled
period are not replayed, even if the daemon was stopped during the toggle.
This key is operator-only and defaults to `true`. It controls automatic
supervisor reply capture; terminal questions and explicit messages addressed
to the operator continue to be recorded. Existing conversation history is retained.

Synthetic Codex supervisors bind only to a rollout carrying their own queue
input, including an ownership envelope older than the 4 MiB reply tail. A first miss
retries after one second, doubling up to 30 seconds; ownership reads resume at
the last complete JSONL row rather than rescanning unchanged file bodies.
A changed harness/account/session or startup envelope invalidates that wait. Finalized sends without delivery proof retain Queued and
use a 30-second receipt watch; active pending sends keep the one-second watch.
A later actual receipt still publishes Delivered.

Commander text waits at confirmation/trust dialogs and at non-Claude in-flight
tool calls. Dialog footers must match a complete native row below the composer;
assistant prose above it never holds a send. Claude's visible native composer remains eligible during working
turns. Held text and its submit are delivered together after that boundary clears.

Each completed native turn creates one visible answer, bound to its latest
verified Commander send; every consumed send still receives its delivery receipt.
Standalone machine notifications create status replies when no operator input is
pending. Only operator envelopes beginning a line can supply reply bindings.

Pending Commander replies retry after one second, doubling to a sixty-second
cap while a paired-device receipt is missing. A valid receipt, a new daemon
WebSocket connection, or a device's first history-page request resets that retry
schedule. Each scheduled retry logs its next delay; intervening daemon passes
neither rebroadcast the reply nor stamp it selected again.

## Commander questions with quick replies

A supervisor can send a question with choices using `coordination action=message
 target=operator kind=ask summary="Choose next step" message="Keep testing?"
 options=["Keep testing","Stop testing"]`. `options` is an optional array of
non-empty labels, allowed only with `target=operator kind=ask`. Labels are
trimmed before storage. The operator's chosen label returns as the message
text with `in_reply_to` identifying the original question.

The CLI equivalent is:

```sh
cas factory message --session <session> --target operator --from supervisor \
  --kind ask --message 'Keep testing?' \
  --option 'Keep testing' --option 'Stop testing'
```

The choices are stored with the operator turn and carried by live replies and
history pages. Empty choices are omitted from the wire; older payloads decode
as an empty list and older clients ignore the added field. Reloading before
an answer preserves the choices; after answering it preserves the selected
label and the durable reply reference.

## Supervisor succession and committed-work recovery

Factory registrations belong to their live harness process. A nested review
that inherits `CAS_SESSION_ID`, name and role cannot re-register that identity
from a different process, publish a socket mapping for it, or apply lifecycle
hooks to its owner. Read-only guidance and receipt identity lookups apply the
same owner check before caching an identity or authorizing later tools.
Replacement MCP servers under the same fingerprinted harness
remain valid. The authenticated cached owner repairs an exited PID left by older
nested registrations before its next heartbeat, preserving its durable session.
Tool permission hooks always retain their policy checks, even for a nested
caller. A live fingerprinted MCP child can prove its current parent when an
older registration lacks a parent fingerprint; an exited registration remains
replaceable after the host's name changes. A matching live owner authenticates
before the hostname comparison, so its hooks and reconnecting MCP can refresh
the stored hostname.

Worker liveness uses concrete turn and process evidence. While a recycled
worker's transcript is unresolved, its own recent activity or outbound message
to the supervisor/director can establish execution within the configured stall
window. Heartbeats and other workers' activity cannot. Resolved terminal turns
and confirmed process exits retain precedence over this fallback.

Supervisor memory handoffs carry a writer-assigned `handoff-machine:` tag.
Supersession, SessionStart and automatic-clear judgement select by project
store, machine and role. A local successor can change its name and factory
session without losing its predecessor's handoff. `handoff-owner:` and author
session provenance are audit stamps, never selection keys. Registry restart
reconciliation cannot replace a same-name supervisor on another machine.
Legacy handoffs without a machine stamp remain readable through memory tools
but cannot inject approvals into startup.

Memory updates discard caller-supplied `handoff-machine:` and `handoff-owner:`
values and retain existing stamps when replacing ordinary tags. Removing and
restoring the `handoff` tag cannot change that lineage. When an ordinary entry
without existing stamps becomes a handoff, its authenticated writer supplies
the machine, owner and default role; editing an unstamped legacy handoff does
not mint new provenance.

Supervisor retirement keeps a project/host-scoped handoff before clearing registry
ownership. The next local supervisor receives its tasks, worker registrations,
parked refs/PRs, pending messages and asks at startup, with a full JSON receipt in
`.cas/supervisor-handoffs/`. Later successors retain fresh task/worker rows for
unadopted inherited lane IDs and references to earlier receipts. Receiving evidence preserves live worker leases;
explicitly adopt the listed epics before dispatching more work. `worker_status`
and text `coordination action=peers` show recent session-end reasons locally.
An epic's live `epic_verification_owner` can recover its stranded workers when
local session metadata and the daemon's PID/start fingerprint confirm the original
daemon is dead. Worker `target=supervisor` follows that owner. Recovery messages
reach the original Claude team inbox and surface at the worker's next turn or tool
boundary; a dead native daemon cannot wake an idle terminal. Reset and shutdown
retain the live-session ownership guard, and shutdown uses only recorded,
fingerprinted process groups while preserving the orphan's worktree.
Harness reasons are retained verbatim; pane exits include code/signal, MCP ends
name transport EOF or the parent watchdog, and unobserved causes stay `unknown`.

A live registered successor can recover a dead worker's committed, unpushed work
with one authenticated action:

```text
factory action=recover_branch task_id=cas-1234 payload='{"branch":"factory/old-worker-cas-1234","head":"<full-40-character-SHA>","expected_remote":""}'
```

The CLI equivalent is `cas factory recover-branch --task-id cas-1234 --request
recovery.json` (global `--json` prints the full receipt). The JSON file contains
the same `branch`, `head` and `expected_remote` fields. Empty `expected_remote`
requires the successor's destination ref to be absent; otherwise supply its
observed full SHA. The source must belong to the task's recorded worker, whose
registration or retirement evidence must exist and whose process/heartbeat must
be dead. Live task leases, moved source tips, unrelated refs, changed remote tips
and divergent successor branches refuse recovery. A destination already at the
requested SHA is an idempotent replay.

Recovery publishes on `factory/<successor>-<task>` through the ordinary private
push guard, records intent and result on the task, and rebinds its assignee and
anchor after verifying the push. A bounded temporary task lease prevents another
worker from acquiring the task during recovery; only that exact lease generation
is released afterward. It preserves the original worker checkout and
ref. A failure retains source work and records the incomplete attempt; a push
that succeeded before a concurrent task edit can be retried with the same exact
source SHA after reviewing its intent. Dirty/uncommitted worker files still
require an explicit separate recovery; this operation only copies commits.

## Peer transcript queries

A registered supervisor can queue `coordination action=peer_transcript
peer=<name@machine> session_id=current message="<regex>" limit=2`, or
`cas peers --peer <name@machine> --transcript '<regex>' --context 2`.
`session_id` / `--session-id` also accepts a known local supervisor session
ID on that peer. The response returns through `coordination action=inbox_poll`
with the Cloud request ID; queued does not mean the transcript was read yet.
The receiving daemon must support this query; its request is quiet even on
older daemons.

Only supervisors of the caller's own Cloud account are addressable. The peer
factory daemon handles the query without waking its supervisor or invoking a
shell. It reads at most the last 1 MiB of that session's JSONL transcript,
skips partial/invalid records, applies the existing secret redactors, then
matches the regex with zero to ten context lines. Regex length is capped at
512 bytes and the reply text at 8 KiB; scan/output truncation is explicit.
Paths, shell commands and arbitrary filesystem reads are not accepted.

## Coordination messages and reminders

An authenticated worker sends `coordination action=message target=supervisor
kind=respawn task_id=<task> summary="Context checkpoint" message="<return contract>"`
after committing/pushing and writing `HANDOFF.md` in its task artifact directory.
This typed status queues the existing `recycle_worker` path with the worker's
recorded recipe and task bindings. The director processes the recycle; its
supervisor copy is a quiet receipt. Requests are limited to one per worker and
factory session every ten minutes, persisted across replacement and daemon
restart. Recycle's existing unsaved-work and recipe refusals still apply.
The replacement's resume brief links its saved `HANDOFF.md` when present.
Ordinary `kind=status` messages, including prose saying `need: respawn`, retain
their action-wake behavior. This authenticated worker contract is MCP-only;
the external `cas factory message --kind` CLI remains for Commander turns.

Supervisor follow-ups about a worker's current InProgress or Blocked task remain
conversation, including `kind=answer` and messages with an explicit `task_id`.
They do not assign work again or replace the recorded delivery start. Ownership
matches the worker's registered name or id; messaging cannot take another
worker's task. Fresh assignments retain their normal stale-task checks.

Agent-facing message provenance and pointer wakes stamp the current render time
in the host's local timezone (`TZ` when set), in twelve-hour form with a zone
(for example `6:36 AM ET`). Queue age and first/replay provenance remain separate.
Supervisor clock times and ETAs require a clock read or measured start that turn.

`cas factory message --session <name> --project-dir <project>` uses the
destination session's store and queue session, even from another factory's
shell with inherited `CAS_ROOT` and `CAS_FACTORY_SESSION`. Either selector
can be used alone. Explicit `--project-dir` and `--cas-root` must resolve to
the selected session's store; conflicts fail before enqueueing. The success
line and JSON `session` field name the session written to the queue.

Planning recall at task creation and worker spawn requires matching content terms;
weak incidental matches stay silent. Each recalled item and operation lesson is
shown at most once in a harness session. Spawn lessons tagged for a harness are
shown only when that harness is selected. Upkeep advisories at close remain in
the immutable receipt details and task notes.

Worker-owned reminders end on shutdown or a terminal bound task, including
cross-session and suspended waits. Supervisor-owned cross-session event watches
retain their completion semantics. A supervisor-stalled prompt requires the
configured interval of observed actionable idle time, readable worker holds,
current open tasks, and the existing pinned-epic assignment boundary.

Authenticated messages directed to a supervisor wake its idle pane, including
peer field reports with no `kind`. The first message opens a fixed burst window
(`factory.supervisor_message_debounce_secs`, default 20 seconds, range 0–300).
Messages selected together share one body-bearing PTY turn, with each message's
identity and delivery receipt preserved. Task-bound action envelopes retain
individual stale-task revalidation. Busy panes, tool calls and drafts defer.

`kind=receipt`, pure acknowledgements such as “thanks, received”, broadcasts and
`wake=false` do not start turns; their bodies remain readable at the next turn.
A rolling sender/recipient cap (`factory.supervisor_message_wakes_per_hour`,
default 6) holds further status wakes and places a readable notice in the inbox.
Held senders receive one deduplicated notice with the earliest cap retry time;
peer feedback is persisted to the existing outbox for automatic delivery.
Substantive linked replies, asks, answers and blockers bypass the cap and debounce.
Explicit quiet and receipt intent remains quiet. Other senders retain their own budget. Restarting the daemon retains
the ledger and cap. Use `coordination action=message_status notification_id=<id>`
for wakes in the last hour, measured turns, median input+output tokens per woken
turn and maximum observed start latency. Claude/Codex usage is matched to the
message id in the harness transcript; missing, truncated or unfinished evidence
reads `unmeasured`. Claude input includes cache reads and writes; Codex usage is
the cumulative input/output delta across that turn. Transport alone never proves
that a model started or consumed tokens.

At each newly recorded supervisor turn, `supervisor_auto_clear` reads the
latest call's input occupancy, including cache reads/writes once. Its threshold
is 80% of the harness-reported `model_context_window`, capped by
`factory.supervisor_wake_input_budget` (default 1,000,000; 0 disables). Without
a reported window, only that configured per-call cap applies; capacity is never
inferred from a model name. Cumulative wake billing cannot trigger a clear.
It acts only at an idle
boundary: the turn has ended, no newer turn is running, no delivery-target
lock is held, no release train is frozen, no `release-repair` task is open, and
no operator ask or operator message is waiting. Otherwise it defers to a later
turn end. It saves a role-supervisor handoff memory from the session-end lane
snapshot (with a JSON receipt under `supervisor-handoffs/`). The auto-clear
projection includes only in-progress and awaiting-merge tasks, active/idle
workers, recent operator decisions and pending actor/kind/task/age summaries.
It caps each section at 100 receipt rows, reports omissions, and bounds the
injected sections separately. Full histories remain in their stores. It gives the
supervisor one turn to save judgement as a handoff, merges that under the
facts, and queues the context-reset control to the supervisor pane. When the
new transcript records the `/clear`, it queues the handoff to the new session.

When the MCP daemon opens a pre-merge candidate dispatch, it launches the
existing task verifier on the standard harness route. The registered child
uses the same sealed capability and verification gate as a manually launched
verifier. Its prompt includes the immutable candidate path, task criteria,
parent plan and task notes, including receipts and adversarial review notes.
The subprocess returns verdict arguments; only the daemon submits them with
the bound task/dispatch identity. No bearer is exposed to the harness.

Approval continues the normal merge admission and leaves a quiet one-line
receipt. A train queue reservation retains control of that continuation.
Rejection, a supervisor question, timeout or a failed review transport wakes
the owning supervisor with findings; none grants a merge override. Review
prompts, verdict output and harness diagnostics live under
`.cas/verification-runs/<dispatch>/<attempt>/`. Automatic reviews use the existing
account-aware launch resolver, including registered named profiles when the
default home is logged out. Failed harness notices include up to eight stderr
lines from the final 2 KiB and the full diagnostic log path. If a harness stops
without a verdict before expiry, one transaction consumes its sealed authority,
returns only that child's exact current claim to Pending, and records the
redacted reason in a structured verification retry event. Retry the same
`factory action=worktree_merge` once: one replacement claims the same dispatch
and candidate. Earlier attempts keep separate output/log paths. A stale child,
settled task, completed verdict or expired/reassigned claim cannot be reset.
A manually claimed dispatch remains with its verifier. Non-daemon callers retain the existing manual handoff.

A nontransactional task merge records a recovery intent before Git changes and
an immutable observation afterward. Once that observation is durable, advisory
CI and explicit CI overrides finish the intent without requesting automatic
close. Green published merges retain it only while close or verification
dispatch still needs recovery. A repeated merge after completion returns
`Already contained`, even if a peer advanced the target.

While holding the target merge lock, the pending check (including first-use
schema installation), intent write, observation write and intent clear each
share a two-second bound across their SQLite busy waits and retries. Thus these
four operations contribute at most eight seconds of deliberate contention
waiting per request, rather than three independent roughly 32-second retry
windows. Git commands and close/verification work have their own execution
bounds; eight seconds is not a wall-clock timeout for the entire merge. A
persistence failure reports the existing exact-merge retry action, preserving
unfinished recovery state.

Worker-idle merge notices wait for completed CI on the delivery's exact tip
and a successful refresh of its integration target. Parked deliveries retry
each minute, including when the director snapshot is unchanged. Pending CI
stays silent; ten minutes with no CI answer produces one "CI unknown" decision
notice. Failed target fetches defer the notice. Each task, delivery tip and CI
conclusion produces one notice per running factory; unrelated integration
commits do not create another. A red conclusion asks for a supervisor decision
instead of instructing an unconditional merge.

Batch QA (`qa.mode=batch`) is the built-in default for fresh projects and machines.
Set `qa.mode=per_task` explicitly to require per-delivery gates. QA keys resolve
from project `.cas/config.toml`, then user `~/.cas/config.toml`, then built-in defaults.
Unmerged user-facing deliveries park with evidence deferred and no automatic
per-task QA pass. A registered supervisor records the shared published ledger
with `verification action=batch_receipt`; each listed delivery must be contained
in that task's configured target. See [batch QA](../../docs/qa/batch-qa.md).
Once per release, after the fix-diff re-review, build an exact epic-tip candidate,
walk its batch matrix and publish the ledger. Record `verification action=batch_receipt`
with the included task IDs and delivery anchors, then close those deliveries.

Message bodies default to 1,200 characters, or 2,500 for blocker and merge
requests; task note bodies default to 1,500. These limits are configurable via
`factory.message_max_chars`, `factory.message_max_chars_escalation` and
`factory.note_max_chars`. An over-limit supervisor message to an agent spills
to an artifact. Messages to `operator` or `commander:<label>` are refused when
over the limit; publish the longer content as an artifact and send its reference.

External event reminders require `cross_session=true` and a JSON `remind_filter`:

| Event | Filter example |
| --- | --- |
| `branch_contained_in` | `{"commit":"<sha-or-ref>","target_branch":"main"}` |
| `tag_exists` | `{"tag":"v1.0.0"}` |
| `ci_completed` | `{"sha":"<full-40-character-sha>","workflow":"CI"}` |

For branch containment, `branch_tip`, `branch` and `sha` are aliases of `commit`;
`target` is an alias of `target_branch`. Creation and execution both select the
first nonempty string without internal whitespace, in that order. A malformed
preferred field does not hide a usable alias. Remote target spellings such as
`origin/main` normalize to `main`. For CI reminders, `workflow` is optional;
omit it to wait for all runs for that full SHA. Local events such as
`task_completed` accept subset filters, for example `{"task_id":"cas-1234"}`.

`remind_ttl_secs` defaults to 3,600 seconds for time and local-event reminders,
and 604,800 seconds (7 days) for external events. Explicit `0` means no expiry.

Supervisor stall notices use the full `epic_status` delivery containment
computation. A parked delivery already contained in its target, including a
rewritten or intentionally evolved delivery, gets an "awaiting worker close"
notice instead of another merge demand. That notice is deduplicated per delivery
and target across later epic commits and supervisor turns. Genuinely unmerged
deliveries retain the merge reminder. Containment proofs run off the daemon
loop and are cached by delivery metadata and local/origin tips; unknown proofs
are retried rather than cached as merged.

Worker quiet detection also counts authenticated MCP calls when they enter the
server, and scoped dirty-file edits or commits in the registered worker checkout.
MCP activity emits at most one event per server-bound worker every 30 seconds,
with the throttle checked in memory before store access. This interval is much
shorter than the default 300-second quiet threshold; concurrent calls share the
same reservation and failed writes can retry.
Confirmed authenticated Supervisor and Director roles are cached per immutable
server core, so their calls skip further activity-related store reads. Standard
bootstrap roles, missing sessions and inactive workers remain retryable; a
Standard row corrected to Worker starts recording activity on the same core.
Heartbeat and registration calls remain telemetry, so heartbeat alone cannot
hide a quiet worker. Call activity uses the server's immutable registered actor;
request fields naming another worker do not credit that worker. Open harness
tool calls and live worker-owned child processes continue to count as activity;
unrelated process trees and persistent MCP sidecars do not.

Claude MCP responses append factory mail only when the call's tool-use id
belongs to the registered parent's top-level transcript. Subagent and untagged
Claude calls preserve the parent mailbox. Transport handoff remains provisional:
the parent's next turn can surface the body without an explicit poll. Overflow
notes name the unread message id and summary; the next eligible turn or tool
boundary retries bodies that did not fit. `message_status` reports
`recipient_receipt` separately for transport handoff and rendered context.

Delivery status and watchdog notices share the recorded `delivery_attempts`
and `next_attempt_at`; a completed handoff has no pending transport retry.
Status uses the watchdog's capped escalation threshold and cites the persisted
`channel_stalled_notice_id` when further notices are paused. Elapsed time alone
does not prove that an alert was sent. Missing message-specific receipt remains
unconfirmed, even when other worker activity is visible; it does not establish
that no turn started or justify recovery on its own.

Abandoned queue messages report `undelivered` in the `message_status` header
and JSON `legacy_status`, even though their queue rows are processed. A sender
notice preserves the wake-gate refusal when one was recorded. Authenticated
messages whose sender has ended use the existing local supervisor fallback;
that supervisor receives each original source, summary and complete body.
Each sweep emits at most three full-body recovery notices per recipient and
one folded reference list for the rest. Each listed `notification_id` names a
recipient-owned card: an explicit `inbox_poll notification_id=<id>` hydrates its
retained abandoned original after the usual recipient/session checks. Cards
stay out of automatic drains and unread counts. Recovery notices do not fold
into the routine ended-worker digest. Large originals remain readable through
`inbox_poll`.
Recovery rows require daemon origin, the delivery-watchdog source and their
recovery dedupe marker; assignment checks in daemon delivery, inbox poll and
turn-start hooks use the same store predicate to treat these bodies as history.
A caller-supplied watchdog name cannot claim that exemption. Recovery rows use
the queue delivery and hook lane. The director's separate event-generated
prompts have no queue provenance; their ordinary stale-assignment recheck
continues to refuse closed work.

For delivery diagnosis, the `prompt_queue` and its receipt tables live in the
project's `.cas/cas.db`. Open it read-only to avoid creating a misleading empty
database at a guessed path. Writable prompt-queue opens remove a regular empty
`.cas/prompt_queue.db` artifact when it has no SQLite recovery sidecars; nonempty
files, symlinks and sidecar-backed artifacts remain untouched for investigation.
Read-only queue opens never perform this cleanup.

Lifecycle latency diagnosis must separate supervisor-bound quiet `lifecycle:` progress notices
from wake-eligible `lifecycle-wake:` action events. Quiet notices wait for the
recipient's next hook or inbox poll; that policy is not a directed-message
latency guarantee. For either class, measure enqueue-to-transport and
enqueue-to-recipient-surfacing separately. A `hook_surfaced` receipt proves
injection into recipient context, not that the model read or acted on the event.
Do not drain another recipient's queue to measure delivery.

The lifecycle `[delayed Nm; held: cause]` summary appears at recipient drain
after nine minutes. It reports queue age and the last recorded hold, such as
a busy pane, debounce or rate limit. `message_status` retains that cause in
`holding_detail` after transport and hook acknowledgment; its current pending
reason/detail and confirmation receipts keep their separate meanings. The
marker says `cause unrecorded` when no hold was recorded, including historical
rows. This is not a history of holds or their durations, and does not prove
compliance with the directed-message probe's 2 s p95 / 10 s normal-transport
maximum. Preserve a bounded read-only metadata capture before retention removes
the evidence, and state unknown causes explicitly.

The GH #1393 follow-up captured five real rows on October 29, 2026 with
`cas 3.88.0 (b0bff67)`: two wake-eligible lifecycle transports took 2.608 s and
43.344 s, two quiet lifecycle notices took 333.700 s and 70.409 s, and one
directed status took 3.025 s. All five had recipient hook-surfacing receipts.
This convenience sample does not establish a p95 or a general SLO verdict;
the 43.344 s wake sample exceeds the probe's normal-transport maximum for that
row. The final records do not establish its earlier holding cause. The original
29-minute incident was on another machine/session and was not reproduced or
explained by this capture. Portable metadata and the executed read-only capture
script are task artifacts `art-e2253955` and `art-12f49e76` on `cas-67bb`.

Cross-project task ownership inspection self-execs the internal
`--internal-task-ownership-probe` route before ordinary CLI startup. Raw database,
WAL and shared-memory reads, immutable SQLite and checkpoint-lock queries run in
those fresh processes, preserving the caller's POSIX locks. Two probe phases
retain the WAL/main fingerprint comparison and checkpoint-race checks; retries
share a ten-second deadline per store. Helper failures, malformed responses and
active checkpointers leave ownership unproven. This route is internal; retention
and simulator callers continue using `KnownProjects`.

`cas doctor` inspects legacy `prompt_queue` columns without initializing the
queue or recording migration detections. If required health-check columns are
missing, its schema finding names them and points to `cas update --schema-only`.
That explicit migration reuses the queue's canonical additive DDL on the
migration transaction's connection, preserving messages, receipts and outboxes.
Before adding lifecycle columns, it copies legacy processed-row identities and
timestamps into reserved compatibility metadata in the same transaction. It
does not change original messages, acknowledgements, receipts or outboxes.
Runtime queue initialization later hydrates delivery stage/time only for those
unchanged candidates, consuming stale candidates too; later `mark_processed`
calls do not become transport evidence. The repair itself does not run runtime
acknowledgement demotion or lifecycle hydration. A store that has never created
a prompt queue keeps it absent. Run `cas doctor` again
after repair to measure communication, relay and retry health.

A parked task can merge its pushed delivery while its worker checkout holds
uncommitted work for a later task. With `cleanup=false`, `worktree_merge` checks
that the local source and a fresh origin fetch both equal the parked anchor,
then merges that immutable commit in an isolated checkout. Its receipt names
the pushed ref and states that the worker checkout was left untouched.
Unpushed or mismatched tips retain the dirty-checkout refusal; cleanup requests
retain the existing protection against deleting uncommitted work.

When a parked delivery hits a content conflict, Cassy returns it Open to its
assignee through `request_changes`, invalidating the old delivery proof and
recording `merge_conflicted`, the conflicting paths and the attempted target tip.
The worker receives one notice and can start, resolve and re-deliver directly.
Target drift and pre-existing checkout residue keep their existing recovery paths.

Factory daemon startup validates both Unix socket pathnames against the target
ABI before logs, processes or agent registration: Linux has 108 address bytes,
macOS 104, including the terminating NUL. An oversized HOME/session path is
refused with its byte count and a shorter-path remedy. A daemon also refuses an
inherited CAS_ROOT that does not belong to its declared project; canonical and
worktree aliases of that project's store remain valid. For an isolated QA rig,
initialize its own project and clear inherited CAS_ROOT before starting it.

After a published merge, an enforced registry or documentation upkeep gate
returns the parked task Open to its assignee through the same `request_changes`
core. Cassy retains the gate text in task notes, sends one corrective notice,
and retires the old automatic-close intent. The worker can start, commit the
source repair and re-deliver with a fresh anchor. Verification and QA waits
keep their existing dispatch ownership.

A supervisor message to a blocked worker records ownership of that wait when
sent with `kind=answer`, `in_reply_to` the worker's blocker message, or as a
legacy direct message without a kind or reply reference. A supervisor decision
note also records ownership using its activity event ID. An unbound message
must identify a single blocked task assigned to that worker; a task-bound
answer cannot clear another task. Typed status pings and replies to ordinary
status leave blocker alerts armed.
Repeated idle and blocked alerts stay quiet while the assignee, supervisor,
structured hold reason and blocker question remain unchanged, including a
verification rejection followed by the same block. A new blocker question
starts a fresh five-minute deadline; a new hold re-arms alerts, and an explicit
Open transition clears the answer. Existing
queued alerts remain readable without waking the supervisor again. The receipt
and question fingerprint are maintained by authenticated coordination messages
and supervisor decision notes, not hand-written execution-state patches.

For Claude supervisors in Teams factories, an authenticated action event whose
complete framed payload fits the hook budget travels in one PTY turn, with its
queue id and body inline. No Teams pointer or relay copy is written. A busy
pane, in-flight tool call or dirty composer holds the event in the durable
queue for a later wake or the recipient's next hook/poll. Progress FYIs such as
`task_started` remain quiet and readable at that boundary. The same authenticated
wake classes still decide which events can start a turn. Worker pointer delivery
and oversized full-body Teams delivery retain their existing paths. A transport
write is provisional; only the existing per-recipient rendering/poll receipts
retire a body from the unread view. Already-held Teams copies retain their
legacy wake recovery; timed pointer-to-body fallback applies to workers.
Fired reminders use the same path and pane/tool/composer vetoes; a recent
completed checkpoint plus continuing heartbeats does not postpone an idle
supervisor's reminder. Self reminders that exhaust a wake budget stay readable
and retry after the existing park interval, without inter-agent abandonment or
unread escalation. Only a retired supervisor target can transfer its reminder to
a replacement supervisor. A missing or retired worker target leaves its reminder pending until
that exact target is available; a worker reminder reaches a supervisor only when
the worker selected that target. Delegated reminders retain the issuing worker
as sender. Supervisor self-reminders retain self-addressing on takeover; original
owner and target IDs stay in the durable reminder record.

A worker's undelivered typed status/ready messages coalesce into one pending
wake, carrying earlier bodies with their original notification ids and summaries.
The fold is bounded at 8,000 bytes. An earlier row that does not fit stays pending
independently and can be delivered after a newer row; delivery order is not a
chronological guarantee. Use each body's original label to identify it. A fold
keeps the strongest wake intent among its messages, including absorbed identical
bodies; a fold containing only acknowledgements or explicit quiet messages waits
for the next turn.

Recipient drains revalidate typed lifecycle relays against the task's current
terminal state before recording surfacing or spending the hook body budget.
Supervisor assignment messages retain their explicit `task_id`, or the worker's
unambiguous current assignment, in a durable task binding. When the sole current
task has started, an assignment clause naming another task binds that next task,
even before its assignee is updated. Binding and merge selection query only the
recipient's tasks in the relevant statuses. A previous task's close receipt or a
caller-supplied `<cas-assignment>` tag cannot change the server binding. A stale
assignment send returns an explicit refusal naming its task and status and a
`pass task_id=<id>` hint, including a withdrawal during the sender's first-delivery
wait.
An obsolete relay is retained with `superseded_stale`, its task/status reason
and a terminal-task reconciliation acknowledgement. Hooks, named wakes, inbox
polls and repeats of earlier polls share this atomic guard; notification ids
are never interpreted as task ids. Current terminal reports and payloads with
no positive terminal-task evidence remain eligible.
The same atomic boundary expires verification-dispatch handoffs after their
RFC3339 deadline or when their task is terminal. CI red-run and worker-attention
snapshots expire after one hour, or sooner with positive terminal-task evidence.
CI resolves a task from an explicit task id or an unambiguous local task branch /
parked branch; it performs no Git or network lookup. Fresh alerts on unknown or
shared branches remain eligible. Age/deadline expiry works without a task table;
malformed envelopes and human messages retain normal delivery semantics.
Expired rows keep their history, acknowledgement and `superseded_stale` reason.

Exhausting a supervisor message's wake budget stops active wakes while keeping
its body readable at the next hook or inbox poll. The delivery detail identifies
the notification and task when available, and reports the failed wake. Merge
requests, typed status messages and blockers stay actionable until a separate
premise check proves them obsolete. Missing task state permits delivery; it
does not prove expiry. Positively stale lifecycle relays retain their withdrawal
notice, and undelivered worker-death relays retain their dedicated failure report.


## Claude factory launch options

Claude agents receive their factory identity as `--name` when the installed
CLI advertises that exact option. By default, Claude workers use a private, mode-0600 MCP
configuration containing only `cas`, together with `--mcp-config` and
`--strict-mcp-config`. The existing project CAS definition is retained when
present; otherwise Cassy supplies `cas serve`. Supervisor MCP discovery keeps
its normal scopes. Native hooks, account selection and Teams settings still
apply. Project files are left intact, and supervisor-only environment denials
also apply to the generated CAS definition.

In the default `cas_only` mode, workers refuse to start if strict MCP flags are unsupported, the project root
is missing, configuration cannot be provisioned, or `supervisor_only_mcp`
denies `cas`. Upgrade Claude or correct the reported project configuration
before retrying. Tasks needing other native MCP servers should go through the
supervisor or the configured Cassy proxy, or use the project opt-out below.

Claude settings apply to initial, dynamic and recycled Claude agents:

```toml
[factory.claude]
worker_mcp = "cas_only" # Default; choose "inherit" for native MCP discovery.
remote_control = false # Opt in for the supervisor only; workers stay network-quiet.
# fallback_model = "sonnet" # Explicit overload fallback for both roles.
```

Use `cas config set factory.claude.worker_mcp inherit` to let this project's
Claude workers load their native project/user MCP scopes. With no configured
denials, `inherit` emits neither strict/config flags nor a private MCP JSON
file and does not require strict-MCP CLI support. Restore the default with
`cas config set factory.claude.worker_mcp cas_only`.
If `supervisor_only_mcp` or `supervisor_only_env` contains any entries,
`inherit` refuses before discovery: use `cas_only`, or clear the denial policy.
Credential grants alone do not prevent inheritance. Supervisors and Codex
keep their existing MCP behavior under either value; hooks, account selection,
Teams and worker credential protections still apply. Invalid values are refused
by both config commands and TOML loading.

Use `cas config set factory.claude.remote_control true` to opt in, and
`cas config set factory.claude.fallback_model sonnet` to choose a fallback.
An empty fallback value clears it. Explicitly requested unsupported flags
produce a launch error; model availability and remote access are resolved by
Claude. Capability detection shares one successful `claude --help` capture
per factory process; restart the factory after changing the installed CLI.

Dynamic system-prompt exclusion remains deferred pending a controlled cache,
cost and context-correctness comparison. This change claims no startup-time,
token or cost improvement.

## Factory launch diagnostics

Starting a new factory writes `~/.cas/logs/factory/<session>/launch.log` after
the attach decision and launch preflight. Attaching to a running factory reuses
that session without creating a new log directory or running launch-only checks.

Refused launches with `--name` append to that session's `launch.log`. Unnamed
refusals append to one stable project file, `<CAS_ROOT>/<logging.log_dir>/factory-launch.log`
(by default `.cas/logs/factory-launch.log`), without reserving a session name.
Before a project store exists, unnamed refusals use `~/.cas/logs/factory-launch.log`
and leave the project's `.cas` absent so `cas init` can initialize it normally.
A refused launch prints its log path alongside its original reason. Repeated
refusals append dated records at the same path.

`launch.log` also identifies the project's runtime tracing file, using the
resolved CAS root and `logging.log_dir` from its configuration. These files use
the local date (`cas-YYYY-MM-DD.log`) and rotate daily. If file logging is
disabled, the launch record says so.

The session's `daemon.log` captures daemon stderr and can be empty or sparse
while runtime tracing is active. Preflight diagnostics live in `launch.log`;
an empty `daemon.log` does not indicate that preflight passed or tracing stopped.

## Epic lanes shared across hosts

With `CAS_PUSH_EPIC_BRANCH` disabled, epic creation keeps the Git lane local;
sharing the task through Cloud does not transfer its ref. Task deliverables
retain the originating host and branch so a peer's missing-lane spawn refusal
names that host and prints `git push origin <epic-branch>` to run there before
retrying. First focus or spawn on the source host still publishes the lane.
The peer never reconstructs the lane from trunk. On the originating host, a
missing lane gets known-tip recovery advice instead of an impossible push.
Commands occupy their own plain line; synced host labels have control characters
removed. Adopting an existing published lane does not invent an origin host. Missing legacy provenance
produces a neutral unavailable-lane message; failed origin verification and
previously published refs retain their separate refusal paths.

## Factory cloud client (disabled by default)

The factory daemon ships with a live-stream WebSocket client
(`cas-cli/src/ui/factory/daemon/cloud_client.rs`) that pushes factory state,
events, and pane output to a Phoenix-framework endpoint
(`/socket/websocket`). That endpoint is **not** implemented on the current
cloud backend (petra-stella-cloud is Next.js on Vercel, which can't host
long-lived Phoenix channels) and the feature it fronts — the Hetzner Slack
bridge / web terminal — is paused (see `project_claude_code_account_banned`).

The client is therefore gated behind a config flag and **disabled by
default**. Flip it on in `.cas/cloud.json`:

```json
{
  "endpoint": "https://your-phoenix-capable-host",
  "token": "…",
  "factory_cloud_client_enabled": true
}
```

Re-enable only when a Phoenix-capable backend is reachable. The REST-based
cloud syncer (`cas-cli/src/cloud/syncer/`) is independent of this flag and
always runs when logged in.

### Team-only project sync

Set `[cloud] team_only = true` in a project's `.cas/config.toml` after linking
that project to a team with `cas cloud team set`.
The registered `cas config set cloud.team_only true` command sets the same key;
`cas config get cloud.team_only` and reset/list use that configuration too.
The default is `false`:
team-eligible project tasks, dependencies, memories, rules, and skills keep
their existing personal plus team queue behavior. With the opt-in enabled,
those project rows use only the team queue. Global rows and private memories
remain in the personal queue. `cas cloud push` and `cas cloud sync` avoid a
personal project push. Existing personal transport is retained until an exact,
itemized team write is acknowledged; changing the setting alone never removes
it. Cloud rows are never deleted by this cleanup. Without
an active team, sync refuses and `cas doctor` reports an error.
The personal push API requires a project identity, so global and private rows
from a team-only root remain queued rather than re-registering its retired
personal project. `cas cloud status` and `cas doctor` show the held count.

For a team-linked project with legacy personal transport, `cas cloud sync
--json` reports `push.held_personal_pending`, `push.held_personal_failed`,
`push.held_personal_rejected` and the team backlog in one final JSON document.
Doctor and factory preflight explain that team-linked sync drains the team
queue only. Retained conflict snapshots are history, not delivery failures.

After the operator declares the project team-only, recover eligible legacy
transport explicitly:

```sh
cas config set cloud.team_only true
cas --json cloud queue migrate-to-team
# Review every row and its decision before applying.
cas --json cloud queue migrate-to-team --apply
cas cloud sync
```

`migrate-to-team` defaults to a read-only preview, makes no HTTP calls and
prints every queue identifier and decision. Apply requires `team_only=true`.
It copies owned, project-scoped, shared upsert snapshots into the active team
queue without replacing existing team intents. Deletes, private/global rows,
foreign or unknown origins, foreign project routes and unsupported types stay
untouched. Dependencies require both locally owned, shared endpoints and a
current local edge. Canonical local routes are supported; receipts bind the personal route.
Snapshot JSON is preserved so the existing exact itemized
ACK can retire the personal copy after delivery; enqueue alone retires nothing.
Missing/changed provenance or sharing policy fails closed. Reopen long-running
writers after changing team_only so their cached policy uses the new setting.

Standalone push/pull, the daemon cycle, and MCP startup use the same team-only
scope policy. A `team_owned_project` rejection establishes project policy, not
remote entity presence or payload equivalence. Keep legacy or mixed rejection
groups until their delivery is proven; keeping team-only off remains a safe
operational workaround while investigating them.

Preview exact IDs with `cas cloud queue --purge-team-owned --dry-run --json`.
The response lists each ID, its eligibility and reason without printing payloads.
Cleanup requires an explicit selection, for example
`cas cloud queue --purge-team-owned --ids 12,18`. Only selected personal rows
with the exact structured rejection **and** a matching durable team delivery
receipt are removed. Missing/differing/legacy proof retains the row; unscoped
cleanup is refused. Local entities and team rows survive, and this command
makes no cloud calls, backfills, promotions or remote deletes. An old verified
server copy without a locally recorded equivalent acknowledgment remains
unproven to this command.

Receipt version 1 uses existing `sync_metadata` keys
`team_delivery_ack:<personal-queue-id>`. The receipt binds the canonical project,
team, entity, operation, queue creation snapshot and SHA-256 of the entire
serialized transmitted JSON; it stores no private payload. Only itemized
`inserted`/`updated` responses can certify it. Aggregate counts, `skipped_lww`,
deletes, canonical remaps, transformed payloads and uncertain results cannot.
Before HTTP, the team pusher captures the personal snapshots; after HTTP it
persists proof and consumes only the exact acknowledged team snapshot in one
transaction. Preview and cleanup in `cloud/sync_queue/team_delivery.rs` are the
receipt authority readers. Cleanup revalidates under the same transaction that
deletes the matching personal row and receipt. A crash before cleanup leaves
proof durable; a changed/reused row or different routing cannot reuse it.
Ambiguous receipts have no age-based deletion authority.

`duplicate_of_other_project` means the same row already lives under another
Cloud project identity. The rejection names that owning project. If both
identities represent the same project, ask the Cloud owner to register and fold
the alias first, then run `cas cloud project --adopt-aliases`,
`cas cloud queue --retry --retry-reason duplicate_of_other_project`, and
`cas cloud sync`. Otherwise, retire the local duplicate. Retrying alone cannot
repair remote ownership; alias adoption rewrites local task provenance and
does not move Cloud rows.

## Cloud cleanup and rate limits

SessionEnd writes an atomic intent under `.cas/cloud_shutdown/` before deleting
the local agent. Hooks make no HTTP requests. The embedded daemon drains intents
at session startup and on its off-loop presence heartbeat, including when it has
no owned active agent. If no daemon starts, cleanup waits for the next daemon.
Intents are keyed by agent, harness session and registration timestamp. Presence
requests share a nonblocking read lease; shutdown takes the exclusive lease.
A resumed session with a newer registration and a matching harness session
supersedes older cleanup, including legacy intents, and its own SessionEnd
creates separate cleanup. Each intent records the ended registration timestamp;
a real SessionStart advances a fenced incarnation even when failed local cleanup,
revival or automatic registration left the row active. Automatic MCP reinsertion
after `/clear` remains fenced without a session id, regardless of PID ancestry.
Shutdown checks supersession without the network lease and takes the exclusive
lease only for due work. Presence shares parsed fences within each pass and
refreshes them after acquiring its lease when the intent directory changes.
Change detection compares mtime plus the exact sorted intent filenames, so a
late intent is detected even when timestamps and entry counts coincide. Readers
remain lock-free; unchanged generations retain their parsed intent cache.
A listing failure fails closed for an unknown harness session; known sessions
can still check their direct fences. Cleanup re-reads pending work before writing
so a stale pass cannot recreate retired files or overwrite a completion. Startup
replies without waiting for Cloud work; concurrent startup requests coalesce by
agent and are drained after the current presence pass. A panicked presence pass
can be restarted by the next kick. A retained ended row advances its registration
timestamp only on a harness SessionStart. A resumed
incarnation resets old presence backoff, and superseded cleanup disappears from
health even before its scheduled retry.

Shutdown, claim release and peer ACK retry HTTP 429 up to three attempts within
five seconds (or a caller's shorter budget). `Retry-After` seconds and HTTP dates
are respected up to 15 minutes; absent or invalid values use 200 ms then 400 ms
backoff. A delay
outside the HTTP budget returns a named rate-limit failure. Durable shutdown
intents and the mailbox poller preserve longer server delays across later passes;
mailbox health remains current while waiting for that scheduled retry. Within a
mailbox pass, later claim chunks, ACKs and metadata requests share the cooldown;
already claimed messages remain eligible for local admission and later ACK.

Presence and worker status show pending cleanup and its cause even after the
agent row is gone. After 24 hours a failed intent expires and stops retrying.
The daemon retains its agent id and cause under `.cas/cloud_shutdown/expired/`
before pruning the work item. Inspect these records alongside Cloud presence;
removing a diagnostic does not perform shutdown. Successful intents stop
appearing in health immediately and retain an incarnation fence until their
24-hour retention ends. Completed-only passes need no store or network lease.
Intents contain no tokens or arbitrary agent metadata.

## Project issue repository reads

`cas config get issues.repo` returns the stored project tracker, or resolves an
unset value from that checkout's local GitHub `origin`. It parses the local URL
without contacting GitHub and accepts HTTPS, SCP-style SSH and `ssh://` remotes.
A missing, invalid or non-GitHub origin leaves the result empty; nested folders
without their own Git repository do not borrow a parent repository's origin.
An explicit `issues.repo` always wins, independently of component repositories.

Plain `get` prints only `owner/repo`, so it remains usable in `gh --repo
"$(cas config get issues.repo)"`. Human `list`, `describe` and `search` label an
inferred value `(from origin; unset in config)`; their JSON views and JSON `get`
include `source` (`origin`, `config` or `unset`) and `configured` for this key.
`cas doctor` warns when the setting is unset and names the resolved origin or
explains how to set a tracker when none can be resolved.

These reads never save the inferred value. Raw `Config::get`/`list`, editing,
serialization and `config diff` retain stored values. Legacy `report_cas_bug`,
automatic issue triage and release-report GitHub intake retain their explicit
configuration contracts; they do not infer a project target from origin.
`report_cas_bug` uses effective `issues.components.cassy`, including the
compiled Cassy repository (`Richards-LLC/cassy`) when that component is absent.
For example, `issues.repo = "example/downstream"` alone still files Cassy bugs
in `Richards-LLC/cassy`, agreeing with `cas config get issues.components.cassy`.
The checkout's inferred origin is never a Cassy filing target.

Legacy opt-out: a raw, explicitly blank or whitespace-only
`[issues.components] cassy` value in config.toml permits a nonblank stored
`issues.repo` fallback. If the project override is also blank or absent, the
compiled Cassy default applies. This opt-out differs from clearing the component
with `cas config set issues.components.cassy ""`, which removes the raw override
and restores the default. Configure a nonblank component explicitly for a fork.

It validates the selected destination as `owner/name` before any GitHub call;
an invalid selected value never falls through to another tracker. Instead the
report is durably staged under the task's artifacts directory and the response
names the selected key and `cas config set <key> owner/name` remedy. Correct
that setting before retrying; a staged report has not been filed. Existing
credential failures and label degradation retain their filing behavior.
Only project initialization may persist a discovered origin; routine reads
and updates do not backfill it.

## Canonical install path

Cassy must be installed to **one** location: `~/.local/bin/cas`. Any other
location (`/usr/local/bin`, `/usr/bin`, `~/.cargo/bin`) creates silent
duplicates: PATH-order changes (interactive zsh vs. a systemd service, or a
subagent invoking `cas` via absolute path) can promote a stale copy and
silently reintroduce fixed bugs.

- `scripts/cas-install.sh` installs to `~/.local/bin/cas` and warns about any
  other `cas` binaries it finds on PATH. It also offers to wire that directory
  onto PATH in the **login** shell's startup file (zsh `.zshenv`, bash
  `.bashrc`/`.profile`), guarded by `# >>> cassy path >>>` markers so a re-run
  is a no-op, and verifies the result with `cas --version` in a fresh login
  shell before claiming the install succeeded. `CAS_WIRE_PATH=1|0` overrides the
  prompt; the rc-edit seam is covered by `scripts/test-cas-install.sh`.
  Before extraction, it verifies the archive against the selected asset's
  SHA-256 in GitHub Release metadata; missing or mismatched receipts fail
  closed while preserving the installed binary. This detects corruption but
  is not an independent signature: GitHub/repository release authority serves
  both bytes and digest, and no separate Cassy signing trust root is named.
- On startup, `cas` itself scans PATH and emits a single-line stderr warning
  when duplicates with diverging mtimes are present. Silence it with
  `CAS_SUPPRESS_DUPLICATE_WARNING=1`, or force it on in non-TTY contexts with
  `CAS_WARN_DUPLICATES=1`. Hooks, `cas serve`, and `cas factory` are never
  warned.
- If you previously installed via `cargo install cas` or a distro package,
  remove those copies so only `~/.local/bin/cas` remains.

## Peer work awareness

`cas peers --detail` shows each peer supervisor's machine, factory session,
epic focus, active task claims, live workers and registered branches.
`--peer <name>` or `--peer <name>@<machine>` selects one supervisor and implies
detail. `--json` returns the full report without truncation. From an agent,
`coordination action=peers` lists the same supervisors without the detail;
its claims and workers view is not on the published coordination surface. The outcome and peer
count appear first, followed by owner-grouped work and presence/mailbox receipts. JSON carries the typed
`cloud_presence` record, `cloud_presence_message`, and `mailbox_status` too.
Agent pagination echoes the returned `next_sort_cursor` with the other cursors.

Claims are scoped to the repository's canonical identity. Their titles and task
status come from this checkout's synced task store when available; an unsynced
task remains visible by its plain task ID, with `title not synced` explaining
the missing title. Human rows use agent names; JSON also keeps `lock_key` and
`agent_id` for exact cloud receipts. Claim age is time since the last renewal.
Only active, unexpired claims count. Live workers have an active/idle heartbeat
within five minutes and must match both the peer's factory session and machine.
A missing branch is shown as unknown. Worker details are read-only: these rows
do not register remote workers locally or make them message or spawn targets.
Cloud failures are reported as unavailable, never as an empty claim list.
Peer mail follows a supervisor's name, stable machine identity and named
factory session across a restart. Different factories remain distinct even
when their supervisors have the same name on the same machine. A live
successor claims verified retired addresses from this project's
registry and unregister history; Cloud message IDs still deduplicate local
admission. A reused live identity, worker, different machine or ambiguous
successor is not a retired address. When multiple live peers share a displayed
address, message-target ambiguity lists distinct agent IDs that can be used as
explicit targets. Legacy deleted rows without retirement identity cannot be
safely recovered from a name alone. Their bodies are not admitted; the daemon
sends an idempotent reply asking the sender to resend to the current supervisor
ID, then rejects the original. This feedback-only fallback requires exactly
one live named factory with that supervisor name in the project. Competing
same-name factories leave legacy addresses unclaimed and warn instead.
If feedback fails, the original remains unacked
for retry. Retirement recovery uses a seven-day window and at most 100 retired
recipients per tick, separately from current recipients. History lookup failure
or truncation is a mailbox warning; current verified addresses keep receiving.
Failed claim batches remain visible while successful batches are admitted and
acknowledged.

Cloud project lock reads include teammates on the same team project (Cloud #155).
Peer details show `Claims visible (team project)` with the owning account, agent
and machine; start, claim and spawn refuse an active teammate claim. Personal
projects retain account isolation. JSON includes `claims_visibility`, `user_id`,
`team_id` and `project_id`; an empty claims array is authoritative only when
visibility is `same_account` or `team_project` and `claims_status.state` is `ready`.
A failed lock read is `unavailable`, not an empty inventory. The viewer account
comes from the token-owned agent list, independently of caller session lookup.
Operator CLI identity uses the same session/ancestor-PID lookup as status.
Peer discovery also excludes every agent registered in this project's local
agent store, so a plain terminal never counts this clone's supervisors as peers.
The detail view fits 72 columns and shows each peer's worktree and scratch roots,
claim-owned epics and target branches, linked task status/owner/claim, observed
PR URLs and head SHAs, workers by machine, and existing waiting-merge records
(`awaiting_merge`, `parked_delivery`, `open_epic_pr`, `merge_queue`). A parked
assignment remains an observation after its lease is released; ambiguous names
never associate a delivery with a machine. Ownership comes only from active
claims. Each section carries its own state and observation time; private work,
stale snapshots, unavailable records and unknown PR details remain explicit.
The default scratch base is shared and labelled accordingly.

PR details reuse the existing bounded CI watcher poll, persisted on its blocking
job and published by the existing background presence worker. No extra GitHub
call runs on the control loop. Older peers without published observations show
`unavailable`, and heartbeat renewal never makes an old PR snapshot fresh. PR
snapshots cap at 100 records/32 KiB and label truncation; branch names never
invent PR details. Per-peer splits come from the peer's existing local split
records. The report is read-only: it shows conflicts but does not prevent them,
register remote workers locally, or include Slack threads/asks.

Presence and local recorded epic splits follow the peer work, then mailbox
health. JSON exposes `epic_splits` separately
from the health-only `mailbox_status`. Each split retains its stored `state` and
an effective `display_state` (`expired` for a lapsed pending proposal), its task
shares, peer, reason, expiry and decision time.

For a single-child handover, request it with `factory action=epic_split
id=<epic> target=<owner> mine=<child>`; the owner accepts or rejects the proposal.
Grant it with `theirs=<child>` and omit `mine`. Either share may be empty or
omitted, but not both. The proposer claims only `mine`; acceptance claims only
the recipient's share. Existing expiry, cancel and start guards apply.

A task with no epic can be its own split scope: its owner offers
`factory action=epic_split id=<task> target=<builder> theirs=<task>`, and the
builder accepts the returned split. No parent task or dependency is created.
For a Slack source, only the current thread claim owner may offer the work.
The receiver checks the authenticated issuer against its own current source-owner
receipt before accepting or claiming work, and again before an accepted split
bypasses source admission. Missing, expired or mismatched receipts grant no bypass.
After acceptance the builder can assign and start that task with `external_ref`
intact; the accepted local share grants work without claiming the thread.
Pending offers and another supervisor's shares grant no work authority. Reply
intents and Violet posts still require the thread owner's separate authority.
Changing a delegated task's source requires the ordinary source admission.

Assignments and starts of a peer's split children name the peer and split and
refuse the change, including `force=true`. To take over one child, a live
registered supervisor uses `task action=update id=<child> assignee=<worker>
supervisor_override=true reason="<agreed takeover reason>"`. This persists a
decision bound to that worker's registration; only its successful start consumes
the decision. A different worker, a claim, transfer or pre-assigned spawn cannot
use it. Cancel or redirect the split to change the ownership agreement instead.
Accepted splits remain binding until cancelled; their expiry limits acceptance
of unanswered proposals.

Accept, reject, cancel and expiry carry typed `epic_split_decided` peer intent,
eligible to wake through the existing mailbox and pane gates. Read
`factory action=epic_split id=<split> decision=status` (or just `id=<split>`) for
state, `decided_at`, authenticated decider, observed claims and unfinished cleanup.
Unavailable claim reads stay explicitly unavailable. Empty shares print `(none)`.

`coordination action=remind remind_event=epic_split_decided
remind_filter='{"split_id":"<split>"}' remind_message="Continue split work"`
fires once for the latest persisted decided state, including late registration
and daemon restart. The existing reminder scan handles it; no extra poller runs.

The mailbox duty persists unanswered expiry and its retryable work. Expiry lifts
the split guard and each side releases only claims held by its own supervisor;
rejection also releases the proposer's own share on receipt. Claims taken by a
worker or a peer remain untouched. Cleanup failures remain visible and retryable.
Expiry notices reuse the existing peer outbox's logical send across restart.
Once expiry is recorded, a delayed acceptance cannot resurrect the split; an
expiry received after acceptance releases the recipient's own split claims.

## Adding Features

**Friction ledger event**: Emit from the code path that observed the fact, through `cas-cli/src/quality_ledger.rs`, after that path has succeeded. Emission fails open and never changes the action it measures. The natural key names the fact, so a retry collapses to one row. Payload fields are metrics, enums and slugs (scope A); free text goes in `local_note`, which has no wire field. An escape is a bug labelled `found-in:vX.Y.Z`, optionally `missed-by:<stage>`.

**New CLI command**: Add variant to `Commands` enum in `cas-cli/src/cli/mod.rs`, create handler file in `cli/`. Prefer a dedicated integration test file at `tests/<feature>_test.rs` (e.g. `team_sync_test.rs`, `memory_share_test.rs`, `team_memories_e2e_test.rs`) over piling into `cli_test.rs` — isolated files surface regressions per-feature and keep compile times down. Shared fixtures (UUIDs, Cli/CloudConfig builders) go in `cas-cli/tests/common/mod.rs`; include via `mod common;` at the top of each test file.

**New MCP tool**: Add handler in `cas-cli/src/mcp/tools/core/` (data tools) or `cas-cli/src/mcp/tools/service/` (orchestration tools). Request types go in `cas-cli/src/mcp/tools/types/`. Register in the tool list via the `CasService` impl.

**New migration**: Create file in `cas-cli/src/migration/migrations/` following naming convention `m{NNN}_{table}_{description}.rs`. Add to the `MIGRATIONS` array in `migrations/mod.rs`. Each migration needs: unique sequential ID, up SQL, and a detect query. See `cas-cli/docs/MIGRATIONS.md` for full details. Migration ID ranges: Entries 1-50, Rules 51-70, Skills 71-90, Agents 91-110, Entities/Worktrees 111+, Verification 131+, Loops/Events 151+.

### Code index ownership across worktrees

Code-file rows retain their normalized absolute source paths. Reconciliation
only retires absent paths owned by the checkout it scanned; sibling and nested
linked checkouts keep their rows. Retirement and scan receipts derive authority
from configured scan roots; recursive watcher events cannot add a nested checkout.
A full reconciliation uses a fresh scan of those roots. An explicitly configured
nested checkout retains its own reconciliation scope. A configured subdirectory
can retire its own absent files; it preserves the full-checkout scan receipt
until a full checkout root is visited. Paths without an identifiable checkout
remain untouched rather than being treated as another checkout's deletions.

Code scan receipts use `worktree:<canonical checkout root>` in the existing
`code_index_state.repository` TEXT key. Historical repository-name receipts
remain stored but do not certify checkout coverage or HEAD. `cas doctor` and
`cas status` count current eligible, decodable source files from disk. The
manual index command and daemon select the current linked checkout when its
Git common directory matches the explicit store's repository.

A busy BM25 writer defers the remaining retirement sweep after one bounded wait;
source rows remain its retry manifest. `cas index code --json` reports
`files_deferred`, separately from errors, and the daemon schedules a fresh
reconciliation without requiring another filesystem event. Doctor autofix keeps
its warning while retirements are deferred and supplies `cas index code` to retry;
it reports the symbol index fixed only after deferred work and errors are clear.

### Worker browser and JS memory admission

Worker browser/Vitest suites, npm test/build/typecheck/journey scripts and
known Node suite entry points run through
`python3 scripts/worker-memory.py -- <command>`. The worker PreToolUse hook
routes those commands automatically when the checkout has that helper.
Without it the hook warns and retains the existing permission guards.
Plain npm reads, inline Node code and arbitrary Node scripts retain their
existing permission decisions. The hub-web build, typecheck, visual-QA
and verified test entry points also acquire admission when run directly.
The helper owns the command's process group and ends it when the command
returns, so it refuses a routed command that backgrounds a job with `&`:
the suite's receipt launcher would die while a detached runner lived on
unreported. Run a long suite in the foreground of a persistent session with
its output redirected to a log, and read the log.

Admission uses `assembly-proof.py`'s fresh host memory snapshot and reserve
(default: greater of 8 GiB or 25% of physical RAM). A suite needs an assumed
4 GiB for browser/unknown commands, 1 GiB for tsc/Vite builds or 2 GiB for
capped Vitest, plus 2 GiB headroom. Concurrent suites take weighted FD-locked
slots while the fresh budget covers all live reservations and headroom. Literal
shell and npm scripts are classified from their actual commands; unknown or
expanding scripts keep the browser estimate. Smaller commands can use remaining
capacity while a browser waits. Proofs take priority and hold exclusive intent
around all assembly producers and consumers; worker commands wait until those
proofs finish. Legacy checkout budget leases still exclude new admissions.
The linker pool remains separate.
Nested commands reuse admission only while its private claim, live lock and
process ancestry validate; setting an environment flag does not waive it.

Waits print `waiting for host memory (proof running), N s` and memory samples.
`CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS` bounds waiting (default 600 s),
with `CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLL_SECS` resampling (default 1 s).
Expiry fails before starting the suite. A running worker command is terminated
with its own process group if fresh budget falls inside the 2 GiB headroom.
These are admission estimates and sampled protection, not OS memory limits;
other applications remain outside this cooperative protocol.

#### The taste lane beside a proof (cas-833e)

A rolling-integration proof holds exclusive intent for its whole run. Without
a lane, every QA browser run during a sweep waited the full 600 s and expired.
The taste lane admits **one** positively bounded worker run beside a proof:

- **Eligible commands** (`worker-memory.py` `taste_eligible`): light commands
  (tsc, Vite build, capped Vitest), a Playwright run capped at four workers
  (cas-bb5e measured about 2.6 GiB peak at four), and the Commander visual QA
  (one Chromium, sequential pages). Unknown, expanded, piped or backgrounded
  commands are never eligible and wait for the proof as before.
- **Memory rule.** Let `R` be the proof's reserve, `A` fresh `MemAvailable`,
  `E` the run's estimate (at most 4 GiB), `H` the 2 GiB headroom and `F` a
  2 GiB floor. The lane admits only when `R >= E + H + F` and `A >= E + H + F`.
  A proof phase plans to use at most `A - R`, so at the proof's planned peak
  the lane still leaves `R - E >= H + F` free. With the default reserve
  (`max(8 GiB, RAM / 4)`), any host can lend one 4 GiB run. A 64 GiB host
  keeps 12 GiB free at the proof's peak.
- **No planning into the lane.** While `taste.lock` is held, the proof's
  memory budget subtracts the lane's full recorded estimate, even though
  `MemAvailable` already shows part of it. Later proof phases and a newly
  admitted proof therefore never plan into it.
- **One at a time.** `taste.lock` is exclusive. A second eligible run waits,
  printing `proof running; taste lane busy`.
- **Guard.** A lane run sits inside the proof's reserve, where the worker
  budget is about zero by design. It is stopped only if `MemAvailable` falls
  below the floor, which means some estimate was exceeded, and never because
  the proof is at its peak.

Verified frontend tests default to one Playwright worker, honour explicit requests
up to four, and enforce two Vitest workers. A proof's own child script tests reuse
its admitted budget rather than waiting on themselves. Use `TMPDIR` on the
approved scratch volume for fixture staging. Script-level admission tests need
no browsers or Cargo: `python3 scripts/test-worker-memory.py`.

### cas-src close surfaces

Before claiming a change done, workers must add one pre-close task-note line for every applicable surface (and state `not applicable` for the rest): builtin skill/agent → Claude + Codex + Grok mirrors (`cas-8921`); MCP tool → CLI parity, docs, dispatch; hook/gate → `config_gen` + `.codex/hooks.json`; migration → bootstrap/reconciliation and typed doctor status checks (`cas-96f9`/m232); behavior contract → grep sibling old-contract tests (`cas-2327`/`cas-bc13`); state transition → reverse states; user-visible behavior → release-notes impact. This compact walk prevents a tested path from silently missing its sibling surfaces.

### Portable proof artifacts

`artifact action=publish task_id=<id> path=<log> note_type=platform_proof
notes="<platform, command, revision, result>"` publishes and appends a proof note
in one MCP call. Use `loaded_proof` for loaded evidence. A failed Cloud commit
keeps the local artifact record but does not append a portable proof claim.
The note carries the committed record (including Cloud id, size and SHA-256),
so normal task sync transports it without syncing machine-local paths or a
new artifact store. Publishing a producer `bundle.json` also archives the evidence
read by the close gate, including top-level aliases into `parts/`, but excludes
per-part-only receipts and traces. If that archive cannot be prepared or
published (including the 25 MiB limit), CLI and MCP report **QA bundle not
portable** while retaining the manifest record and its storage outcome. JSON
reports `metadata_state: "qa_bundle_not_portable"` and `qa_portability_error`;
rebuild the evidence bundle and republish before asking a peer host to close.
Concurrent restores verify the winning cached evidence before reusing it.

An existing committed id cited in `task action=notes` is
attached the same way. Citing an unpublished artifacts-root path warns the
writer without discarding the note.

On the receiving machine, `artifact action=fetch task_id=<id> id=<art-id>` or
`cas artifact fetch --task <id> <art-id>` resolves that note through the existing
Cloud signed-view endpoint, verifies size and SHA-256 using the artifact
relay's download path, and atomically writes a local copy under the receiving
task's scoped artifacts directory. It imports the record into the existing
artifact ledger, making `show`, `list` and close citations usable there too.
`--json` returns `artifact_id`, `path`, `sha256` and `size_bytes`. No signed URL
or bearer is included in the note, output or saved record. Fetch requires the
Cloud credentials. If the task note is not cached locally, fetch first reads
the task in the configured personal/team project scope, so unrelated account
projects do not block a same-project proof. Scoped entity reads use
`/api[/teams/<team>]/sync/task/<task>?project_id=<project>`. Missing scoped proof
can use bounded cross-project discovery; both paths share one fifteen-second
metadata deadline and validate the returned task/project before reading proof.
Proof interpretation remains the verifier's responsibility. Task note mutation
remains MCP-only; CLI publish is unchanged.

### Artifact revocation

A live registered supervisor can run `cas artifact revoke <id> --reason <reason>`
or call `artifact action=revoke id=<id> reason=<reason>`. Both use the same
registry-backed authority check; a role environment variable alone cannot
revoke. The CLI resolves its nearest harness ancestor through shell/timeout
wrappers and verifies that process against the registered owner's fingerprint;
a nested harness cannot reuse an outer supervisor's identity. MCP keeps its
own front-harness binding. Refusals name the underlying authority failure and
MCP supplies a stable `authority_stage` beside the rule ID.
Cloud authenticates owner/team access but cannot distinguish supervisors
from other team members; supervisor-only authority is a client rule. Unknown
and other-team artifact IDs both return 404. The first immutable local tombstone retains the artifact ID, actor ID,
reason and revocation time alongside the original upload metadata. Repeats
preserve that audit. `show` and `list` retain the metadata and report revoked
access; JSON carries an optional `revocation` object while `status` retains the
original upload phase. Managed fetch, signed views, Commander previews and
new Slack shares refuse tombstoned artifacts, including cached proof metadata.

For a known Cloud object the client sends authenticated
`POST /api/artifacts/{cloud_id}/revoke` with `{"reason":"...","actor_id":"..."}`.
The server contract requires an idempotent audit tombstone, object deletion
(including denial through previously issued signed URLs), and refusal of later
URL/fetch/complete requests. A positive receipt must identify the same object:
`{"artifact_id":"<cloud_id>","revoked":true,"revoked_at":"<RFC3339>"}`.
Only that receipt confirms Cloud deletion locally. A missing client, unsupported
endpoint (including 404/405/501), rejected request or mismatched receipt leaves
`cloud_state=pending` and explicitly reports:
`cloud revocation pending: bytes may still be retrievable`.
The MCP result is an error with the local/Cloud states retained; the CLI exits
nonzero after printing that same partial outcome. Retry the same ID to finish
Cloud revocation; it never replaces the first local actor, reason or timestamp.
HTTP 503 means Cloud retained its tombstone but object deletion failed; this is
a retryable failure and retains the same pending warning and original audit.
After revocation, Cloud URL issuance and completion return 409 with
`error=artifact_revoked`; fetch reports that stable revocation state instead of
an uncommitted upload. Previously issued signed URLs expire within at most ten
minutes under the Cloud contract; deletion and access denial are not confirmed
locally until the positive receipt arrives.
No Cloud ID is reported as `not_applicable`, not as confirmed Cloud deletion.
The operation reads metadata only; it never downloads or reposts artifact bytes.

### Verification tiers and the spawn threshold

Close measures the delivered diff from the task's recorded start to its anchor and records the tier as a task note. `lifecycle/review_tier.rs` owns the path rules for verification and QA, and the highest tier any path reaches applies.

| Tier | Paths | Verification | QA |
| --- | --- | --- | --- |
| 0 | `docs/**` except `docs/design`, `docs/review` and the QA catalog, Markdown outside `src/` that does not instruct agents, `CHANGELOG.md`, `hub-web/e2e/**`, `**/tests/**` | No independent verifier; CI and mechanical close gates still run | No journeys and no ledger row |
| 1 | Everything else, including skills, agents and an unmeasurable diff | In `qa.mode=batch`, one release verdict naming every delivery; an unmeasurable diff, `force-verifier` or `qa.mode=per_task` gets its own verifier | The impacted journeys |
| 2 | Hooks and guards, `.github/workflows/**`, `scripts/release*`, merge-queue and close-gate code | Its own verifier plus an adversarial probe, even when a release verdict names it | An `Adversarial probe:` row in the task ledger, in batch mode too |

A missing assignment, expired heartbeat or retired worker never waives required
verification, including a no-code task carrying external proof. Close reports a
named assignee absent from the registry as no longer registered. A live supervisor
may explicitly request `supervisor_override=true` with a close reason when no
current proof cycle or unresolved legacy verdict prevents it; the waiver records
a durable `Skipped` row. Pending, timed-out and resolved dispatches retain their
exact verdict authority, including an error requiring recovery. Measured Tier 0,
light-depth and other existing policy exemptions remain explicit in close output;
identity availability is reported separately from the exemption actually applied.

Factory deliveries requiring a task verifier prepare a disposable candidate before publication. Candidate preparation binds to the fetched published `origin/<target>` tip, including when the local target carries an unpublished merge; a local-only repository uses its local target. Rolling back an unpublished local merge therefore cannot invalidate a review of an unchanged delivery when the published target advances. The merge queue still checks CI for interactions with other deliveries. The same required verifier runs outside the target lock; PASS publishes the reviewed delivery content and close consumes that approval, with no post-merge verifier or worker close retry. FAIL or an unresolved verdict holds only that delivery. Before mutation, Cassy revalidates the source, target base, task/epic/spec plan, evidence and policy; a changed candidate cannot reuse an old PASS. Unrelated sibling target advances retain approval only when the shared repository-proof overlap rule finds no reviewed-file change and the recomputed delivery content is unchanged. CHANGELOG.md, changelog.d/ and hub-web/dist/ use the shared append-only/generated exemption. The immutable mapping binds the original candidate and actual landed commit; a failed push restores its actual first parent, retaining admitted siblings. A rejected reviewed push restores only its own local merge and preserves unrelated operator edits. Source cleanup and the immutable landing mapping follow confirmed publication. Candidate preparation time is recorded once per task. Completion receipts accept an unpublished source and bind the same candidate atomically with the delivery dispatch. Outside the eligible hotfix fast lane, Tier zero and batch tier one exemptions above are unchanged. Eligible lane deliveries take their own pre-merge verifier; their trusted post-merge receipt replaces batch QA/release waiting, never that approval. Ineligible deliveries retain the normal batch verdict gate.

A server-bound task verifier may finish a pre-merge review after its dispatch
work deadline, including a TimedOut dispatch, when the same source, candidate
tree, plan and proof remain current and no newer dispatch supersedes it. The
record transaction rechecks that exact boundary and consumes the original
one-time handoff. The original deadline remains in the audit; it drives missing
verdict alerts and recovery, not the validity of completed work. Raw bearer
capabilities and unbound handoffs retain their expiry rules. Official server
credentials keep the existing hard 30-minute lifetime, independently of the
10-minute dispatch work deadline; expired credentials always refuse. Changed
or superseded candidates cannot reuse late authority.

A merge queue need not publish the synthetic premerge candidate commit itself.
Close validates that candidate's immutable task/proof and exact target/source
parents, requires the bound delivery source in the authoritative target history,
and reverse-checks the complete binary source-versus-reviewed-base patch in an
isolated index at the first first-parent target commit containing that source.
Source ancestry is still checked against the current target. Later ordinary
edits, including edits to the same lines, do not revoke the landed approval;
a merge that dropped delivery content and another task's candidate still refuse.
Commit subjects and messages locate candidate metadata but grant no authority.

A confirmed publication shares a portable approval through the existing project
peer mailbox. The daemon checks the Cloud-stamped sender against the project
supervisor registry, matches the task/source/candidate/landing, and atomically
imports a peer-attested dispatch and verdict into the receiving store. Repository
paths are rebound locally; acknowledging or retiring a prompt never removes the
proof. Generic verification writes cannot claim peer authority, and replay cannot
replace an existing cycle or revive a reopened delivery. Publication sharing is best-effort: a failed peer never changes confirmed
publication, other peers are still attempted, and the task records the failure.
A later publication retry can recover its abandoned outbox send with the same
logical identity. Receipts must explicitly say approved. Permanent refusals
(malformed, unauthorized, stale, closed or an existing local proof cycle) are
acknowledged as rejected; only transient store, sync, registry or transport
availability failures remain unacknowledged for retry.


An existing dispatch or verdict keeps its authority, and the `force-verifier` label keeps a verifier on Tier 0.

A release verdict is one review of the assembled tip. Publish its report, which must name every delivery by task ID, and record it with `verification action=batch_receipt verification_type=release`. It binds each delivery's anchor as a batch receipt does; for release-owned Tier 1 deliveries it also clears deferred batch QA debt. A corrected anchor needs a new verdict, and a worker that has gone does not let a supervisor close without one. Every covered hold also queues an owner-addressed supervisor action with the complete gate and an ordinary close retry command, even after the implementer retires. Replaying the same receipt does not duplicate its task outcome; repeated evidence for the same anchor and gate does not repeat the supervisor action.

Verifier improvements stay in the verdict; verifiers never file or request tasks. A supervisor staffs a follow-up for a reproduced defect affecting someone today, carrying `repro` and `affected` plus severity, reach and journey. Code-read findings remain theoretical P4 backlog, except theoretical money/data loss on a declared main journey is P2. Visible main-journey failures rank ahead of those hypotheses. `cas qa polish --epic <id>` lists an epic's warning findings by file at release prep, for one polish task per release or none. After a release's scope freeze, findings go to the next board unless they lose data, hide state or hurt someone today.

`task action=create` records finding provenance and derives priority from severity × reach. A follow-up from a finding, meaning an explicit `finding_origin`, a caller that is a verifier or holds a QA task, or text naming a finding, carries the `from-finding` label and a note recording origin, `repro` and `affected`. Missing runtime evidence is retained as theoretical backlog instead of refusing intake. Main reach requires a project-declared main journey and durable nonempty reproduction evidence; caller labels alone cannot attest it. A P0/P1 bug that an operator or supervisor observed directly need not infer an origin from finding-like text, and `finding_origin=none` declares that such text is not a finding; reviewers cannot use it. `qa_record` also retains theoretical pre-existing findings as backlog follow-ups with their triage axes. `lifecycle/review_loop.rs` counts labeled follow-ups against the epic's last 10 completed verifications and QA rounds. With at least 5 reviews, at 0.5 or more the supervisor gets one notice for that epic. `epic_status` shows the ratio whenever the epic has a completed review.

After an exact task dispatch is approved, Cassy closes eligible merged deliveries
through the ordinary close gates. Remaining close recovery, including no-code
worker deliveries, belongs to that dispatch's supervisor; a durable recovery
notice names the task and exact dispatch once. The supervisor runs `task
action=close` after resolving any independent hold, without `supervisor_override`.
Workers never retry close after approval, including when an older runtime queues
such a retry. Replaying the same supervisor verdict repairs the recovery notice
without adding another verdict. Changed repository proof still invalidates the
approval and requires a fresh dispatch.

### Evidence-only task close

Close reasons claiming third-party or future proof must cite a receipt beside
each claim, or say `deferred to <stage>`. An `art-…` citation must exist in the
project artifact store; a local receipt path must name an existing file.
Relative paths resolve from the task's worker checkout, falling back to the
owning project; `~/` expands to the caller's home. Use a qualified path such as
`./receipt.log`, and backticks for paths containing spaces. Run IDs and
`/actions/runs/…` URLs retain their existing citation check without network calls.

Successful QA reports, ledgers and release drafts intentionally retained outside
integration can close through the MCP task tool with `action=close`,
`evidence_only=true`, `evidence_only_artifact_path=<existing durable evidence>`,
`evidence_only_reference=<PR URL or branch:factory/name>`, and a non-empty
`reason`. Only a live registered supervisor can authorize this disposition.
There is no CLI task-lifecycle command; the unified MCP task tool owns dispatch.

After approving a worker's fresh delivery branch, a live registered supervisor
can run `task action=rebind_branch id=<task> branch=<factory-branch> reason="<approval>"`.
Use the caller harness's normal tool prefix. The task must be unparked and keep
its assignee and declared target; use `request_changes` first for a parked delivery.
The assigned worker must have the existing replacement branch checked out cleanly
in its registered repository. Canonical worker branches, the worker's per-task
branch, and that per-task branch with a recovery suffix are supported. Cassy
derives the base from the unchanged target and refuses foreign task history;
callers cannot supply a base that hides commits. The old branch/base/observed tip,
new binding, approving supervisor and reason are recorded together. A missing
old ref is recorded as unavailable. After a history rewrite, rebinding the same
branch name refreshes its obsolete base or unreachable anchor with the same audit.
After a target correction, an empty replacement at the new target is also
accepted when the same assignee's original branch still equals its recorded
start base and the task has no parked branch or current/historical delivery
anchors. Start preserves the old binding until this supervisor approval;
neither branch needs a placeholder commit.
Repeating an unchanged binding leaves its base and audit unchanged. Rebinding does
not change Git, assignment, leases or claims,
and it does not waive proof, merge, QA or verification. The worker closes normally;
the supervisor merges the recorded branch with an explicit task id.

For a gone worker's parked delivery, spawn a replacement with the task id (or
transfer the task to a fresh worker with `adopt_branch=true`). Spawn preassignment
force-releases the stale holder while retaining its start as evidence. Restore
the delivery on the replacement's own task branch in its clean registered
checkout, then start the task. For a transfer that remains parked, use
`request_changes` before starting. The supervisor can now approve that branch with
`rebind_branch`, even though the recorded start names the prior assignee. Rebind
validates the current assignee, lease, repository and task history and records the
prior owner, base, tip and anchor in its audit. The replacement can continue and
close to park; merge its new recorded branch with the task id to finish delivery.
An adoption still refuses a receiver carrying unrelated unmerged work; preserve
that work and use a fresh checkout rather than moving it into this task.

CAS measures the recorded delivery anchor (or the assigned worker branch before
parking) against the fresh integration target. Every unmerged commit must touch
only regular, non-executable evidence formats under `docs/` or `artifacts/`:
Markdown, text, HTML, PDF, SVG, images, JSON, CSV/TSV, logs or YAML. Source changes,
code renamed into docs, reverted code, symlinks and submodules are refused.
Measurement is bounded to 256 commits and fails closed when Git cannot prove it.
The artifact must exist beneath this project's configured task artifacts directory;
local paths and secret-shaped values are forbidden in the portable PR/branch reference.

The successful `evidence_only` terminal outcome counts completed report delivery,
requires no parent-epic code integration, and logs supervisor identity, rationale,
base SHA, delivery SHA and measured paths. Reopening clears the structured receipt.
It cannot combine with negative-result, completion or external-verification receipts,
or close a Gate/Epic. Ordinary closes keep the merge gate; measured negative
experiments retain their separate `negative_result=true` outcome.

### Factory worker MCP and credential access

A supervisor can waive independent QA before a delivery parks with
`verification action=qa_waive task_id=<id> head_sha=<full pushed SHA> summary="<reason>"`.
The supplied SHA must equal the live origin tip of the task's factory branch;
the waiver covers only that commit. Without `head_sha`, the existing recorded
delivery and rebase rules choose the binding. This operation is MCP-only.

Declare resources that stay on the supervisor in `.cas/config.toml`:

```toml
[factory]
supervisor_only_mcp = ["vercel", "neon"]
supervisor_only_env = ["VERCEL_TOKEN", "NEON_API_KEY"]
```

Both lists default to empty and appear in `cas config list`. Server names are
exact.

Operator-authenticated CLIs and connector tools are refused to workers at the
pre-tool hook (GH #1219). `factory.worker_denied_clis` (defaults `vercel`, `neonctl`) names
executables a worker's shell may not run, matched on basename through wrappers
(`env`, `sudo`, `timeout`, `sh -c`, `eval`, `npx`, `pnpm dlx`, paths).
`factory.worker_denied_mcp_tools` (defaults `*vercel*`, `*neon*`) holds case-insensitive
tool-name patterns, also applied to each `server.tool` an `mcp_execute`
dispatches. The refusal sends the worker to the supervisor with
`blocker=true`; supervisors are unaffected. Both keys are operator-only, and
an empty list disables its check. These denials override project proxy credential grants and read-only
server declarations for every worker harness. The supervisor's configuration
and environment are preserved. Spawn diagnostics record denied names only.

An explicit Claude `config_dir` can opt in to selected account connectors while
keeping `--strict-mcp-config`. The account owner provisions a regular file at
`<config_dir>/cassy-worker-connectors.json` (after `~` expansion), for example:

```json
{
  "mcpServers": {
    "claude.ai Gmail": {
      "type": "claudeai-proxy",
      "url": "https://<exact-upstream-from-this-account>/mcp",
      "id": "<exact-connector-id-from-this-account>"
    }
  }
}
```

Copy the exact connector name, URL and ID from this account's authorized MCP
status metadata; the example values are placeholders. The manifest is an
operator grant of the listed connectors, so keep it owner-only. It accepts
those three descriptor fields plus optional native `toolPermissions` (a map of
tool names to `allow`, `ask` or `blocked`), HTTPS URLs without embedded credentials or
query strings, and names beginning with `claude.ai` followed by a space.
Other transports, fields, symlinks and
files over 64 KiB are invalid. A missing or invalid manifest leaves strict
CAS-only launch working and emits a one-line notice, also available in
`CAS_FACTORY_CLAUDE_CONNECTOR_NOTICE`. Configured supervisor-only connectors
and deployment connectors Vercel/Neon are omitted. Claude's managed restrictions
and connector-disable settings still apply; the manifest cannot override them.
When the account has organization tool controls, provision their exact native
`toolPermissions` map too and refresh it when those controls change. Cassy
preserves supplied ceilings; a status export without that metadata does not
prove the account's tool restrictions are preserved.

Only an explicit account parameter reads this file. Host/requester account
inheritance, default workers, supervisors and Codex do not import it. Cassy
copies no credentials and fetches no connector metadata; Claude authenticates
using the selected account. Operator verification must confirm tool discovery
on the actual Claude version and absence of denied/other-account tools before
relying on this path. A visible connector does not enforce read-only use.
The `inherit` mode above retains its existing semantics and ignores manifests.

The existing protected-operator-token policy removes inherited `NEON_API_KEY`
and `VERCEL_TOKEN` from workers by default, even though `supervisor_only_env`
is empty. Explicit project credential grants can authorize these names;
`supervisor_only_env` still wins over grants. Changing a CLI/tool denial list
does not grant a credential. Workers retain their own `.env.cas-db` and its
`DATABASE_URL`; the supervisor provisions that disposable task branch with
`factory action=db_branch_create`. Supervisors retain Neon CLI/MCP access and
their credentials. An explicit CLI/tool list replaces its stock defaults,
including an empty list to disable that check.

Configured workers get a private, materialized MCP configuration under
`<cas_root>/worker-mcp/<name>.json`. Tracked and existing worktree `.mcp.json`
files stay unchanged, so provisioning keeps the tree clean and cannot commit
supervisor-only removals into the project. Claude's default `cas_only` mode
uses `--strict-mcp-config` with only the project CAS definition (or a generated
`cas serve` definition), including when CAS was registered only in local scope.
`factory.claude.worker_mcp = "inherit"` restores native discovery only when the
MCP/environment denial lists are empty; otherwise it refuses before discovery.
Private worker configurations use the same store path, including shared-cwd
workers. Codex disables the named native MCP servers, and
its Cassy proxy filters those upstreams before startup and reload.
Codex addresses dotted or otherwise special server names with a quoted key in
a parent-table override so the name remains literal.
Worker snapshots cannot overwrite the supervisor's shared proxy catalog/health.
Listed environment names are removed from inherited and explicitly granted
values, including machine credential bootstrap and retained proxy stdio
servers' explicit environment maps, before credential resolution and reload.
Codex workers remain spawnable with environment restrictions. Before launch,
Cassy parses the selected user Codex home (`CODEX_HOME` or `~/.codex`) and the
project `.codex/config.toml` files along the launch directory's ancestors,
without running native inventory or discovery. A server whose literal `env`
table contains any denied name is disabled for that worker. Keys retained
from any parsed layer count, even if a later layer changes their values.
Other native servers remain enabled, and the supervisor's files stay unchanged.

This inspection covers parsed TOML, not expanded plugin or runtime
contributions. Unknown plugin/layer contributions and unreadable or malformed
Codex files produce one warning naming the unevaluated sources; the worker
still spawns. Review those contributions before granting credentials. Process
environment removal and the explicit supervisor-only server overrides still
apply. This bounded policy deliberately preserves the standard Codex worker
lane; it does not promise complete isolation of unknown native contributions.
Grok and other native harnesses without supported per-launch MCP/environment
isolation refuse a worker launch when either list is nonempty. They retain
ordinary native discovery with empty lists; the supervisor remains unrestricted.
The refusal does not select a different harness or provider. Invalid
configuration or a failed materialization also refuses the worker launch.

Deployment and production operations needing denied resources run through the
supervisor. Workers never source an interactive shell to obtain operator
credentials. Existing explicit project grants and the operator-provisioned
read-only GitHub token remain available unless denied here; this policy adds
no credentials to any harness.

### Claude workspace trust and the hook canary

Claude Code runs no hooks (SessionStart, PreToolUse, from any settings source)
in a workspace it has not trusted, and factory agents launch with `IS_DEMO=true`,
which skips the trust dialog without trusting. Before cas-0f5b every Claude
worker therefore ran unguarded: no capped cargo runner, no worker-memory
admission, no Slack, publication or browser guards. Two pieces now close that:

- **Trust at spawn.** The Claude backend's `prepare_workdir` merges only
  `projects["<cwd>"].hasTrustDialogAccepted = true` into the agent's
  `$CLAUDE_CONFIG_DIR/.claude.json` (or `~/.claude.json`), under the shared
  `.claude.json.cas-lock`, via temp file, fsync and rename, then re-reads it
  and retries once (live sessions rewrite the file). A config it cannot parse
  is never rewritten; the launch is refused instead. The supervisor's own
  checkout is trusted the same way.
- **The canary refuses.** A factory agent's SessionStart hook writes
  `.cas/factory/hook-canary/<agent>.json`. Spawn verification (including
  respawn and recycle) kills a Claude worker whose marker has not appeared
  within 60 seconds of launch, marks it crashed and tells the supervisor.
  There is no silent degrade.

A canary refusal retains the bounded, redacted worker boot output in the spawn
record before removing the pane. Inspect a retained failed request with
`cas factory debug <worker-or-request-id>` or
`cas factory is-wedged <worker-or-request-id> --json`, even when the harness
never registered. The JSON reports `boot_state: "failed"`, its worker/task and
request identity, and `state: "unverified"` (exit 4): a failed admission alone
does not establish process death. Only a completed PTY teardown releases the
matching preassignment; an unconfirmed teardown retains it for investigation.
These read-only records do not authorize `kill` or permission responses.

Rollout: after this merges, respawn every live Claude worker. Hooks start
denying things those workers never saw before: raw `cargo`, `&` background
jobs under worker-memory admission, non-Violet Slack writes, publication and
unfiltered browser runs. Codex workers are unaffected (their trust was
already pre-seeded by `codex_trust.rs`).

### macOS Claude account linking

`cas accounts link` asks `claude auth status --json` about each macOS Claude
profile. Both Claude directory selectors bind named profiles; the default
profile keeps the legacy unscoped Keychain store with its own home. Active
or unavailable authentication keeps the original directory in place, including
when a credential file also exists. Only confirmed logged-out profiles follow
the ordinary rename/retirement rules. Shared items can still link to the base;
keeping an authenticated default at its original path is not itself drift.
Link health and launch-time repair use the same preservation rule. No raw
credential or Keychain secret is read. Account unit fixtures replace only the
external status executable through `CAS_TEST_CLAUDE_BIN` in test builds.

### Factory worker account selection

`factory action=spawn_workers` accepts an optional `config_dir` for all
workers in the request. Claude workers use the tilde-expanded directory as
`CLAUDE_CONFIG_DIR`; an explicit parameter wins over the requesting
supervisor's own `CLAUDE_CONFIG_DIR`, which is captured when the request is
queued so the daemon cannot silently substitute its environment. With neither
value, spawning retains ordinary daemon-environment inheritance. Codex and
Grok workers ignore a resolved Claude directory and emit a warning. Only an
explicit `config_dir` removes `ANTHROPIC_API_KEY`, because that key overrides
Claude subscription OAuth; propagated supervisor settings retain existing API
key inheritance.

Explicit Claude profiles may omit `settings.json` (empty settings); an existing
file must be readable JSON. Preflight checks macOS OAuth credentials by Keychain
item existence, using Claude Code's `Claude Code-credentials-<8 hex>` service
(first eight SHA-256 hex digits of the NFC-normalized config path). It uses the
current user's default Keychain search list and never requests the password.
Other platforms still require `.credentials.json`. Both paths require resolvable
`agents` and `skills` directories before queueing a spawn.

For CLI chores that need operator credentials, explicitly grant environment
names in the project's `.cas/config.toml`:

```toml
[factory]
worker_credential_env = ["GITHUB_TOKEN", "VERCEL_TOKEN"]
```

This list defaults to empty, appears in `cas config list`, and applies equally
to Claude and Codex workers. Only listed names override the default removal of
protected operator tokens. Values come from the operator environment or the
existing private credentials-file/login-profile reader; Cassy never executes
an interactive shell. Configure these grants only when workers should perform
the associated operations. Factory identity variables (`CAS_*`, except
protected credential names such as `CAS_CLOUD_TOKEN`) cannot be granted.

Missing names generate one names-only warning in the worker spawn receipt and
the worker still starts. A name also listed in `supervisor_only_env` stays
denied, with the conflict named in the same warning. An operation needing an
unavailable credential then fails visibly; one missing token does not stop the
fleet. Supervisors keep their own credentials and configuration.

With no explicit `config_dir` and no captured requester account, Claude/Codex
workers use the shared recommendation. The receipt names the account and reason;
an explicit directory or requester account wins. `cas claude` / `cas codex`
and Commander's New-session sheet highlight the recommendation but still ask.

`cas config set --global accounts.selection soonest-reset` (default) chooses the earliest
weekly reset group, within 24 hours of the earliest eligible reset, then the
most weekly and 5h headroom. `most-headroom` ignores reset proximity. Set
`accounts.threshold` (default `90`, range `1..100`) to exclude accounts at or
above that usage in either window. Disabled, logged-out/stale and unknown or
stale usage accounts are excluded. No eligible account preserves the existing
default/inherited behavior, with each exclusion explained. Project selection/threshold
override global policy; Commander uses global policy. `accounts.disabled` retains
the absolute folder list managed by `cas accounts disable` / `enable` in
`global_cas_dir()/config.toml` (XDG config on Linux), regardless of project policy.

`cas accounts rename` and `cas accounts remove` refuse accounts held by readable
process environments or registered agents. On macOS, idle Apple system shells
(`/bin/zsh`, `/bin/bash`, `/bin/sh`) can hide their exported `CLAUDE_CONFIG_DIR`,
`CLAUDE_SECURESTORAGE_CONFIG_DIR` or `CODEX_HOME` from process inspection. A local
reproduction held each shell on a blocking `read`, confirmed the selector inside
the shell, then inspected it with `ps eww` and direct `KERN_PROCARGS2` sysctl calls:
the calls succeeded but returned arguments without the selector. Calling sysctl
directly therefore does not recover it. This is a known macOS guard limitation;
close shells that export the account's selector before renaming or removing it.
A later launch from a stale shell can recreate the old profile folder. Readable
Claude/Codex descendants of these shells and registered agents remain checked;
an unrelated opaque shell does not block every account mutation.

The session catalog also reports optional `mcp_version` (the live supervisor's
fingerprinted MCP daemon epoch) and `installed_version` (that machine's
installed CLI). Both reuse the Factory MCP versions source; MCP observations
match supervisor role, name and session. Conflicting live MCP versions remain
unknown. A ten-second per-project cache bounds epoch/process probes. Neither
field borrows the hub's own binary version; absent versions are omitted.

The hub session catalog reports the supervisor's optional `harness`, `model`
and `effort`. Model version is part of the reported model ID or display name
(for example `claude-opus-5-5` or `gpt-6.1-sol`), not the CLI version. The
additive `agents` array carries `id`, `name`, `role` and the same optional
identity fields for registered supervisors and workers in that session;
`workers` remains the existing array of worker names. `/status` uses these
role-specific identity fields in its `agents` rows too. Absent observations
are omitted so Commander can show “Not reported”. Neither endpoint borrows the
hub process's model, worker defaults or another session's metadata.

Factory launch records the model and effort explicitly passed to that pane.
Claude's native `SessionStart.model`, `PostModelSwitch.to_model` and effective
hook `effort.level` (or `CLAUDE_EFFORT`) replace launch evidence. Hooks are
installed by `cas hook configure`; Codex's generated native hook projection
keeps its supported lifecycle/tool events. A confirmed Codex Commander `/model`
selection records the model and effort from its fresh native success cell.
Opening a chooser, navigation, cancellation and failed saves do not change the
record. A changed model clears an unreported old effort; repeated MCP startup
and compaction cannot restore the original launch defaults over an observation.

The shared account registry reads Claude email/account/organization metadata
only from non-secret `.claude.json` `oauthAccount` fields. It never opens
provider credential files for catalog metadata. Codex email, account and
workspace identity stay absent: `auth.json` and its `id_token` are not metadata
sources. Codex plan and capacity come from native rollout `rate_limits` fields;
without those observations the plan stays unknown. Login state still comes
from the official CLI status command.

Codex folder aliases remain launch keys. Without a non-secret identity source,
email-based lookup, automatic email renaming/retirement, login duplicate
prompts and identity-based sharing of observations across distinct Codex
folders are unavailable. Explicit aliases and physical symlink deduplication
still work. Claude's non-secret identity behavior is preserved. The registry's
optional fields and interface remain unchanged for Commander consumers.

### Operator credential stores are masked from workers

Workers share the operator's uid and `$HOME`, so removing environment tokens
does not hide credential *files*. On Linux, when a non-empty Neon or Vercel
store exists (`neonctl` and `com.vercel.cli` under `$HOME/.config`,
`$HOME/.local/share`, the XDG roots or `VERCEL_CONFIG_DIR`), `Pty::spawn`
starts the worker under bubblewrap (`crates/cas-pty/src/store_isolation.rs`):
a user and mount namespace that shows the host filesystem, PIDs, environment
and working directory unchanged, but covers each store with an empty
read-only tmpfs. No reader inside the worker can see a store: interpreter
one-liners, piped shells, variable indirection, archives, or
`/proc/<host pid>/root`. A user-owned copy of `/etc/ssh/ssh_config.d` is
bound in so that ssh and `git push` still accept their config. Root-owned
files appear owned by `nobody` inside the namespace, and `sudo` is
unavailable.

- No store to hide: the launch is unchanged.
- A store exists but `bwrap` or unprivileged user namespaces are unavailable:
  the spawn fails with a remedy. Install bubblewrap, or set
  `CAS_WORKER_STORE_ISOLATION=off` in the factory daemon's environment to
  accept the exposure.
- macOS has no backend yet. The worker starts with a warning, and the
  PreToolUse credential-path guard is the only control there. This is a
  known gap.

Supervisors and plain sessions are not wrapped. The PreToolUse guard still
runs for every worker as defense in depth.

### Tool-cache paths in Codex children

Cassy forwards these four tool-owned variables through Codex's restricted
`cas serve` MCP environment: `NODE_COMPILE_CACHE`, `PWTEST_CACHE_DIR`,
`PYTEST_DEBUG_TEMPROOT`, and `V8_COMPILE_CACHE_CACHE_DIR`. Servers started through
that MCP process inherit them. Export the desired paths before starting the
factory or its owning daemon, for example:

```sh
export NODE_COMPILE_CACHE="$HOME/.cache/node-compile"
export PWTEST_CACHE_DIR="$HOME/.cache/playwright-tests"
export PYTEST_DEBUG_TEMPROOT="$HOME/.cache/pytest-temp"
export V8_COMPILE_CACHE_CACHE_DIR="$HOME/.cache/v8-compile"
```

Factory spawns forward the final pane values, including explicit overrides;
`supervisor_only_env` removals still win. Changing a shell or user-manager
environment does not change an already running process's environment. Cassy
does not read these paths back from shell profiles or the user manager.

`cas init` and `cas update` also append these names to the managed
`mcp_servers.cs.env_vars` array in Codex configuration, preserving existing
entries and explicit `cs.env` settings. Only names are stored there, so each
Codex launch reads its own environment. An unset variable adds no value or
default; an explicitly configured MCP value retains its existing behavior.
No other cache, credential, or account variables gain passthrough.

The forwarding policy uses Codex configuration on supported factory platforms;
it does not depend on systemd. Paths, spaces and Unicode are forwarded without
expansion or rewriting. The consuming tool determines platform support, cache
creation and cleanup. Cassy does not migrate existing caches or restart servers.

## Testing

New QA ledgers use `schema: 2`. Graded runtime expectations cite a spec
`path:line`; PASS observations include captured `visible:`, `terminal:` or
`http:` output. Database-only PASSes and uncited verdicts are refused by the
shared CLI/close/verifier parser. Schema 1 receipts remain valid; fixture
labels remain weaker evidence. The shipped `cas-qa-craft/scripts/walk-kit.mjs`
is copied and pinned once per project under `scripts/qa/`, with project adapters
for selectors, actual served builds and cleanup. Every generated row identifies
the kit version. Run its regressions with `node --test scripts/test-walk-kit.mjs`.

### Duplicate-task warnings

Task creation excludes common planning words and prose such as `NOT` and
`before/after` from distinctive identifiers. Generic-only title overlaps
require near identity; exact duplicate titles and concrete code/path overlap
still warn. Intentional duplicates retain the `confirm_warning=true` escape.

### Assignment on task creation

When `epic`, `target_repo` and `target_branch` are all omitted, task creation
inherits the caller's active task's epic and exact WorkTarget together. A leased
epic supplies its own ID and live delivery lane, so an automatically leased
parent agrees with its child. Creating a new epic never uses this default: it
gets its own branch and target. Only valid leases on InProgress tasks count;
held or expired work does not. Multiple eligible tasks must agree on both
bindings, or creation refuses and requests an
explicit binding. The receipt names the source task, epic, repository and branch.
Any explicit binding keeps the explicit resolver: an alternate epic follows
that epic's lane, and `epic=""` deliberately creates standalone work. Omitted
fields are never inherited independently. Parent edges record the caller's
identity, so the caller's own epic updates and dependency additions do not
trigger a planning-race warning; legacy anonymous `mcp` edges prove no other
planner's identity.

In factory mode, `task action=create assignee=<worker>` uses the same worker
identity normalization and freshness policy as `task action=update assignee=`.
A refused assignment rejects the entire create before saving a task or queuing
a worker prompt. Successful creates show the stored assignee, including any
normalization or freshness warnings. `task action=show` always shows the stored
assignee or `(unassigned)`. Parked-delivery and clean-idle reset exemptions,
warning-only configuration, and assignments to workers not yet registered retain
the update policy.

### Task start cancellation

`task action=start` commits its local lease and InProgress status before awaiting
Cloud handoff or parent-epic claim work. Those requests share the existing
8-second peer budget. A response deadline after the local transition reports
COMMITTED, including when notification and sync wrappers are disabled. Re-query
the task before retrying. A failed lease acquisition leaves status unchanged;
a failed status write releases only the lease generation acquired by that start.
A sync-publication error after the local commit keeps the matching lease.

Cloud acquisition and local lease publication share one per-task claim fence.
Synchronous release and pending-release retry take that fence before inspecting
a local lease or releasing its Cloud mirror. Operations needing both locks
take the per-task fence before the global pending-records lock; the global lock
never spans a network call. Settled fences release their OS lock but retain
`.cas/claim-release-locks/<task-hash>.lock`: one small file per encountered task.
Do not unlink, replace or truncate these files during normal operation.

Only unreleased v57 development builds deleted settled fence files; released
3.71.0 has no per-task claim fence. In those development builds, a generation
check protected only callers that used it. An older development process could
open a file before settlement deleted it, then lock that detached inode while
another process held a replacement. Keeping the same inode prevents that split;
generation checks cannot repair an already detached old handle. Retire every
unlinking development process to end that existing window.

**3.71 → 3.72 rollout:** after installing 3.72, restart every `cas serve`, hub
and worker session on the host, including harness-owned MCP servers and pinned
sessions. A still-running 3.71 process takes no per-task fence and can race a
3.72 process on the same task. This restart requirement applies to released
3.71 installations as well as unreleased v57 development builds.

`cas update` replaces the binary and can restart the managed hub; it does not
restart harness-owned stdio `cas serve` processes or worker sessions, or verify
that every running process has loaded 3.72. Binary replacement and a hub restart
alone do not complete this rollout. End the old sessions and relaunch them with
the installed 3.72 binary before resuming task claims.

Fence-file removal belongs only to offline GC after explicit quiescence.
Prevent every operation from holding or reopening these files throughout
collection. Verify that no live process uses an older fence protocol, or reload
every `cas serve`, hub and pinned session on the persistent-fence build before
collecting. Prevent older executables from starting again during collection.
Age, an idle agent row or a file version marker is not proof. There is no
automatic online fence-file GC; retained files are intentional compatibility
state.

Windows claim fencing has not been exercised and remains an implementation
limitation. The generation recheck in `lock_task_claim_release_with` reads the
path through a second handle after taking the exclusive whole-file lock.
Windows mandatory locking can reject that read even without an old process or
external cleanup. Persistent files and restarting sessions do not resolve this
second-handle problem. A Windows fix must preserve path-identity validation as
well as read through the locked handle; reading that handle alone would miss a
replaced path. No Windows asset is built by the release workflow today.

On a fence acquisition error, acquisition reports PEERS NOT CHECKED and continues
with the local lease, synchronous release preserves the Cloud claim, and a
pending release stays queued for retry. Windows cross-process claim exclusion
is therefore **not verified**; do not rely on this advisory fallback for it.

If the peer budget expires just after start cancels a pending release, Cloud
acquisition can still be unavailable. Start reports PEERS NOT CHECKED and takes
the local lease; normal claim renewal reacquires the missing Cloud mirror. The
cancellation protects the new local holder from the old release and is retained
in this advisory fallback.

### Task lease release

`task action=release` lets a live registered supervisor release a worker's
lease and records the supervisor identity in lease history. Other callers
may release only their own lease; `force=true` does not grant that authority.
Releasing an InProgress task returns it to Open and clears its assignee.
AwaitingMerge retains its delivery state, including when no active lease remains.

### Branch-only target correction

`task action=update target_branch=<branch>` preserves the task's repository
binding or defaults a legacy targetless task to the current project repository.
The corrected branch is validated, so a deleted old epic branch can be repaired.
The same default applies to supervisor `proof_scope_fix=true` corrections.
An unchanged correction leaves the proof cycle intact and reports the last
recorded close rejection for that task, including pre-close hook failures.
Retry `task action=close` to refresh the current gate before correcting scope
again; historical diagnostics do not replace a fresh close attempt.

### Server-list MCP contract

`factory action=server_list` reports verified running servers by default.
Use `status=stopped`, `status=dead`, `status=unverified`, or `status=all` to
inspect history or entries whose process identity cannot be verified.
`task_id` filters by exact owning task; `owner` accepts an exact worker name
or registered agent ID. Filters combine. `limit` defaults to 20, must be
positive, and is capped at 50. Each server occupies one line of at most 512
bytes; long fields and port lists are abbreviated. A truncation notice gives
the number of matching entries. The legacy coordination route uses the same
filters and limits. Regression coverage lives in `server_registry_mcp_test`
and the service's `server_ops_tests`; no new CLI command is introduced.

### Task branch adoption

For an inherited factory delivery, the supervisor runs `task action=transfer
id=<task-id> to_agent=<worker> adopt_branch=true supervisor_override=true`
from its own context. Cassy adopts the task's recorded delivery on
`factory/<receiver>-<task-id>` in the receiver's registered worktree, then
updates its assignment while retaining that delivery tip as the anchor. Commit
there; the isolation guard still rejects the old owner on that branch.

The receiver must have a clean isolated worktree in the task's repository,
with its own HEAD able to fast-forward to the delivery or already containing
it. A HEAD or existing receiver task branch ahead of the delivery is retained
only when the integration target also contains it; commits outside that target
are refused. Push deliveries use the origin target ref when present; local
merges use the local target. If the receiver is already on this task's exact
source branch, adoption copies the recorded tip without moving the source,
even when that source has advanced. An existing receiver task branch ahead of
the recorded tip still needs the same integration proof.
No worker checkout is needed first. Dirty worktrees, divergent receiver
branches, unrelated foreign branches and other repositories are refused before
assignment or lease changes. The original branch is preserved as handoff
history. Omitting `adopt_branch` keeps the existing assignment-only transfer
behavior.

### Ordered worker task queues

Queue one worker's work with `factory action=spawn_workers count=1 task_ids=["cas-a","cas-b"]`.
Use either `task_id` or `task_ids`; a batch-level task list belongs to one worker.
Ordered spawns default to `isolate=true`; explicit `isolate=false` is refused.
An existing worker also needs its own registered factory checkout for queue assignment.
The first task controls the spawn base. Provisioning durably binds the queue
by factory session and worker name. Registration selects its first eligible task;
a head blocked, closed or assigned elsewhere during boot cannot strand the tail.

For an existing worker, `task action=update assignee=<worker> task_ids=["cas-b","cas-c"]`
replaces pending work. If `id` is supplied, it must name the first entry. This
form accepts queue fields only; apply title/status/other task edits separately.
An idle worker starts the first eligible entry immediately. Future entries
stay unassigned until selected.

Inspect with `factory action=worker_queue id=<worker>`; `worker_status` also
shows the current task, ordered pending tasks and last queue decision. Replace
or reorder with `factory action=worker_queue id=<worker> task_ids=["cas-c","cas-b"]`.
Clear pending work with `task_ids=[]`. Add `expected_generation=<revision>` to
reject a stale queue replacement. Queue edits require the worker's live owning
supervisor in the same factory session; they preserve the current execution.

A successful close or park releases the preceding lease, then starts the next
eligible task through the normal start gates as that worker. It cuts the next
task's branch from its own declared target and preserves the previous delivery
ref. Repeated closes cannot consume another entry. An execution that becomes
blocked keeps its current slot until it resumes, closes or parks. Reassigned, missing,
terminal, blocked, foreign-leased and hard-start-gated entries are skipped with a
durable note. A checkout/start failure retains the selected entry and records a
deferred reason. The daemon reconciles durable cursors off its control loop,
retrying after lock contention, restart or an unavailable brief recipient.
Inspect and resolve the deferred cause; no repeated close is required.
Queue dispatch never changes process identity or environment variables.
The single-active-task start gate applies while a worker has a current,
reserved or pending queue entry; workers without a queue retain their existing
start API behaviour.

If delivery A is returned while queued successor B is running, checkpoint B's
unfinished changes, then run `task action=update id=B status=blocked`. This
releases B's lease and preserves its current queue slot. Switch to A's recorded
delivery branch (shown by `task action=show id=A full=true`), then run
`task action=start id=A`. Correct and re-deliver A on that branch. After A closes
or parks, switch back to B's recorded branch, run
`task action=update id=B status=open`, then `task action=start id=B`. Pending
work stays behind held B throughout recovery; B does not need an unfinished park.

### MCP mutation timeout receipts

The MCP response budget is 55 seconds; timeout diagnostics report the measured
elapsed time and budget separately. Message enqueues and task writes carry
request-scoped commit evidence. A timeout after an observed commit reports
`COMMITTED`; message receipts include `notification_id`. `UNKNOWN` means this
request's commit was not confirmed. Re-query state before retrying either case.
The error's structured data includes `mutation_outcome`, `notification_id`,
`elapsed_ms` and `budget_ms`. A read-only `search action=search` deadline
reports `mutation_outcome=NOT_APPLICABLE` without write-reconciliation advice.
Its `timings` field and tracing retain inclusive `search_index_open`,
`search_task_load`, `search_task_refresh` and `search_lookup` measurements;
`+` marks a stage still running. Even exact task-ID queries currently load all
canonical tasks and reconcile the task index before lookup, with a full task
rewrite on the first request in each server process. `include_source` applies
to code search, not this context search action.

Factory merge and task-close handlers run synchronous Git/store work on the
blocking pool. Once publication or the checked Closed/AwaitingMerge disposition
commits, the response adds no post-commit grace: it names that outcome while
close follow-up, dispatch and notes finish in the background. A committed park
is still awaiting merge. Gate/audit writes alone do not certify a close, and a
close still evaluating gates reports `IN_PROGRESS` at its deadline. The final
continuation outcome is recorded in task notes. Receipts and tracing include
inclusive merge-tree, push, close-gate, dispatch and note timings; `+` means a
phase is still running and zero means no measured work in that phase yet.

Optional resource notifications and factory recall have bounded response waits.
Late recall output is retained for the next successful response, including mail
already consumed by the background reader. Message delivery remains asynchronous;
use `coordination action=message_status` to inspect handoff and recipient evidence.

For a gated message, status distinguishes a **held inbox** (a successful Teams
write; surfaces at the next tool call or turn start without expiring while the
recipient remains in the session) from a **delivery retry** (no held handoff
recorded; transport failures spend the retry budget). A protected operator draft,
pending submit or settling urgent inject is a recoverable readiness hold: it
spends neither transport retries nor re-nudges and is re-offered when the pane
becomes input-ready, with a two-minute durable recheck as a restart fallback.
Default status and doctor warnings allow pending relays sixty seconds to arrive;
explicit detailed status also lists fresh holds and their blocking cause.
A fresh transport reservation clears any older inbox deferral before injecting,
so a worker that changes harness cannot have its PTY delivery replayed by a hook.
The actual primary channel decides whether a handoff can be held: a missing
Teams folder falls back to PTY and never receives a new inbox deferral stamp.
Status checks the deferral, current transport claim and absence of completed
transport in one database snapshot; a stale marker alone cannot imply a hold.
If the deferral stamp cannot be saved, the Teams copy remains available at the
turn boundary and Cassy queues one supervisor warning per affected message.

Abandonment digests keep the first batch within the hook's message budget by
showing whole IDs plus an omitted-ID count. The episode's `+N more` trailer
counts abandoned messages, including every message in a later folded batch.
Full delivery state remains on each original message for `message_status`.

Integration tests are in `cas-cli/tests/`. Key test files:

- `cli_test.rs` — CLI command integration tests
- `mcp_tools_test.rs` — MCP tool handler tests
- `mcp_protocol_test.rs` — MCP protocol compliance
- `factory_server_test.rs` — Factory WebSocket server tests
- `distributed_factory_test.rs` — Multi-agent factory tests
- `proptest_test.rs` — Property-based tests
- `e2e_test.rs` / `e2e/` — End-to-end tests
- `team_sync_test.rs` — `cas cloud sync` team-queue drain path
- `memory_share_test.rs` — `cas memory share|unshare` CLI behavior
- `team_memories_e2e_test.rs` — end-to-end team-memories flow (share → push → pull)
- `artifact_publish_test.rs` — `cas artifact publish` path guard, size ceiling, and the assertion that a signed upload URL never reaches disk or stdout
- `credential_debug_guard_test.rs` — repo-wide guard: no struct holding a token, API key or pre-signed URL may derive `Debug` (a derived one prints the credential verbatim). Add a redacting `impl fmt::Debug` plus a test rather than an allowlist entry.

### Terminal output gate

Anything a command prints for a person (`cas doctor`, `cas update`, `cas factory status`, a
table, a progress line, an error) is designed under the `cas-cli-craft` builtin skill and gated by
`scripts/terminal-qa.mjs`: it runs the command in a pty at 80 and 120 columns on the dark, light
and both Solarized palettes plus piped, `NO_COLOR` and `LC_ALL=C` runs, and fails on wrapped rows,
split tokens, colour under 3:1 (marks) or 4.5:1 (text), truncation with no escape flag, glyphs on a
C locale, SGR under `NO_COLOR`, redraws in a pipe, and a `--json` stream that is not one document.
BSD/macOS and util-linux `script` are supported. Empty captures fail; unavailable runners
exit 2 without retaining a PASS receipt. Diagnostic stderr is captured and checked, while
JSON stdout stays separate. Expected nonzero command exits do not fail rendering QA.

```bash
node scripts/terminal-qa.mjs --label cas-doctor --escape-flag --verbose --json-flag --json -- cas doctor
node --test scripts/terminal-qa.test.mjs   # the gate's own suite (planted-defect fixtures)
```

Paste the `terminal-qa: PASS …` receipt line into the pre-close note; briefs and captures for the
shipped commands live in `docs/design/cli/`.

Dev dependencies include: `insta` (snapshot testing), `wiremock` (HTTP mocking), `rstest` (parametrized tests), `proptest` (property-based), `criterion` (benchmarks), `cas-tui-test` (TUI testing).

## Git hook authority and checkout moves

`cas doctor` reports an effective configured `core.hooksPath` whose directory
is missing. Git resolves the path from the checkout root, including relative
paths and worktree overrides. When doctor runs in a worker sharing the main
checkout's store, it reports the main checkout and active worker separately.
This finding is read-only, including under `--fix`; a stale host-registry entry
is a separate diagnostic.

Before repairing a moved checkout, run these **in each affected checkout**:

```bash
git config --show-origin --show-scope --get core.hooksPath
git rev-parse --path-format=absolute --git-path hooks
```

Review the old configuration and any existing hook files, and save a backup
before changing authority. Never unset `core.hooksPath` to make a warning go
away, or point the main checkout at a worker's `hooks-cas-guard` directory.
Dependency-generated hooks such as `.husky/_` belong to that project's setup;
doctor does not install dependencies or disable them.

For this repository's **main checkout**, an operator outside a factory session
can run `bash scripts/install-git-hooks.sh` after reviewing the source. It uses
the committed executable `.githooks/pre-push` and existing
`scripts/fast-rows-pre-push.sh`. It refuses factory identities, linked worktrees,
uncommitted/missing source and existing executable hook authority. A reviewed
repair of an existing authority requires preserving/backing it up separately;
the installer never overwrites it. Repeating the same owned installation is
safe. With `extensions.worktreeConfig` enabled it writes the main checkout's
`--worktree` override, preserving the shared setting and worker overrides.
There is no repository pre-commit or Clippy policy installed by this script.

The main pre-push delegate enforces the existing no-build fast rows for pushes
to `factory/*`; other refs pass through. Receipts live under
`<git-common-dir>/cas/fast-rows/<tip>.pass`. Only an exact-tip PASS is reusable;
a failing row or missing tool refuses the push. This source is not a worker
ownership guard.

**Workers need their own private guards.** Factory spawn and reuse call
`TeamsManager::install_worker_pre_commit_hook`, which installs the commit guard
and refreshes an older push guard in `<worker-git-dir>/hooks-cas-guard`, retaining
its existing project-hook chain and `--worktree` scope. Installing a new Cassy
binary alone does not refresh already running workers. The supervisor must
coordinate a safe spawn/reuse after the new runtime is active, then inspect the
effective worker pre-push hook for delegation and prove a rejected push against
a disposable remote. Existing 3.62 private guards may still enforce ownership
while lacking the fast-row delegate; inspect them rather than assuming absence.
Do not copy those guards into the main checkout. A hook-install warning during
spawn/reuse means enforcement is not proven.

At park, `receipt_scope::park_refusal` reads passing capped test receipts for
every changed Rust crate; missing tests produce `FOCUSED PROOF REQUIRED`.
Park executes neither checks nor tests. Local fast-row receipts are not park
proof because workers can write them, and park reads no fast-row exception
file. CI runs the fast rows before merge admission; the pre-push hook provides
early local feedback, and a reason-bearing `CAS_SKIP_FAST_ROWS` push is logged
to `overrides.log`.

Close measures a delivery's recorded `parked_branch` (local, then origin)
before deriving branches from its current assignee. Its recorded `work_target`
remains authoritative after parking, including squash-integrated deliveries;
a later epic lane does not redefine that proof boundary. Ownership, start-range,
publication and receipt checks still apply. QA reads project-scoped evidence
before historical flat task directories. A flat directory is a fallback only
when the task explicitly cites its historical bundle and no scoped QA evidence
exists; a colliding short task ID alone never selects another project's ledger.

## Project capture configuration

`cas config list --section qa.capture` lists the project capture settings.
Use `cas config set qa.capture.base_url http://localhost:3000` for an existing
target, or set `qa.capture.serve` to a checkout server command with `{port}`.
`ready_path` defaults to `/`; `shots` defaults to `docs/qa/shots.json`.
`mask` is a comma-separated selector list. `viewports` defaults to `1280x800`;
`390x844` is available for shots that request mobile. `max_shots` defaults to 6
and `timeout_secs` to 240. `max_html_mb` is a capture-side ceiling of 8 MB;
report embedding budgets belong to finalize.

For authenticated surfaces, set `login_path`, an anchored `success_url` such as
`^https://[^/]+/dashboard`, and `credentials_env` to an environment-variable
**name**, for example `PROJECT_QA_CREDENTIALS`. Optional `email_sel`,
`password_sel` and `submit_sel` override the standard form selectors. The named
variable must resolve to an existing readable credentials file at capture time.
Set validates the variable name without requiring it in the setting shell;
neither secrets nor resolved file paths are stored in project configuration.

Consumers use `QaCaptureConfig::is_configured() -> Result<bool, String>`:
`Ok(false)` means both `base_url` and `serve` are empty. Budget and login
settings alone do not enable capture. A configured target with broken runtime
credentials returns a finding in `Err`; evidence gates must refuse that finding.
`credential_file()` checks the pointer without reading its contents.

Use `cas deliverable capture --task <task> --target <branch> --side both` for
one bounded merge-base/head run. The default target comes from the task's
work target; the explicit flag is useful for a disposable project. Capture
clones immutable revisions (only Git objects are hardlinked), reuses delivered
`node_modules`, leases ports, masks requested selectors and tears down servers
on every exit. It writes `<task-artifacts>/qa/pairs/pairs.json` plus PNG/JPGs.
A failed login or missing mask gives a finding without an unmasked fallback.

The committed shot catalog is an array, for example
`[{"id":"roster","path":"/drivers/","ready":"main","mask":[".email"]}]`.
Optional `touches` globs select affected shots. Without a catalog, affected
journey runs at each revision supply their last-stage screenshot, only when
both assertion receipts pass. `journey-eval.sh --compare BASE HEAD` and
`JOURNEY_CAPTURE_MASK` are opt-in; ordinary QA runs keep their existing path.
The base uses only head's producer instrumentation, recorded by SHA, while its
application, bundle and tests remain at the base revision.

`cas deliverable pairs --task <task>` verifies complete pairs and their image
digests. `--range BASE..HEAD` resolves tasks through commit provenance; commits
without an unambiguous link stay explicitly unlinked. `--side before` or
`--side after` supports deployed targets, preserving their deployment source.
A missing side remains unpaired and is excluded from report evidence.
Global `--json` returns the full receipt. Exit codes are 0 for success,
1 for an evidence finding and 2 for invalid input or unavailable runtime.

## Cloud request correlation

Cloud mutation clients attach `X-Cassy-Attempt-Id` (Cloud #256), a random v4
UUID for one logical operation. `mutation_request` in `cloud/coordinator.rs`
owns the shared transport: build the request outside retry loops and clone it
for each send, keeping the ID across 5xx, timeout and 429 retries. A later
operation gets a new ID. The embedding, ordinary binding and operator clients
use the same tracing middleware with their existing deadlines and redirects.
Signed operator mutations retain the ID across clock/generation recovery;
the diagnostic header is outside the PoP signature, and reads pass through.
Signed object-storage uploads and separate
Violet/Hub service requests keep their own contracts. This ID is diagnostic,
with no idempotency, replay or ownership meaning.

`CloudCoordinator` also keeps correlation on GET `/api/agents` and
GET `/api/agents/tasks/:task/lock`; every keyset page gets its own ID. The ID
and a `pre_dispatch` INFO receipt precede ureq dispatch and DNS.
`ATTEMPT_ID_HEADER` and `RESPONSE_REQUEST_ID_HEADERS` in `cloud/coordinator.rs`
own the wire names. Transport failures, including timeouts, emit a WARN
receipt with the attempt ID; status receipts remain INFO.

The existing local tracing logger records route templates, method, UTC start,
configured budget, page ordinal/cursor presence and monotonic elapsed time.
`response_headers` captures status, the first safe available request ID from
`X-Cas-Request-Id`, `X-Request-Id`, then `cf-ray`, and separately `x-vercel-id`.
Response IDs must be 1–128 ASCII letters/digits or `-_.:`; unsafe or oversized
values are omitted. Only the local generated ID survives without a response.
`transport_error` records the SDK error kind; `body_complete` records body-read
and decode completion or failure together, when that body is actually consumed.
No bodies, raw errors, task IDs, cursors, tokens or full URLs enter these receipts.
Logging follows the existing logger's enablement and level configuration.

DNS/connect/TLS/TTFB boundaries remain unknown: ureq exposes no boundary to
this wrapper. Body reading and decoding share one measurement; elapsed to
headers also includes unobserved transport work. The unchanged ureq 2.12.1
synchronous resolver has no DNS deadline (`src/stream.rs:364–365`).

One ID means one **explicit application dispatch**, never one wire attempt.
Pinned ureq 2.12.1 (`src/unit.rs:264–274,300–305`) may retry a recycled
connection's failed prelude, or a closed recycled connection for an eligible
idempotent empty-body request, reusing its Unit and headers. Convenience
`ureq::get/post` constructs a fresh Agent (`src/lib.rs:513–540`), but redirects
and SDK behavior are still opaque here. No retry, timeout, redirect, auth,
status, body or ownership policy changes. No automatic claim replay: losing a
response does not establish rollback; mutation outcome remains unknown until
authoritative owner-scoped reconciliation. These receipts do not attribute
previous incidents or establish deployed Cloud instrumentation.

## QA ledger preflight

`cas qa ledger init --task <id>` writes an honest scaffold in the project's task
artifact directory, with predicted live journeys and no-code route exclusions.
The build is the recorded task source branch tip (start binding first, then a
legacy parked branch); an unreadable source fails before creating evidence.
Tasks without a recorded source use the resolved repository's HEAD. Journey
selection still uses the target repository. It preserves existing evidence.
Edit rows after exercising the named build,
then run `cas qa ledger check <path> --task <id>` (or `--json`). The shared
parser checks structure, counts, labels, defect ownership and readable PASS
captures; it does not judge what a capture proves. Close applies its diagnostics
before either verification dispatch path mutates state.

### Hosted queue precheck and pending-CI merge retry

`cas release precheck <sha>` pushes the resolved commit to `precheck/<full-sha>`
and dispatches `ci.yml` on that ref. The binary includes its dispatcher: the
checkout needs Git, authenticated `gh` and Python, with no Node, Playwright or
Rust setup. HEAD and the live hub stay untouched. The receipt in the Git common
directory records the full SHA and hosted run URL. `--receipt <path>` selects a
portable receipt location; `--json` prints the complete machine-readable receipt.

`cas release precheck <sha> --status` reads that run without a second dispatch,
records the job results and elapsed minutes, and compares them with October's
48- and 53-minute dispatches. A completed red run exits nonzero. Precheck refs
use the queue's required jobs on hosted runners, excluding advisory builds and
queue-only receipt publication. A historical SHA runs its own workflow version.
This command replaces the local assembly-proof precheck.

When an authenticated supervisor's merge of a parked delivery is refused only
for pending code CI, Cassy persists the original options, source SHA, CI SHA,
repository, target and supervisor session. The daemon retries on its next CI
watch pass after that exact CI evidence turns green. Normal merge, ownership,
QA, verification and target checks still apply. A red result, a moved lane or
a new admission refusal sends one durable supervisor notice and consumes that
intent. Missing CI and transport failures retain it without merging; daemon
restarts resume the persisted intent. In-progress and unparked tasks never arm
an automatic retry.

## Epic design-review staffing pause

A report counts when its task type is `bug`, priority is P0 or P1, and its
labels include exactly `stakeholder-reported`. Use that label only for a defect
reported by a customer, operator, or other stakeholder; internal review findings
do not count unless a stakeholder also reported the defect. Report time is the
task's `created_at`, so adding the label later does not reset its age. Closed
reports count too. Reports belong to each ancestor epic through parent-child
links; unrelated epics and non-epic tasks remain available.

`factory.design_review_bug_threshold` defaults to 5 (range 1–1000), and
`factory.design_review_window_hours` defaults to 48 (range 1–8760). The window
includes reports created exactly at its lower boundary and excludes future dates.
Task creation, updates and parent linking evaluate the trigger; start and spawn
admission also evaluate it for imported reports. A pause persists across restarts
and beyond the window until the operator lifts it. Active work, delivery, merges
and shipping to staging continue normally.

The supervisor receives one durable prompt per pause asking for a design review
against the epic plan. Start refusals identify the epic and the trigger reports.
After reviewing the design, the operator runs from their own terminal:

```sh
cas factory design-review-lift <epic> --reason 'Review outcome and why staffing can resume'
```

The non-empty reason is recorded on the epic. The lift acknowledges reports
created through the lift time; a fresh threshold of newer reports can pause the
epic again. Agent contexts, including supervisors, cannot lift a pause. Lifts
live under `.cas/operator/epic-design-review-lifts.json`; pause state lives in
`.cas/epic-design-reviews.json`, with serialized atomic writes. This policy is
per epic and independent of any project hold.

## Measured phase outcomes

A supervisor declares an immutable numeric target on an existing epic, phase
sub-epic or `gate` task with `task action=notes note_type=outcome_target`.
The JSON note maps metric names (including units) to `min`/`max` (inclusive)
or `gt`/`lt` (strict) bounds, for example:

```json
{"stability_percent":{"min":95},"p95_seconds":{"lt":1}}
```

Record the evaluation with `note_type=outcome_receipt`:

```json
{"measurements":{"stability_percent":72.7,"p95_seconds":1.48},"receipt":"/absolute/path/to/evaluation.json"}
```

The measurements must name every target metric and include a non-empty receipt
reference. The supervisor records the measured result; this action does not run
an evaluator. A missing or failed receipt holds the phase's `blocks` and
`requires_start` dependants, including children of dependent phases, at start,
close and merge. The refusal names the target and measurement. Closed phase
status does not stand in for a passing result. Its own implementation children
and independent work can continue. A sibling dependency does not evaluate the
shared phase's own target: a task's own ancestor chain stays outside the outcome
hold, while external prerequisite phases still gate it. Ordinary open blockers
retain their existing parallel-preparation policy.

Phase and model-eval receipts share the same saved evaluation JSON artifact
format below. Both verify its measurements, thresholds, model, command and run
log, binding the files by content. A model-eval target additionally fixes the
dataset identity and cannot be waived by an operator phase exception.

Only the operator, from their own terminal, can accept a miss:
`cas qa outcome-override <phase-id> --reason '<why>'`. This writes a decision
note and binds the exception to the exact target and receipt; changed measurements
require a new decision. Agent and supervisor shell contexts cannot issue it.
A subsequent passing receipt releases the outcome hold. Other close, CI and QA
gates still apply.

## Model evaluation close proof

A model-backed task or phase can declare an evaluation bar. A registered
supervisor records `task action=notes note_type=model_eval_target` with:

```json
{"dataset":"reader-44-v1","thresholds":{"accuracy_percent":{"min":95}}}
```

Save the evaluator's result under the task artifacts directory, for example
`evaluation.json`:

```json
{"dataset":"reader-44-v1","model":"provider/model","command":"reader-eval --dataset reader-44-v1","run_log":"/absolute/path/to/evaluation.log","thresholds":{"accuracy_percent":{"min":95}},"measurements":{"accuracy_percent":87.9}}
```

The assignee or a registered supervisor records `note_type=model_eval_receipt`
using the same receipt shape as a phase outcome:

```json
{"measurements":{"accuracy_percent":87.9},"receipt":"/absolute/path/to/evaluation.json"}
```

For a remote artifact, fetch it with `artifact action=fetch` first and use the
returned local path. An arbitrary artifact label or inline claims alone do not
count as proof.

The dataset and thresholds must match the immutable target, and the receipt's
measurements must match the artifact. The artifact names a model and evaluation
command, plus a readable run log at an absolute path. Both files must resolve
inside the owning task's project-scoped artifacts directory; checkout files,
other tasks' files and symlinks escaping that directory are refused. The evaluator
must emit exactly one machine-readable result line in its run log:

```text
MODEL_EVAL_RESULT {"measurements":{"accuracy_percent":87.9}}
```

Every receipt metric must match that result's value. Missing, ambiguous or
mismatched results are refused as `MODEL_EVAL_UNBACKED`, naming the unsupported
metric and claimed value. A JSON artifact claiming 96 beside a logged 80.3 cannot
pass. Cassy binds both files by content and rejects either changing after
recording. Missing prerequisite files ask to re-record the receipt. The evaluation
runner supplies the result line; recording proof does not execute the command.

A below-bar evaluation is saved as `MODEL_EVAL_MISS`. Close refuses and names
the bar and measured values. Missing proof also refuses. A passing receipt is
saved as `MODEL_EVAL_PASS` and permits ordinary close checks to continue. An
operator phase exception cannot waive a model-evaluation miss. Phase evaluation
misses also hold dependent work through the existing outcome graph. Tasks with
no declared evaluation bar retain their existing close behavior.

## Verification timing

`verification action=add` records `duration_ms` even when the caller omits it.
`duration_source` distinguishes three measurements:

| Source | Interval |
| --- | --- |
| `explicit` | Caller-supplied `duration_ms`, including zero; overrides automatic timing. |
| `capability_bound_to_consumed` | Persisted capability `bound_at` to `consumed_at`; excludes dispatch wait before the child binds. |
| `dispatch_elapsed_fallback` | Dispatch `requested_at` to verdict `created_at`; includes review/queue wait and is not isolated verifier runtime. |

Direct supervisor verdicts retain `capability_id=NULL`; no child authority is
created to measure time. The fallback also covers a legacy capability missing
its bind timestamp. Child verdict `created_at` equals the persisted consume
boundary; terminal dispatch `resolved_at` records resolution. Explicit duration
overrides do not replace those audit timestamps. Negative clock deltas clamp to
zero. `verification action=show` displays the source, and the quality ledger uses
the recorded duration. Identical retries retain the original measurement.
Migration m293 adds a nullable source; legacy rows stay unknown without backfill.

### Protected default-branch delivery

`worktree_merge` keeps its review, CI, repository and source checks before
publication. When GitHub confirms the default branch has a merge queue, it
pushes the exact source through the existing clean hook checkout, creates or
reuses the delivery PR with a review body, and enables auto-merge pinned to
that PR head. The tool returns the PR URL, records the task as awaiting merge,
and retains the source checkout; the queue and existing watcher own landing
and reconciliation. No local merge or direct default-branch push is attempted.

If queue metadata is unavailable, normal delivery remains available. A
`PROTECTED_DEFAULT_BRANCH_REQUIRES_PR` refusal restores the exact pre-merge
local target through the same guarded ref/checkout repair for reviewed and
no-dispatch merges. It preserves unrelated working edits and declines to
rewind a concurrent writer. Requested source cleanup waits for publication,
so a refused push leaves the branch and checkout available for the PR route.
A missing/moved PR identity or head never enables auto-merge; retry the same
merge request after correcting the cause.

## Advisory merge-queue health

See the [branch-protection health policy](../../docs/branch-protection/README.md#2-merge-queue)
for setup, doctor and release-preflight diagnostics and manual repair.

## Attach socket input diagnostics

Attach `PaneInfo` snapshots report `input_bytes_received` (bytes accepted by
that pane's input queue) and `input_bytes_written` (bytes physically written
to its PTY master, including partial writes before a failure). Counts include
harness input and terminal control replies; they do not establish that the
child read the bytes. Older payloads decode with zero counts. A one-shot
attach sender may close immediately after sending: outbound write failure
must leave its inbound frames available for the daemon's input phase to drain.

## Explicit hub stops during updates

`cas hub stop` records stop intent in `~/.cas/hub/operator-stopped` before
stopping the hub. `cas update`, including the update invoked by
`scripts/install-on-publish.sh`, leaves that hub stopped. An explicit
`cas hub start`, `cas hub restart` or non-dry-run `cas hub service install`
clears the intent. Foreground service launches and crash recovery do not clear it.
A hub that crashes without an explicit stop remains eligible for update recovery.

## Build, assembly and CI policy

While a release train is open in the target repository, `worktree_merge` refuses
direct main/trunk merges, including `allow_trunk=true`. Merge fixes into the
train; ship a one-item hotfix train with `scripts/release.sh --hotfix` from its
lane, through the existing release PR, merge queue and publication workflow.
With no train open, the existing merge behavior applies.

Commander **Reload Cassy** (`/mcp`) is a Claude-only native reconnect of the
managed `cas` MCP server. It preserves the conversation and confirms a new
live server PID and its recorded version. A command echo is not success.
The exact native `/mcp reconnect cas` interaction was verified against a
healthy, isolated stdio server on Claude Code **2.1.311, 2.1.316 and 2.1.318**.
The minimum supported version is **2.1.311**. Older or unknown running Claude
versions are refused as unverified; the current session's
transcript supplies the version, rather than a potentially newer installation.
The owning factory daemon must already run a build containing this action;
an old daemon requires its normal update/relaunch before Commander can use it.
Codex, Grok and unknown harnesses are refused by name. This action does not
replace the factory daemon or implement automatic in-place server updates.

Merge-queue proof reuse accepts successful completed `merge_group` runs from
`.github/workflows/full-validation.yml` (the canonical queue wrapper) and legacy
`.github/workflows/ci.yml`. The current close reason may cite
`merge_group_run=<id>` followed by ordinary separator punctuation, including a
colon. Repository identities, queue branch, exact tree receipt and landed
containment remain mandatory.

Tool and CLI status text follows [Status words and their evidence](../../docs/status-words.md).
Name cached or recorded state as such; a successful mutation does not establish
that its follow-up, release publication, or installation on another host completed.

This section moved here from the repository `CLAUDE.md`, where it loaded into every session.

### Commands

`cas hub restart` validates its plaintext bind address before restarting a
supervised service or stopping a detached hub. A refused `--bind` keeps the
existing hub's PID and listener available.

```bash
# Supervisor/operator only — factory workers never run these
cargo build                          # Dev build
cargo build --release                # Release build (LTO, strip)
cargo build --profile release-fast   # Fast release (thin LTO, 16 codegen units)
cargo check -p cas --lib --tests     # Compile feedback, no test linking/runs
scripts/run-scoped-tests.sh -p cas --lib module_name
scripts/run-scoped-tests.sh -p cas --test integration_cli cli_test::
cargo nextest run -p cas             # Full suite: final-tree integration gate
cargo test -p cas --doc              # Doctests (nextest does not support them)
cargo bench --bench code_indexing    # Benchmarks
make test-release-panic              # Verify A2/A3/B3 panic isolation under release profiles
```

The integration suites link through ten explicit Cargo harnesses. Use the
source suite's module prefix to select its tests; the six standalone release
and fixture targets keep their names. See [integration harnesses](../tests/integration/README.md)
for the inventory and per-test process isolation rules. Run
`python3 scripts/cas-test-targets.py cas-cli --check` from the repository root
after adding an integration suite so a missing module cannot silently drop tests.

Hub/doctor/status fixtures that inject a private `HOME` and `PATH` set
`CAS_TEST_HUB_PATH` to the desired candidate PATH. Hub discovery then uses
only those supplied absolute entries (plus Claude's native slots under that HOME), without a login
shell or fixed host directories. Lib tests set this candidate input rather
than mutating process-global PATH. This narrows discovery; minimum-version
verification still applies. A fake Claude used to test missing profiles must
answer `--version` with a compatible version before its normal auth payload.

Install the standard local runner once with `cargo install cargo-nextest` (or
`make -C cas-cli install-tools`). `scripts/run-scoped-tests.sh` defaults to
nextest and rejects a silent zero-test success.

### Worker checks, targeted tests and merge-queue integration

Workers may type-check their committed change with exactly
`cargo check -p <affected crate> [-p <crate> ...] --lib` for lib-only edits or
`--tests` when test files changed; choose one target flag. Include consumers of
changed shared interfaces. The PreToolUse guard routes that command through
`cas factory worker-check`, which holds an OS builder-slot lock until Cargo
exits. The route covers Claude's Bash and Codex's `exec_command`, including
calls made from code mode (`functions.exec`). Codex applies a hook's rewritten
input only with `permissionDecision: "allow"`, so the hook emits that for Codex
(cas-980d). Lock descriptors are close-on-exec so compiler-cache daemons cannot
retain slots after the runner exits. It checks the existing build guard, and enforces `max_concurrent_builders`
even across simultaneous launches. A refusal requires retrying later. Run long
checks in the background with a log. A backgrounded check or targeted test
runs as `worker-check --detach` in its own session, since Codex `exec_command`
kills the shell's process group, including any `&` job, when the shell exits
(cas-3d04); without a log redirect it is refused. Compile checks execute no tests.

Detached capped runs register their worker/task, output log and PID incarnation
before returning. Their existing worker-check command inherits a hidden
`--completion-id`; after its capped producer returns, it records the original
exit status and queues one completion message to that worker. The message names
the log and task without claiming a test or task verdict. Completion delivery
failure leaves durable state for the daemon's existing reminder scan to retry.
If the callback is lost, the scan detects the exited PID and reads only this
launch's bounded log tail for `worker-check: EXIT`; unavailable status stays
explicitly unknown. PID reuse cannot keep an exited run active. These local
records live under `.cas/worker-runs/`; no migration or authority capability is
created. Linux and macOS resolve the actual stdout log file, and detached calls
without a regular log are refused.

Before recycling a worker, Cassy sends TERM to each of its registered,
fingerprinted detached producers and lets the capped runner drain its guard
and Cargo descendants. It checks the inherited target lease before retiring
the worker or killing its cgroup. A producer that cannot settle within eight
seconds, or a surviving lease holder, refuses recycle while the worker remains
running. Completed runs retain their recorded result; retries can acquire the
target after the original work ends. Lease refusals omit the contender's own
descriptor when reporting the remaining holders.

Live registered runs and worker/session-matched candidate requests suppress
worker-idle relays. A completed run ID, task or delivery-condition change can
start a new idle episode; elapsed time, changing message ages and temporary wait
expiry cannot repeat an unchanged episode. Its one idle alert includes the
registered job's exit and log path. Restart recovery queues a completion once;
notification acceptance does not prove the worker model acted on it.

The runner requires a clean committed worktree, forces its private seeded
`target/`, and records `check: PASS <sha>` with the selected packages. Task close
copies matching exact-delivery receipts into worker evidence. Dirty trees,
failed retries, other worktrees and other SHAs cannot supply this receipt.
Check receipts are optional compile-only evidence and do not waive test proof.

When worker admission is unavailable but a supervisor has already run the
selection in a scratch checkout, a **live registered supervisor** may record
that run without executing it again. Use `factory action=record_test_receipt
 task_id=<task> payload=<JSON>` (MCP), or `cas factory record-test-receipt
--task-id <task> --request <evidence.json>` (CLI). The JSON is:

```json
{
  "head": "<full tested commit SHA>",
  "repo": "/absolute/supervisor-tested-checkout",
  "cargo_args": ["nextest", "run", "-p", "cas", "--lib", "-E", "test(module::name)"],
  "log_path": "/absolute/selected-nextest-run.log",
  "exit_code": 0
}
```

This is an explicit supervisor attestation of the recorded process exit code.
The task must be assigned to a registered worker; its checkout and the tested
checkout must both still be clean at `head`. The command must satisfy the same
single-package, named-test and explicit-harness rules as a worker run. The
non-empty UTF-8 log (at most 25 MiB) must contain exactly one nextest summary,
with a nonzero selection and every executed test passed. Supply the selected
run's log, without separate guard-run summaries. A failed recorded retry removes
that exact command's previous receipt. An active worker run refuses recording.
The action writes the normal worker-check receipt key and adds
`supervisor-recorded` provenance to both its displayed receipt and task audit
note, including the issuer, tested checkout, command and log SHA-256. Park reads
it through the existing receipt contract; the recording action runs no Cargo
command and changes no worker admission guard.

Build admission reads Linux `/proc/pressure/cpu` and `/proc/pressure/memory`
`some avg10` stall percentages, plus `MemAvailable` from `/proc/meminfo`.
Valid CPU PSI replaces the raw load check, so unrelated I/O-heavy work does not
block an otherwise healthy host. Missing or malformed CPU PSI falls back to
one-minute load divided by CPU count. Missing memory probes remain unknown.
Memory pressure and the memory floor are rechecked even when a proof continuation
carries its earlier CPU admission. Swap usage alone is not a refusal signal;
PSI measures time lost to contention. macOS reads one-minute load with `getloadavg`
and estimates available memory from free plus inactive pages via `host_statistics64`;
PSI stays unavailable there.

Supervisors advertise `build_capacity_v1` in existing registration and heartbeat
metadata: free builder slots, one-minute load, CPU count and available CPU/memory
PSI. A passive guard sample reports zero free slots if pressure, disk headroom or
the builder cap would refuse the next build; heartbeat sampling never runs GC.
`factory.auto_route_builds` defaults to `true`. If local admission would wait,
a single-task `spawn_workers task_id=...` request can instead ask a live
supervisor of the same account and repository on another machine to staff it.
Teammates and peers whose account cannot be confirmed are never selected.
Task creation and assignment retain their existing local behavior. Both the
heartbeat and capacity sample must be at most 90 seconds old, with no future timestamps. Unknown or unavailable capacity retains local
behaviour. Selection prefers the most free slots, then the lowest load per CPU;
the result names both measurements. The existing peer outbox limits each peer
to one new route per 90 seconds, including concurrent requests; a replay of
the same logical ask does not consume another slot.

Only open, unleased and previously unassigned build tasks are eligible. Existing
workers keep their work, ordered task queues remain local, and `force=true` on
spawn retains the explicit local override. Routing stores a `BUILD_ROUTE` note
with the stable `build-route:<repo>:<task>` identity, leaves the local assignee
empty and sends one `kind=ask` through the existing durable peer outbox with the
full task and supervisor brief. Retries reuse its selected recipient and send
key only while the peer is live, its heartbeat and sample remain fresh, and the
route is less than 15 minutes old. Otherwise an expired note restores local
admission; history still prevents routing onward. Unsent retry entries are
cancelled when a route expires. A peer receiving a task with a route note staffs
it locally or waits; it never routes it onward. A failed send retains the route for recovery;
after three failed attempts (or a permanent refusal), the route expires with
`cause=send_failed` and restores local admission. Mailbox duty rechecks routing
config and local staffing before retries, cancelling obsolete asks even before
their scheduled retry time. Ordinary peer messages retain their retry policy.
`cas config set factory.auto_route_builds false`
disables new automatic routes and releases existing routes to local admission.
A forced local spawn also releases its prior automatic route.
The peer supervisor still owns staffing and local admission; an ask grants no
remote process-control authority.

Spawn and capped Cargo admission also probe the target/scratch filesystem using
the target-cache capacity policy. They reserve an estimated 10 GiB of growth for
each live and requested builder and refuse when that would reach
`factory.target_cache_high_watermark_percent` (85% by default). Occupied capped
builder-slot locks count on macOS as well as Linux. No-code spawns reserve no
target growth. Disk headroom is rechecked on proof continuations, and a failed
capacity probe refuses admission until the target volume can be inspected.
At projected disk pressure, admission first makes one attempt to reclaim stale,
verified worker targets under `factory.scratch_root`, retaining the configured
warm caches and all live, recent or unverifiable output. It logs the reclaimed
bytes and remeasures actual free space before deciding whether to refuse. This
attempt does not remove unmanaged scratch directories or run full factory GC.
Worker shutdown and replacement also retire safe terminal or parked-and-pushed
targets, copying proof logs to durable artifacts before removal. In-progress,
unpushed or unverifiable work keeps its target and checkout.
QA candidate builds first install locked web dependencies and invoke the same
source-bound production snapshot builder as journeys, in the isolated checkout.
Only that disposable checkout receives the generated dist before Cargo embeds
it. After snapshot verification, only its tracked `hub-web/dist` index entries
are marked assume-unchanged in the disposable checkout, preserving the clean
source revision without suppressing dirty checks on other tracked inputs.
Normal checkout builds keep their existing dirty-version behavior.
A private loopback hub then serves every expected asset under a disposable
HOME and CAS_ROOT: publication refuses a source/served digest mismatch and
preserves the previous candidate. RECEIPT.json records `web.source_sha`,
`web.dist_sha256` and `web.served_dist_sha256`.

QA candidate builds use a private bounded target inside their owned release
scratch directory. Candidate artifacts are copied out before that directory is
removed; existing shared supervisor targets remain review-only. Interrupted
candidate directories use the release-scratch GC naming and ownership protocol.

| Config key | Default | Meaning |
| --- | --- | --- |
| `factory.build_guard.max_load_ratio` | `1.0` | Positive finite fallback load/CPU ceiling |
| `factory.build_guard.max_cpu_pressure_percent` | `90.0` | Maximum CPU `some avg10`, 0–100% |
| `factory.build_guard.max_memory_pressure_percent` | `10.0` | Maximum memory `some avg10`, 0–100% |
| `factory.build_guard.min_available_memory_gib` | `2` | Minimum available GiB; 0 disables this floor |

For example, `cas config set factory.build_guard.max_load_ratio 1.5` changes the
fallback threshold. PSI limits and `factory.max_concurrent_builders` still apply.
Invalid hand-edited thresholds use defaults. A denied worker command prints
`Waiting for build admission` with its measured reason and asks for the same
capped command to be retried; it does not hold a builder slot while waiting.
Registered workers retain their InProgress task and lease during capacity
waiting. Each refusal updates one durable worker/task/lease-generation episode,
with its original start, current cause and next retry (120 seconds). worker_status
shows that episode once. Higher-priority queue waits never wake the supervisor.
Disk-watermark waits share one pressure notice per factory session after 20
minutes, listing waiting workers and reclaimable target caches. The background
reminder scan persists the pressure snapshot and updates it only when the
waiting set or reclaimable bytes change; incomplete inventories report an
unknown total. A resolved pressure episode can recur. Other resource refusals
retain their one-notice-per-worker escalation after 20 minutes. Notice wakes
follow normal supervisor readiness policy and retain the task's active state. A delivered explicit assistance
message consumes that episode's automatic notice even after a gap longer than
four minutes between retries. A stale retry stops reserving queue position;
the live task lease still preserves its original wait age and notice state.
Workers cannot change priority on any non-terminal task, including Open and
AwaitingMerge; reopening does not grant permission to promote it. Priority
updates refuse when caller authority cannot be read. Successful admission clears
the wait, and a changed task/lease invalidates it. Capacity waiting never changes task
status; genuine blocked/yielded tasks retain their existing semantics.
`force=true` only overrides the existing spawn-time soft check, never capped
worker slot locks.

On Linux, the capped runner launches each build in a dedicated
`systemd-run --user --scope` with `MemoryHigh`, keeping compiler reclaim separate
from the supervisor and daemon. `factory.worker_memory_high_gib` sets the limit
in GiB; default `0` divides half of total RAM by `max_concurrent_builders`, bounded
to 256 MiB..16 GiB per build (8 GiB on a 64 GiB host with four builders).
Unavailable user scopes and other platforms log a fallback and run unscoped.
Only scope startup failures fall back; a failed build is never rerun unscoped.
Scope launches disable systemd's argument expansion so embedded proof scripts
reach Bash and Python unchanged. A bounded version probe omits the unsupported
switch on systemd older than 254, whose scopes already pass arguments literally.
Daemon startup also tries to give its inherited interactive cgroup (including
the supervisor) at least 256 MiB `memory.low`, preserving stronger operator
settings. Missing controllers or user-manager permissions leave startup working.

Task verifiers use the same budget. Run `cas factory verifier-test
--cas-root <root> --task-id <task-id> --dispatch-id <dispatch-id>
--head <bound head SHA> -- nextest run -p <crate> [--lib|--test <harness>]
-E 'test(name)'`. Omit `--dispatch-id` when no dispatch exists. The helper
creates a managed detached checkout, resolves Zig, seeds a private Cargo target
and runs through the existing counted builder slot, PSI and capacity checks.

For a supervisor walk or source review, create the same disposable checkout:

```bash
cas factory scratch-checkout --cas-root <root> --task-id <task-id> --head <full SHA>
```

Use the printed absolute path as the review directory. Add `--dispatch-id` when
reviewing a dispatch. With `factory.scratch_root` configured, the managed home
is `<scratch_root>/<project-key>/verify/<task-id>/<run>/repo`; otherwise it is
`<root>/review-scratch/<task-id>/<run>/repo`. Each run has a private ownership
record and exclusive lease; retries reuse the same task/dispatch/full-head
checkout, so admission refusals cannot multiply source trees. Changed source
or ownership refuses reuse; no existing ad-hoc directory is adopted.
Publish durable proof outside that disposable home, in the task artifact root.

`gc_report` includes managed review checkout status and flags unmanaged scratch
root siblings of at least 1 GiB (apparent file bytes) as review-only. The
unmanaged scan stops at that threshold and reports a lower bound, not a full
recursive total.
`gc_cleanup force=true dry_run=false` reclaims a managed review target as soon as
the dispatch resolves, times out or is invalidated, or a task is Closed/Cancelled,
provided no Pending/Claimed dispatch remains for that task.
Verdict recording and task close await only a guarded atomic rename into private
trash; measurement and deletion run in the background under the build-lane lock.
The collector also reclaims other eligible checkouts' `repo/target` under those
same guards. GC and disk-pressure admission recover interrupted trash, including
legacy trash missing its marker when the private parent owner validates. Deletion
removes the target first, then its marker, then the empty trash directory, so a
process exit leaves recoverable state. Cleanup reports deletion as deferred when
the project collector lock is busy or unavailable. Each pass visits at most 16 checkouts.
The small source checkout and Git registration retain the two-hour idle grace period.
`REVIEW_SCRATCH_STATUS_JSON` reports total and per-checkout `reclaimable_bytes`; a
young terminal checkout contributes its disposable target bytes, not its source bytes.
The default human report also shows that total. Both review and target-cache
reports count queued trash as `pending_reclaim_bytes`, never as already freed space.
Successful checkout reuse refreshes `last_used_at`; older records without that
field use creation time. It
checks leases and native process evidence (open files, cwd, mappings and argv),
rechecks terminal ownership, and retains any checkout with edited tracked files
or untracked files, even if local Git configuration hides untracked status.
Existing checkouts use non-force `git worktree remove`, which also refuses
concurrent source edits; ignored build output is disposable. Force is permitted
only to remove a registration whose checkout directory is already gone. It then
deletes the disposable home. Unknown/malformed ownership,
changed heads, another repository's registration, live or unverifiable users,
and pending/claimed dispatches are retained. Young source checkouts are retained. Force alone remains a preview for this tier.
These rules make interrupted verifier/supervisor output collectable without
relying on an agent remembering to remove a large checkout.

Shell redirection opens logs before the runner checks that commit. Use a
Git-ignored `target/worker-check.log` or the task's artifacts directory; create
the parent directory first. In-repo logs that Git does not ignore are refused
by the hook. The clean-head gate still rejects source changes.
Capped-run rewrites append to the log even when the command uses `>`, preserving
earlier attempts and refusals. Slot admission and release records include UTC,
slot index, process ID and lock root; use those records to audit concurrency.
`released` is logged immediately before unlock, so an `admitted` record for
that slot always follows the prior `released` record. Locking and close-on-exec
behavior are unchanged.
A reused log's creation time does not identify a later retry's admission time.

Workers may also run exactly
`cargo nextest run -p <crate> [--lib|--test <harness>] -E 'test(module::name)'`.
An omitted target selects `--lib`; `--test` must name one explicit harness from
`scripts/cas-test-targets.py`. Select exactly one package and a mandatory
positive named-test filter (`test(name)` or `test(=name)`, joined by `|`/`&`).
Empty/all(), regex/glob, negated and binary-wide selectors are refused, as are
unapproved environment prefixes, compound commands, repeated packages and broader flags.
Named tests may carry literal assignments for `HOME`, `CARGO_HOME`, `RUSTUP_HOME`,
`CI`, Git author/committer name/email and Git global/system configuration inputs.
`env -u NAME` (also `--unset`) removes those same allowed inputs. Quoted literal
paths are supported; shell expansions are not. The hook transports these as
child-only metadata through the capped runner and detached execution. Admission,
tool discovery and receipt ownership keep the inherited environment; only the
final test child receives the requested values. Identity, PATH, target-directory,
compiler and builder-limit overrides remain refused. This does not admit
environment prefixes on compile-only checks.

For a credential-free CI reproduction, first create a private config directory.
The hook also accepts this complete fixed profile before a named nextest command
(use an existing, literal absolute path without shell expansion):

```bash
env -u VIKTOR_API_KEY -u VIKTOR_KEY -u NEON_API_KEY -u NEON_TOKEN -u VERCEL_TOKEN -u VERCEL_API_TOKEN XDG_CONFIG_HOME=/absolute/private-config cargo nextest run -p cas --lib -E 'test(module::name)' > target/worker-tests.log 2>&1 &
```

The hook carries this as runner metadata, including detached execution. The
runner removes those six credentials and sets XDG only on the final Cargo/test
child, after Cassy credential bootstrap and normal builder admission. Parent
identity, private targets, memory limits and clean-tip receipts remain in force.
Other environment wrappers, partial profiles, identity/build overrides and shell
substitutions remain refused; this profile is for targeted tests, not checks.

The same capped runner invokes the shared `run-verified-tests.sh` zero-test
guard, forces the private target, and records only successful nonzero counts
against an unchanged clean commit: `test: PASS <sha> <package> <filter> <count>`.
The bounded nextest discovery guard requires `python3` on PATH, including in
projects using the embedded runner without this repository's scripts. It forwards
TERM/INT/HUP to its owned Cargo process group and bounds cleanup after stdout EOF.
On Unix, named tests and their guard processes run with private links to resolved
Rust tools first on PATH. Rustup proxies are resolved to concrete toolchain
executables, and the selected Cargo invocation uses that private link; this
prevents rustup from restoring `$CARGO_HOME/bin` when it dispatches. A PATH
directory containing `cas`, `codex` or `claude` files is replaced in place by a
private mirror of its other entries, without those binaries or rustup proxies, so
neighbours such as Homebrew `git` keep their precedence over `/usr/bin`. Other
entries, including local build-tool and Homebrew directories, retain their order
so tools such as mold, node, protoc and newer Bash remain available. Compile-only checks
retain their ordinary PATH. The runner prints the PATH setup cost for measurement.

Harness fixtures must supply their own fake executables. The shared
`test_env_guard::fake_cas_on_path()` returns a private no-op Cassy
stand-in and a PATH value; pass `fixture.path()` through `Command.env` and retain
the fixture until all children exit. For a PTY fixture, re-execute the selected
test in that private child before calling `Pty::spawn`: its preflight reads
ambient PATH, so `PtyConfig.env` alone is insufficient. The helper does not
mutate process-global PATH. This is for the PTY preflight and harmless hook calls;
tests of Cassy behavior must use the built binary. The same guard source is
compiled by cas, cas-pty and cas-mux. To deliberately test an installed host
harness, set `CAS_WORKER_TEST_ALLOW_HOST_PATH` to a nonempty, single-line reason
for that invocation. The runner prints the reason and stores it in the test
receipt (`host_path_opt_out`); an empty reason refuses instead of silently
relaxing the rule. Do not use this opt-out to repair an incomplete fixture.

#### Operator live headful mode (macOS)

From an operator terminal in a clean isolated worktree, combine
`CAS_HEADFUL_E2E=1` with a reasoned `CAS_WORKER_TEST_ALLOW_HOST_PATH` opt-in.
Use a candidate `cas` binary containing this runner change and retain the real
`HOME`, XDG environment, `CLAUDE_CONFIG_DIR`, secure-storage selector and any
Claude/Anthropic credentials. Do not use `--isolated-test-env`. For example,
replace the two absolute paths with the candidate binary and main project's
`.cas` directory:

```bash
env CAS_HEADFUL_E2E=1 CAS_WORKER_TEST_ALLOW_HOST_PATH='operator live headful credential proof' /absolute/candidate/cas factory worker-check --cas-root /absolute/project/.cas -- nextest run -p cas --test e2e_test -E 'test(=e2e::factory_tui_headful::factory_tui_headful_reuse_window)' > target/headful-live.log 2>&1
```

This mode accepts only that exact test selection on macOS. It keeps clean-HEAD,
private-target, builder-slot, load/memory admission, nonzero-test and receipt
checks, inherits host PATH and credentials, and prints successful test output
immediately. It works with `--detach` too, retaining those environment opt-ins.
The factory's operator-only ancestry guard remains active: an agent terminal
cannot supply live proof, even with both environment opt-ins.

Read the complete log, not just nextest's passed count. Without explicit opt-in,
headful selection reports `NOT EXERCISED`. Missing or failing `claude --version`
fails as `headful dependency unavailable`; a selected account whose
`claude auth status --json` does not confirm `loggedIn=true` fails as
`headful authentication failed`. Auth output and credentials are never printed.
Worker-registration failures after that preflight are distinct and do not prove
credential loss. Actual success prints `headful live: PASS` only after three
worker replies and reuse of the same Terminal window. Preserve the log with
the tested SHA and macOS version for the task's platform proof.

In the Cassy source repository, every named run first executes the fixed cheap
guards: `test_support::lib_test_process_env_mutation_is_isolated`,
`config::mod_tests::serialized_factory_fields_have_registry_entries_cas_f7b9`,
and `scripts/check-test-shape.py --changed-since <task-target-base>`. Guard tests
run separately from the requested selection, so their passes cannot hide a
zero-match requested filter. The runner prints `guards: PASS` with the separate
guard count and elapsed seconds; the `test: PASS` count remains the requested
selection's count. Both invocations share the builder slot, private target and
memory scope. Other repositories retain their existing named-test behavior.

The same preflight compares removed string literals overlapping changed hunks
against tracked test references and prints `Worker changed-string warning:
<file>:<line> test <name>` with the matched literal or fragment. Multiline SQL
schemas stay whole; Rust SQL fixture setup is excluded while actual assertion
macros remain eligible. Unchanged tests can still assert a changed contract.
Run or update the named assertions before push. This is a warning heuristic,
not a complete dependency analysis. The diff
uses the active task's integration target, with configured epic/default-branch
fallbacks for standalone runs; pushing the worker branch never clears it.
Failed retries invalidate that scope's receipt; close imports all matching
exact-delivery receipts. Targeted receipts complement full-suite assembly proof.

Each check runs once (operator decisions D1–D3, cas-eac2). A factory worker's
pre-push hook runs the no-build fast rows (`scripts/fast-rows-pre-push.sh`) and
writes a PASS receipt for the pushed tip; a missing tool fails the push. Failed
attempts print a retained log directory under the Git common directory's
`cas/fast-rows/<sha>.logs.*`, containing `gate.log`, detailed row logs and timings.
Each attempt has its own directory so a later push preserves the original failure.
With the repository variable `CAS_LANE_SLIM` set to `true`, lane CI runs the
fast rows once in `lane.yml`'s classification job, for both docs and code;
non-slim lanes retain their existing fast-row steps in `ci.yml`. The merge
queue does not add another fast-row run. CI is authoritative for these checks;
local PASS receipts are reusable pre-push feedback, never a park requirement.
Slim lanes omit broad Rust tests for factory pushes. Release-train pushes and
merge-group events call the same reusable `ci.yml` through
`full-validation.yml`: preflight, every script/CI-tier test, Commander web,
cumulative changed-web journeys, doctests and the exhaustive Rust archive/shards.
Both share quarantine plus one rerun. The schema-2 `train-full-gate` artifact
binds every required job to the source tree and main baseline; train cuts refuse
legacy or partial receipts. The queue can reuse this complete proof after only
release-plumbing changes with main unchanged, then runs compile/source-built
smoke and asset drift checks. Otherwise the queue runs the full graph. Explicit
non-train hotfixes retain their emergency cut path, disclose the absent receipt
and always receive full queue validation. Train and queue wall clocks enter the
friction ledger once per immutable run attempt. CI retains complete native logs;
the daemon still holds a red train and wakes its supervisor once.

Each full-set test/build job also publishes a schema-2 job receipt through
`.github/actions/full-gate-job`. Its key binds the Git tree, stable job/matrix
identity, workflow/actions, effective selection, main baseline and runner inputs.
A requeue can reuse a successful producer job from a failed overall run; it
validates the completed producer workflow and that particular job through GitHub.
Failed jobs, changed inputs, missing proof and lookup errors execute normally.
The Rust archive must still exist before its builder can skip; remaining shards
download from the original producer run. Reused native train reports feed the
existing full-suite collector. Cached flake observations retain their original
run identity, so reuse cannot count as another independent failure. The final
full-set receipt still requires every selected job and durable flake publication;
a per-job receipt alone never authorizes a release.
Park reads, and never
runs, a capped test PASS in every workspace crate whose sources or tests
changed. A receipt in another crate does not count, and a missing test receipt
is refused with its fix. Pull requests keep the targeted `ci-test-impact.py`
fallback. The merge queue is the final-tree integration gate
and the only gate to `main`; its required checks either run on the exact merged tree or reuse the complete
source-tree receipt after the strictly checked release-plumbing delta. Commander journeys run there only when `hub-web/`
product inputs change. Do not add lane-merge compile checks, park re-runs or a
separate assembly journey run. If more than two merge-queue ejections per epic
follow from this, unset `CAS_LANE_SLIM`: that one setting is the D1 rollback.

Factory pushes start in `.github/workflows/lane.yml` (cas-32e9), also named
`CI`. With `CAS_LANE_SLIM=true` it runs only Classify CI diff, the two Scoped
Validation checks (Docs Lint for a docs-only diff) under their usual names, so a
lane is not held up by dozens of skipped `ci.yml` jobs. Unset, it calls `ci.yml`
and the lane runs exactly as before. When the repository variable
`CASSY_LANE_SELF_HOSTED` is `enabled` (cas-14e0), those slim jobs run on the
local light lane slots instead of waiting for a hosted slot: runner slots 3
and 4 (cas-3b5e) carry only the `cas-lane-light` label, so merge-queue and
release Rust jobs keep slots 1 and 2 (`cas-ci-32core`). The light units run at
`Nice=10`, `CPUWeight=25`, `CPUQuota=200%` and `CARGO_BUILD_JOBS=2`, with no
sccache; `scripts/install-cassy-actions-runner.sh` provisions them
(`RUNNER_SLOT=3|4`). `lane.yml` has no pull request trigger, so forks
never reach those runners; `scripts/ci_tiers/test-policy.py` pins the routing.
Rollback is one setting: `gh variable delete CASSY_LANE_SELF_HOSTED --repo
Richards-LLC/cassy` keeps the slim lane on hosted runners, and unsetting
`CAS_LANE_SLIM` restores the full lane. Retiring the light slots also means
`systemctl disable --now cassy-actions-runner-3 cassy-actions-runner-4` and
reverting `lane.yml`'s label.

The normal epic close follows the green queue landing. In downstream repositories without a merge queue, epic close instead requires completed successful CI check-runs on the exact default-branch SHA, or a supervisor `ASSEMBLY_PROOF` for that SHA recorded on the epic. Missing, skipped, cancelled and budget-blocked checks refuse close; an authenticated supervisor may use `supervisor_override=true` with a reason, which is audited. The gate reads existing receipts and starts no test run. Where the
epic close gate accepts a local `ASSEMBLY_PROOF`, it covers the Rust build and
suite only; it does not replace the queue's path to `main` or require a
journey receipt.

Workers may run the capped compile checks and named targeted tests described
above, but the full Rust build and suite run in the merge queue. Child closes
reference the epic integration proof. For a delivery targeting the default
branch directly, close can reuse the queue's full-suite proof without a
supervisor-run scoped `--proof`. Cassy discovers the run through the tree
artifact for its recorded published landing; a close reason can instead cite
`merge_group_run=<id>`. Both paths verify completed successful repository CI,
the live Fast Validation artifact, and the exact tested tree's first-parent
landing containing the delivery. An ordinary close needs no override. Missing,
expired or mismatched queue evidence retains the scoped-proof requirement;
declared-risk, content, CI and QA gates still apply. Non-Rust suites retain
their own task checks; a documentation-only change proves its documentation
acceptance check.

The opt-in assembly command runs the gate's `ci-script-tests` row:
`make -C cas-cli test-ci-tiers`, with factory identity and inherited make
dry-run/ignore-error modes removed. A failing script suite retains its output
and stops assembly before either Rust suite. The full release gate runs this
same mandatory row beside its build rows; a failure aborts the gate and
cancels the build and Rust suite rows still running. The short `--fast-rows`
lane checks run only `ci-script-tests-changed`: the tier entries whose script
or subject (`scripts/<stem>.*` for `scripts/test-<stem>.*`) the lane changed.
Entries that take over 20 seconds alone are reported as deferred to the full
row. A changed `hub-web/package.json` runs the one named worker-memory weight
assertion (0.001 s) rather than the full worker-memory suite.

The provisioned script cohort in `scripts/ci-script-tests-for-diff.py` runs
inside the existing scoped validation jobs in both `ci.yml` and slim `lane.yml`.
The shared `run-script-suite-tests` action installs only selected suites'
dependencies; unrelated inputs run no suite, Node setup, pip or npm install.
These suites are deferred out of the bounded fast rows, then included in the
exhaustive `test-ci-tiers` assembly target. Visual QA uses the locked web
Playwright package; the PDF renderer checks receive pinned PyMuPDF. The terminal
PTY suite reuses the CLI already built by full-lane preflight, with no additional
Rust build. Script-only contexts log its missing source binary explicitly;
native Codex cases likewise report explicit skips when Codex is unavailable.

The queue's CI tooling tier uses the same Makefile catalog through
`ci-script-tests-for-diff.py --all --scope-base <comparison-base>`. Its five
visual-QA tool fixture suites run only when their registered inputs change;
other commands still run. Dependency provisioning shares that selection.
An unavailable base runs every fixture, and the local assembly Make target
remains exhaustive. The codemap receipt test uses a tiny isolated Git repository
and a process fixture for the CI policy checker, whose own tier entry retains
the real policy tests.

Fast rows include the no-build checks below. The factory pre-push hook, or lane
CI for a pull request, selects each row when its inputs change; the merge queue
runs its required fast-row job on the final tree:

- `journey-catalog`: `journeys-for-diff.py --check`, so every catalog step
  still matches a `test.step` title in its journey spec.
- `builtin-skill-limits`: skill description, size and line limits checked on
  the exact `include_str!` catalog bytes, mirroring the Rust tests.
- `migration-registry`: every migration file is declared, registered and
  listed in id order.

`builtin-doc-hygiene` checks private operator-data leakage. Vocabulary and
contract-phrase requirements do not gate delivery or releases.

`scripts/test-fast-release-rows.py` holds a fixture for each fast-row failure:

- `test-shape`: integration tests under `cas-cli/tests` and `crates/*/tests`
  may not use `env!("CARGO_BIN_EXE_<name>")`, which is a build-host path that
  archive shards lack.
- `version-literals`: fast mode also refuses the next patch, minor and major
  version in the lane's added lines, comments included. The release gate
  scans for the bumped version. Pre-existing next-version literals, including
  unchanged lines in touched files, are printed as a NOTE.
- `markdown-lint`: the changed-Markdown lint runs with the CI config and pin.
- `test-env` and `fixture-paths-src`: the environment failures.
- `builtin-projections-src`: the no-build half of `builtin-projections`. It
  checks that the root task-verifier projection matches its embedded body.

A failed row prints `FIX <row>: …` so lane CI or the merge queue identifies the
row to repair.

The `call-shapes` row is the only implementation of the MCP call-shape lint
(cas-043b). It runs `scripts/check-call-shapes.py` over shipped builtins and
runtime Rust templates, using the committed `scripts/mcp-call-surface.json`.
`mcp_action_surface_test::call_shape_surface_snapshot_matches_the_published_surface`
keeps that snapshot equal to the live surface. On drift the test prints the
`cp` command that refreshes the snapshot.

Release scripts used from macOS source `scripts/release-portable.sh` for
timestamp parsing and canonical paths. The timestamp helper retains GNU date
results on Linux and falls back to Python for timezone-qualified ISO values
on BSD hosts, including offsets and fractional seconds. The path helper
resolves symlinks and missing trailing components without GNU `realpath -m`
or `readlink -f`. Receipt paths and their worktree use the same canonical
form before containment checks. `scripts/test-release-portable.py` exercises
the report stage with GNU date and path commands unavailable. Provisioning
and persistent Actions cache maintenance remain Linux-only and explicitly
require GNU tools and Linux kernel interfaces.

When memory permits, the script tier overlaps the native nextest precompile
and archive producer compile. Both producer phases finish and the script tier
passes before native full-workspace tests run, followed by the archive consumer.
The two test consumers remain sequential: nextest groups apply within one
process, and host ports and hub processes may be shared between suites. Native
and clone builds use separate Cargo targets; each row has separate logs and
gate scratch directories. The plain clone and archive remap remain outside
disposable roots and every `.cas` ancestor. Archive extraction stays on disk
with `--extract-to`; only disposable test temp directories and fixture HOMEs
use the native temp filesystem. Missing-wrapper, empty Cargo home, reduced
PATH and the component-output snapshot exclusion remain archive requirements.

Assembly reads Linux `MemAvailable`, or macOS `hw.memsize` and `vm_stat`
free/inactive/speculative pages, before admitting producers and each consumer.
`CAS_RELEASE_GATE_ASSEMBLY_BUILD_JOBS` sets a per-producer job ceiling; the
default shares available cores equally, and memory may lower it further.
`CAS_RELEASE_GATE_ASSEMBLY_RESERVE_GIB` overrides the reserve; its default is
the larger of 25% of physical RAM and 8 GiB. Both knobs accept positive integers.
The producer budget uses 8 GiB for the large cas compile/link unit, rounded up
from soundwave's measured 7,293,348 KiB maximum RSS (serial proof `7e4c6f50`,
head `abd6817b5`), plus an assumed 256 MiB per dependency job and 2 GiB for
scripts. A shared host/user linker pool bounds both producers, budgeted at
2.1 GiB per link. Soundwave's 2026-10-05 incremental relink sampler
(`.cas/perf-98a0/link-rss.log`, 0.5s samples) measured 2,190,228 KiB maximum
`ld.mold` RSS (2.089 GiB), with `rustc` peaking at 4,775,752 KiB (4.555 GiB).
The cold-proof 8 GiB producer bound remains because incremental code generation
does not establish the cold peak. Link admission rechecks memory while holding
an atomic admission lock. The live slot count is
`clamp(floor((MemAvailable - reserve - 2 GiB) / 2.1 GiB), 1, maximum)`,
where `CAS_RELEASE_GATE_ASSEMBLY_LINK_JOBS` sets the maximum (default 8,
positive integer). A count of 1 still waits if one link cannot fit. All active
leases count, including higher slots after memory shrinks or a different cap
is chosen. Queued children reserve a full estimate before starting; the pool
is host/user-wide under `/var/tmp`, independent of producer `TMPDIR`.
Every memory-sampled attempt records capacity and occupancy; every admission
records its slot. `execution.link_jobs` is the configured maximum, while
`link_slots` in each admission is the fresh capacity. Each invocation records
`peak_waited_driver_rss_bytes` in `link-rss.jsonl`: `wait4` RSS for the exact
waited driver, excluding unwaited workers and unrelated children. The same
invocation also records `peak_mold_worker_rss_bytes` (largest observed single
`mold`/`ld.mold` worker) and `peak_process_tree_rss_bytes` (largest sampled sum
of the driver and its observed descendants). Sampling runs every 100 ms using
Linux `/proc/*/stat` or macOS `ps`; observed descendants stay attributed by PID
and start identity after reparenting. The worker peak includes its PID, start
identity and sample timestamp for comparison with a synchronized external
sampler. Tree sums may double-count shared pages, and short-lived workers that
fork and reparent between samples may be missed. These are sampled lower
bounds, not a complete whole-link high-water mark. `rss_sampling_status`,
errors, sample count and bounded post-driver drain report incomplete evidence;
no samples produce null values rather than a fabricated zero peak. A zero
worker peak with samples means no named mold worker was observed.
The added observations do not change native linker flags, process groups,
leases, admission estimates or the memory reserve guard. `estimate_exceeded`
still checks the waited driver; `sampled_tree_estimate_exceeded` is observational.
The external 2.1 GiB estimate above still sizes links pending same-link native
assembly calibration. The producer start budget reserves one link; additional
links require fresh pool admission.
Supervisor memory/PSI samples must validate the estimates on each host.
Insufficient concurrent capacity selects sequential legs with a fresh memory
admission before each phase. `CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS`
(default 600) bounds memory/slot waits and compile pauses;
`CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLL_SECS` (default 1 for guards, 2 for phase
admission) controls resampling. Both accept positive integers. Each refusal and
later admission is recorded, including on timeout. The compile guard pauses the
producer process group within 2 GiB of the reserve and resumes once 4 GiB is
available above it; a deadline or observed reserve breach aborts the producer
and prevents PASS. It records every sample and pause/resume in
`compile-memory.jsonl`; the assembly receipt includes both guard and link logs.
Native linker selection, Cargo target rustflags, explicit environment flags and
the worker job ceiling are preserved. Immutable helper paths keep clone-path
changes out of the shared Cargo dependency fingerprint. Configurations using
`cfg(...)` target rustflags must supply explicit native flags for this guard.
Missing memory probes fail admission. Consumers have a fresh
thread ceiling using an assumed 4 GiB base plus 256 MiB per test thread, and
never overlap a producer. These are admission estimates, not OS memory limits.

The gate prints a reuse hit for both suite rows or a `MISS assembly key=…`
reason. Environment misses from new receipts also name the first changed
variable; receipts store only per-variable hashes. `CAS_RELEASE_ARTIFACTS_ROOT`
and `CAS_RELEASE_RECEIPTS_RUN_DIR` are output locations and do not invalidate
proof. Compiler flags, HOME, PATH, local environment/config files and the
resolved Zig binary remain inputs.

The environment fingerprint includes every variable passed to the test rows by
default, including unknown `CAS_*` variables. Assembly first removes the exact
harness/session names in `scripts/assembly-proof.py`'s `IDENTITY` set from both
the test environment and the fingerprint: factory/agent/session identity,
`CAS_ROOT`, `CAS_CLONE_PATH`, `AI_AGENT`, `CLAUDECODE`,
`CLAUDE_CODE_CHILD_SESSION`, `CAS_FACTORY_MODE`, `CAS_FACTORY_SUPERVISOR_CLI`
and `CAS_FACTORY_WORKER_CLI`. It also removes the `HARNESS_NOISE` names
(terminal, login-session, ssh-agent, interactive-editor, agent-harness and
factory worker-spawn settings), every `CLAUDE_*` and `CODEX_*` variable, and
credentials (`*_TOKEN`, `*_API_KEY`, `*_SECRET`, `*_PASSWORD`), each with a
recorded reason (cas-398c). A factory shell, the factory daemon's background
proof and a scrubbed release shell can therefore share one proof, so `--cut`
reuses the daemon's proof of the same code input. Each receipt's
`environment_policy` lists the included names and every excluded name with its
reason; a miss names the differing included variable.

Row proofs (cas-398c) extend the same key to `release-binary-isa` and
`macos-check`, which depend only on the code input, toolchain and target.
After a green integration run, the factory daemon calls
`assembly-proof.py prove-rows` on the integration tip (deferred when the build
guard reports the host busy; a newer merge cancels it). Each receipt, under
`.cas/merge-sweeps/row-proofs/`, is keyed on the proof inputs plus the row,
its target and a digest of the row's tools (cargo and `rustc -Vv`, plus
cargo-zigbuild and objdump for the ISA audit; Zig is keyed by its bytes). A full gate
runs `assembly-proof.py check-row` before the row cache: a hit records the row
`REUSED` with its source SHA, and a miss prints the differing key and runs the
row as before. `--only` and a no-reuse full gate never consume a row proof.
A `release-binary-isa` hit, from either source, counts only while
`scripts/release-binary-artifact.py verify` finds this tree's audited archive
(cas-c096); without it the row rebuilds in its persistent release target
(`<main checkout>/.cas/release-binary/target`). Releases no longer ship this
archive: since cas-f95b the merge queue's build ships (see Steps to cut a
release).
There is no blanket `CAS_FACTORY_*` exclusion: build controls such as
`CAS_FACTORY_CARGO_BUILD_JOBS`, test safety controls such as
`CAS_TEST_PROTECTED_DBS`, and compiler flags such as `RUSTFLAGS` remain inputs.
The explicit `VOLATILE` set excludes shell bookkeeping, build/output locations
and `CAS_RELEASE_ENV_FILE` (the publisher's env-file locator); values loaded
from that file still count under their own names. Release gate/train
orchestration variables are also excluded. Zig is keyed by binary contents
instead of its worktree path; ignored local environment/config files are keyed
by contents too. New exclusions require confirming that they cannot change the
compiled candidate or test behavior.

Since 3.54.0 (cas-fd0a) the factory daemon runs no suite, gate or journey
evaluation after a merge: the rolling integration sweep, background epic gate
and `cas factory integration-recover` are retired. The merge queue run on the
epic PR is the integration proof.

All gate suite children use `scripts/release-test-env.sh`: it removes harness
identity, train/gate control namespaces and receipt destinations, provides a
clean temporary HOME and disables global Git configuration. Cargo and Rustup
locations remain explicit. The parent retains its orchestration environment;
validated host memory admission and compiler linker resource context survive
so nested suites and linkers remain admitted without inheriting release knobs.

Manual assembly commands need no exported scratch base or Zig (cas-9f61).
`scripts/release_env.py` resolves both for every release tool (assembly-proof,
lane compile, the release gate and the release train). Its scratch base is on
the checkout filesystem, outside `/tmp`, `/var/tmp`, `/private/tmp`,
`/private/var/tmp`, the configured `TMPDIR`, and every `.cas` ancestor. Candidates
in order: an exported `CAS_RELEASE_GATE_HOME_DIR`; `<staging.scratch_root>/cas-release-gate`;
`<mount>/cas/scratch/cas-release-gate`; `<mount>/cas-release-gate/base`
(for example `/home/cas-release-gate/base`); the platform default; and
`cas-release-gate` beside the checkout. The plain clone must be outside Cassy's
disposable roots so discovery and update tests exercise durable projects.
Assembly refuses an unsafe or missing base before tool probing or either suite
and names the provisioning command. Zig resolves from `ZIG`, then the
checkout's `.context/zig`, the main checkout's, then `PATH`. Both resolve
every time; nothing is persisted. `python3 scripts/release_env.py doctor`
checks Zig, scratch per filesystem, the toolchain, the env file, `gh auth` and
the merge-queue query in seconds, with one fix command per gap.

The script tier must pass, and both Rust contexts must report nonzero passed
tests, before an atomic PASS is written
under the shared `.cas/merge-sweeps/assembly-proofs/` directory. The receipt
records the tested Git tree, script-tier status/tree/log, each Rust context's
tree and pass count, per-leg and compile-phase intervals and CPU timings,
memory scheduling decisions and serial fallback reasons, toolchain,
environment and archive size. Full Cassy integration sweeps and the train's
assembly stage use this same command; retries cite the existing receipt.

Before the pipeline lands, `--cut --resume` compares the integration tip/base
with the input recorded by assemble. A changed integration input archives the
old stage receipts and reruns assemble, prep, ledger and every later stage.
Release prose, journey-evaluation reports, member-version bumps and the
generated ledger are replayed onto the new tested tip; source edits block
automatic replay. A rebase conflict
restores the checkout and prints a named blocker with a recovery command.
After a valid pipeline/publish receipt exists, resume finishes that landed
release without adopting a newer integration tip.

A release cut assembles from `main` (`--assemble --from-main`), so its code
input is one the merge queue already proved. Assemble runs
`python3 scripts/assembly-proof.py reuse-merge-queue <release-worktree>`: it
walks the release checkout's first parents while the code input is unchanged
(release prose, journey reports, member-version bumps and the ledger keep it)
and accepts the first commit whose exact tree has a successful `merge_group`
run of `.github/workflows/ci.yml` that published
`merge-queue-validated-tree-<tree>`. It writes a receipt under
`.cas/merge-sweeps/merge-queue-proofs/` and records
`integration_proof.source=merge-queue` in `assemble.integration.json`. No local
suite runs. A miss names its cause (a code change since the queue run, no
successful run, an expired artifact, GitHub unavailable, or
`CAS_RELEASE_TRAIN_MERGE_QUEUE_REUSE=0`) and assemble runs the two-context
proof fresh once, recording `integration_proof.source=assembly`.

The from-main full gate still reuses no row cache, sweep receipt or row proof
(cas-846f), and refuses `--reuse` and `--only`. It reuses only the recorded
integration proof, through `CAS_RELEASE_GATE_INTEGRATION_PROOF`. A
`merge-queue` proof stands for the rows the queue ran: `ci-script-tests`,
`hub-web-tests`, `hub-web-dist-drift`, `hub-web-visual-qa`,
`fixture-paths`, `nextest`, `doctests` and `archive-mode`. The gate's offline
`assembly-proof.py check-merge-queue` revalidates the receipt: same repository
and code input, under 24 hours old, the queue-proved commit's tree and code
input, and that commit an ancestor of HEAD. An `assembly` proof stands for its
three rows (`ci-script-tests`, `nextest`, `archive-mode`). The queue no longer
proves `macos-check`; the standalone gate runs its own macOS proof. Every other
row runs fresh, including the version-delta rows: `workspace-tests` (the compile check of
the bumped manifests and lock), `release-binary-isa`, `builtin-projections`,
`snapshot-portability`, `version-literals` and `changelog-and-versions`. A
receipt miss prints `MISS merge-queue key=…` and runs that row fresh.

The first full release gate of a sweep-backed assembly automatically reuses its
nextest and archive-mode rows from a matching receipt. `--only` remains a fresh diagnostic. Receipts
expire after 24 hours; dirty checkouts, changed code/manifests/scripts/workflows,
toolchain or test environment cause a miss. `CHANGELOG.md`, release prose under
`docs/release-notes/` and `docs/release-reports/`, and Markdown under
`docs/qa/journey-evaluations/` are excluded from the code-input hash; embedded
Rust documentation fixtures remain inputs.
The prep stage's workspace-member `[package]` version values and corresponding
source-less member `[[package]]` lock versions are normalized. The generated
`cas-cli/src/builtins/reference-history.json` ledger is excluded; its source
references and generator remain inputs. Every other manifest or lock byte,
including dependency and non-member versions, still requires a new proof.
The helper uses Python 3.11's standard-library TOML parser.

Gate evidence: PR #655/run 33430464567; PR #657/run 33435093275.

The gate runs independent rows concurrently. The 3.51.1 gate ran its rows one
after another for 29 minutes; its longest row, `release-binary-isa`, took about
10 minutes. Each row in `scripts/release-gate.sh` declares the resources it
needs:

- `cpu`: it builds or checks Rust. At most one such row runs per eight cores,
  and never fewer than two.
- `mem=<GiB>`: its estimated peak. Admission uses the assembly proof's memory
  budget, which is MemAvailable minus the reserve and the taste lane. With no
  heavy row running, one row is always admitted.
- `excl=host-tests`: it runs test binaries or script fixtures, so only one such
  row runs at a time, because they share host ports, hub processes and tmpfs.
- `excl=hub-web`: it uses `hub-web/node_modules`.
- `writes-tree`: it rewrites an input that `cas` embeds at compile time:
  `hub-web/dist` or the reference-history ledger. Such a row runs alone, so
  every other row's reuse lookup and cache receipt see a stable tree.
- `abort`: its failure cancels every running row after it in canonical order.
  Every row before it still runs and reports.

The receipt and `timing.tsv` keep canonical row order. Scheduling events go to
`schedule.tsv` in the row log directory. `CAS_RELEASE_GATE_PARALLEL=1` restores
sequential execution. `scripts/test-release-gate.sh --replay-timing
<timing.tsv> [scale]` replays a real gate's row durations through the fixture
at the given speed-up, and reports the gate's wall time against its longest
row.

Every train stage has a time budget in `scripts/release-stage-budgets.json`.
Loosening a budget is a reviewed change. `release-metrics.py` compares each
completed stage in `stage-events.tsv` with its budget, and writes the results
to `release-latency.receipt` as `STAGE_SECS`, `STAGE_BUDGETS` and
`BUDGET_BREACHES`. The first measured stage, `commit-to-main`, covers the
tasks merged since the previous release tag. Each task is timed from its last
park (`Task awaiting merge` in `.cas/cas.db`) before its epic merge, to the
main commit that landed that merge. The receipt's `COMMIT_TO_MAIN` holds the
task count, the median and the worst task, and the worst task is the value
budgeted. When a stage is over budget:

- `cas release report` marks its Pipeline latency subsection (under Evidence and scope) FAILED and names the
  stage. The section also shows each stage's times from the previous five
  release receipts.
- post-publication files one P0 task in the budget file's epic, through
  `cas serve`. A marker in the run directory keeps a resumed stage from filing
  it twice.

### Release delivery completion

After user-facing changes reach `main`, a source merge is not the delivery
completion receipt. The existing release train's `--host-update` stage also
runs `scripts/release-completion.py`; `--cut` requires its PASS before finishing,
including when an external host-update stage is used or a previous stage-done
marker exists. No additional CI lane is involved.

`delivery-completion.json` binds the bumped runtime manifest/version, previous
release tag, complete commit interval through the landed SHA, refreshed
`origin/main`, exact successful publication workflow, and published asset
digests. The gate downloads the host's release archive, installs its binary into
an empty temporary home, checks its version and clean build commit, and compares
its bytes with the updated host binary. Existing host, hub and refresh convergence
proof is also required. A later merge on `main`, a stale same-version build,
missing publication, or a deferred update fails completion. The main ref is
checked again after installation. Failure replaces any earlier PASS receipt.

Set `CAS_RELEASE_TRAIN_ANNOUNCEMENT_EMBARGO` to the operator's explicit reason to
hold announcements. The cut records it in `announcement-embargo.txt`, keeps
announce/report/receipts pending, and continues runtime publication and install
proof. Standalone announce/report also honor it. Omission on resume preserves
the embargo; explicitly setting it to an empty string lifts it. Resume then
finishes the pending announcement evidence without republishing the runtime.
An embargo never waives the publication or install requirement.

### Supervisor proof targets

Run `python3 scripts/assembly-proof.py prove <epic-worktree>` for an opt-in
assembly diagnostic (for example, after a merge-queue failure).
Native and archive-clone producers each use their own `<worktree>/target`;
`CARGO_TARGET_DIR` inherited from a different lane is replaced. For a scoped
check, use `python3 scripts/proof_target.py run <worktree> -- cargo check --workspace --tests`.
The scoped test wrapper applies the same target isolation. Command-line
`--target-dir` overrides are refused, and every proof log records the source
worktree, HEAD and target path. A target whose recorded source root differs is
refused, including symlinked targets.

The immutable `.cas/build-cache/current` snapshot seeds compiled dependencies
with hardlinks where supported. Cargo freshness metadata is copied privately;
workspace fingerprints/artifacts, incremental state and Cargo locks are never
seeded. Workspace freshness is also discarded when adopting a legacy target or
changing HEAD, so old source mtimes cannot bless another tree's exports.

### Worker build caches

Factory worker spawns use `sccache` automatically when it is installed, while
keeping a separate target directory per worktree so concurrent Cargo builds do
not serialize. An existing `RUSTC_WRAPPER` wins; set
`CAS_FACTORY_DISABLE_SCCACHE=1` for the emergency opt-out. CI uses the GitHub
cache-v2 backend and keeps the cold Build Benchmark explicitly uncached.

Capped worker checks and named tests override both `RUSTC_WRAPPER` and
`RUSTC_WORKSPACE_WRAPPER` with empty strings, including Cargo-config and
`CARGO_BUILD_*` fallbacks. Their Cargo/rustc descendants inherit a private target
lifetime lease; a compiler-cache daemon must not retain it after Cargo exits.
Prestarting sccache cannot prevent its client from spawning another daemon if
the server exits. Supervisor and CI builds keep their configured wrappers.

When a worker delivery parks awaiting merge or closes, Cassy keeps only
`factory.target_cache_retention_count` warm parked check caches (default: 1).
It prunes the other private `target/debug` outputs under the same per-worktree
lane lock used by the capped runner, with a Cargo-lock and open-output check.
Place durable check logs at `target/worker-check.log`, nextest reports under
`target/nextest`, or in the task artifact directory; these survive pruning.
The next check re-seeds missing debug outputs from the immutable baseline.
Active builders and open test/output handles prevent reclamation. Source files,
receipts and the baseline stay intact; concurrent workers keep independent targets.

Actual worker shutdown also reclaims the complete `target/` after lane, Cargo
lock, registered-checkout and process checks. Before deletion, non-build files
including check logs and `nextest` receipts are copied, synced and verified in
`<project-artifacts>/<last-task>/retired-target/<worker>/<head>-<attempt>/`.
A worker without an associated task uses the inventory-only `_retired-workers`
namespace. Recycle keeps its warm target; parked checkouts remain registered.
Failed evidence copying preserves the target and reports the deferred path and
retained bytes. Newly CAS-created private targets have a durable external
record in `.cas/worker-target-owners`, binding checkout and target device/inode,
plus a unique generation marker inside the target to defeat inode reuse,
lease inode, creator/builder PID start time and Linux boot identity. This record
precedes seeding or build data. The capped runner holds the target lifetime
lease. Checkout directories may use ordinary group-writable Git permissions;
the target and ownership directory are created privately, and the marker,
record and lease files reject group or world writes. The runner passes the
lease to descendants; lane and slot locks remain private to the
runner. A held lease or matching live owner prevents retirement, including after
the runner dies. Ownership markers require `target/` to be ignored already.
Otherwise acquisition places no marker, reports legacy output in the trace and
continues with retirement disabled. CAS never edits the operator's Git
configuration or shared exclusions to make a checkout clean.
Retirement holds the same lease through evidence copying and
quarantine, revalidating ownership before deletion. On Linux only, verified
lease-managed targets tolerate opaque unrelated processes while still checking
every readable output handle, executable and mapping. Unknown legacy targets,
replaced inodes and unavailable owner identities remain retained with bytes;
they are never silently adopted. macOS keeps the conservative `lsof` probe.

Explicit target-cache GC has a Linux-only exception for canonical,
non-symlink `.cas/worktrees/<lane>/target` caches. Unrelated opaque processes
no longer count as positive liveness. Reclamation requires no readable process
use, no fresh registered owner with a matching PID start identity, a gone owner
or terminal tasks, and no writes within `factory.target_cache_min_idle_secs`
(default: 7200 seconds). GC holds the lane lock, every Cargo `.cargo-lock`, and
any existing target lifetime lease through quarantine and deletion. It checks
registry, recency, containment, target/worktree inodes and Cargo-lock identities
again before renaming. Preview takes no lane or Cargo locks; selected bytes are
what cleanup would try. One stat walk records recency, directory identities and
the Cargo lock set; checks under the locks re-stat directories and known locks
without walking every artifact again. Output includes per-target `scan_ms` (JSON
`scan_elapsed_ms`). A busy lock without positive readable process evidence is
`indeterminate`, with its lock path. Reports distinguish `live_process`,
`indeterminate` and reclaimable (`eligible` or `selected`) caches. Integration trees, custom targets,
previews and interrupted quarantines keep conservative opaque-process handling;
legacy targets are never adopted into the lifetime-owner registry.

Under filesystem pressure, a registered dead worker with nonempty, entirely
terminal task ownership loses its warm-cache reservation. Cleanup rechecks that
ownership and holds the lane, Cargo and lifetime locks; recent writes, live
users and uncertain evidence still preserve the cache. Missing-gitdir checkouts
are measured with newest-write timestamps and manual-review guidance, but are
never adopted or selected automatically. An operator can authorize one exact
unknown-owner cache with `factory action=gc_cleanup
reclaim_unknown_target=/absolute/checkout/target force=true dry_run=false`.
The opt-in also supports the exact retired `.cas/merge-sweeps/assembly-target`.
Parents, patterns, aliases and arbitrary paths do not grant ownership or consent.
The named operation runs only target-cache GC: source, task evidence, processes
and journey recordings are untouched. It cannot combine `artifacts=true` or
prompt expiry. Omit either destructive flag to preview with a refusal reason.

Named reclaim still requires a positive `factory.target_cache_min_idle_secs`
(default 7200 seconds), same-user nonsymlink files, no native process/open-handle
use (opaque evidence refuses), and the existing lane, Cargo and lifetime guards.
It rechecks recent writes under those guards and the target/worktree identities
before and after quarantine. Neither path conventions nor explicit consent
create an ownership/adoption record.


Assembly and release scratch uses an owner record, PID start-time identity and
an inherited lifetime flock. TERM, INT and HUP stop and reap child groups before
cleanup; the parent allows 20 seconds for a nested guard's 5-second escalation.
A dead lease-managed owner may be swept despite opaque unrelated processes.
Unknown-provenance paths remain fail-closed. Fresh archive remaps record a
`cas-release-remap-v1` receipt binding the scratch owner's PID/start identity
and base inode to the detached HEAD, checkout/Git-pointer inodes and exact Git
common/admin/registry directory identities. After the owner dies and its
inherited lifetime lease is exclusively acquired, cleanup revalidates this
receipt and removes only the clean, unlocked generated remap with
`git worktree remove` (without force). The same targeted operation removes a
receipted missing checkout's stale admin entry; cleanup never runs a global
`git worktree prune` that could discard missing parked deliveries.
Normal guard teardown uses the same checks after children are reaped and the
inherited lease is released; live output handles preserve the remap first.
Malformed receipts, symlinked Git metadata, changed identities, branches,
dirty checkouts and Cassy worker/parked provenance preserve the base. A report
validates identities without changing the checkout or registry. Whole-base
reclamation follows only after the exact remap is unregistered and remaining
registrations are rechecked. Other registered remaps and their bases stay
intact; only dead owned bases' `suite.tar.zst`, `extract`, `tmp`, `cargo-home`
and `bin` siblings may be reclaimed. The operator's main checkout stays
registered and intact.
`gc_report` summarizes release-scratch inventory by default. Its explicit
`--full` view (MCP `full=true`) includes paths, reclaimed/reclaimable and retained
bytes in `RELEASE_SCRATCH_STATUS_JSON`; `--json` (MCP `json=true`) exposes the
same records in typed `machine_status`. Scratch `gc_cleanup` requires both
`force=true` and `dry_run=false`. Reports never create locks or owner records.

`cas factory gc cleanup` also bounds its default inventory, retaining counters
and up to three largest byte-counted items per category. `--full` (MCP
`full=true`) preserves complete text and status payloads; `--json` (MCP
`json=true`) emits one document with `report_text`, `full_report` and
`machine_status`. Human CLI cleanup emits a periodic stderr heartbeat during
blocking inventory work; JSON mode suppresses progress. Presentation flags
never authorize deletion or change the existing force/dry-run gates.

Verified dead owners are swept on the next start, including fresh SIGKILL
leftovers; age only protects unknown provenance. A surviving descendant's
inherited flock defers cleanup even after its parent exits. The guardian owns
Bash temporary directories too, so an EXIT trap cannot outrun child teardown.
`CAS_RELEASE_SCRATCH_MAX_AGE_HOURS` defaults to 6 for unknown paths.
Each assembly clone uses its own `repo/target` inside owned scratch. A
`BoundedCache` lease encloses that target's actual use and records cap/age
cleanup before and after the proof: `CAS_ASSEMBLY_TARGET_MAX_GIB` defaults to
20 GiB and `CAS_ASSEMBLY_TARGET_MAX_AGE_DAYS` to 7 days. Whole-clone teardown
still waits for inherited child leases. Only immutable worker dependency
snapshots seed the clone; workspace artifacts and freshness are private.
Receipts and `gc_report` retain both legacy `assembly-target` and
`assembly-target-leased-v1` inventory paths. Neither is a Cargo proof target.
Unknown, live or opaque caches are retained without silent adoption. Explicit adoption is
available only in a quiet window and refuses held leases or unknown/live users:

```bash
python3 scripts/release_scratch.py --repo "$PWD" \
  --cache .cas/merge-sweeps/assembly-target --adopt-legacy-cache clean
```

On soundwave, opaque `systemd --user` evidence makes adoption refuse. The
supervisor must remove the reported legacy cache by hand in a quiet window,
after confirming no Cargo process is running and no cache lease is held. It was
29 GB at discovery; use the inventory receipt for its current byte count.
Never silently adopt or delete an unknown cache to bypass the liveness check.

`gc_report` includes per-session serve RSS and a summed byte count for registered
local project sessions. PID start-time fingerprints exclude recycled or foreign
process records, aliases count once, and read failures are reported unavailable.
Legacy registrations without a fingerprint are omitted. RSS includes shared
pages; this is not a host-wide PSS/swap measurement. Linux uses `/proc`, macOS
uses native `ps` (KiB converted to bytes).

Durable task evidence has a separate cleanup opt-in: `gc_cleanup force=true
dry_run=false` reaps processes and eligible caches. It also removes only
`trace.zip` and `.webm` recordings under the project's artifact namespace when
the owning task is closed for at least 24 hours. Missing close dates, open tasks,
unknown task directories and symlinks are preserved; screenshots, ledgers and
JSON/Markdown receipts stay in place. The cleanup reports files and bytes reclaimed.
The report also measures sibling artifact namespaces, including renamed project
keys, legacy bare task directories and shared builds, and flags `DerivedData`
and `target` build outputs stored as evidence. These sibling roots remain
review-only even when artifact deletion is enabled for the current project.

With `artifacts=true force=true dry_run=false`, GC also removes regular candidate
files named `cas` after their task has been closed for at least 48 hours, when
their actual SHA-256 matches a hash recorded in that task's `LEDGER.md` alongside
a recorded `cas` version. It keeps the ledger, logs and checksum records, reports
removed files and reclaimed bytes, and never follows symlinks. Open tasks, missing
close dates and unrecorded or mismatched binary identities preserve the binary.

Deleting whole task artifact directories additionally requires `artifacts=true`.
Only tasks closed at least seven days ago are eligible; missing close dates,
references from open tasks or tasks updated within seven days (including notes),
and unavailable reference evidence preserve artifacts. References may name the
task/path or a published artifact ID or durable URL. Unknown directories and
ambiguous legacy artifact roots remain review-only. Orphan cleanup reaps a
private process group or fingerprinted descendants, preserving shared groups.

`cas worktree sweep` fetches origin before judging merge ancestry, prints the
comparison ref and commit, and preserves worktrees when fetching fails. Repos
without origin use a verified local default branch. `gc_report` also inventories
checkouts directly under `.cas/` or `.cas/worktrees/` whose `.git` file points at
a missing Git directory. Review these for uncommitted work before manual removal.

Lane compile previews carry provenance and a lifetime owner lock.
Previews are direct children of `.cas/worktrees` so the existing private target
ownership check admits them. Their sibling metadata directory binds the exact
checkout path, Git common directory and commit and holds the lifetime lock.
GC also recognizes older nested `lane-compile-*/preview` checkouts.
Explicit `gc_cleanup force=true dry_run=false` removes stale owned detached previews,
including their Git registration, after revalidating ownership, process liveness
and `factory.target_cache_min_idle_secs`. Recent or live previews survive;
previews created before provenance was recorded remain inventory-only. The
explicit `gc_report --full` view (MCP `full=true`) includes preview target sizes
and `lane_previews` dispositions in `TARGET_CACHE_STATUS_JSON`; `--json` (MCP
`json=true`) includes the typed record in `machine_status`. The default summary
omits raw status records. The existing high/low watermark configuration
controls cache pressure warnings; preview cleanup does not need disk pressure.
On macOS, liveness uses NUL-delimited `lsof` field output and fails closed if
that probe is unavailable or reports errors. Only the evictor's held Cargo-lock
file descriptor is exempt; other same-process handles and executable/mapped
artifacts preserve the cache. Linux also inspects `/proc/PID/exe` and `maps`,
and unknown/inaccessible evidence keeps the cache; reclamation therefore needs
a readable process table. Same-HEAD parks have distinct marker generations,
so an older retention inventory cannot evict a newly parked warm cache.
Interrupted `.cas-parked-debug-*` quarantines remain preserved for explicit
whole-target GC once the worktree is inactive and the recency/pressure policy
permits it. They are included in target byte inventory; automatic park-time
cleanup does not retry a quarantine or delete proof logs to recover it.

New isolated workers also seed their private `target/` from compiled artifacts
hardlinked out of the quiescent snapshot named by `.cas/build-cache/current`;
small Cargo dep-info files are copied with their target root rebased. The release
train's host-update stage refreshes that baseline automatically from a detached
checkout of the released tag on main. `host-update.json` records the completed
snapshot ID and source commit; a refresh failure warns without failing the
published release. Run `scripts/refresh-worker-build-cache.sh` for a manual refresh
after an epic integration merge. The script builds a new snapshot to
completion and only then publishes its pointer, so no worker ever seeds from a
live Cargo writer. Old snapshots remain valid for in-flight seeders and should
only be removed during a maintenance window. Set
`CAS_FACTORY_DISABLE_TARGET_SEED=1` to skip seeding. Do not replace this with a
shared live `CARGO_TARGET_DIR`: its Cargo lock serializes the worker fleet.

Worker provisioning (including store and Git base resolution) runs in a separate
process group with a five-minute deadline. Timeout, targeted shutdown and
`restart_spawn_queue` kill only that generation's provisioner and descendants;
the daemon keeps processing shutdowns and messages. Reset drops already-dequeued
spawn actions and reports them; persistent queue rows continue draining. Retirement cleans newly-created checkout/branch metadata and worker-specific
Git locks best-effort; reused worktrees are preserved. Inspect any reported
leftovers before retrying.

Before creating or reusing a checkout, `factory.spawn_min_free_gib` checks
available space on the worker filesystem (default 25 GiB; 0 disables the floor).
Below the floor, the spawn fails with `spawn_disk_floor` before a checkout or
branch is created. Set it with `cas config set factory.spawn_min_free_gib 30`.
The target repository's config applies to cross-repository workers. Existing
usable targets and `CAS_FACTORY_DISABLE_TARGET_SEED=1` bypass seeding, while
all spawn paths still apply the disk floor. Seeding normally hardlinks artifacts;
the floor reserves space for subsequent worker writes and builds.

#### Scratch root for worker build output

`factory.scratch_root` (unset by default) moves regenerable worker build output
to a scratch drive, for example `cas config set factory.scratch_root /mnt/rewind/cas`.
The value must be an absolute path. Once it is set, each project gets its own
`<scratch_root>/<project-key>/` directory, where the key is the
`<name>-<sha256>` label the durable artifacts root uses:

- Each managed worker (`.cas/worktrees/<name>`) builds into
  `<scratch_root>/<project-key>/targets/<name>`. The spawn environment's
  `CARGO_TARGET_DIR`, the capped worker check, target seeding, ownership
  leases, parking and retirement all resolve this same path. Sources stay in
  the checkout, and the main checkout and lane previews keep their in-tree
  `target/`.
- Capped worker and verifier Cargo children set `TMPDIR`, `TMP` and `TEMP` to
  `<scratch_root>/t/<checkout-key>`, a short private `0700` directory whose
  marker records the full project and checkout identity. Without a configured
  scratch root the base is the project `.cas` directory. This avoids the long
  project/target namespace that exceeds Unix socket pathname limits. Test
  temporary HOMEs and binary copies stay off the host's potentially
  memory-backed `/tmp`. Worker retirement and verifier GC reclaim only matching
  owned directories under their existing lane and live-process guards, without
  copying temporary payloads into evidence. The caller's environment stays intact.
- Published baselines live in `<scratch_root>/<project-key>/build-cache`.
  `scripts/refresh-worker-build-cache.sh` reads the key through `cas config get`
  and prints `Worker build-cache root:`, and the release host update reads that
  line to find the root.
- Seeding hard-links from the baseline into the target, so both must be on one
  filesystem. If their device ids differ, the seed is refused before anything is
  written, the spawn reports why, and Cargo builds privately. The baseline is
  never copied.
- The `factory.spawn_min_free_gib` floor checks the scratch filesystem first,
  then the checkout's. Factory launch also checks the scratch filesystem's
  watermark.

When the root is unset, every path stays as it was: `<worktree>/target` and
`.cas/build-cache`. Running workers keep their current target. They move only
when they respawn, and live targets are never moved. Run the refresh script
after you change the root, so new workers have a baseline to seed from.
Retirement and park-time pruning cover relocated worker targets.

The release scratch base follows the same root (cas-baa8).
`scripts/release_env.py` resolves it first, as
`<scratch_root>/<project-key>/release-gate/base`. Every release-gate archive and
assembly clone sits beside that base in `release-gate/`, including each clone's
`repo/target`. This base may
sit on a different device from the checkout, but it must share the scratch
root's filesystem: assembly clones hard-link the baseline from
`<project-key>/build-cache`, which `scripts/proof_target.py` now reads from the
same root. A mount below the root is refused. Any other base on another
device, including a `staging.scratch_root` base, is still refused. The gate's
`scratch-base` row asks `release_env.py` about a device mismatch, so the
boundary rule lives in one resolver. The legacy in-tree
`.cas/merge-sweeps/assembly-target` inventory stays where the old caches were.

`gc_report` lists the relocated caches with their sizes under "Scratch root
caches": the build-cache, each worker target and the release scratch base. Its
release-scratch section scans the relocated base. `gc_cleanup` also inventories
and reclaims stale worker targets in the exact project scratch namespace. It
holds the worker lane, Cargo locks and any durable lifetime lease through a
sibling quarantine and deletion; live owners and recent writes remain protected.
The newest `factory.target_cache_retention_count` eligible scratch targets stay
warm. Missing-checkout targets and unmanaged scratch directories stay review-only.
GC reports the same filesystem path as build admission and reserves the configured
builder cap's 10 GiB per lane when deciding pressure, even below the high
watermark. Selection and warm retention are computed separately per device, so
reclaim on the checkout device cannot count against scratch pressure.

Local sccache 0.10.0 does not produce cross-worktree Rust hits because absolute
checkout paths remain in its cache keys (measured 0/45 hits even with
`--remap-path-prefix`). Supervisor and CI builds keep sccache enabled for when
[upstream path normalization](https://github.com/mozilla/sccache/pull/2678)
lands; hardlink seeding is the current cross-worktree mechanism. Capped worker
builds reuse their private Cargo target without a compiler-cache daemon.

### CI-load policy

Standing operator policy: factory/* pushes and epic-targeted PRs run
import/dependency-selected tests in Scoped Validation. Protected-default PRs run
the required Fast Validation admission lane and an advisory macOS notice. The release
merge queue runs the complete workspace suite on its synthetic tree once. Only
at this queue stage, `scripts/ci-queue-flakes.py run` reruns the exact failed Rust
tests or failed journey specs once in the same job on the same tracked tree.
The Rust recovery attempt has a 29-second execution deadline and bounded child
cleanup, keeping its added time below 30 seconds; passing suites add no rerun.
A second failure still ejects the queue entry. Fail-then-pass is **FLAKY**, and
both attempts' reports, output and journey traces remain in the queue artifacts.
The required Fast Validation fan-in refuses green until `reconcile` has durably
filed or updated a `priority:p1`, `flaky` issue in the configured Cassy issue
route (`Richards-LLC/cassy` here), with its test identity, run link and original
failure. Impact recall still records the original failing status and log.

Open issues labeled `flaky-quarantine` are the quarantine source of truth. Each
has one `<!-- cas-queue-flake: JSON -->` test identity/observation line. Shards
read them with an issues-read `GITHUB_TOKEN`, exclude only those identities and
print the issue links in the job summary. State/label-filtered GitHub lists can
lag acknowledged updates, so discovery enumerates unfiltered issue history and
reads each flake candidate directly before deciding its open state and labels.
Reconciliation adds that label to native test identities after
two distinct GitHub run IDs; duplicate shards, cached observations and run attempts count once.
Individual script and web test commands retry once through the same publication
fan-in. A recovered command receives the label immediately, but remains covered
on later trees: quarantining a command never excludes its whole suite.
Only recognized cleanup diagnostics after a positive all-pass summary become
warnings; mixed pass/fail summaries and later assertions retain failure.
The fix PR uses `Fixes #N`: closing the issue removes the exclusion from the next
queue run. No bot PR, new credential or evaluated-tree edit is needed.

The hosted publication fan-in alone has issues-write and contents-write. An
atomic `heads/cas-queue-flake-publication-lock` ref serializes issue creation
and updates across runs without changing the evaluated tree. Concurrent writers
fail closed and retain their receipts. If a cancelled publisher leaves this ref,
confirm the owning run (named in its commit) has finished before deleting the
ref and reconciling the retained receipts. Transport, malformed quarantine and
missing-shard evidence failures also refuse green. Workers, lane CI, schedules,
main/tag pushes and ordinary manual checks keep zero retries and ignore queue
quarantine. The explicit `test-ci-queue-flakes.py --live-acceptance <dir>` mode is
only a throwaway-branch workflow proof using synthetic native runner fixtures;
normal script tests use local HTTP fixtures and never create live issues.

When the queue's successful tree is pushed unchanged to main, the main-push Fast Validation lane
reuses that receipt and names the validating run. `macOS Check` is advisory and
never runs on `merge_group` (cas-04c3): GitHub-hosted runner non-acquisition
must not eject a green release. Its existing non-queue routes remain: a cheap
protected-PR notice, main-push Darwin cache maintenance, tag pushes, schedules
and dispatches. On a queue-proven main tree, a dependency-cache hit skips Cargo;
a miss compiles to seed the cache. This does not prove every landed source change
on Darwin. Release Prebuild's `build-macos` remains the pre-publish macOS proof;
its queue builds are non-required and publication still requires packaged-asset
audits (see the [release-flow map](../../docs/ci/release-fast-publication.md)).
Direct pushes, bypass merges, receipt lookup failures, and changed trees still
run every lane. The non-required full/heavy tier (Clippy, Test Compile Guard, Build
Benchmark, and both Panic Isolation profiles) belongs only to
supervisor-controlled main pushes, schedules, or manual dispatches—never
factory/*, epic/*, tags, or pull requests. Keep this policy pinned by
`scripts/test-ci-test-tiers.sh`, rather than relying on convention.
[Change-scoped CI](../../docs/ci/test-impact.md) describes selection, additive
failure history, count/time receipts and full-suite recall measurements. Docs-only
diffs (paths under `docs/` or Markdown files outside embedded
`cas-cli/src/` content) on pull-request, push, and merge-group events route
only to the `Docs Lint` job; it runs Markdown lint, validates any changed
release-note drafts, and checks that generated AGENTS.md files are current.
The required Fast Validation context remains present and skips its full work
for that class; the advisory macOS job is skipped. Mixed and
code diffs keep the full required tier.

### Build profiles and the binary

The MCP server is always included because factory agents depend on `cas serve`; the optional `mcp-proxy` feature is enabled by default. Binary is `cas` (lib + bin in `cas-cli/`). Build script embeds git hash and build date.

Build profiles must use `panic = "unwind"`. The MCP tool-dispatch panic catcher relies on `tokio::spawn` + `JoinError::is_panic`, which only observes a panic if the worker thread unwinds. A compile-time guard in `cas-cli/src/lib.rs` refuses non-test builds with `panic = "abort"`; do not work around it, because the catcher is what keeps `cas serve` alive across handler bugs.

## Skill & Rule Sync

Rule-driven code and tests carry a `cas-rule:` comment with the rule id and
JSON-quoted verbatim scope. The brief or close receipt declares each causal
site; close checks adjacent markers at the delivered tip and verifier review
checks omissions. `cas rules refs <id> --json` lists tracked sites. See
[Rule references](../../docs/RULE_REFERENCES.md) for syntax, legacy behavior and
the optional stakeholder provenance seam.

Cassy auto-syncs rules to `.claude/rules/` and skills to `.claude/skills/` as SKILL.md files with YAML frontmatter. The sync logic lives in `cas-cli/src/sync/`. Rule promotion uses configurable outcome evidence: `sync.promotion_threshold` defaults to 2 and `sync.promotion_evidence` accepts `helpful` and/or `retrieval`; one `mcp__cas__rule action=helpful` call never promotes. A reviewer's explicit decision uses `mcp__cas__rule action=promote id=<id> change_note="<why>"` instead of voting. Judgement-call decisions require two distinct `source_ids`; `rule action=update source_ids="<all contributing IDs>"` preserves merged evidence. An observed mechanical constraint tagged `enforceable:lint`, `enforceable:test`, `enforceable:hook`, `enforceable:gate`, or `enforceable:type` can be promoted after its first verified occurrence. Only an operator-authorised write from the registered non-factory operator session files its idempotent encode chore, even while draft. Agent and supervisor writes create no encode chore and name the operator command (`rule action=update id=<id> tags="<current tags>"`); previous rule authority does not grant a later writer permission to file one. Retired or harmful rules do not file chores. `factory action=epic_status` includes pending encode chores from the project backlog. Retrieval promotion requires useful outcomes across at least two distinct privacy-preserving sessions. Harmful feedback and negative retrieval outcomes require `sync.demotion_threshold` (default 2) before demoting Proven rules to Stale and removing their synced files. Existing Proven rules are grandfathered until new evidence crosses the configured threshold.

Unpromoted drafts expire from their creation time during the existing maintenance
pass. `rules.draft_ttl_days` is a positive day count, default 30; set/get/list/reset
use the ordinary config interface. Surfacing a draft does not renew its window.
The versioned store retires eligible drafts and records their history under the
same write lock, with `auto-retired: not promoted within 30 days` at the default
window. Proven rules are excluded. Filesystem/cloud projections follow the
existing rule-store wrapper, and cited sites get the existing retirement cleanup.
Stores without lifecycle history refuse expiry rather than discard that history.

A verifier's single verdict is not a rule candidate. `rule action=observe` needs
at least two completed verdicts from different tasks in comma-separated
`source_ids="<task>:<verdict>,<other-task>:<other-verdict>"`. Citations are checked
against persisted verdicts before the complete bundle is recorded atomically;
repeated verdicts on one task do not establish independent incidents. The
supervisor and operator still decide whether to adopt a candidate as a rule.


Built-in skills ship to every project, so they carry no cas-src-only procedure. This repository's own factory guidance (release prebuild, worker build-cache refresh, the assembly gate command, cargo triage) lives in [docs/factory/cas-src-factory-notes.md](../../docs/factory/cas-src-factory-notes.md) and [docs/factory/cas-src-worker-notes.md](../../docs/factory/cas-src-worker-notes.md).

### Skill validation contract

`validation_script` is an opt-in create/update gate. When present, Cassy runs it
before writing the skill to SQLite or syncing its `SKILL.md`; a zero exit status
admits the change and a non-zero status (or timeout) rejects it without creating
a version row. The probe runs through the platform shell from a fresh temporary
directory, with a scrubbed environment that retains only `PATH` for executable
lookup. On Linux, a one-second capability probe checks whether bubblewrap can
create the required network namespace before selecting it. On hosts without
usable bubblewrap, the default is a degraded plain-shell sandbox with the same temporary cwd and scrubbed
environment; Cassy reports an explicit warning that network isolation is
unavailable. Set `skill_validation.require_sandbox = true` to fail closed
instead. The one-second capability probe, five-second validation timeout and
process-group cleanup bound the MCP request. Scripts are local, deterministic availability checks: they must not
depend on network access, inherited CAS credentials, project files, or
persistent relative writes. There is currently no network opt-in declaration,
so all validation scripts use the no-network policy. Validation output is
included in rejection errors and capped to keep responses bounded.

Skill `preconditions` and `postconditions` are advisory metadata: Cassy does
not execute or evaluate them. They are surfaced in `cas skill show` and in
generated `SKILL.md` sections so consumers can evaluate them in their own
runtime.

### Editing supervisor or worker guidance: the SessionStart budget is a hard constraint

SessionStart also checks Slack channels and thread links named in its current
handoff. `hooks/handoff_updates.rs` inserts newer human posts above that handoff,
using Violet's author names without treating those labels as operator authority.
A thread reference includes a channel read so newer sibling posts are visible.
The lookup has a four-second outer deadline (including Cloud memory lookup),
three seconds for Violet setup and reads, at most six reads of 50 messages,
and at most ten excerpts in a 1,800-byte section. Known additional posts have
a `+N more` line. Bot/app posts and configured Violet bot IDs are excluded.
Empty results add no section; service failures, partial reads and read caps
leave a one-line couldn't-check notice. This check adds no Slack writes.

`cas-supervisor.md` and `cas-worker.md` are not ordinary docs. Their bodies are
injected verbatim into the SessionStart `additionalContext`, which has a **9 216 B
aggregate budget** (`hooks::handlers::session_budget`). Role guidance is
*protected*: it is never compacted. Everything else — Ready Tasks, Helpful
Memories, Available Skills, the GitHub issue triage and its open-issue titles —
is degradable, so when the guidance grows, one of those is silently replaced by a
one-line "run this command instead" summary. Nobody sees an error; a section just
stops being there.

That is not hypothetical. In cas-caaf a 796 B role note pushed the payload over
budget and a supervisor lost the open-issue titles in live sessions, while CI
stayed green because the note only appears when `CAS_FACTORY_WORKER_CLI=codex`.

Three tests hold the line; if you grow the body, expect to meet them:

| Test | What it bounds |
| --- | --- |
| `test_supervisor_guidance_under_8kb` | the guidance as a component (8 000 B soft cap, 8 192 B ceiling) |
| `supervisor_guidance_leaves_room_for_the_rest_of_the_session_start_payload` | what the guidance *leaves* — at least `SESSION_START_GUIDANCE_REMAINDER_FLOOR_BYTES` (2 400 B) of the budget for every other section |
| `the_codex_worker_matrix_still_fits_the_session_start_budget` | the same bound with the Codex worker note appended (`CAS_FACTORY_WORKER_CLI` unset and `codex`) |

The remedy when one fails is always the same: move detail into
`cas-supervisor/references/` (or `cas-worker/references/`) and leave a pointer.
The body is for rules that apply every session; everything else is one named
command away.

### Builtin skill references

`finish` is a shared builtin skill; Claude Code exposes it as `/finish`, and the
other harnesses receive the same `SKILL.md`. It needs no personal HOME seed.
Update also refreshes finish in named Claude/Codex accounts. The shared bases
(`~/.cas/claude-base`, `~/.cas/codex-base`), which every account's `skills`
links to, receive the full builtin set. The exact old cas-f4ea command seed is
removed in favor of the skill.
A customized Claude `commands/finish.md` blocks builtin installation: Cassy leaves both
entrypoints untouched and skips installing its finish skill for that profile.
An existing customized finish `SKILL.md` is preserved, even when it retains
`managed_by: cas`. Delete the custom entrypoint and rerun `cas update` to accept
the builtin. Other builtin skills keep their existing ownership policy.

For Codex, a personal `~/.agents/skills/finish/SKILL.md` takes precedence over
Cassy's default, named-account, shared-base and project copies. Update skips
installing another finish and removes an earlier Cassy copy only when its hash
matches the recorded baseline or a shipped version. Customized copies and
symlinks are preserved; if both entrypoints exist, update warns with both paths
so the operator can choose which to keep. Named-account/shared-base finish
updates and preservations appear in human output and JSON `finish_accounts`;
account-discovery failures appear in `account_discovery_warnings` and do not
abort default-profile or shared-base refresh.

Every file in a managed builtin skill directory other than its `SKILL.md` (references, scripts, examples, templates) is owned by that skill and synced with a baseline ledger: a destination that differs from both the recorded baseline and every version Cassy has shipped is preserved as a local customization (and surfaced in a SessionStart banner). The set of "versions Cassy has shipped" is the embedded `cas-cli/src/builtins/reference-history.json`. The ledger keeps deleted files too: sync prunes an installed file Cassy no longer ships only when its content proves it is Cassy's (a shipped hash, the recorded baseline, or `managed_by: cas`). Retired managed agents are pruned the same way, and `cas doctor` reports installed-vs-catalog drift as `host install parity`.

Builtin Markdown that only applies in this repository (Cargo and nextest
shapes, `CONTRIBUTING`, `scripts/journey-eval.sh`, `scripts/assembly-proof.py`,
hub-web, the merge queue) sits between whole-line (optionally indented) `<!-- cas-src:begin -->` and
`<!-- cas-src:end -->` markers, optionally split by `<!-- cas-src:else -->`
with a project-neutral replacement. Sync renders the cas-src branch only into
this checkout's harness dirs; every other target, including the personal dirs
that outrank project skills, gets the neutral branch. Markers never reach an
installed file, and the history ledger records both renders.

**After changing, adding or deleting any builtin skill file — and before cutting a release — run:**

```bash
./scripts/gen-builtin-reference-history.sh
```

and commit the regenerated JSON. Skipping it means the version you just replaced is not recognized as Cassy content downstream, so installs that still hold it will keep it forever instead of upgrading (cas-0c0a).

Lanes never hand-merge the ledger (cas-fb11). `.gitattributes` marks it `merge=cas-generated`, so a merge keeps the target's copy without a conflict, and `scripts/regenerate-generated-artifacts.sh <pre-merge-head> <merged-head>` reruns the generator on the merged history and commits the result. Cassy's merge runs that script. After a manual `git merge`, run it yourself; run `python3 scripts/cas-merge-drivers.py install` once per clone first. The committed ledger then equals a fresh generator run, which the release gate's `builtin-projections` row checks. `scripts/test-regenerate-generated-artifacts.py` merges two lanes that both add builtin history.

### Viktor distribution

The managed Viktor surface has three coupled user-facing pieces: its Claude/Codex/Grok builtin
skill mirrors, `cas viktor` credential-safe provisioning output, and the proxy's configured
allowlist. Keep the skill body compact and put operational detail in its reference. The mirrors
must retain identical meaning after their mechanical tool-prefix substitutions; run the builtin
flavor-drift test. Never add a credential literal to source, fixtures, docs, or artifacts. The
only supported user entry is `cas viktor key`; it prompts, validates, then saves the
key in machine-scoped state, while `cas serve` still holds only the `VIKTOR_API_KEY` reference.
For a new CLI or proxy-facing behavior, add a clean-project `cas init`/command assertion and a
direct registry test so the source cannot exist without reaching all three downstream harnesses.

## Releasing

### Version policy

- `cas-cli/Cargo.toml` version is the release version (currently 2.0.0).
- Internal crates (`cas-core`, `cas-mux`, `cas-mcp-proxy`, etc.) stay at `0.1.0` unless published separately.
- **Patch** (x.y.Z): Bug fixes, doc updates, performance improvements.
- **Minor** (x.Y.0): New features, new CLI commands, new MCP tools.
- **Major** (X.0.0): Breaking changes — cloud protocol changes, CLI flag removals, MCP tool schema changes.

### Breaking changes

These require a major version bump:

- Cloud sync protocol changes (push/pull shape, endpoint paths)
- CLI flag or subcommand removals/renames
- MCP tool parameter schema changes (field renames, type changes)
- Migration format changes that break older DBs without a migration path

### Steps to cut a release

One command cuts a release (cas-f95b). The version bump rides in the epic PR,
the merge queue proves the exact tree that ships and builds it once, and the
version change landing on `main` publishes it with no agent in between.
Platform support: macOS: compile-checked and launch-tested; suite runs on Linux.

1. On the epic branch, after closeout is complete (one full review, one batch fix,
   fix-diff re-review, one QA round), check the release entries: each
   delivery's `changelog.d/<task-id>.md` fragment plus anything still under
   `## [Unreleased]` in `CHANGELOG.md` (Keep a Changelog format). Deliveries
   write fragments, not `CHANGELOG.md`, because GitHub's merge queue ignores
   the local `cas-changelog` merge driver and conflicts two insertions at the
   top of the section (cas-f2be).
2. Run `scripts/release.sh --next-version`. It prints the next version (one
   minor step above `main`) and the release-notes draft path
   `docs/release-notes/<date>-v<x.y.z>-slack.md`. Write the Slack draft there
   (`docs/release-notes/RUBRIC.md`), or anywhere and pass `--notes <file>`;
   `{{VERSION}}` in it becomes the version.
3. Run `scripts/release.sh` (in the background with a log: it watches the PR
   until it merges). It merges `main` in when the branch is behind, bumps the
   release crates and `Cargo.lock`, runs `npm ci && npm run build` in `hub-web`
   so the committed embedded Commander bundle carries that version, folds the
   `changelog.d/` fragments and `[Unreleased]` into `## [x.y.z] - <date>` and
   deletes the folded fragments, names the draft, commits `release: vx.y.z`, pushes,
   opens the PR to `main`, waits for its Scoped Validation run to go green,
   and only then enables auto-merge. When another release lands first it
   merges it, takes the next version and re-enables auto-merge itself. Exit 5
   is a red PR CI, reported once with the failing tests (the PR never entered
   the queue); exit 4 is a code conflict with `main`; exit 3 is a merge-queue
   ejection. Repair, push, then rerun the command.
4. Hotfix: branch from `main`, commit the fix and its changelog fragment,
   then `scripts/release.sh --hotfix` (the next patch version). It takes the
   same PR, queue and workflow path.
   Release admission requires `maximumEntriesToMerge=1` and `ALLGREEN` on the
   target queue; five or more builds can still run concurrently. The singleton
   landing prevents another PR from changing the prebuilt release tree.

5. The merge-queue run also runs `release-prebuild.yml` for a PR that changes
   the version: Linux `release-fast` and macOS ARM64 (LTO off) start at
   enqueue, in parallel with the tests, on hosted runners. They are not
   required checks, and the queue lands its tested merge commit as `main`.
6. `release.yml` runs when the version change reaches `main`. It adopts the
   queue run's archives by commit or tree id (`find-release-prebuild.sh`) and
   builds them only when none exist. It runs the ISA audit and the operator
   launch smoke on the packaged Linux archive, and the signature check and
   launch smoke on the packaged macOS archive. It then creates the annotated
   tag `vx.y.z` and the GitHub release (`release-publish.sh`), whose notes end
   with the release timer, and dispatches the install-path proof.
   After publication, a separate `deploy-hub` job uploads that tag's
   `hub-web/dist` to Vercel `cas-hub-static` in `richards-llc`, without a
   rebuild. It discovers the current deployment through the live domain,
   skips an unchanged dist tree, verifies anonymous bytes before promotion
   and on the domain afterward, and restores the previous deployment if
   promotion or verification fails. The `hub-deployment.json` release asset
   records the source tag/commit/tree, deployment IDs, verification times
   and outcome; failed checks also retain a workflow artifact. Authentication
   uses the repo secret `VERCEL_HUB_TOKEN`, passed through the CLI environment.
   This job cannot delay main or release publication. If it fails after
   publication, rerun the failed job; do not republish the existing release.
7. The supervisor host installs on publish: `scripts/install-on-publish.sh
   --install` sets up a systemd user timer on Linux or a LaunchAgent on
   macOS once per host. While the user session is active, every minute it
   compares the latest release with `cas --version`, runs
   `cas update --yes --version x.y.z` when it is newer, and completes the
   release timer in the notes after verifying the installed version: PR
   opened to the first host's actual installation, the one latency figure.
   On macOS, inspect `launchctl print gui/$(id -u)/dev.cassy.install-on-publish`
   and `~/.local/state/cassy/install-on-publish.log` (under `XDG_STATE_HOME`
   when set); `--uninstall` unloads and removes the per-user scheduler.
8. Derive announcement digests from the published bytes with
   `./scripts/release-published-receipt.sh vx.y.z`; it fails closed while
   either asset is missing or a digest disagrees with GitHub.

Recovery and dry runs: `gh workflow run release.yml -f dry_run=false` on
`main` republishes after a failed run and refuses an existing release. On any
branch, `gh workflow run release-prebuild.yml --ref <branch>` followed by
`gh workflow run release.yml --ref <branch>` runs the whole path against a
throwaway `dryrun-` prerelease that it deletes at the end; `cas update` never
sees a prerelease.
Dry runs never run the hosted hub job or send any Vercel requests.

The retired release train has been removed. `release-gate.sh --fast-rows`
remains the factory pre-push gate; its no-build row catalog lives in
`scripts/release-integration-gates.py`.

### Task close delivery attribution

Green lane CI does not declare worker completion. Director merge readiness
requires `awaiting_merge` or an explicit `status: ready` return contract naming
the exact pushed tip, regardless of the task's methodology. Red checkpoints and
ordinary check/test receipts do not satisfy that boundary. A manually requested
merge of an `in_progress` task remains available and returns an unparked-task
warning; its existing CI, QA and authorization gates still apply.

Close posture checks (`additive-only`, `value-only`), no-code intent, and the
receipt diff stat share task delivery attribution in
`mcp/tools/core/task/lifecycle/close_ops/task_attribution.rs`. The remote-tracking
integration target is preferred when present. Unmerged commits are bounded by
the task work window; already integrated commits need task identity. An explicit
commit receipt caps the displayed history and includes unnamed predecessor
commits within the work window, stopping at another task's commit. Receipt
inputs remain hexadecimal commit IDs, including unambiguous abbreviations.

The close gate resolves the recorded factory branch locally or on origin before
counting commits. An unavailable branch produces a missing-evidence error and
is not parked for merge. A valid integrated `commit_receipt` proves delivery
even after a suffixed promotion branch was deleted and the canonical task
branch never existed. If that receipt is invalid, close reports its validation
reason before parking; `merged_into` does not override the declared work target.
An attributable first park binds the validated pushed branch HEAD, including target
freshness merges and canonical regeneration commits restricted to their known
generated outputs. A generator's provenance task ID is not delivery ownership.
Stacked deliveries retain their prior attributed boundary; the supervisor merge
still refuses a source that differs from its immutable parked tip.
After rebasing a parked delivery, push the rewritten tip and re-run close with
checks for that exact tip before requesting its merge. Close preserves the full
parked patch set, allows attributable corrections while the worker exclusively
owns the delivery, and audits the replacement anchor. An explicit receipt or
stored anchor absent from the pushed source refuses at close with recovery
guidance. Sending a merge request cannot advance the persisted park anchor.
An empty checkpoint atop real task commits retains its parked anchor while
content proof measures the attributed task commits beneath it. A delivery
containing only empty commits still refuses; existing attribution and content
loss checks remain in force.
A transferred delivery also resolves its recorded parked or handoff branch
when the receiving worker's lane belongs to another task, including a branch
available only on origin. Already integrated work needs no branch adoption or
alias ref under the receiving worker. A source branch recycled for another
task still needs evidence linking it to this delivery.
A no-code task whose stale code target and delivery
anchor were cleared closes on a portable `external_ref`; retained code anchors
and commit receipts still require delivery proof.
An undeclared helper that never delivered code may instead close with
`no_delivery=true reason="<completed helper outcome>"`. This declaration is
required on every worker attempt and audited with the registered caller, repository,
target SHA and measured refs. Cassy requires readable Git and task attribution,
no recorded delivery or binding QA round, and no task commits or unmerged work
on any local/origin candidate lane or closing HEAD. Empty diffs alone grant no
exception. The declaration skips QA and factory-branch delivery gates while
preserving verification and uncommitted-work checks; it sets no methodology,
external reference or delivery anchor. It cannot be combined with other close
receipts, dispositions or overrides. The unified MCP and per-action close
requests expose this flag; there is no separate CLI task-close command.
When verification is required, Cassy saves the explicit no-delivery reason and
assignee atomically with the exact dispatch. Approval resumes that close through
the ordinary absence and proof gates, lifting only that dispatch's verification
wait. Independent holds stay blocked, and no worker close retry is queued.
A reopen retires the saved intent; identical supervisor verdict replay after
closure acknowledges history without closing or notifying again.
A read-only no-code task also closes on portable PR or artifact evidence when
its allocated task branch contains only foreign commits. An earlier false park
on those commits is retired; allocating a branch does not establish ownership.
Named task commits and unclaimed unmerged changes still require delivery proof.
A live registered supervisor can close a verification-only task whose throwaway
CI commits must never merge with `evidence_only=true` and
`evidence_only_reference=never-merge:branch:<proof-ref>`, retaining the required
task-root artifact and rationale. Cassy measures the reference, retained anchors
and task lanes against the fresh target, with a combined 256-commit limit.
Only regular YAML files directly under `.github/workflows/` may be declared
non-delivery; product paths, executable files, symlinks, merges, unknown anchors
and code methodologies refuse this disposition. The proof reference must be
unmerged. Its immutable SHA and measured commits are audited, while evidence
records the target SHA with no delivery paths. The explicit never-merge
declaration does not predict or authorize a future Git merge. Ordinary evidence
references and negative-result closes retain their existing rules. Stale-anchor
recovery suggests a replacement receipt only when its tip belongs to the task.
QA classification of an integrated merge receipt uses that merge's own
first-parent diff. Later PR merges cannot add their UI paths to the receipt's
evidence requirements or hide UI that the receipt itself delivered.
A worker tip that merged the epic into its lane uses its later integration
merge's diff; only a merge on the target's first-parent chain is the receipt.

Typed merge requests refresh the published source before deciding it already
landed. If that bounded fetch fails, the typed request is still queued with a
short freshness-unavailable note. Its uncertainty survives daemon and inbox
revalidation, so cached source refs cannot suppress it as already landed.
Unsafe source refnames still refuse. Local-merge tasks and repositories without
origin retain their local source authority.

Renaming an epic's lane keeps the immutable epic ID in the factory branch name.
Task display, close, worktree merge, spawn and sync resolve inherited previous
names to that epic's current lane, without redirecting an unrelated explicit
repository or branch pin. First publication requires creation provenance for
that exact current name; close never recreates a lane deleted from origin.
To repair a proof-locked target, a live registered supervisor can use
`task action=update id=<task> target_branch=<current-epic-lane> proof_scope_fix=true reason="repair target after epic rename"`.
This audits and invalidates the old proof cycle and reopens the task; workers
cannot use this recovery, and ordinary close proofs still apply.

A live registered supervisor may repair an incorrect execution methodology with
`task action=update id=<task> proof_scope_fix=true execution_note="" reason="<why>"`
to clear it, or supply a valid replacement methodology. Switching to `no-code`
requires a portable `external_ref`, either stored or supplied in the same update.
This correction invalidates the old verification cycle and reopens the task with
its assignee, delivery anchors and immutable merge facts preserved. It supports
task-only investigation proofs as well as merged code deliveries. Ordinary close
proofs still apply: declaring `no-code` never hides delivered code. Correct only
one of methodology, work target, proof targets or risk in each update.

A registered supervisor's first `verification action=add` may omit
`dispatch_id` when exactly one unexpired active dispatch exists for the task.
Child verifiers retain their sealed handoff binding; explicit IDs remain
supported. Terminal or expired dispatches are never inferred or silently
replaced: supervisor recovery must name the exact expired dispatch. Verifier
spawn briefs carry the dispatch ID directly.

A merged parked delivery gets an exact target-bound verification dispatch before
its content check can refuse close. This lets the independent verifier record a
verdict while integration proof still blocks close; an approval never waives
missing delivery content. After a failed review, a supervisor can use
`task action=request_changes` from AwaitingMerge or Blocked with a recorded
parked delivery. It preserves the assignee and historical merge facts and
invalidates the declined proof before the next implementation cycle. The assignee
receives a waking notice with the decision and branch handling. The response
names the queued prompt, or explicitly reports why no worker was notified so
the supervisor can contact it. A worker start surfaces any unread correction
before acquiring the task; the worker reads it, then retries start.

When the correction requires importing an unlanded release or sibling batch,
the supervisor includes `merge_source=<full commit SHA>` in that same
`request_changes` reason (repeat the marker for multiple sources). Cassy records
the immutable source, supervisor identity and task target atomically with the
review decision. Its ancestors count as upstream imports during source and park
attribution; later descendants and unrelated work do not. Movable branch names
are refused. A worker copying the marker into notes grants no authority, and a
grant for a different task target does not apply.
Only the authenticated supervisor request handler parses these markers and
passes explicit SHA grants to storage. QA/verifier findings and queue-ejection
details remain review text: quoting a valid or malformed marker grants nothing
and cannot prevent the delivery from returning to its worker.

Re-parking a rejected QA delivery opens the next round automatically within the
configured round limit. Cassy reuses the prior reviewer only when its current
registration is live in this factory, independent of the implementer, and has
no assigned active work or lease. The reviewer gets a new QA task assignment
and still has to start/claim the round. Busy/unavailable reviewers leave the
normal supervisor spawn handoff in place; maximum-round escalation remains.

A passed or waived independent QA round remains bound to its reviewed tip after
a squash merge. Close proves that the integrated receipt carries the same trees
over the aggregate delivered paths, or the same stable aggregate patch ID.
Unrelated target files do not change coverage. An unresolved or changed delivery
still refuses, with review coverage and integration proof reported separately.

A live registered supervisor may use `supervisor_override=true` with a non-empty
reason to waive additive-only/value-only posture checks and the receipt epoch
check for a retroactive record task. Close records the decision. Repository
binding, ancestry, non-empty delivery, and target-content checks still apply.

A delivery's final file that is byte-identical on the authoritative target and
still differs from the task's delivery base is present, even when historical
intermediate builds disappeared during recovery. This exact-file proof applies
to any path; it grants no exemption for minified or generated files. Child close
and epic accounting use task-attributed history for merge deliveries rather
than crediting only the epic changes imported by a worker's sync merge. Epic
accounting compares the recorded child anchor, so a later task on the same lane
cannot supply its final snapshot. Exact-file recovery also requires a
task-attributed path effect to survive, including attributed side-parent work. Restoring any imported path baseline, even
one newer than the task's original base, rejects both explicit and unlabeled
inverse changes unless a supervisor records the audited supersession below.

For an older runtime that falsely reports dropped historical bundle lines,
first inspect the recorded anchor and authoritative target with `git ls-tree`
and `git diff <anchor>..<target> -- <paths>`. Preserve branches and evidence.
Upgrade to the runtime containing the exact-file proof, or use the authenticated
supervisor review below with the real integration/replacement commit and exact
blob comparison in the review. Do not manufacture a corrective commit or edit
task metadata directly. The cas-c2cb incident (cas-5f0b) used the tracked merge
64d1740bd7bb065049dea3b9c7eee2e712738e68 as its reviewed-drop receipt; anchor
6ff3c4d6e and target held app.js blob 5bde1a1d1bf2b077538c054ee758bc8ffcdd4006.

Epic status reports `anchor superseded by <sha> (<task>)` as information when
an ordinary later commit retires still-owned anchor lines and its subject names
another task, or it is that closed child's recorded delivery. Body references
and path touches alone grant no authority. A merge-resolution loss remains
`content dropped`; later task edits cannot revive ownership already lost there.
This epic accounting rule does not relax the individual task close proof.

A deliberately superseded delivery that still fails automatic content attribution
can close with `supervisor_override=true` and
`reason="reviewed-drop: <superseding SHA>[,<SHA>...] -- <why>"`. Each named commit
must differ from the delivery anchor, be reachable on the authoritative target,
and change a dropped path in its first-parent diff. Together they must cover
every dropped path's final target state: a path counts only when its state at
the named commit matches the target, including deletion and file mode. The
superseding commits may predate the anchor, as when migration renames or a
deliberate revert landed before the delivery was re-anchored (GH #1160).
Cassy records the full resolved commit IDs, anchor, measured target, paths and
review. A narrative without commit receipts cannot waive the content gate.

When the content proof cannot decide (`DELIVERY CONTENT UNVERIFIABLE`, for
example a fix committed inside a merge resolution), a live registered supervisor
who has inspected the delivery can close with `supervisor_override=true` and
`reason="reviewed-content: <delivery SHA>[,<SHA>...] -- <what was inspected>"`.
Each named commit must be the delivery anchor or a descendant of it, be
reachable on the target, and carry a non-empty first-parent diff. Cassy records
the resolved commits, anchor, target and review as a decision note.

The epic close gate bounds ref/cache metadata at 8 s, then uses an 8 s soft
proof budget. The first uncached child's delivery/history proof may finish
beyond that soft budget under a separate 20 s hard cap, guaranteeing progress
when a loaded host takes longer than a call's budget to prove one child.
Missing-anchor fetches and subsequent proofs retain the soft budget; summary
views and zero-budget cache-only calls never receive this allowance. Bounded
close/status collection takes at most 8 + max(soft budget, 20) s (28 s for
the production budgets), leaving room below the 55 s MCP deadline. A probe that
exhausts its hard deadline is terminated with its descendants, and its unfinished
child remains unchecked rather than becoming a measured verdict. When it stops
early (`EPIC CLOSE CHECK INCOMPLETE`), the verdicts it already proved for
closed children are saved under the repository's common Git directory
(`cas/epic-close-verdicts.json`), keyed by the exact refs and anchor each proof
read. Retrying the same close reuses them and continues from the first
unchecked child, so a large epic closes after a few retries. A moved target,
lane or anchor invalidates only the affected verdicts. A measured stranded
child still rejects immediately. When the check is incomplete, no measured child
blocks and the task store shows every child terminal, a live registered
supervisor may instead close with `supervisor_override=true` and a reason; the
waived output, including every unchecked child, is recorded as a decision note.

An empty `execution_note` update may clear a constraint after approval when its
exact repository proof is unchanged. Pending, skipped, unbound, and changed
proofs remain locked; changing other scope fields or replacing the constraint
still requires a fresh proof cycle.

### Scratch builds for exact-tip journeys

When a factory lane changes Commander product inputs, commit the source changes
and run `scripts/journey-eval.sh <task-artifact-dir>`. The runner builds the
production bundle into a unique `journey-builds/<source-sha>-*/dist` directory
under those artifacts. No generated `hub-web/dist` commit is needed or allowed
on a lane. Token generation still belongs with the source changes; if a build
changes tracked inputs, commit them and run again.

`journey-build.json` records the exact source SHA, scratch path and SHA-256 hash
of every output filename and byte. The server verifies this manifest before
listening; receipt writing verifies it again after the run. Bundles retain the
same source/hash pair. A changed source tip, dirty product/catalog/runner input,
or altered snapshot refuses proof. Prior attempts and their manifests stay
archived. An empty affected selection does not build or start browsers.

Standalone named journeys use the same scratch builder, defaulting to ignored
`target/journey-dist`. A producer may pass its existing manifest through
`JOURNEY_BUILD_MANIFEST`; an arbitrary dist-directory override is not accepted.
Background reuse keys include the source/build inputs and retain the original
build provenance rather than rebinding a snapshot to a different source tip.

The integration owner produces and commits canonical dist in `scripts/release.sh`
preparation, after merging sources and setting the release version. The queue's
existing Commander build requires committed-output equality for version changes,
dist changes, or unreadable base/version inputs. A proven unchanged-version,
unchanged-dist source-only lane builds and tests without requiring generated
commits. A version-only change selects the same Commander step even with no web
path diff, so omitting release bundle generation still rejects stale assets.
Journey manifests bind scratch proof to its source/hash; final release equality
binds that source output to the committed assets embedded by the prebuild and
adopted by commit/tree. This adds no CI stage or duplicate gate.

### Test shape and runner evidence

`python3 scripts/check-test-shape.py` checks tracked Rust tests and test-only
helpers for constant/literal equality and reads of Rust source as text.
Rust assertion diagnostic and formatting arguments are excluded from prose-pin
checks; literals in the assertion operands remain checked.
`--changed-since <ref>` checks the merge-base diff, including working changes;
files imported only by a released-trunk sync merge are excluded, while task
edits before and after that merge remain checked. As with task attribution,
every side parent must belong to `origin/<trunk>` (or local trunk when the
remote ref is absent); other merges stay in scope.
the release fast rows and Scoped Validation run this form. Intentional external
wire or structural contracts carry `// pin: <reason>` immediately above the
statement or test/helper declaration, or on the assertion line. A reason does
not convert a source-order assertion into behavior coverage.

Tests assert behavior or stable error/rule IDs, never human-message wording.
The source guard checks newly changed error assertions at park with
`--messages-only --changed-since <target>`; `pin:` comments do not waive it.
Rust assertion diagnostics and exact caller-supplied payload preservation remain
valid. Exact CLI flag values passed in literal `try_parse_from` / `parse_from`
argument arrays can be asserted on their immutable parsed fields, including
let-else command routing. Unrelated fields, generated values and substring or
inequality prose checks retain the ordinary rejection rules.

Workers run `npm test`, `npm run typecheck` in `hub-web`, and
`scripts/journey-eval.sh <task-artifact-dir>` for source-impact-selected journeys
at four workers. Independent QA consumes matching passing worker receipts
(base/head, selected IDs, native results and execution environment), then runs
independent adjacent/demo/visual cells; missing or incompatible evidence needs
its own affected checks. The wrapper resolves the task's declared target;
`--affected <base>` binds an explicit base. No browser runs for an empty impact
selection; it emits an explicit receipt. Caller spec/grep filters are refused.

Factory PreToolUse denies worker/reviewer `--full`, unfiltered Playwright and
unfiltered journey npm scripts. Named spec or canonical-ID runs remain allowed
for iteration; a failing spec may be rerun at one worker, retaining the original
failure. Do not rerun browsers for doc/ledger-only commits with unchanged
evaluated inputs.
Do not require a journey receipt at task close, independent QA, or epic
assembly. The merge queue runs Commander journeys only when `hub-web/` product
inputs change. Receipts fold all native parts by actual catalog ID and record
full base/head, pass/fail/skip counts, Playwright version and the native exit
code. Native runners refuse
successful zero-test summaries and export the passing count to `VERIFIED_TEST_COUNT_FILE`
when requested. Rust re-exec helpers require the exact child name, one selected
test, and one passing result; intentional signal/atexit children instead prove
entry into the test body before their early exit.

The full release gate's `hub-web-tests` row runs `npm ci`, `npm run typecheck`,
and `npm test` beside the build and Rust suite rows. A failure aborts the gate,
cancels the rows after it, and prevents pipeline admission. Its cached PASS depends on `hub-web`, `scripts`, and `.github`,
including the repo-level `scripts/visual-qa.mjs` that web tests import and read.
CI's `web-check-needed` (both scoped lanes and the merge-queue Commander step:
build, visual QA, dist drift and journeys) is narrower: `scripts/classify-web-inputs.py`
lists hub-web, the journey catalog, design tokens, `scripts/visual-qa.mjs`, the
worker-memory wrapper with its imports, and the classifier itself. Any other
`scripts/` or `.github/` change skips them, including a `.github/workflows/ci.yml`
edit, whose Commander step wiring `scripts/test-hub-web-gate.py` pins instead.
Release-version changes and unreadable versions also select the existing web
checks, because the bundle embeds that version; a classifier failure runs them.
`scripts/test-hub-web-gate.py` fails when hub-web reads a repo-level file that
the list omits. Markdown fixtures beneath hub-web still require web tests. The
scoped lane and merge queue select web checks from their changed inputs.

Closing a task that changes a committed `*.snap` or
`opencode_projection.snapshot.json` requires one task decision note per
changed snapshot file:
`snapshot-approved: <relative file> — file-sha256:<64 lowercase hex digits> — <why>`.
The digest covers the file's complete blob at the delivered tip, so one note
approves every changed line in it (a shared partial that alters 20 rendered
lines needs one note, not 20). Any later edit to the file changes the digest
and requires a new approval. The refusal prints the exact token for every
unapproved file. A snapshot deleted at the tip has no blob and uses the
line-level form, which also remains accepted for any file:
`snapshot-approved: <relative file> — <actual +added or -removed line> — <why>`.
The equivalent line-digest form is
`snapshot-approved: <relative file> — sha256:<64 lowercase hex digits> — <why>`.
SHA256 covers the complete UTF-8 diff line, including its leading `+`/`-` and
whitespace, excluding the newline. The refusal prints this bounded form when
the changed line exceeds 256 characters, so long prompts fit the default
1500-character note limit. Copy the suggested token and explain the reviewed
change; existing short-line literal approvals remain valid.
The close gate checks the task-attributed Git diff, even after merge, and names
the exact `task action=notes` command when approval is missing. Approval for a
different file or a line absent from that diff does not satisfy the gate.

## Operator write roots (GH #1169, GH #1182)

The factory workspace contract normally lets agents write only to their
worktree, `factory.artifacts_root/<project-key>/<task-id>/`, the configured
scratch root and the harness scratchpad. An operator can add directories
outside these:

- **Downloads, built in.** The user's Downloads folder is a root for every
  project's supervisor and workers with no setup: on Linux the
  `XDG_DOWNLOAD_DIR` from `$XDG_CONFIG_HOME/user-dirs.dirs` (default
  `~/.config/user-dirs.dirs`), else `~/Downloads`; on macOS `~/Downloads`. It
  passes the same safety checks as a configured root, and a missing folder
  admits nothing. On its own it does not turn on the in-place edit judging
  described below.
- **User-level write roots.** Running `cas config set factory.write_roots
  "<dir>,<dir>"` from `$HOME` writes `[factory] write_roots` to the user-level
  `~/.cas/config.toml` (that is the store `$HOME` resolves to). These roots
  apply to every project, merged with each project's own list. Folders
  inside `~/.cas`, such as `~/.cas/qa-rig`, may be roots here; `~/.cas`
  itself, its config files and `~/.cas/operator/` may not. Each root is
  checked against both the user store and the project.
- **Project write roots.** Run `cas config set factory.write_roots
  "/mnt/shockwave/ai/pooled_assets/llm"` in the project. This writes
  `[factory] write_roots = [...]` to the project's `.cas/config.toml`, so
  `cas config list` and `cas config get factory.write_roots` show it. Each
  root lets the project's supervisor and workers create, edit, rename and
  delete files beneath it. An empty value clears it. Project roots are per
  project: a worker reads them from its owning project's `.cas/config.toml`,
  also in an isolated worktree, and another project's agents never get them.
  `cas config get factory.write_roots` prints the project's list and notes
  the user-level and Downloads roots on stderr.
- **One-off task grants.** Run `cas config grant-write --task <id> --path <dir>
  --mode create+edit --reason "<why>"`. The grant applies to the worker
  holding the task and to the supervisor until the task closes. It is
  recorded as a DECISION note on the task. `cas config revoke-write --task
  <id> [--path <dir>]` removes it. Grants are stored in
  `.cas/operator/write-policy.toml`.

Root paths are resolved to canonical absolute directories when set: `~`
expands to `$HOME`, symlinks resolve to their target, and an entry with a `..`
component is refused. `/`, `$HOME` and its ancestors, the project root, and
anything that contains or lies inside the project's `.cas/` are refused, so
`.cas/config.toml` is never under a root. The hook re-checks each entry and
skips one that no longer resolves or is unsafe. Roots that 3.50.0 stored in
`.cas/operator/write-policy.toml` are still honoured, and the next `cas config
set factory.write_roots` moves them into `config.toml` (an empty value clears
them too).

For a support loop, the operator can include `~/.gabber-support` in the
owning project's `factory.write_roots`, preserving any existing entries when
setting the list. This admits `state.json` beneath that directory; a sibling
such as `~/.gabber-support-other` stays blocked. Setting it in the user-level
`~/.cas/config.toml` instead grants it to every project.

Each grant admits only its modes; a project root admits all three:

- **create**: a new file, an `apply_patch` add or move, or a `cp`, `tee` or
  `touch` destination;
- **edit**: an existing file, or, once roots or a policy file exist, `sed -i` /
  `perl -i` operands and `mv` sources (without them these keep their
  previous, unjudged behaviour);
- **delete**: `rm` or an `apply_patch` delete.

Every write admitted this way is logged as a `workspace_write_root_used`
event (tool, path, mode, root, task, agent) in the factory session log. A
refusal lists the roots in effect.
Shell writes are checked against their possible effective directories after
`cd`, including writes in literal nested shell payloads. Earlier simple
directory bindings are resolved in order; conditional bindings retain their
possible values, and subshell directory changes stay scoped. Literal shell
bodies may use status/PID values and read-only substitutions as data; their
recognized filesystem effects are inspected recursively. Unresolved shell
programs, executables, directory operands/searches, write targets and excessive
nesting fail closed. Use explicit directory operands and bounded literal
payloads so each write can be attributed to a root.

**Operator-only, guardrail-grade (not security-grade).** The operator and the
agents share a Unix user. The PreToolUse hook is the actual gate against
agents; the CLI checks are defence in depth. With no configured roots and no
policy file, the default contract is unchanged apart from the Downloads
root. The layers are:

- `cas config set factory.write_roots` refuses only inside a factory worker or
  server cgroup. The operator usually runs it as `! cas …` inside an agent
  pane, which carries agent environment and has no terminal, so it does not
  check either;
- the grant commands also refuse when they detect any agent environment
  identity variable (`CAS_AGENT_NAME`, `CAS_AGENT_ROLE`, `CAS_AGENT_ID`,
  `CAS_SESSION_ID`, `CAS_FACTORY_MODE`, `CAS_FACTORY_SESSION`,
  `CAS_FACTORY_WORKER_NAMES`, `CAS_CLONE_PATH`, `CAS_SUPERVISOR_NAME`,
  `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`, `CODEX_SANDBOX`,
  `CODEX_SANDBOX_NETWORK_DISABLED`, `CODEX_THREAD_ID`), an agent or Cassy
  server among the process's ancestors, or no interactive terminal, and need
  a typed confirmation. Markers are exact names, never prefixes: operator
  build knobs (`CAS_FACTORY_NICE_WORKER`, `CAS_FACTORY_CARGO_BUILD_JOBS`,
  `CAS_FACTORY_NICE_LEVEL`, the first two exported by `cas` itself), Claude
  Code preferences (`CLAUDE_CODE_NATIVE_CURSOR`, `CLAUDE_CODE_NO_FLICKER`)
  and settings such as `CAS_AGENT_MAX_TASKS` are not agent markers;
- `Config::set` refuses the key, so MCP, the config TUI and `cas config reset`
  cannot set roots, and `cas config import` keeps the current value;
- PreToolUse refuses any agent Bash call of these commands, even when wrapped
  by `timeout`, `env`, `nice`, `nohup`, `xargs`, `stdbuf`, `flock`, `sudo`,
  `setsid`, `$(which cas)`, `eval`, `sh -c` or `script -c`. It also refuses
  any command that names `factory.write_roots` with `set` (or `grant-write` /
  `revoke-write`) unless every statement only reads (`grep`, `cat`, `git
  log`, `cas config get`, …), which covers `python -c`, `perl -e` and
  heredocs. A command hidden in a script file the agent runs is out of
  reach of a tool-call hook. It also refuses any agent write,
  edit, delete or rename of the project's `.cas/config.toml`,
  `.cas/config.yaml` or anything under `.cas/operator/` (file tools,
  redirections, `tee`, `cp`, `mv`, `sed -i`, `perl -i`, `rm`). Agents change
  other keys with `cas config set`, which stays allowed. No root ever admits
  any project's `.cas/config.*` or `.cas/operator/`. This applies to workers
  and the supervisor alike.

A process that deliberately hides a same-user write from these checks is out
of reach. A hard boundary needs agents running as a separate Unix user.

## Default Codex worker account

Set `factory.codex_home` with `cas config set factory.codex_home "~/.codex-work"`
from the project on each machine that should use that account. This local
project setting accepts an existing account path or alias. `spawn_workers`
uses it for Codex slots whose `config_dir` was omitted, including lane requests,
before inherited accounts or quota recommendations. Explicit batch and per-worker
`config_dir` values take precedence. The spawn receipt names
`source=factory.codex_home`; invalid accounts are refused before queueing. Clear
it with `cas config set factory.codex_home ""` to restore account recommendations.

## Cloud reader slots

Supervisor registration derives a stable Cloud `factory_id` from the machine id,
canonical project id, supervisor role and the registered agent's factory session.
Concurrent factory sessions on one clone receive separate slots. A successor
agent after `/clear` or a restart of the same named factory session keeps its
slot. Non-factory supervisors retain the legacy machine/project/role slot.

Set `factory.slot_id` to retain a particular slot across different factory
sessions or machines. Sharing an explicit override intentionally supersedes the
earlier registration, so concurrent supervisors should use the default session
slots. Without enough identity to derive a slot, registration falls back to
`CAS_FACTORY_ID`, then the agent's factory session. Local
`metadata.factory_session` still names the actual launch for fleet routing.

## Bounded MCP task reads

`task show` (including `get`), `task notes` reads and `task start` default to
responses of at most 2,000 UTF-8 bytes. Show/start include the current scope;
large note histories keep the latest decision, proof and checkpoint with cursors.
A clipping notice marks shortened fields; literal ellipses in task text do not
trigger it. Read clipped instructions before acting.

For show/notes, `full=true` returns complete text. `offset=0 limit=1600`
starts a byte page of that same text; follow its `next=` cursor until `end`.
The page header is separate from the body, and concatenated bodies reproduce
the full response. Limits clamp to 4–1,600 body bytes; cursors must be UTF-8
boundaries. Note summary cursors address the full `task notes` response.
For start, `brief=false` (or `full=true`) explicitly includes sibling and epic
context. Start's default still performs the same claim and epic lifecycle work.
Complete start advisories, including worktree directory instructions, are saved
as a START CONTEXT task note before budgeting. An unchanged latest context reuses
its note and cursor; transient claim/lease text stays in the start response.
Warnings take precedence over scope, notes and recall; an oversized warning block
is replaced by an explicit instruction to read its saved note before acting.
The start response gives a `task notes offset=0` paging path (follow `next=` to
`end` and read the latest `START CONTEXT`) and a full-notes path. It never gives a tail cursor from the
start snapshot: Cloud reconciliation can reorder note blocks before the next
read, making that cursor invalid or hiding the saved instructions.
Default show retains Type and Demo for builtin gates, and names full-only sections
(including dependencies, design, deliverables and terminal outcomes) explicitly.

## Durable task artifacts

`factory.artifacts_root` is the shared parent (default `~/.cas/artifacts`).
New task evidence lives in `<base>/<project-key>/<task-id>/`; assignment briefs
print the exact path. The key combines the project folder label with a SHA256
of the canonical shared Cassy store path, so equal folder names and task IDs
in different stores stay separate, while symlink aliases and factory workers
using that shared store agree. Keep evidence paths in task notes or published
artifact records when a project moves.

Existing `<base>/<task-id>/` files remain readable and publishable. Completion
receipts and QA citations accept those historical paths. They are excluded
from automatic project cleanup: a flat directory can contain more than one
project's evidence. New writes, issue attachments, QA rounds, message spills,
search discovery and cleanup use the scoped namespace. No automatic file move
or ownership guess is made for legacy directories.


### Factory delivery scope and reference admission

The shared CI classifier resolves every factory push against the unique
maximal merge base among fetched `origin/epic/*` refs, preserving the task fork
when sibling merges advance the epic. It falls back to the configured default
remote branch when no epic parent is available. Local refs,
unknown bases and incomparable epic parents cannot authorize a Rust skip.
A code push followed by a docs-only push still validates the delivery code, even
when concurrency cancels the earlier run. Direct branch pushes, PRs and merge
groups retain their event deltas. All scoped consumers use the resolved base;
the merge queue still proves the complete integration delta.

The Violet/fixture repository reference scan runs in the existing `ci-diff`
admission job on factory and epic pushes and PRs, including direct release
edits. It was removed from the late `test-ci-tiers` queue recipe. Its independent
boundary self-tests remain there; merge-group admission does not repeat the
content scan.

## Policy hygiene and stakeholder evidence

Use `rule action=drift` for a read-only comparison of the rule source and the
configured generated rule directory (normally `.claude/rules/cas/`). Its
`structured_content.snapshot` is an export: copy that JSON object into the
`peer_snapshot` string of the other machine's `rule action=drift` request. Both
snapshots must name the same canonical project. The report names missing or
changed generated files, peer status/content/scope/provenance differences, and
missing or different `[cloud]` sync settings. Capture time is shown; an imported
snapshot does not prove the peer's current state. No rule is retired, synced or
otherwise changed by this check. Absence of a peer snapshot is reported explicitly.

When an instruction is attributed to a stakeholder, supply `stakeholder`,
`stakeholder_quote` and `stakeholder_scope` on `rule action=create` or `update`.
The quote is stored exactly as provided, separately from the interpreted rule
content. Attributed hard rules also require a future RFC3339 `review_after`.
`rule action=show` displays the evidence and deadline. Existing rules remain
unattributed until someone supplies evidence; migration never guesses or backfills
someone's words. Optional provenance travels with normal rule/cloud serialization.
An update that omits attribution preserves the stored evidence, including older
client writes. `rule action=update stakeholder=""` explicitly clears live
attribution and its `stakeholder:` tags; omit quote/scope in that request.
The clear and its resulting history snapshot commit together.
This does not change delivery or staging policy.

Tag a temporary stakeholder constraint memory `stakeholder:<name>,constraint`
(or `stakeholder-constraint:<name>`) and supply an explicit RFC3339 `valid_until`.
When a decision is replaced, update the old memory's tags to include
`superseded,superseded-by:<replacement-id>` while retaining its other tags.
Supersession ends its validity and removes it from session context, including
pinned memories; explicit retrieval by ID still preserves the historical words.
A leading word such as `SUPERSEDED` in prose never retires a memory or a
handoff. A deadline alone remains sufficient for ordinary time-limited memories.

## Release freeze

`factory action=train_freeze id=<train> reason="release-freeze: <reason>"`
needs a live registered supervisor and a nonempty reason after that explicit
confirmation prefix. Without it the call refuses before freezing. This is a
release freeze, not a temporary CI hold: only the operator can lift it in
Commander, or a published release covering its tip ends it.
Commander's `freeze_train` operation does the same for the operator. Both
`freeze_train` and `lift_train_freeze` require only a verified pairing's baseline
`machine:read` scope, without worker-management permissions. Preview pairings
cannot mutate a train. The freeze is a `release-train-frozen:<unix>:<tip>`
label on the train epic, which syncs to peer machines with the task row. One
gate in `workflow/release_freeze.rs` refuses `worktree_merge` into the frozen
lane (through `check_cut_merge`, before and under the target lock), CI retry
arming (`factory_ci_retry::arm`) and staffing (`ensure_staffing_allowed`,
covering spawn, assignment and start of tasks that are still open). A
`release-repair` task is admitted only while the train is red, and each
admission is noted on the train. Task create retargets new work aimed at a
frozen train to the next train. The freeze ends when a published release covers its tip (the
`Release` ledger event that `release-published-receipt.sh` records, whose version tag
contains the frozen tip), or through the operator-only Commander
`lift_train_freeze` operation. There is no supervisor lift.

### Temporary CI merge hold follow-up

A supervisor-reversible, peer-synced merge hold tied to a CI run is a separate
follow-up, not an existing `train_freeze` mode. That route should let the owner
clear the hold and end it when the named CI run completes, without freezing
staffing or CI retries. Until available, leave the train unfrozen, record the
CI wait on the train, and coordinate peer supervisors to defer `train_merge`
and `worktree_merge`; resume those calls after the run. This manual wait does
not enforce a shared hold across machines.

Auto-cut (`workflow/release_auto_cut.rs`) runs inside the daemon's cut-notice
poll (`train_guardrails::notify_train_cuts`). For an uncut train whose exact tip
has a green train-suite receipt, it acts when the tip holds `changelog.d/`
fragments, no merge-queue run is in flight, the operator has not paused it
(Commander's `set_auto_cut_paused` operation, a `release-auto-cut-paused`
label), the train's supervisor is registered on this machine and the cadence
allows it: fewer than `release.auto_cut_max_per_day` (default 4) other trains
frozen in 24 hours, the last at least `release.auto_cut_min_interval_hours`
(default 2) ago. It freezes the train, then waits for the supervisor's
`release-notes.md` in the train's artifact directory, waking the supervisor
once per tip with a deadline. With the draft present it runs the
repository's `scripts/release.sh --notes <draft>` once per tip in a dedicated
lane worktree and logs to `auto-cut.log` beside it; a nonzero exit wakes the
supervisor once. `release.sh` arms landing and publication reminders, and the
post-publish notice has the supervisor post the approved notes. Cassy never
writes or posts notes. Declined trains get the ordinary ready-to-cut notice,
which names the reason.

## Ordered train merge queue

A registered supervisor can submit `factory action=train_merge id=<epic>` to
merge that train's parked deliveries in updated-time order, or add
`task_ids=["cas-first","cas-second"]` for an explicit order. The CLI parity is
`cas factory train merge <epic> [task ...]`. Only exact parked anchors from the
same train and factory session are admitted. A known red train suite holds the
queue, including a repair tip still pending. A registered supervisor can accept
that observed run with `supervisor_override=true reason="<decision>"` (CLI:
`--supervisor-override --reason "<decision>"`); Cassy records the decision on the
train. The override never waives a later red or a delivery's own merge gates.

The action records the queue and returns immediately. The existing factory
daemon's CI scan resumes it through `worktree_merge`, reusing the current
candidate approval or requesting review when context changes. Pending reviews
and CI retain the current position. A content conflict reopens only its own task
with the conflicting files and keeps its assignee; later deliveries continue.
Single merges into the target are held throughout the queue, including between
verdicts. Other targets remain independent. Repeat the action without `task_ids`
to inspect an active queue; a successor may adopt a dead owner's queue. Queue
state and results live in `.cas/train-merges/`. No CI stage is added.

## Operator project holds

Use `cas factory hold set --reason "<reason>"` from your own interactive
terminal to stop project dispatch and merges. The state is local to the
project's shared `.cas/operator/project-hold.json`, protected by the existing
operator write policy. It records the operator, reason and time, and retains
both set and lift decisions. Agents cannot change it through MCP or the CLI.

`cas factory hold status` shows the current hold.
`cas --json factory hold status` returns `active` and the complete `history`.
Lift it with `cas factory hold lift --reason "<reason>"`.

While held, startup leads with the hold, ready and awaiting-merge lists hide
action invitations, merge requests and stalled-supervisor relays are withheld,
and spawning and merging refuse even with force or override flags. Unread
relay rows receive no surface receipt; lifting restores visibility and normal
admission. Ordinary mail remains readable. Unreadable hold state stops
dispatch and merges until the operator repairs it. A hold has no automatic
expiry, and an unheld project's staging delivery flow is unchanged.

The hold filters parked rows at MCP list and startup presentation boundaries;
internal task lists retain them for ownership fences, re-close exemptions and
recovery checks. READY TO MERGE generation stops before caching or queueing an
alert during the hold; lifting it alone generates no stale READY backlog.

### Measured ledger estimates

`cas ledger estimate pipeline <stage> --instance <sha>` and MCP `system action=estimate query=pipeline:<stage>:<sha>` record promises from successful measured runs; task/epic lookup uses closed project tasks. `--preview` is read-only. `cas ledger backfill --sync` derives historical facts and synchronizes the ledger without inventing promises. Estimate/outcome kinds use migration m297 and store-open reconciliation, preserving append-only sequence and history. See [ESTIMATES](../../docs/ESTIMATES.md) for endpoints, calibration, outcome semantics and supervisor-owned two-machine backfill proof.
