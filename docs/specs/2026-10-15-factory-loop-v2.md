# Factory loop v2: typing never waits

Task: cas-5c24. Epic: cas-759c (GH #1211; also closes #1210, informs #1213).
Status: **draft for supervisor review**. Base: `7b518440c` (v42 epic tip).
Related contract: spec-6194 (communication contract v2) for the queue and
relay semantics this spec moves off the loop.

The operator's verdict (GH #1211): "we need to rethink how we are doing this
cause it sucks". Typing into a factory pane pauses because one daemon loop
relays input, reads PTY output, runs the prompt queue, refreshes the director
(task store, reminders, git, `/proc`) and renders, in sequence. Every point
fix so far (#1165, #1208, #1210, cas-7156) removed one slow step; the next one
appeared because the structure still lets any step stall key echo.

This spec replaces the structure. It has six parts:

1. A map of today's loop, step by step, with measured cost (§2).
2. An input/echo path isolated from the loop (§3).
3. An off-loop I/O model: the loop never blocks on store, git, network,
   `/proc` or transcripts (§4).
4. A capped render scheduler (§5).
5. A step-naming watchdog (§6).
6. Named latency tests with budgets, each owned by an implementation child
   (§7), and the migration order (§8).

## 1. Evidence

All numbers are from soundwave (32 CPUs, 64 GB), cas 3.55.0, 2026-10-14/15,
unless marked otherwise.

### 1.1 Loop-level (`loop.json`)

`.cas/factory-daemon/<session>.loop.json` is written by the daemon every pass
window. Live snapshot of `cassy-zen-newt-73` (this factory, 16+ agents during
the day):

| Field | Value |
|---|---|
| passes | 868,526 |
| window_passes | 346–367 |
| p99_pass_ms | 104 → 182 (two snapshots 3 min apart) |
| max_pass_ms | 223–246 |
| passes_over_100ms | 5–9 per window |
| worst phase: prompt queue | max 208–245 ms |
| worst phase: refresh: director data | max 212–218 ms |
| render | max 3 ms |
| client input, pty output, cloud relay, spawn queue poll | max 0 ms |

`cassy-swift-gazelle-77` (2026-10-14, 18 workers): p99 206 ms, max 243 ms,
14 of 198 passes over 100 ms; prompt queue max 241 ms.

The loop's own input and PTY steps cost nothing. **Stalls come from the steps
that do I/O.** The `loop.json` window is short (one pass window), so it shows
the current regime but not the multi-second spikes; those are in §1.2.

### 1.2 Director refresh steps (`cas::refresh_profile`)

`director_refresh.rs:134` logs every refresh step slower than 50 ms as
`slow director refresh step on the daemon loop step=<name> elapsed_ms=<n>`.
Aggregated from `.cas/logs/cas-2026-10-14.log` and `-15.log` (all factories in
this repository; only steps ≥ 50 ms are logged, so p50 is the median **slow**
step):

| Step | Day | Slow steps | p50 ms | p99 ms | max ms | Blocked total |
|---|---|---|---|---|---|---|
| reminders | 10-14 | 1,408 | 2,657 | 3,402 | 8,369 | 3,814 s |
| apply director read | 10-14 | 12,034 | 93 | 785 | 2,555 | 1,516 s |
| change detection | 10-14 | 9,168 | 107 | 794 | 2,261 | 1,198 s |
| supervisor stall | 10-14 | 2,514 | 72 | 268 | 1,053 | 196 s |
| apply director read | 10-15 | 5,564 | 303 | 1,913 | 3,449 | 2,169 s |
| reminders | 10-15 | 725 | 2,563 | 3,077 | 3,442 | 1,839 s |
| change detection | 10-15 | 3,858 | 272 | 836 | 1,154 | 1,169 s |
| supervisor stall | 10-15 | 4,060 | 128 | 1,820 | 3,264 | 946 s |
| auth-failure / permission / usage-limit relays | 10-15 | 646 | 50–86 | 50–203 | 282 | 50 s |

`reminders` is GH #1208 (synchronous `git fetch` per external reminder); the
fix cas-a222 is merged into v41 but the measured daemons still ran 3.55.0. The
other three rows are not fixed by anything in v41: **apply director read,
change detection and supervisor stall together blocked the loop for about
4,300 s on 2026-10-15**, before the reminders.

### 1.3 Lock contention on the loop's stores

`.cas/logs/cas-2026-10-15.log`, WARN counts:

| Message | Count |
|---|---|
| `SQLite busy; deferring stale reminder expiry to the next daemon tick budget_ms=100` | 6,207 |
| `supervisor Commander mirror queue write failed error=database error: database is locked` | 2,264 |
| `... shared SQLite connection ...` (same writer) | 74 |

GH #1165 measured the extreme: the main thread in `locks_lock_inode_wait` on
`task-sync-intents.lock` for 141–249 s, held by worker `cas serve`
processes.

### 1.4 Findings from today's tasks

| Task | Finding | Status |
|---|---|---|
| cas-a222 (GH #1208) | `process_reminders → external_wake_condition_observation` ran one `git fetch` per pending external reminder on the loop every 60 s: 2.5–9 s stalls | Fixed in v41: probe off the loop, one fetch per ref, backoff, TTLs |
| cas-7156 | `transcript_confirms_stall` ran on every working pass per worker: agent-store open, 2 `list()`s, a full `/proc` cmdline/environ scan (`wedged.rs:1441`), a child walk and a transcript-tail read, all inside the director refresh at `director_refresh.rs:240` | Gated behind threshold and backoff in v42; still synchronous on the loop |
| cas-0028 | Worker-attention relays: the daemon snapshots episode baselines on the loop, then hands the store write to the store worker | Precedent for "snapshot on loop, write off loop" |
| cas-874e | Merge-request revalidation resolves live branch tips (`prompt_revalidation.rs:962`) and now does a bounded source fetch on the MCP side | Daemon-side revalidation must stay ref-local or move off-loop (§4.3) |
| GH #1210 | `dirty = had_output || input_activity || …` (`lifecycle.rs` ~2680): every pass with any worker PTY output redraws; codex workers write 38–236 times/s; ~30 frames/s × ~16 ms in Konsole, GUI thread ~60% busy | Not fixed |
| GH #1213 | Worker builds share the TUI's cgroup; at 13:50:45 a daemon pass took 1,618 ms and keys sat unread for 2.0 s; the refresh thread was in D-state on page I/O | Out of scope here (cgroup); §3 makes echo independent of it |

## 2. Today's loop, step by step

> TODO(map): filled from the code map below.

## 3. Target architecture: three lanes

v2 splits today's single loop into three lanes with one rule each.

| Lane | Thread | Owns | Rule | Budget |
|---|---|---|---|---|
| **io** | dedicated OS thread | client socket, every PTY master (read and write), per-pane terminal state, render scheduler, client frames | never calls a store, git, gh, network, `/proc` or a transcript | one iteration < 5 ms; key → echo p99 < 50 ms |
| **control** | today's loop thread | prompt-queue decisions, director state machine, spawn/shutdown orchestration, relays | only in-memory work and channel traffic; never blocks on I/O | one pass p99 < 100 ms with 16 agents |
| **jobs** | the store worker plus a bounded blocking pool | every call that can block: SQLite, flocks, git, gh, HTTP, `/proc`, transcript reads | single-flight per key, results posted back, budgets and backoff per kind | per kind, §4.4 |

### 3.1 Input/echo isolation (io lane)

Today a key travels: client socket → the loop's `client input` step → PTY
write → agent echoes → the loop's `pty output` step on a later pass → `render`
→ client socket. Each hop waits for the loop to come round, so one slow step
anywhere (§1.2) delays echo in every pane.

In v2 the io lane does the whole round trip on its own thread:

1. It reads the client socket and writes keystrokes straight to the focused
   pane's PTY master. No other lane is involved.
2. Per-pane PTY reader threads (or one `poll` set) feed bytes into that pane's
   terminal emulator, which the io lane owns.
3. Any output from the pane that received input in the last 250 ms is an
   **echo frame**: it is rendered and flushed immediately, damage-only for
   that pane, ahead of any capped output frame (§5).

The control lane talks to the io lane only through two channels:

- **commands** (control → io): `InjectPrompt { pane, bytes, urgent }`,
  `Interrupt { pane }`, `Layout(..)`, `Notice(..)`, `SpawnPane(..)`,
  `ClosePane(..)`. Prompt injection, the only PTY write the control lane needs,
  becomes a command. An attached operator draft still blocks injection
  (cas-eacc), now checked by the io lane, which owns the composer state.
- **pane snapshots** (io → control): one small struct per pane, published by
  the io lane after each iteration and read by the control lane without a
  lock (`ArcSwap` or a seqlock): `last_output_at`, `last_input_at`,
  `silent_since`, `composer_dirty`, `bytes_received`, `alive`. These are what
  the wake gate and the stall detector read today from `self.app.mux`.

Nothing on the io lane may allocate unboundedly or wait on the control lane.
If the command channel is full, the control lane waits, not the io lane.

### 3.2 Who owns state

| State | Owner | Readers |
|---|---|---|
| PTY masters, terminal emulators, composer, focus, layout | io | control via pane snapshots |
| Prompt queue rows, receipts, stages | SQLite, written only by jobs | control via job results |
| Director data (tasks, agents, leases, reminders, epics) | jobs build an immutable `DirectorSnapshot` | control applies it; io renders the parts on screen via `Notice`/`Layout` |
| Delivery decisions (wake gate, nudges, relays) | control | io executes the resulting commands |
| Loop metrics, watchdog ring | each lane writes its own counters; the watchdog thread aggregates | `loop.json`, TUI status |

## 4. Off-loop I/O model (jobs lane)

### 4.1 The rule

The control lane may only do in-memory work and channel sends and receives.
Every operation that can block for more than 1 ms is a **job**: the control
lane submits `Job { kind, key, generation, payload }` and keeps going. A job
runs on the store worker (SQLite) or the blocking pool (everything else) and
posts `JobResult { key, generation, outcome, elapsed }` to the completion
queue. The control lane drains completions at the top of each pass and
applies them; a result whose generation is stale is discarded.

Store writes that today use `store_worker::defer` (GH #1165) already follow
this shape. v2 makes it the only path and extends it to reads.

### 4.2 Job semantics

- **Single-flight per key.** A second submit for a key that is in flight
  coalesces into the first (for example `DirectorSnapshot`, or
  `GitFetch(remote, branch)` as cas-a222 does).
- **Generations.** Every result carries the generation it was computed for;
  the control lane drops results older than the state it has already applied.
- **Budgets and backoff.** Each kind has a budget (§4.4). Over budget is
  reported by the watchdog; a failing key backs off exponentially, as
  cas-a222 does for fetches and cas-7156 does for stall probes.
- **Bounded queues.** Each kind has a cap. A full queue drops the oldest
  non-critical submit (a refresh) and never blocks the submitter. Critical
  writes (receipts, stage stamps, lifecycle relays) are never dropped; their
  queue is unbounded but its age is a watchdog metric.
- **One writer.** The store worker owns the only write connection to
  `cas.db` from the daemon. SQLite busy waits and flock waits (GH #1165,
  6,207 busy deferrals and 2,264 "database is locked" mirror failures on
  2026-10-15) then happen on that thread, never on io or control.

### 4.3 What moves where

| Today (on the loop) | v2 job kind | Notes |
|---|---|---|
| Director refresh: task, agent, lease, epic, reminder reads (`apply director read`, `change detection`) | `DirectorSnapshot` | One read transaction builds the snapshot **and** its change set off-loop; control applies a diff in memory. Today's 1,915 ms p99 becomes a job metric, not a stall. |
| Supervisor-stall evaluation (`supervisor stall` step) | input to `DirectorSnapshot` | Needs lane CI state (cas-207e cache) and task state; both arrive in the snapshot. |
| Stall confirmation: agent-store open, `/proc` cmdline/environ scan (`wedged.rs:1441`), child walk, transcript tail (cas-7156) | `WorkerHealthProbe(worker)` | Single-flight per worker, cas-7156's backoff kept; the result feeds the detector. |
| Reminders: time reminders (store), external wake probes (git, gh) | `ReminderScan`, `GitFetch(remote, branch)`, `CiStatus(sha)` | cas-a222 already moved the external probe; the expiry write (6,207 busy deferrals) moves to the store worker too. |
| Prompt-queue pass: selection, stage stamps, receipts, claims | `QueueSelect`, `QueueStamp` | Control decides wake and gate from pane snapshots; io executes `InjectPrompt`; receipts are written by jobs. spec-6194 semantics are unchanged. |
| Merge-request revalidation (`prompt_revalidation.rs:962`, cas-874e) | `RefTip(ref)` | Ref reads are local and fast but still a `git` subprocess; they become a job. A bounded fetch is never allowed on control. |
| CI watch, Violet activity, cloud relay, Commander mirror writes | `CiWatch`, `VioletPoll`, `CloudRelay`, `MirrorWrite` | Network and store. |
| Relays (auth failure, permission, usage limit) | built from pane snapshots on control; their store writes are `QueueStamp` jobs | Today 50–282 ms each on 10-15. |

The full inventory of blocking calls reachable from the loop, with file:line,
is in §2.

### 4.4 Budgets per job kind

| Kind | Budget (watchdog) | Backoff on failure |
|---|---|---|
| `DirectorSnapshot` | 250 ms | none; next tick retries |
| `QueueSelect` / `QueueStamp` | 100 ms | retry next tick; receipts never dropped |
| `WorkerHealthProbe` | 100 ms | 5 → 60 s (cas-7156) |
| `ReminderScan` | 100 ms | next tick |
| `GitFetch` / `RefTip` | 5 s / 500 ms | per ref, exponential (cas-a222) |
| `CiWatch` / `CiStatus` | 10 s | exponential |
| `VioletPoll` / `CloudRelay` / `MirrorWrite` | 2 s | exponential |

## 5. Render scheduler (io lane)

GH #1210 measured ~30 output-driven frames/s × ~16 ms each in Konsole for a
factory with codex workers; cost is per frame, not per byte. v2 renders from
damage, at a capped rate, and only what is visible.

### 5.1 Frame causes and priority

| Cause | When it renders | Cap |
|---|---|---|
| **Input echo** | output from the pane that received input in the last 250 ms | uncapped; flushed immediately |
| **Layout / focus / notice** | the next io iteration, coalesced within 16 ms | uncapped, coalesced |
| **Output, focused pane** | at most every `1000 / render_max_fps` ms | `render_max_fps`, default 15 |
| **Output, other visible panes** | at most every `1000 / render_unfocused_fps` ms | `render_unfocused_fps`, default 5 |
| **Output, panes not on screen** | never; terminal state updates only | 0 |

Input frames always go first: an echo frame due at the same time as an output
frame is sent first, and the output frame is merged into the next slot.

### 5.2 Damage tracking

Each pane's emulator marks damaged rows. A frame contains only the damaged
cells of visible panes, plus chrome that changed. A frame with no damage is
not sent.

### 5.3 Config and metrics

- `[factory] render_max_fps` (default 15) and `[factory] render_unfocused_fps`
  (default 5), via `cas config`.
- `loop.json` gains `frames` per cause per window (`input`, `layout`,
  `output_focused`, `output_unfocused`), `frames_coalesced`, and
  `frame_bytes_p50`.

## 6. Watchdog

The loop already writes `loop.json` with `phase` and per-phase `max_ms` and
`over_100ms`, and logs slow director steps. v2 turns that into a watchdog that
names the slow step where the operator is looking.

1. **Step markers.** Each lane sets an atomic "current step" marker (a
   `&'static str`) before every step. Every step in §2 has a name.
2. **Budgets.** io iteration 5 ms; control step budgets: prompt-queue
   decisions 20 ms, snapshot apply 10 ms, every other step 10 ms; job budgets
   per §4.4.
3. **Over budget.** The lane records `{lane, step, elapsed_ms, budget_ms, at}`
   in a ring of the last 32 events, logs
   `cas::loop_watchdog lane=<l> step=<s> elapsed_ms budget_ms` at WARN, and
   the TUI shows `slow: <lane>/<step> <n> ms` in the status line for 10 s.
4. **Wedge.** A separate watchdog thread checks each lane's heartbeat every
   250 ms. A lane that has not finished an iteration in 1 s is wedged: the
   TUI shows `wedged: <lane>/<step> for <n> s`, and `loop.json` records the
   step, the duration and `/proc/<tid>/wchan` (today's `loop_thread_wait`).
5. **Job health.** The oldest pending job age per kind is in `loop.json` and
   in the wedge check. A store worker stuck on a flock is a named job wedge,
   not a frozen TUI.
6. **No blocking call on a lane.** In debug and test builds, io and control
   set a thread-local `ON_LANE` flag. Store opens, `std::process::Command`,
   and reads under `/proc` or of a transcript assert `!ON_LANE`. Release
   builds log once per call site instead of asserting (report mode), until
   C6 (§8) flips the tests to enforce.

## 7. Named latency tests

Prefix `flv2_`, so `cargo nextest run -E 'test(/flv2_/)'` selects them all.
The load harness is cas-b8a9's (cas-98b24): 16 concurrent agents, codex-heavy
PTY output (≥ 3 panes at 38–236 writes/s, from GH #1210).

| Test | Budget | Level | Owner child |
|---|---|---|---|
| `flv2_key_to_echo_p99_under_50ms_under_load` | p99 key → echo < 50 ms, 16 agents, codex-heavy load | load harness | C4 io lane |
| `flv2_echo_unaffected_by_stalled_control_lane` | inject a 2 s sleep into a control step; echo p99 still < 50 ms | integration | C4 |
| `flv2_echo_unaffected_by_held_store_lock` | hold `task-sync-intents.lock` 3 s (GH #1165); echo p99 < 50 ms | integration | C4 |
| `flv2_control_pass_p99_under_100ms_with_16_agents` | control pass p99 < 100 ms | load harness | C2 + C3 |
| `flv2_no_blocking_call_on_lanes` | the §6.6 assertion trips on any store open, `Command`, `/proc` or transcript read on io or control | unit + load harness | C1 (report), C6 (enforce) |
| `flv2_director_snapshot_off_lane` | a 2 s `DirectorSnapshot` job leaves control p99 < 100 ms | integration | C2 |
| `flv2_worker_health_probe_single_flight` | N passes over a stuck worker run one probe per backoff step | unit | C2 |
| `flv2_queue_stamps_on_store_worker` | queue selection and stamps run as jobs; spec-6194 `ct2_` tests still pass | integration | C3 |
| `flv2_output_frames_capped` | a pane at 236 writes/s yields ≤ `render_max_fps` + 1 output frames/s | unit | C5 render |
| `flv2_unfocused_frames_capped` | unfocused visible pane ≤ `render_unfocused_fps` + 1 | unit | C5 |
| `flv2_invisible_pane_output_sends_no_frame` | output from a pane off screen sends 0 frames | unit | C5 |
| `flv2_input_frame_precedes_output_frame` | echo frame is flushed within one io iteration and before a pending output frame | unit | C5 |
| `flv2_watchdog_names_slow_step` | a stub step over budget appears in the ring, the log and the status line with its lane and step | unit | C1 watchdog |
| `flv2_wedge_reports_step_and_duration` | a lane blocked 1.5 s is reported as wedged with its step | integration | C1 |
| `flv2_external_reminders_never_on_lane` | regression for GH #1208 with ≥ 5 external reminders | integration | C2 (verify cas-a222) |

## 8. Migration order and shared files

| Child | Scope | Main files | Depends on |
|---|---|---|---|
| C1 watchdog | step markers, budgets, ring, TUI status, wedge thread, `ON_LANE` assertion in report mode, `loop.json` fields | `lifecycle.rs` (loop), `director_refresh.rs` (profile), loop.json writer | none; lands first so every later child is measured |
| C2 off-lane director | `DirectorSnapshot`, `WorkerHealthProbe`, `ReminderScan`, CI/Violet/cloud jobs | `director_refresh.rs`, `director/events.rs`, `wedged.rs`, `store_worker.rs`, `ci_watch.rs` | C1 |
| C3 off-lane queue | `QueueSelect`, `QueueStamp`, `RefTip`; control keeps decisions | `queue_and_events.rs`, `delivery.rs`, `prompt_revalidation.rs`, `prompt_queue_store.rs` | C1; parallel with C2 |
| C4 io lane | io thread, command and snapshot channels, input/echo fast path | `lifecycle.rs`, `cas-mux` pane/PTY, `delivery.rs` (injection becomes a command) | C2, C3 (the control lane must already be I/O-free) |
| C5 render scheduler | damage, caps, visibility, config knobs, frame metrics; absorbs cas-77f39 (GH #1210) | render path, config | C4 |
| C6 enforcement | load-harness receipts for §7, `ON_LANE` enforce mode | tests, CI | C2–C5 |

Shared hotspots: `lifecycle.rs` (C1, C4), `queue_and_events.rs` (C3, C4),
`director_refresh.rs` (C1, C2). Children on a hotspot land in the order above
and rebase on each other's merged tips.

cas-77f39 (GH #1210) can still ship its point fix (an output-frame cap in the
current loop) before C5; C5 then replaces it.

## 9. Out of scope

- Cgroup isolation of worker builds (GH #1213). This spec makes echo
  independent of a stalled control lane, but not of the io thread being
  paged out; #1213 handles memory.
- Konsole's own paint cost (fixed per frame; the cap in §5 bounds it).
- spec-6194 semantics. They move to jobs unchanged.
