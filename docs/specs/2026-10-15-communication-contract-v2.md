# Factory communication contract v2

Task: cas-805e. Spec record: spec-6194. Epic: cas-a6dc (burn-down v41). Status: **proposed; awaiting
supervisor approval**. Base: `033143b50` (v3.55.0). Source review: cas-447d,
`docs/research/2026-10-14-agent-communication-review.md` at `5bb36bd0`, §5–§6.

This contract states three properties that every Cassy communication path must
hold. The twelve v41 communication tasks implement against it:

| Task | Short name | Clauses it owns |
|---|---|---|
| cas-d88c | Operator rows swept as unknown targets | P3.1, P3.3, P1.3 |
| cas-f7b0 | Honest receipts and terminal reasons | P1.2, P1.3, P2.5 |
| cas-1f0c | One body per reader on Claude Teams | P1.4 |
| cas-7156 | Nudge storms and false stalls | P2.2, P2.3, P2.4, P1.1 |
| cas-6853 | Broadcast backlog, supersession, self-echo | P1.5, P2.4, P2.6, P3.7 |
| cas-3a82 | `close_rejected` twin on park | P1.5 |
| cas-b06f | Shutdown relay storm | P2.6, P2.8 |
| cas-0028 | Episode-keyed idle/stalled relays | P2.1 |
| cas-5c2c | Supervisor handoff, owner liveness, conflicts | P3.2, P3.5, P3.6 |
| cas-8063 | Peer mail across a supervisor restart | P3.4 |
| cas-d5ab | Truthful send outcome; gate declines are activity | P1.6, P2.3 |
| cas-48e5 | One message per event in skills; typed status | P1.5, P3.8 |

A task is done when its own acceptance criteria pass **and** the contract tests
listed against it in §6 pass. If a task's acceptance criteria and this contract
disagree, the contract governs, and the task records the difference in a
decision note.

## 1. Relationship to spec-19c4

spec-19c4 (v1) measured transport: enqueue → route → transport → wake → model
turn → acknowledgement, across Claude, Codex and Grok. That matrix stays as
**layer 0**: v2 assumes transport works and constrains what the layers above
it say. v1 asked "did the bytes arrive?". v2 asks "did each reader learn one
true thing, once, from a live address?".

## 2. Vocabulary

- **Message**: one `prompt_queue` row, or one `supervisor_queue` notification
  plus the row that carries it. It has one id.
- **Party**: an addressable participant: worker, supervisor, peer worker, peer
  supervisor on the same machine, peer supervisor on another machine, operator
  or Commander, and the system producers (director, daemon, CI watch).
- **Lane**: the code path that owns delivery for one target kind, from enqueue
  to terminal stage. Every row belongs to exactly one lane.
- **Reader view**: anything that shows a message, or a fact about one, to an
  agent or a human. This covers the recipient's context, `message_status`,
  send results, director notices, banners, the TUI, dashboards, release SLO
  queries and abandonment notices.
- **Body**: the full text of a message in a reader's context. A **pointer** is
  a reference to a message id that does not contain its text.
- **Episode**: a span of real-world state with a start edge and a resolution
  edge, such as "worker W idle since T" or "lane L red at SHA S". It has a
  stable identity: `(kind, subject, episode_start)`.
- **Alarm**: a system-originated message that asks a reader to act, for
  example nudges, `worker_idle`, `worker_stalled`, `supervisor_stalled`,
  "delivery stalled", "unread on idle worker", `worker_died`, `ci-red-run`,
  MERGE REQUIRED and merge-queue ejection.
- **Receipt**: a single message that reports the outcome of an action the
  reader started itself, such as a shutdown it requested or a send it made.
- **Severity class**: `blocker`, `action` or `info` (§4, P2.7).

Normative words: MUST and MUST NOT are requirements. SHOULD is the default,
and a task may depart from it only with a decision note.

## 3. Property 1: one truth per message

A row's stage and receipts are the single record of what happened to it. Every
reader view is derived from that record. Each reader sees one body.

