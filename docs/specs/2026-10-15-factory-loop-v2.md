# Factory loop v2: typing never waits

Task: cas-5c24. Epic: cas-759c (GH #1211; also closes #1210, informs #1213).
Status: **draft for supervisor review**; spec record spec-4c5b. Base: `7b518440c` (v42 epic tip).
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

The timers nest: "apply director read" (LC:3106) wraps all of
`apply_director_refresh`, which contains the "change detection"
(ADR:238) and "supervisor stall" timers. Those two rows are parts of the
first, not additions to it.

`reminders` is GH #1208 (synchronous `git fetch` per external reminder); the
fix cas-a222 is merged into v41 but the measured daemons still ran 3.55.0.
Nothing in v41 fixes the director apply: **it blocked the loop for about
2,170 s on 2026-10-15** (1,516 s on 10-14), on top of the reminders. Change
detection and the supervisor-stall check account for most of it.

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
| cas-a222 (GH #1208) | `process_reminders → external_wake_condition_observation` ran one `git fetch` per pending external reminder on the loop every 60 s: 2.5–9 s stalls | Fixed in v41: probe off the loop, one fetch per ref, backoff. It also delivered GH #1211 item 5 (reminder hygiene): 7-day default TTL for external reminders, expiry of dead-session cross-session reminders after a 24 h grace, `origin/` target normalisation at remind time. Not repeated here. |
| cas-7156 | `transcript_confirms_stall` ran on every working pass per worker: agent-store open, 2 `list()`s, a full `/proc` cmdline/environ scan (`wedged.rs:1441`), a child walk and a transcript-tail read, all inside the director refresh at `director_refresh.rs:240` | Gated behind threshold and backoff in v42; still synchronous on the loop |
| cas-0028 | Worker-attention relays: the daemon snapshots episode baselines on the loop, then hands the store write to the store worker | Precedent for "snapshot on loop, write off loop" |
| cas-874e | Merge-request revalidation resolves live branch tips (`prompt_revalidation.rs:962`) and now does a bounded source fetch on the MCP side | Daemon-side revalidation must stay ref-local or move off-loop (§4.3) |
| GH #1210 | the dirty flag is set by any PTY output, input or refresh (LC:2829–2842): every pass with any worker PTY output redraws; codex workers write 38–236 times/s; ~30 frames/s × ~16 ms in Konsole, GUI thread ~60% busy | Not fixed |
| GH #1213 | Worker builds share the TUI's cgroup; at 13:50:45 a daemon pass took 1,618 ms and keys sat unread for 2.0 s; the refresh thread was in D-state on page I/O | Out of scope here (cgroup); §3 makes echo independent of it |

## 2. Today's loop, step by step

Code map at `7b518440c`. Abbreviations:

| Short | File |
|---|---|
| LC | `cas-cli/src/ui/factory/daemon/runtime/lifecycle.rs` |
| QE | `cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs` |
| ADR | `cas-cli/src/ui/factory/app/director_refresh.rs` |
| DR | `cas-cli/src/ui/factory/daemon/runtime/director_refresh.rs` |
| APP | `cas-cli/src/ui/factory/app/mod.rs` |
| EV | `cas-cli/src/ui/factory/director/events.rs` |
| PR | `cas-cli/src/ui/factory/director/prompts.rs` |
| LW | `cas-cli/src/ui/factory/daemon/runtime/loop_watchdog.rs` |
| SW | `cas-cli/src/ui/factory/daemon/runtime/store_worker.rs` |

### 2.1 Shape

`FactoryDaemon::run` is a sequential `while` loop of polls (LC:2365–2949),
driven by `rt.block_on` on one OS thread. Cadence comes from `Instant`
checks: refresh every 2 s (`DAEMON_REFRESH_INTERVAL`, LC:5), prompt and spawn
polls every 100 ms (LC:2340). The only `select!` is the idle wait at the end
of the pass (LC:2937–2948): it sleeps 4 ms after output, 8 ms with a client,
100 ms while spawning, 500 ms headless, and only `notify_rx` wakes it.
**Keystrokes do not wake the loop**; client sockets are read at the top of
the next pass.

Each pass runs under `bound_waits_for(PASS_STORE_WAIT_BUDGET = 50 ms)`
(LC:2372–2375, SW:28). That budget only bounds SQLite busy handling, the
connection mutex and the intents flock. It does not bound git, gh, `/proc`,
transcript reads, the Teams inbox flock or PTY writes.

### 2.2 Steps per pass

Costs: **M** = measured (§1); **E** = estimated from the code.

| # | Step (loop.json phase) | Location | Cadence | Blocks on | Cost |
|---|---|---|---|---|---|
| 1 | accept clients (client input) | LC:2385, client_input.rs:6 | every pass | 50 ms `write_all` of init bytes, new clients only | M: 0 ms |
| 2 | client input → PTY (client input) | LC:2394, client_input.rs:65–236 | every pass | **unbounded PTY `write_all`** (cas-pty pty.rs:2684); Ctrl+R runs a full director read and apply on the loop (client_input.rs:860) | M: 0 ms |
| 3 | PTY output (pty output) | LC:2402, terminal_exchange.rs:37–115, mux.rs:1384 | every pass | CPU: VT feed of every queued byte; on a worker exit, `handle_worker_crash` (QE:4102) opens the agent store | M: 0 ms |
| 4 | session summarizer | LC:2411 | rate-limited | CPU (transcript build); LLM call is off-loop | E: small |
| 5 | cloud relay (cloud relay) | LC:2423 | every pass | PTY write | M: 0 ms |
| 6 | **prompt queue** (prompt queue) | LC:2432, QE:5681–7816 | every 100 ms or on notify | see §2.3 | **M: max 208–245 ms; ≥ 1.2 s on urgent injects (E)** |
| 7 | spawn queue (spawn queue poll) | LC:2455–2466, QE:7979 | 100 ms | SQLite (budgeted), audit file appends | M: 0 ms |
| 8 | pending spawns (pending spawns) | LC:2473, QE:8191 | only while spawning | shutdowns awaited inline; up to 20 × 25 ms sleep (APP:2386–2401) | M: 0 ms in window |
| 9 | CI-watch start | LC:2485–2564 | 30–60 s | off-loop, except `git rev-parse HEAD` (LC:2508) without a worktree manager | E: small |
| 10 | Violet, peer mailbox | LC:2609–2660 | 15 s | off-loop | — |
| 11 | spawn verifications (refresh: spawn verify) | LC:2675, QE:4305 | while verifying | agent store, hook-canary reads, `kill_worker` awaited | M: 0 ms |
| 12 | **director read + apply** (refresh: director data) | LC:2687, LC:3090, ADR:173–247 | start every 2 s; apply when the reader thread finishes | read is off-loop (DR:36); **apply is on the loop**, see §2.4 | **M: apply p99 1,913 ms, change detection p99 836 ms, supervisor stall p99 1,820 ms (10-15)** |
| 13 | delivery revalidation (refresh: delivery revalidation) | LC:3104–3158, APP:1363–1516 | per applied read | **`git fetch` up to 5 s** per AwaitingMerge idle event (PR:977 → git_evidence:192–270); `/proc` scan per idle prompt; a full DirectorData load on the loop if the reader is busy (LC:3124) | E: up to 5 s per fetch |
| 14 | relays (refresh: relays) | LC:3166–3245, QE:2609–2804 | 5–60 s each | agent store, transcript reads, Teams mailbox scans | M: 50–282 ms |
| 15 | **reminders** | QE:9203 | ~2 s | reminder store (100 ms busy budget); external probe off-loop since cas-a222 | **M: p50 2,563 ms before cas-a222; expiry busy deferrals 6,207/day** |
| 16 | epic events | QE:9356 | rare | `create_epic_branch` does a `git fetch` (branch_ops.rs:395); merges on completion | E: seconds, rare |
| 17 | inbox retraction | LC:3261–3357, teams.rs:1409–1460 | per applied read | **unbounded inbox flock**, and a **bounded `git fetch` per unread tagged row while holding it** (LC:3302, PR:1040–1069) | E: up to 5 s per row |
| 18 | inject loop (refresh: prompt injection) | LC:3361–3537, APP:1540 | per applied read | `prompt_is_still_deliverable`: **full DirectorData load on the loop**, merge freshness (`git fetch`), `/proc` scan; task-store read and assignment enqueue write (LC:3394–3397); inbox flock or PTY write | E: 100 ms – 5 s |
| 19 | resize, redraw reset | LC:2699–2816 | on event | CPU | — |
| 20 | **draw** (render) | LC:2829–2885 | when dirty | CPU, in-memory `BufferBackend` | M: max 3 ms |
| 21 | flush | LC:2887–2901 | when buffered | non-blocking sockets | M: in render |
| 22 | idle wait | LC:2921–2948 | every pass | sleep 4–500 ms | — |

### 2.3 Inside the prompt-queue step (step 6)

Not sub-timed today; all of this is on the loop every 100 ms:

- Store opens per pass: prompt queue (QE:5685), agent store + `list` (QE:5692), supervisor queue (QE:5842), event store (QE:5897), spawn queue (QE:7980). Each `open_*` runs `init()`'s schema batch (cost unmeasured).
- **Per selected row:** another `open_agent_store().list(None)` (QE:5912); envelope rows open the task store (QE:6012, 6154); merge-request rows run `git rev-parse` and `merge-base --is-ancestor` (prompt_revalidation.rs:967–1075, no fetch).
- **Wake gating reads transcripts:** `recipient_tool_call_evidence` (QE:5485) reads the **whole transcript line by line** (wedged.rs:600–613, 785); `recipient_mid_turn`, `recipient_wake_evidence`, `recipient_turn_completion_after` and `claude_reaction_observed` (QE:3617–3800) each open the agent store and read a tail.
- **Due delivery probes** (QE:3244, 120 s window): `idle_worker_liveness` (PR:680) loads `Config` and does a **full `/proc` scan** (wedged.rs:1365–1441).
- Commander mirror every 5 s (QE:5715): `Config::load` and a supervisor transcript read.
- Poison sweep every 60 s (QE:5767–5817), with `Config::load`.
- **Transport:** Teams inbox write under an **unbounded `fs2::lock_exclusive` flock** (teams.rs:170, 1154–1216). **Urgent rows await `interrupt_and_inject_preserving_composer(settle = 1200 ms)` inline** (QE:6979–6983, 7158–7177), and `wait_for_injection_readiness` polls for up to 4 s more (mux.rs:1169–1215). **The whole loop is suspended for 1.2–5.2 s per urgent message**: no input and no frames.

### 2.4 Inside the director apply (step 12)

- The read is off-loop on `factory-refresh-reader` (DR:36): `DirectorData::load_with_stores_for_project` (cas-factory director.rs:358) plus `git status --porcelain` per worktree every 10 s.
- `apply_director_refresh` (ADR:173–247) runs on the loop. Every applied refresh calls **`detect_supervisor_stall`** (APP:1721), even when nothing changed. For each AwaitingMerge task in the focused epic it runs `classify_delivery_hold` (task store, QA passes, `delivery_ci_pending`), **uncached** `git rev-parse` + `merge-base --is-ancestor` twice (APP:1610, 102), and `classify_merged_close_blocked_task` (2 × rev-parse + is-ancestor, APP:1568–1605). On a CI cache miss, `delivery_ci_pending` runs git and **gh** with 2 s timeouts (worktree_ops.rs:289–326).
- On change it adds `sync_session_mappings` and `reconcile_phantom_worker_panes` (agent store, spawn queue) and **`detect_changes`** (ADR:238 → EV:1551). `detect_changes` is in memory except `transcript_confirms_stall` (EV:1465): agent store with 2 `list`s (wedged.rs:1033), a **full `/proc` cmdline/environ scan** (wedged.rs:1441), a process-tree walk (wedged.rs:1560) and a **whole-transcript read**. cas-7156 now gates and backs this off; it is still on the loop.

### 2.5 Blocking-call inventory on the loop thread

| Kind | Call sites | Worst case |
|---|---|---|
| `git fetch` (5 s cap, 50 ms sleep poll) | git_evidence/mod.rs:209, 225–270, via APP:1461, APP:1546, LC:3302 (under the inbox flock), APP:1817; branch_ops.rs:395 | 5 s per call |
| git `rev-parse` / `merge-base` / `rev-list` | git_evidence/mod.rs:50–100; APP:102, 1576–1627; prompt_revalidation.rs:974–1075; LC:2508 | tens of ms each, several per task per 2 s |
| gh (CI status) | worktree_ops.rs:312–326 via APP:1696 | 2 s per call on cache miss |
| `/proc` full scan | wedged.rs:1365–1441 via `idle_worker_liveness` (PR:680) and `transcript_confirms_stall` (EV:1489) | O(processes); hundreds on this host |
| Whole-transcript reads | wedged.rs:600–613, 785; EV:1465 | O(transcript size) |
| Inbox flock (unbounded) | teams.rs:170, 1409–1460 | unbounded |
| PTY `write_all` (unbounded) | cas-pty pty.rs:2684 | unbounded on a full PTY buffer |
| Inline awaits | urgent and context-reset inject (QE:5395, mux.rs:1169–1215) | 1.2–5.2 s |
| SQLite and intents flock | every `open_*` and query | budgeted 50 ms per pass (GH #1165 fix) |

### 2.6 Already off the loop

`factory-refresh-reader` (director and delivery reads), `StoreWorker`
(lifecycle outbox, CI persistence, attention relays, deferred writes),
task-sync reconcile thread, external-wake scan (cas-a222), branch visibility,
CI watch, Violet, peer mailbox, session-summarizer LLM call, provisioning,
and the `cas-loop-watchdog` thread (LW:415) that writes `loop.json` every 5 s
and records `loop_thread_wait` after 60 s.

### 2.7 Existing instrumentation to build on

- `LoopProgress` (LW:111–288): pass and per-phase windows with `entries`,
  `max_ms`, `over_100ms`. The prompt-queue phase has no sub-phases; CI,
  Violet and peer mailbox count under `refresh`; draw and flush under
  `render`.
- `timed()` (ADR:122–136) logs director steps ≥ 50 ms on
  `cas::refresh_profile`. Not timed: prompt-queue internals, inbox prunes,
  the inject loop, `prompt_is_still_deliverable`.
- `loop_latency_tests.rs`: pass < 100 ms while flock and SQLite are held
  (:130); the input/exchange/draw path touches no store (:201); reminders
  never wait on external fetches (:372). §7 extends this file's pattern.
- Store-access violation counter (wait_budget.rs:200–205): today it only
  counts and logs.

### 2.8 What the map says

1. Input, PTY and render cost 0–3 ms. Echo is slow only because it waits
   behind steps 6, 12–18.
2. Four call families account for every multi-second stall: `git fetch`
   (steps 13, 16–18), urgent-inject awaits (step 6), `/proc` + transcript
   reads (steps 6, 12, 13, 18), and lock waits (inbox flock, SQLite).
3. Moving the director **read** off the loop (cas-ee9ab) was not enough: the
   **apply** still does git, gh, `/proc` and store work.

## 3. Target architecture: three lanes

v2 splits today's single loop into three lanes with one rule each.

| Lane | Thread | Owns | Rule | Budget |
|---|---|---|---|---|
| **io** | dedicated OS thread | client socket, every PTY master (read and write), per-pane terminal state, render scheduler, client frames | never calls a store, git, gh, network, `/proc` or a transcript | one iteration < 5 ms; key → echo p99 < 50 ms |
| **control** | today's loop thread | prompt-queue decisions, director state machine, spawn/shutdown orchestration, relays | only in-memory work and channel traffic; never blocks on I/O | one pass p99 < 100 ms with 16 agents |
| **jobs** | the refresh reader's dedicated read connection, the store-worker writer, and a bounded blocking pool | every call that can block: SQLite, flocks, git, gh, HTTP, `/proc`, transcript reads | single-flight per key, results posted back, budgets and backoff per kind | per kind, §4.4 |

### 3.1 Input/echo isolation (io lane)

Today a key travels: client socket → the loop's `client input` step → PTY
write → agent echoes → the loop's `pty output` step on a later pass → `render`
→ client socket. Each hop waits for the loop to come round, so one slow step
anywhere (§1.2) delays echo in every pane.

In v2 the io lane does the whole round trip on its own thread:

1. It reads every client socket and classifies each input event (§3.3).
   Keystrokes for the focused pane are written straight to that pane's PTY
   master. No other lane is involved.
2. Per-pane PTY reader threads (or one `poll` set) feed bytes into that pane's
   terminal emulator, which the io lane owns.
3. Any output from the pane that received input in the last 250 ms is an
   **echo frame**: it is drawn and flushed at once, ahead of any capped output
   frame (§5).

Two io-lane hazards found in §2 must be handled inside the lane:

- **PTY writes are blocking `write_all` today** (cas-pty pty.rs:2684). The io
  lane writes non-blocking and keeps a per-pane pending buffer; a pane whose
  buffer is full stops taking input for that pane only.
- **Keystrokes don't wake the loop today** (§2.1). The io lane waits on the
  client sockets and every PTY reader with one `poll`/epoll set, so a key is
  handled on arrival, not on the next timer tick.

### 3.2 What the io lane draws: panes plus a published view model

Today `terminal.draw(|f| self.app.render(f))` (LC:2853) renders the whole
`FactoryApp`: panes, director panels, dialogs, sidebar and status line, and
`mux` is a `FactoryApp` field (app/mod.rs:921). v2 splits that object:

- **Panes** (the `mux`: PTY masters, emulators, per-pane scrollback and
  scroll offset, composer state, focus) move into the io lane. The io lane is
  their only owner.
- **Everything else** (director data, panels, dialogs, sidebar contents,
  selection, notices, task-dialog state) stays in the control lane's
  `FactoryApp`. After every change the control lane publishes an immutable
  `ViewModel` (an `Arc` swapped atomically): the rendered-ready text, rows and
  styles for each non-pane region, plus the layout (which panes are visible
  and where).
- **Drawing.** The io lane renders each frame from its own panes plus the
  latest `ViewModel`. `FactoryApp::render` is split into a pane renderer (io)
  and a view-model builder (control). A new `ViewModel` is a **layout frame**
  (§5.1): drawn on the next io iteration, coalesced within 16 ms.

The control lane reads pane state only through **pane snapshots**: one small
struct per pane, published by the io lane after each iteration and read
without a lock (`ArcSwap` or a seqlock): `last_output_at`, `last_input_at`,
`silent_since`, `composer_dirty`, `bytes_received`, `alive`. These are what
the wake gate and the stall detector read today from `self.app.mux`.

### 3.3 Input routing

`process_client_input` (client_input.rs:163–938) handles more than pane
keystrokes. In v2 the io lane classifies each event:

| Event | Handled by | How |
|---|---|---|
| Keys and paste for the focused pane | io | written to the PTY |
| Pane scroll (keys, mouse wheel over a pane), focus changes between panes | io | updates io-owned pane state; a layout frame |
| Mouse on panels, sidebar or dialogs; dialog keys; panel navigation; operator commands | control | sent as `UiInput(event)` on the io → control channel; control updates `FactoryApp` and publishes a new `ViewModel` |
| Ctrl+R (`refresh_data()`, client_input.rs:860) | control | submits a `DirectorSnapshot` job instead of reading on the loop |

The io lane decides by hit-testing against the current `ViewModel` layout, so
routing needs no round trip to the control lane.

### 3.4 Other clients

All client sockets are io-lane I/O: Full clients, compact clients, the GUI
and WS clients, and the cloud relay (LC:2846–2900). The io lane accepts them,
reads them, and flushes frames to them non-blocking, as today's
`flush_outputs` does.

- **Full and compact clients** get frames from the io lane's renderer (the
  compact layout is a second `ViewModel` layout).
- **GUI and WS clients** get structured state, not terminal frames. Today
  `send_state_update` (gui_client.rs:258) builds it on the loop; in v2 the
  control lane builds it alongside the `ViewModel` and the io lane only sends
  it. GUI and WS operator commands are `UiInput` events to control.
- **Cloud relay** input that targets a pane (relay.rs:~325) is pane input on
  the io lane; relay control messages go to control.

### 3.5 Channels and who owns state

The lanes talk through three channels and two published values:

- **commands** (control → io): `InjectPrompt { row, generation, pane, bytes,
  urgent }`, `Interrupt { pane }`, `SpawnPane(..)`, `ClosePane(..)`,
  `SendState(..)` (GUI/WS). An attached operator draft still blocks injection
  (cas-eacc), now checked by the io lane, which owns the composer.
- **events** (io → control): `UiInput(..)`, `InjectOutcome { row,
  generation, outcome }`, `PaneExited(..)`, `ClientAttached(..)`.
- **completions** (jobs → control): `JobResult`s (§4).
- **published**: `ViewModel` (control → io) and pane snapshots (io →
  control).

No lane ever waits on another. A full channel means the sender keeps the item
and retries on its next iteration.

| State | Owner | Readers |
|---|---|---|
| PTY masters, emulators, pane scrollback and scroll, composer, focus | io | control via pane snapshots |
| `FactoryApp` non-pane state: director data, panels, dialogs, sidebar, notices | control | io via `ViewModel` |
| Prompt-queue rows, receipts, stages | SQLite, written by the jobs writer | control via job results |
| Director data | built by jobs as `DirectorSnapshot` | control applies it to `FactoryApp` |
| Delivery decisions (wake gate, nudges, relays), in-flight deliveries (§4.5) | control | io executes the resulting commands |
| Loop metrics, watchdog ring | each lane its own counters; the watchdog thread aggregates | `loop.json`, TUI status |

## 4. Off-loop I/O model (jobs lane)

### 4.1 The rule

The control lane may only do in-memory work and channel sends and receives.
Every operation that can block for more than 1 ms is a **job**: the control
lane submits `Job { kind, key, generation, payload }` and keeps going. A job
runs on a reader (SQLite reads), the store-worker writer (SQLite writes) or
the blocking pool (everything else) and
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
- **Readers and one writer.** Reads keep the dedicated read connection of
  today's `factory-refresh-reader` thread (DR:36–48), so snapshots and
  revalidation reads never queue behind writes. Writes go through the store
  worker, the daemon's only write connection to `cas.db`. A GH #1165-style
  flock or busy wedge then holds up writes only, and the watchdog names it as
  a job wedge (§6). SQLite busy waits and flock waits (6,207 busy deferrals
  and 2,264 "database is locked" mirror failures on 2026-10-15) happen on
  those threads, never on io or control.

### 4.3 What moves where

| Today (on the loop) | v2 job kind | Notes |
|---|---|---|
| Director refresh: task, agent, lease, epic, reminder reads (`apply director read`, `change detection`) | `DirectorSnapshot` | One read transaction builds the snapshot **and** its change set off-loop; control applies a diff in memory. Today's 1,913 ms p99 becomes a job metric, not a stall. |
| Supervisor-stall evaluation (`supervisor stall` step) | input to `DirectorSnapshot` | Needs lane CI state (cas-207e cache) and task state; both arrive in the snapshot. |
| Stall confirmation: agent-store open, `/proc` cmdline/environ scan (`wedged.rs:1441`), child walk, transcript tail (cas-7156) | `WorkerHealthProbe(worker)` | Single-flight per worker, cas-7156's backoff kept; the result feeds the detector. |
| Reminders: time reminders (store), external wake probes (git, gh) | `ReminderScan`, `GitFetch(remote, branch)`, `CiStatus(sha)` | cas-a222 already moved the external probe; the expiry write (6,207 busy deferrals) moves to the store worker too. |
| Prompt-queue pass: selection, stage stamps, receipts, claims | `QueueSelect` (claims rows atomically), `QueueStamp` | Control decides wake and gate from pane snapshots and tracks in-flight rows (§4.5); io executes `InjectPrompt`; receipts are written by jobs. spec-6194 semantics are unchanged. |
| Merge-request revalidation (`prompt_revalidation.rs:962`, cas-874e) | `RefTip(ref)` | Ref reads are local and fast but still a `git` subprocess; they become a job. A bounded fetch is never allowed on control. |
| CI watch, Violet activity, cloud relay, Commander mirror writes | `CiWatch`, `VioletPoll`, `CloudRelay`, `MirrorWrite` | Network and store. |
| Relays (auth failure, permission, usage limit) | built from pane snapshots on control; their store writes are `QueueStamp` jobs | Today 50–282 ms each on 10-15. |
| Merge freshness `git fetch` in delivery revalidation, inbox retraction and the inject loop (§2.2 steps 13, 17, 18) | `GitFetch(remote, branch)` then `RefTip` | Never under a lock; the inbox prune decides from the last fetched tip. |
| `prompt_is_still_deliverable` full DirectorData load (APP:1540) and the fallback load at LC:3124 | `RevalidateFresh(row)` | Stays a **fresh read**, run as a job on the read connection. app/mod.rs:1541–1547 states "Never remove a legitimate notification based on cached data", so neither delivery revalidation nor retraction may decide from the cached `DirectorSnapshot`. |
| Teams inbox writes and prunes under `fs2::lock_exclusive` (teams.rs:170, 1409–1460) | `InboxWrite(recipient)`, `InboxPrune(recipient)` | Single-flight per recipient file; the flock is taken only on the jobs lane. Retraction (the prune decision) is a fresh read inside the job, never the cached snapshot. |
| Inject loop's task-store read and assignment enqueue write (LC:3394–3397) | `RevalidateFresh(row)` read, `QueueStamp` write | |
| `idle_worker_liveness` (`Config::load`, `/proc` scan, transcript) and wake-gate transcript reads (QE:3617–3800, 5485) | `WorkerHealthProbe(worker)` | One cached result per worker, refreshed on backoff; the wake gate reads the cache plus the io lane's pane snapshot. Whole-file transcript reads become seek-to-tail. |
| `Config::load` per pass (QE:2929, 5715, 5767) | none: config cached, reloaded on file change | |
| Per-pass and per-row `open_*_store` + `init()` (§2.3) | none on control: the store worker keeps open handles | Schema init runs once per daemon. |
| Urgent and context-reset injects awaited inline for 1.2–5.2 s (QE:5395, mux.rs:1169–1215) | io-lane `Interrupt` + `InjectPrompt` command | The io lane runs readiness as a per-pane state machine with deadlines, never a sleep; control gets a `InjectOutcome` event back. |
| Epic branch creation and merges (`git fetch`, merges; QE:9356) | `EpicBranch`, `EpicMerge` | Rare, but seconds long. |

The full inventory of blocking calls reachable from the loop, with file:line,
is in §2.

### 4.4 Budgets per job kind

| Kind | Budget (watchdog) | Backoff on failure |
|---|---|---|
| `DirectorSnapshot` | 250 ms | none; next tick retries |
| `QueueSelect` / `QueueStamp` | 100 ms | retry next tick; receipts never dropped |
| `WorkerHealthProbe` | 100 ms | 5 → 60 s (cas-7156) |
| `ReminderScan` | 100 ms | next tick |
| `RevalidateFresh` | 250 ms | none; the row stays pending |
| `GitFetch` / `RefTip` | 5 s / 500 ms | per ref, exponential (cas-a222) |
| `CiWatch` / `CiStatus` | 10 s | exponential |
| `VioletPoll` / `CloudRelay` / `MirrorWrite` | 2 s | exponential |

### 4.5 Prompt delivery: in-flight rows and crash safety

Delivery crosses three lanes: `QueueSelect` (jobs) → `InjectPrompt` (io) →
`InjectOutcome` (io → control) → `QueueStamp` (jobs). Without a guard, the
next `QueueSelect` 100 ms later could pick the same row before its stamp
lands and inject it twice. The rules:

1. **Claim at selection.** `QueueSelect` takes the per-recipient transport
   claim atomically in the same transaction that selects the row, as
   `claim_recipient_transport` does today. A claimed row is not selectable
   again.
2. **In-flight set on control.** Control records `(row, recipient,
   generation)` in an in-flight set when it sends `InjectPrompt`, and skips
   any selected row already in the set. The entry clears when the
   `QueueStamp` for that generation completes.
3. **Outcome timeout.** If no `InjectOutcome` arrives within 10 s, control
   submits a `QueueStamp` that releases the claim (today's
   `release_recipient_transport`), and the row is retried on its normal
   cadence.
4. **Idempotent stamps.** Stage stamps are already monotonic
   (`atomic_stage_stamp_in_tx`); a repeated stamp for the same generation is
   a no-op.
5. **Crash between inject and stamp.** After a daemon restart, a row that is
   claimed but not stamped is treated as today: it stays eligible for the
   recipient's hook and tool-boundary surfacing (cas-b5ad, cas-7bfa), Teams
   re-offers are content-deduped, and the daemon re-offers it by PTY only
   after the claim is older than the outcome timeout. A crash can therefore
   cause at most one duplicate PTY injection of that row; normal operation
   never injects twice.

## 5. Render scheduler (io lane)

GH #1210 measured ~30 output-driven frames/s × ~16 ms each in Konsole for a
factory with codex workers; cost is per frame, not per byte. v2 caps how
often output produces a frame and skips panes that aren't visible.

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

### 5.2 No separate damage tracking

ratatui already diffs the previous buffer against the new one and emits only
the changed cells, so v2 adds no damage tracking of its own. The frame cap
(§5.1) and the visibility rule are what cut the frame count. A frame whose
diff is empty is not sent.

### 5.3 Config and metrics

- `[factory] render_max_fps` (default 15) and `[factory] render_unfocused_fps`
  (default 5), via `cas config`.
- `loop.json` gains `frames` per cause per window (`input`, `layout`,
  `output_focused`, `output_unfocused`), `frames_coalesced`, and
  `frame_bytes_p50`.

## 6. Watchdog

Today's pieces (§2.7): `LoopProgress` per-phase windows, the
`cas-loop-watchdog` thread writing `loop.json` every 5 s, `loop_thread_wait`
after a 60 s wedge, and `cas::refresh_profile` logs for director steps
≥ 50 ms. They have three gaps: the prompt-queue step has no sub-steps, the
wedge threshold is 60 s while a 1 s stall is already felt, and nothing shows
in the TUI. v2 extends these pieces rather than replacing them.

1. **Step markers.** Each lane sets an atomic "current step" marker (a
   `&'static str`) before every step. Every step in §2 has a name.
2. **Budgets.** io iteration 5 ms; control step budgets: prompt-queue
   decisions 20 ms, snapshot apply 10 ms, every other step 10 ms; job budgets
   per §4.4.
3. **Over budget.** The lane records `{lane, step, elapsed_ms, budget_ms, at}`
   in a ring of the last 32 events, logs
   `cas::loop_watchdog lane=<l> step=<s> elapsed_ms budget_ms` at WARN, and
   the TUI shows `slow: <lane>/<step> <n> ms` in the status line for 10 s.
4. **Wedge.** The existing watchdog thread checks each lane's heartbeat every
   250 ms (today 5 s). A lane that has not finished an iteration in 1 s
   (today 60 s) is wedged: the
   TUI shows `wedged: <lane>/<step> for <n> s`, and `loop.json` records the
   step, the duration and `/proc/<tid>/wchan` (today's `loop_thread_wait`).
   *C1 status (cas-6931):* until the io lane runs on its own thread (C4b),
   the loop draws the status line itself, so it can never draw its own
   wedge; by the time it draws, the wedge is over and the stalled step shows
   as `slow:`. C1 therefore shows `wedged:` live only for jobs, and a live io
   or control wedge is visible in `loop.json` (written within one heartbeat)
   and `worker_status`. Live TUI display of a loop wedge waits for C4b or a
   client-side "daemon silent" signal. The `slow:` badge holds the worst
   step of its 10 s window and changes at most once a second, since every
   change is a redraw.
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

Wall-clock p99 budgets flake on a shared CI host with four builders, so tests
come in two kinds:

- **Gate** tests decide merges. They use a fake clock (or `tokio::time::pause`)
  and synthetic stalls, or wall-clock margins wide enough that load cannot
  flip them (a 2 s stall against a 50 ms budget).
- **Receipt** tests measure the real p99 budgets with the load harness
  (cas-b8a9's, cas-98b24) on a reference host, and are attached to the
  child's close as a receipt. They do not gate CI.

The load harness drives 16 agents plus **synthetic PTY writers** that
reproduce GH #1210's rates (38, 63 and 236 writes/s), not real codex agents,
so runs are repeatable.

| Test | Budget | Kind | Owner |
|---|---|---|---|
| `flv2_job_single_flight_and_generations` | a second submit for an in-flight key coalesces; a stale generation's result is dropped | gate (fake clock) | C0 |
| `flv2_job_queue_never_blocks_submitter` | a full non-critical queue drops the oldest refresh; the submitter returns at once | gate | C0 |
| `flv2_watchdog_names_slow_step` | a stub step over budget appears in the ring, the log and the status line with lane and step | gate (fake clock) | C1 |
| `flv2_wedge_reports_step_and_duration` | a lane blocked 1.5 s is reported as wedged with its step | gate | C1 |
| `flv2_no_blocking_call_on_lanes` | the §6.6 assertion trips on any store open, `Command`, `/proc` or transcript read on io or control | gate (report in C1, enforce in C6) | C1, C6 |
| `flv2_director_snapshot_off_lane` | a 2 s `DirectorSnapshot` job leaves every control pass < 100 ms | gate (2 s vs 100 ms margin) | C2 |
| `flv2_worker_health_probe_single_flight` | N passes over a stuck worker run one probe per backoff step | gate (fake clock) | C2 |
| `flv2_retraction_reads_fresh` | a retraction decision issues a fresh read; a stale cached snapshot that would retract a legitimate row does not | gate | C2, C3 |
| `flv2_external_reminders_never_on_lane` | regression for GH #1208 with ≥ 5 external reminders | gate (exists: loop_latency_tests.rs:372) | C2 |
| `flv2_queue_stamps_on_writer` | queue selection and stamps run as jobs; spec-6194 `ct2_` tests still pass | gate | C3 |
| `flv2_no_double_inject` | a row selected twice before its stamp lands is injected once; an outcome timeout releases the claim | gate (fake clock) | C3 |
| `flv2_inject_is_a_command` | an urgent inject with a 1.2 s settle leaves every control pass < 100 ms | gate (1.2 s vs 100 ms margin) | C4a |
| `flv2_pty_write_never_blocks_lane` | a pane whose PTY buffer is full stops taking input; other panes' echo is unaffected | gate | C4a |
| `flv2_echo_unaffected_by_stalled_control_lane` | a 2 s control stall leaves key → echo < 50 ms | gate (2 s vs 50 ms margin) | C4b |
| `flv2_echo_unaffected_by_held_store_lock` | `task-sync-intents.lock` held 3 s (GH #1165); echo < 50 ms | gate | C4b |
| `flv2_output_frames_capped` | a pane at 236 writes/s yields ≤ `render_max_fps` + 1 output frames/s | gate (fake clock) | C5 |
| `flv2_unfocused_frames_capped` | unfocused visible pane ≤ `render_unfocused_fps` + 1 | gate (fake clock) | C5 |
| `flv2_invisible_pane_output_sends_no_frame` | output from a pane off screen sends 0 frames | gate | C5 |
| `flv2_input_frame_precedes_output_frame` | the echo frame is flushed before a pending output frame | gate | C5 |
| `flv2_control_pass_p99_under_100ms_with_16_agents` | control pass p99 < 100 ms | receipt | C2 + C3 |
| `flv2_key_to_echo_p99_under_50ms_under_load` | p99 key → echo < 50 ms with 16 agents and synthetic writers | receipt | C6 (and C4b if built) |

## 8. Migration order and shared files

| Child | Scope | Main files | Depends on |
|---|---|---|---|
| C0 job framework | `Job`/`JobResult`, completion queue, single-flight, generations, bounded queues, reader vs writer routing, per-kind budgets | `store_worker.rs`, a new `jobs.rs` | none |
| C1 watchdog | step markers (including prompt-queue sub-steps), budgets, ring, TUI status, 250 ms heartbeat and 1 s wedge, job-age metrics, `ON_LANE` in report mode, `loop.json` fields | `loop_watchdog.rs`, `factory_daemon_health.rs`, `lifecycle.rs` (markers only), app `director_refresh.rs` (`timed()`), status line | none; with C0 lands first so every later child is measured |
| C2 off-lane director | `DirectorSnapshot`, `WorkerHealthProbe`, `ReminderScan`, `RevalidateFresh`, merge-freshness `GitFetch`/`RefTip`, CI/Violet/cloud jobs | app `director_refresh.rs`, app/mod.rs, `events.rs`, `prompts.rs`, `wedged.rs`, `worker_liveness.rs`, `git_evidence`, `ci_watch.rs`, `worktree_ops.rs`, `queue_and_events.rs` (reminders at QE:9203, relays at QE:2609–2804) | C0, C1 |
| C3 off-lane queue | `QueueSelect` with claim, `QueueStamp`, in-flight set (§4.5), cached probes and `Config`, `InboxWrite`/`InboxPrune`, `RefTip`, mirror and sweep jobs; control keeps decisions | `queue_and_events.rs` (`process_prompt_queue`, probes, sweep, prunes), `delivery.rs`, `teams.rs`, `prompt_revalidation.rs`, `commander_mirror.rs`, `prompt_queue_store.rs` (APIs only) | C0, C1; parallel with C2 |
| C4a input and inject on the current loop | input routing table (§3.3), non-blocking PTY writes with per-pane buffers, urgent/context-reset inject as a per-pane state machine (no inline await), keystrokes wake the loop | `client_input.rs`, `cas-pty` write path, `cas-mux` `mux.rs`/pane, `delivery.rs` | C3 (shares `delivery.rs`) |
| C5 render scheduler | frame causes and caps (§5.1), visibility rule, config keys, frame metrics; absorbs cas-77f39 (GH #1210) | `lifecycle.rs` draw/flush, app render, config meta/seed | C1; can land before C4b |
| C4b io thread (conditional) | the io lane on its own thread: panes move out of `FactoryApp`, `ViewModel`, channels, other clients (§3.2–3.5) | `lifecycle.rs` split, `terminal_exchange.rs`, `output.rs`, app render split, GUI/WS clients | C2, C3, C4a, C5 — **and evidence** |
| C6 enforcement and receipts | `ON_LANE` enforce mode; harness with synthetic writers; receipts for the §7 p99 budgets | tests, harness, CI | C2–C5 (and C4b if built) |

**C4b is conditional (rule-176: remove before adding).** Measured input, PTY
and render cost is 0–3 ms (§1.1). If, after C2, C3, C4a and C5, the control
pass p99 stays under 100 ms in the C6 receipt, key → echo already meets its
budget and C4b waits for evidence that it is needed. C4b is cut only if the
receipt misses the echo budget.

Order: C0 and C1 → C2 ∥ C3 → C4a and C5 → C6 → C4b only if needed.

Shared hotspots, landed in the order above with rebases on merged tips:

| File | Children |
|---|---|
| `lifecycle.rs` | C1, C5, C4b |
| `queue_and_events.rs` | C2 (reminders, relays), C3 |
| app `director_refresh.rs` | C1, C2 |
| `delivery.rs` | C3, C4a |
| `store_worker.rs` | C0 (C2 and C3 only use its API) |

cas-77f39 (GH #1210) can still ship its point fix (an output-frame cap in the
current loop) before C5; C5 then replaces it.

## 9. Open questions for review

1. **Thread or async for the io lane.** This spec asks for a dedicated OS
   thread with its own epoll set, so it never shares a tokio worker with
   jobs. A separate single-threaded tokio runtime would also satisfy §3;
   C4b, if built, chooses and records why.
2. **Revalidation cost.** Revalidation and retraction stay fresh reads
   (§4.3, app/mod.rs:1541–1547), now as `RevalidateFresh` jobs. A row waits
   for its job (≤ 250 ms budget) before injection; confirm that delay is
   acceptable for urgent rows.
3. **Urgent-inject readiness.** Today's 1.2 s settle plus up to 4 s polling
   exists for cas-eacc (operator drafts). As an io-lane state machine it
   blocks only that pane; confirm no caller relies on the loop pausing.
4. **Load harness.** cas-98b24's harness drives 16 agents; C6 adds the
   synthetic PTY writers at GH #1210's rates and records receipts on a
   reference host.

## 10. Out of scope

- Cgroup isolation of worker builds (GH #1213). This spec makes echo
  independent of a stalled control lane, but not of the io thread being
  paged out; #1213 handles memory.
- Konsole's own paint cost (fixed per frame; the cap in §5 bounds it).
- spec-6194 semantics. They move to jobs unchanged.