**P1.1 Single record.** Every reader view MUST be computed from the row's
`highest_stage`, `last_pending_reason` and surfacing receipts
(`DeliveryStage` at `crates/cas-store/src/prompt_queue_store.rs:845`,
`PendingReason` at `:946`, `SurfacingSource` at `:1167`). No view may keep a
private counter or state of its own. A system message that bypasses
`prompt_queue` MUST be recorded in an equivalent durable, queryable lane.
Today's violation: director nudges are direct prompts with
`durable_retry: false` (`cas-cli/src/ui/factory/director/prompts.rs:1810-1836`),
so queue metrics cannot see them. Owner: cas-7156.

**P1.2 Receipt ladder.** Surfacing evidence has a strict order. Every view
MUST respect it:

| Rung | Sources | Means | Counts as seen? |
|---|---|---|---|
| reserved | `transport_claimed` | daemon won the per-recipient claim | **No** |
| handed off | `transport_delivered` | transport accepted the body; harness may still drop it | No, provisional |
| surfaced | `hook_surfaced`, `inbox_poll`, `observed_wake` | body entered the recipient's turn | Yes |
| acknowledged | `acked_at` | recipient acknowledged | Yes |

A gated row deferred to the inbox MUST NOT carry a surfaced-rung receipt until
a hook surfaces it or `inbox_poll` reads it. `events.supervisor_injected` MUST
log a gated row as `deferred`, not `ok`. Owner: cas-f7b0.

**P1.3 Truthful terminal reasons.** A row that reached the surfaced rung MUST
NOT later become `abandoned`. A row whose recipient left the roster unread
MUST end with a distinct reason, `recipient_ended_unread`. `abandoned_unknown_target`
is reserved for a target that never resolved, or stopped resolving, in the
row's own lane (P3.1). An abandonment notice to a sender MUST be sent only for
a row that is truly undelivered. Owners: cas-f7b0 (worker rows), cas-d88c
(operator rows).

**P1.4 One body per reader.** For each message id and each recipient, the body
MUST enter that recipient's context exactly once, whatever the harness. Every
other path carries at most a pointer. Wake text MUST describe what actually
happens: no "(body not repeated here)" when the body is then repeated. Today
the turn-start hook expands the wake pointer and the Teams inbox then delivers
the same body again (`cas-cli/src/hooks/handlers/handlers_middle/factory_inbox.rs`).
Owner: cas-1f0c.

**P1.5 One message per event.** One state transition produces one message per
interested reader:

- A park in `AwaitingMerge` sends `task_awaiting_merge` only.
  `task_close_rejected` is for real refusals. Today both are pushed with the
  same occurrence (`cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs:5951-5985`).
  Owner: cas-3a82.
- A sender never receives its own broadcast. Owner: cas-6853.
- A worker never echoes a lifecycle relay. The relay is the close report, and
  one `merge_request` replaces "prose + request". Owner: cas-48e5.

**P1.6 Truthful send outcome.** A send MUST return the row's current named
outcome, and SHOULD do so in one line:

- `delivered`
- `deferred: <reason>`, e.g. "recipient mid tool call; surfaces at its next
  tool boundary"
- `queued: <reason>`
- `failed: <named failure>`

The ~540-char generic "not yet confirmed delivered" text is retired. Owner:
cas-d5ab.

**P1.7 Notices agree with the agent's actions.** A notice that reports on a
message ("unread on idle worker", "delivery stalled") MUST be revalidated
against current state when it is surfaced. If the recipient has already done
what the message asked (for example the named task started, closed or parked
after the message was enqueued), or a newer message supersedes it, the notice
is withdrawn with `superseded_stale` and no notice is sent. Owner: cas-f7b0
(see its 2026-10-14 21:56 and 22:20 notes).

## 4. Property 2: every alarm is an episode

Each alarm is keyed to a real episode. It is debounced, aware of activity,
coalesced when the reader is busy, and classed by severity. An action the
reader started produces one receipt, not alarms.

**P2.1 Episode identity.** Every alarm key MUST include an episode identity
`(kind, subject, episode_start)`. Within one episode, duplicates are deduped.
A new episode for the same subject produces a new alarm. Today `worker_idle`
uses `"{kind}:{worker}:{task}"` with an empty task
(`cas-cli/src/ui/factory/daemon/runtime/lifecycle.rs:125`), so a worker's
second idle episode in a session is silently deduped. Attention keys MUST age
out with their session. Owner: cas-0028.

**P2.2 Debounce.** At most one alarm per `(kind, subject)` per threshold
window, however often the detector's internal streak resets. Today a fresh
transcript clears `stall_nudged`
(`cas-cli/src/ui/factory/director/events.rs:1932-1965`), so a sleep-poll loop
restarts the streak and re-nudges. Owner: cas-7156.

**P2.3 Activity-aware.** These count as activity, and an inactivity alarm
(nudge, `worker_stalled`, "gone quiet") MUST NOT fire while any holds:

- an open tool call;
- a wake-gate decline whose reason is "tool call in flight" or "pane not
  silent";
- transcript activity within the threshold;
- a task transition by the subject after the alarm's premise.

Activity MUST be read per worker, not from a global window of the last 50
project events. Owners: cas-7156 (detector), cas-d5ab (gate evidence).

**P2.4 Coalesce while busy.** If a reader is busy, undelivered alarms with the
same `(kind, subject)`, and broadcasts with the same supersede key, collapse
to the newest one. Superseded rows end `superseded_stale`. Owners: cas-7156
(nudges), cas-6853 (broadcast `supersede_key`, e.g. `epic-tip`).

**P2.5 Resolved premise means no alarm.** An alarm whose premise has resolved
before it is surfaced MUST be withdrawn at the source (P1.7). Examples: the
task is terminal, the message was acked, the lane went green, or the worker
resumed. A relay MUST NOT re-fire after `message_ack` of the same episode.
Owner: cas-f7b0, with every alarm producer.

**P2.6 Self-initiated actions yield one receipt.** When an event matches an
action the reader requested, the reader gets one receipt that lists the
per-subject outcomes, instead of one alarm per subject:

- A requested shutdown of N workers yields one receipt (stopped, failed, tasks
  released). It keeps the GH #197 task-release semantics. Owner: cas-b06f.
- `workers_died_together` folds the per-worker `worker_died` relays into
  itself, and does not add to them. Owner: cas-b06f.
- A supervisor's own broadcasts are never echoed back (P1.5). Its own
  lifecycle transitions are already not echoed.

Deaths that do not match a request still alarm individually as `blocker`.

**P2.7 Severity classes.** Every alarm carries one class. The class decides
whether it may wake, whether it may be coalesced, and where it is rendered.

| Class | Examples | Wakes idle reader | Coalesced with others | Rendering |
|---|---|---|---|---|
| `blocker` | red CI on a merged or integration lane (`ci-red-run` with `merged_into`), merge-queue ejection, unexpected worker death, verification rejection, a Commander ask nearing its deadline | yes | never across subjects; never rate-limited away | separate from routine traffic, first in any digest |
| `action` | `task_awaiting_merge`, `task_blocked`, a ready merge whose lane CI is green, `worker_stalled` (episode-true) | yes | per P2.4 | normal |
| `info` | `task_started`, `task_closed`, `worker_idle`, reminders without a wake stamp | no | yes | digest |

Two further rules:

- An alarm MUST NOT instruct an action that standing policy forbids. Example:
  "merge now" while lane CI is pending contradicts cas-6c13. This was fixed
  for `supervisor_stalled` by cas-207e (`609b16782`, v3.55.0): pending-CI
  deliveries are `DeliveryHold::LaneCiPending`, listed as "waiting on CI" and
  not nudged. It is the reference implementation for this rule.
- No new wake class without a severity class. `wakes_idle_supervisor`
  (`cas-cli/src/mcp/tools/core/task/lifecycle/supervisor_push.rs:75-82`) and
  `is_supervisor_wake_envelope` (`cas-cli/src/prompt_revalidation.rs`) are the
  current wake gates. A task that touches them states the class it assigns.

**P2.8 Bounded, clean bodies.** Alarm bodies MUST be capped. Pane tails MUST
be stripped of TUI spinner and box glyphs before they are embedded. Owner:
cas-b06f.

## 5. Property 3: every party has a live address

Every party has one delivery lane, a liveness check and a named failure.
Identity follows the name across restarts.

| Party | Lane (owner) | Liveness check | Named failure | Identity rule |
|---|---|---|---|---|
| worker | prompt_queue direct row | roster + heartbeat | `recipient_ended_unread`, `shutdown_cancelled` | agent name in session |
| supervisor | prompt_queue + supervisor_queue | `resolve_owning_supervisor` | `undelivered_lifecycle_relay` | successor by registration recency (`supervisor_push.rs:500-545`, cas-7787) |
| peer worker | prompt_queue direct + supervisor CC | same session roster | `abandoned_unknown_target` (never resolved) | name in session |
| operator / `commander:<label>` | operator delivery outbox (`crates/cas-store/src/prompt_queue_store/operator_delivery.rs`) | outbox enrollment/relay receipt | operator-lane failure only, never a roster sweep | label |
| peer supervisor, same machine | local agent store | agent status Active/Idle | named error at send | name@machine → live id |
| peer supervisor, cross-machine | `peer_message_outbox` → cloud mailbox | cloud claim by a live supervisor | outbox abandon notice | name@machine, successor claims (P3.4) |
| ended owner (epic or integration) | handoff to live peer, else operator | owner session alive | orphan notice | — |

**P3.1 One lane per target kind.** Every target kind maps to exactly one owning
lane. One lane's sweeps and retries MUST NOT touch another lane's rows. Today
`prompt_poison_sweep_targets`
(`cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs:271-286`) omits
`operator` and `commander:*`, so `abandon_ineligible_session_targets`
(`prompt_queue_store.rs:6568`) abandons every operator row older than
`PROMPT_RETRY_MAX_AGE_SECS` (900 s, `:43`) and sends false abandonment notices
(`notify_bulk_abandoned_senders_in_tx`). Operator rows are owned by the
operator outbox and are excluded from the roster sweep. Owner: cas-d88c.

**P3.2 Liveness before routing.** A notice addressed to an owner, such as a
merge-queue ejection, a delivery-merged notice, a worker-attention relay or an
integration conflict, MUST resolve to a live recipient before it is enqueued.
If the owner is not live, it reroutes in this order: live successor of the same
name, then a live peer supervisor on the project, then the operator. A notice
MUST NOT be enqueued into a dead session, where it would wait 900 s and then be
abandoned. Owner: cas-5c2c. Note: the `rolling_integration.rs` `notify_owner`
cited in cas-5c2c was deleted with the post-merge sweeps (cas-fd0a,
`9fa37c766`). The clause applies to the surviving owner-addressed producers.

**P3.3 Named failure, once, only when true.** Every undeliverable route
produces one named terminal reason, visible to the sender through
`message_status` and the send result (P1.6). It produces at most one sender
notice. No notice is sent for rows still owned by a live lane. Owners:
cas-d88c, cas-f7b0.

**P3.4 Identity follows the name.** Addressing resolves
`(project, name@machine)` to the current live agent id at delivery time, not
at send time. Mail held for a superseded id of the same name and project is
claimed by the successor exactly once. An ambiguity error MUST offer
distinguishable choices. Owner: cas-8063. Local precedent: `best_supervisor`
successor ordering (`supervisor_push.rs:515-545`).

**P3.5 Ownership survives session end.** When a session that owns epics ends,
SessionEnd MUST either transfer them to a live peer supervisor or mark them
orphaned and notify peers and the operator. It never ends silently with live
ownership. Owner: cas-5c2c.

**P3.6 Readable at the destination.** A body MUST be readable on the receiving
machine. Over-cap peer bodies are inlined, split or uploaded. A local artifact
path is never sent cross-machine. Owner: cas-5c2c.

**P3.7 A broadcast's address is the current membership.** `all_workers`
resolves to the workers registered when the row was enqueued. A worker that
registers later never receives it, unless it is marked `sticky` standing
policy; the spawn brief SHOULD then carry a digest. A pre-registration
broadcast never gates task start (`TASK START DEFERRED`,
`cas-cli/src/mcp/tools/core/task/lifecycle.rs:1886`). Owner: cas-6853.

**P3.8 Every lane is documented and typed.** The worker↔worker lane and its
supervisor CC are documented in the worker skill. A worker can send a typed
`status` or `ready` message that may wake an idle supervisor, so it no longer
escalates plain status into a merge request to get a wake. Owner: cas-48e5
(design or filed follow-up). The new class is `action` under P2.7.

## 6. Acceptance tests

Test names are the contract. A task may place them in the module that owns
the code, but MUST keep the `ct2_` prefix, so that
`cargo nextest run -E 'test(/ct2_/)'` selects the whole contract. Levels:

- **store**: a cas-store unit test on a temp DB.
- **daemon**: a cas-cli daemon or director unit test with a fake clock.
- **hook**: a hook handler test with a recorded transcript.
- **int**: an integration test across modules, two DBs or a fake cloud.

### Property 1

| Test | Clause | Level | Assertion | Owner |
|---|---|---|---|---|
| `ct2_p1_gated_row_has_no_seen_receipt` | P1.2 | store | A wake-gated row deferred to the inbox has only `transport_claimed`; seen-count views report 0 until `hook_surfaced`/`inbox_poll`. | cas-f7b0 |
| `ct2_p1_supervisor_injected_logs_deferred` | P1.2 | daemon | A gated delivery writes `supervisor_injected` with outcome `deferred`. | cas-f7b0 |
| `ct2_p1_surfaced_row_never_abandoned` | P1.3 | store | A surfaced row whose recipient leaves the roster keeps `delivered` after the sweep. | cas-f7b0 |
| `ct2_p1_unread_row_recipient_ended` | P1.3 | store | An unsurfaced row whose recipient leaves ends `recipient_ended_unread`, not `abandoned_unknown_target`, and no sender notice is sent unless it is truly undelivered. | cas-f7b0 |
| `ct2_p1_one_body_per_message_teams` | P1.4 | hook/int | One pointer-woken message produces exactly one body occurrence in the Claude Teams recipient context (wake line + hook + teammate block replayed). | cas-1f0c |
| `ct2_p1_wake_text_truthful` | P1.4 | hook | Wake wording matches the chosen path: no "(body not repeated)" when the body is then delivered. | cas-1f0c |
| `ct2_p1_park_sends_one_relay` | P1.5 | int | An AwaitingMerge park writes exactly one `supervisor_queue` row (`task_awaiting_merge`); a real refusal writes `task_close_rejected`. | cas-3a82 |
| `ct2_p1_sender_never_gets_own_broadcast` | P1.5 | store | The sender and the `supervisor` alias have no receipt or drain result for their own `all_workers` row. | cas-6853 |
| `ct2_p1_skill_one_message_per_event` | P1.5 | unit (skill text) | The cas-worker skill forbids post-close summaries and prose + merge-request pairs; contract-phrase test. | cas-48e5 |
| `ct2_p1_send_returns_named_outcome` | P1.6 | int | A send to a busy recipient returns `deferred: <reason>` in one line; to an idle one, `delivered` or `queued`. | cas-d5ab |
| `ct2_p1_notice_suppressed_after_action` | P1.7 | daemon | "Unread on idle worker"/"delivery stalled" for a message is withdrawn (`superseded_stale`, no notice) once the named task has started, closed or parked after enqueue. | cas-f7b0 |
| `ct2_p1_director_nudge_is_measurable` | P1.1 | daemon | Every director nudge leaves a durable, queryable row (prompt_queue or equivalent) with stage and episode key. | cas-7156 |

### Property 2

| Test | Clause | Level | Assertion | Owner |
|---|---|---|---|---|
| `ct2_p2_second_idle_episode_relays` | P2.1 | daemon | Idle → active → idle for one worker yields two `worker_idle` relays; a repeat within one episode yields one. | cas-0028 |
| `ct2_p2_attention_keys_age_out` | P2.1 | store | `worker-attention:` keys from an ended session are removed by retention. | cas-0028 |
| `ct2_p2_sleep_poll_one_nudge_per_window` | P2.2 | daemon | Alternating open and closed tool calls for 3 threshold windows yields ≤ 3 nudges, not one per streak reset. | cas-7156 |
| `ct2_p2_no_stall_while_tool_open` | P2.3 | daemon | No `worker_stalled` or nudge fires while a tool call is open. | cas-7156 |
| `ct2_p2_gate_decline_counts_as_activity` | P2.3 | daemon | A worker with a "tool call in flight" gate decline inside the window is not reported stalled. | cas-d5ab |
| `ct2_p2_activity_read_per_worker` | P2.3 | daemon | A worker's activity is found even when 50+ newer events from other agents exist. | cas-7156 |
| `ct2_p2_queued_nudges_coalesce` | P2.4 | daemon/store | Three undelivered nudges for one task while busy surface as one (the newest); the others end `superseded_stale`. | cas-7156 |
| `ct2_p2_superseded_broadcast_not_surfaced` | P2.4 | store | Two broadcasts with one `supersede_key`: a recipient that has not read the first sees only the second. | cas-6853 |
| `ct2_p2_no_refire_after_ack` | P2.5 | daemon | After `message_ack` of a relay, the same episode does not relay again. | cas-f7b0 |
| `ct2_p2_requested_shutdown_one_receipt` | P2.6 | int | Shutdown of N workers requested by the supervisor yields one receipt listing per-worker outcomes, and zero `worker_died` relays; GH #197 task release still happens. | cas-b06f |
| `ct2_p2_unexpected_death_still_alarms` | P2.6 | int | A worker death with no pending shutdown request still relays as `blocker`. | cas-b06f |
| `ct2_p2_died_together_folds` | P2.6 | daemon | When `workers_died_together` fires, the per-worker relays are folded into it. | cas-b06f |
| `ct2_p2_pane_tail_clean_and_capped` | P2.8 | unit | Embedded pane tails contain no spinner or box-drawing glyphs and are within the cap. | cas-b06f |
| `ct2_p2_no_merge_nudge_on_pending_ci` | P2.7 | daemon | Already exists in substance in cas-207e; keep it under this name or alias it. | cas-207e (done) |
| `ct2_p2_blocker_class_not_coalesced` | P2.7 | daemon | A `blocker` alarm queued behind routine `info` traffic for a busy reader is neither coalesced nor rate-limited, and sorts first. | first task that touches severity classing; default cas-7156 |

### Property 3

| Test | Clause | Level | Assertion | Owner |
|---|---|---|---|---|
| `ct2_p3_operator_row_survives_sweep` | P3.1 | store | An operator row and a `commander:<label>` row, each 901 s old, survive `abandon_ineligible_session_targets` and produce no sender notice. | cas-d88c |
| `ct2_p3_dead_owner_notice_rerouted` | P3.2 | int | A notice for an owner whose session ended goes to the live successor, then a live peer, then the operator; nothing is enqueued to the dead id. | cas-5c2c |
| `ct2_p3_integration_conflict_notifies_both` | P3.2 | int | Overlapping integration lanes notify both owning supervisors. | cas-5c2c |
| `ct2_p3_session_end_hands_off_epics` | P3.5 | int | Ending a session that owns epics yields a transfer to a live peer or an orphan notice. | cas-5c2c |
| `ct2_p3_overcap_peer_body_readable` | P3.6 | int | An over-cap cross-machine peer body arrives readable; no local path in the delivered text. | cas-5c2c |
| `ct2_p3_peer_mail_survives_restart` | P3.4 | int (two DBs + fake cloud) | Mail sent while the peer supervisor is down is delivered exactly once to its relaunched successor. | cas-8063 |
| `ct2_p3_late_worker_skips_old_broadcasts` | P3.7 | store | A worker registered after a non-sticky broadcast never drains it, and task start is not deferred by it; a `sticky` broadcast is still delivered. | cas-6853 |
| `ct2_p3_worker_lane_documented` | P3.8 | unit (skill text) | The cas-worker skill documents the peer-worker lane and supervisor CC, and `peer-supervisors.md` no longer says peer workers are never addressable. | cas-48e5 |

### Release SLOs

These are measured with the cas-447d §2 queries
(`5bb36bd0:docs/research/2026-10-14-agent-communication-review.md`) over one
full factory session after the v41 epic lands:

| SLO | Target | Clause |
|---|---|---|
| false-abandon rate: abandoned rows with a surfaced-rung receipt | 0 | P1.3 |
| operator rows ending `abandoned_unknown_target` | 0 | P3.1 |
| duplicate body reads per message id (transcript classifier) | 0 | P1.4 |
| director nudges per worker-hour | ≤ 1 | P2.2 |
| `task_close_rejected` rows sharing an occurrence with `task_awaiting_merge` | 0 | P1.5 |
| stale broadcasts drained by late-registered workers | 0 | P3.7 |

## 7. Convergence rules for the implementing tasks

1. **Shared primitives land first.** P1.2/P1.3 (cas-f7b0) and P3.1 (cas-d88c)
   both edit `abandon_ineligible_session_targets` and the receipt sources in
   `prompt_queue_store.rs`. Land cas-d88c, then cas-f7b0 rebased on it, before
   cas-6853, which edits the broadcast drain in the same file.
2. **One episode-key helper.** cas-0028 introduces the episode identity
   `(kind, subject, episode_start)` in `lifecycle.rs`. cas-7156, cas-d5ab and
   cas-b06f reuse it rather than inventing new key formats.
3. **One activity source.** cas-7156 owns the per-worker activity read in the
   director. cas-d5ab feeds gate declines into that same source, and does not
   build a parallel suppression.
4. **Severity class stays one enum.** The first task to need P2.7 classes adds
   one `AlarmClass { Blocker, Action, Info }` beside `NotificationPriority`
   (`crates/cas-store/src/supervisor_queue_store.rs:19`). Later tasks use it.
   Do not overload `Critical`, which a requested shutdown misuses today.
5. **Terminal reasons go in the existing enum.** `recipient_ended_unread` goes
   into `PendingReason` with its `implied_stage`, `as_str` and `parse` arms,
   keeping `can_transition_to` legal. No ad-hoc strings.
6. **Hotspots.** `prompt_queue_store.rs` (cas-d88c, cas-f7b0, cas-6853);
   `director/events.rs` (cas-7156, cas-d5ab); `daemon/runtime/lifecycle.rs`
   (cas-0028, cas-b06f). Workers on the same hotspot rebase on each other's
   merged tips rather than in parallel.

## 8. Out of scope

- Transport mechanics already covered by spec-19c4.
- The Commander cloud relay binding itself (cas-5c89). This contract only
  requires that operator rows are owned by that lane (P3.1).
- Reminder wake stamps (cas-835e), SendMessage bypass (cas-dd09), threading
  (cas-009c), schema text (cas-3349), dead-letter cleanup (cas-5871) and Violet
  watches (cas-639c). These SHOULD conform when they land, but are not v41
  communication tasks.
- UI redesign of the TUI or dashboards beyond reading from the single record.

## 9. Already conforming

These are reference implementations:

- **P2.7 policy-consistent nudges:** cas-207e (`609b16782`). Pending-CI
  deliveries are held and not nudged.
- **P2.7 blocker class:** cas-6c13 (`e2574ee70`). A red run on a merged lane
  carries `merged_into` and is a blocker
  (`cas-cli/src/ui/factory/daemon/runtime/ci_watch.rs:59-62`).
- **P3.4 locally:** `best_supervisor` prefers the successor identity after a
  restart (cas-7787).
- **P1.5 partially:** a supervisor's own lifecycle transitions are not echoed
  back to it.
- **Typed wake envelopes** come from server-stamped senders only
  (`prompt_revalidation.rs`, `is_supervisor_wake_envelope`).
