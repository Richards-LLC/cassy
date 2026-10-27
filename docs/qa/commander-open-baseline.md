# Commander conversation opening baseline — 2026-10-27

Opening the real long supervisor conversation starts two independent inputs:
one bounded history page **and hundreds of pending live replies**. Measuring
only the page misses the operator's complaint. The live backlog grows the
conversation from 49 bubbles to 324 and repeatedly moves an older reply into
the bottom position. Warm reopening also rebuilds the accumulated view.
No product code was changed for this investigation.

## Reproduce

From `hub-web`, after `npm ci`, run this single finite command:

```bash
node scripts/conversation-open-perf.mjs --db "<project>/.cas/cas.db" --session "<session>" --output "<task-artifacts>/rerun" --flow backlog --repeats 1
```

For the identical frozen input, replace `--db ... --session ...` with
`--capture "<task-artifacts>/final/private-history.json"`.
Replace the angle-bracket placeholders with the project, session, artifact directory and candidate build for your machine. Use a new output directory per run. `--flow history` measures the bounded
page and repeated explicit Load earlier separately, without the live backlog.
`--repeats 3` produces median/min/max summaries. The measurement does not
assert that hundreds of bubbles must remain, so the fix can reuse it.

The command reads only selected prompt-queue columns with SQLite `mode=ro`,
builds the production bundle with source maps, starts a finite local asset
server, runs one named Playwright spec with one worker, and closes its owned
server/browser. It never pairs a live device, reads existing credentials,
sends a supervisor message, changes queue state, or starts a model provider.
The private capture contains real conversation text and stays outside Git
and the published evidence bundle.

## Scope and measurement definitions

The supervisor approved this Soundwave/Cassy thread: `cassy-quiet-panther-51`,
3,419 queue rows, **354 Commander turns**, including **327 pending operator
replies**. Internal worker traffic is excluded by the store's Commander
history predicate. Page size is 50, with up to 30 other-session turns beside
the first page. Short status coalescing means turn count and bubble count differ.

This is **real production bundle + real supervisor data + protocol replay**.
The replay preserves the daemon's batches of ten, priority/id ordering,
individual live frames, history/live optional-field differences and the
effect of browser persistence ACKs on remaining pending rows. Batches are
paced by two browser animation frames, not a claimed daemon poll interval.
It is a fast-delivery rendering baseline, not real hub/network latency.

Desktop is 1440×900 at native CPU speed; phone is 390×844 with touch/mobile
context and Chrome CDP 4× CPU throttling. This is not a physical-phone test.
The host and browser version are in `summary.json`/each receipt. Host load
and profiling affect timings; the delivery has one sample per profile/flow,
so no percentile or population claim follows from it.

First paint is the first visible bubble after a two-rAF presentation
opportunity, timed from the actual list click. It does **not** mean that the
stream is finished. Completion/input is the end of the supplied backlog plus
an unsent composer edit acknowledged through two rAFs. It is an operational
interactive upper bound, not Lighthouse TTI. Warm means Back → same list row,
keeping the in-memory history. Reconnect closes/reopens the session socket
and replays still-unacknowledged replies. Reload/resume replaces the document
with IndexedDB retained, exercising discarded-tab restoration; it does not
claim to reproduce an Android/iOS suspension.

## Results

| Flow | First visible turn, ms | Final paint opportunity, ms | Completion/input, ms | History requests / live reply frames | WS payload bytes | HTTP encoded bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| desktop: Cold list open + pending drain | 246 | 8498 | 8682 | 1 / 282 | 299,739 | 1,319 |
| phone: Cold list open + pending drain | 495 | 36799 | 36829 | 1 / 323 | 335,897 | 5,681 |
| phone: Warm list reopen | 753 | 857 | 870 | 0 / 0 | 0 | 498 |
| phone: Socket reconnect + pending drain | 113 | 19340 | 19371 | 1 / 296 | 314,516 | 2,657 |
| phone: Reload/resume + pending drain | 474 | 58023 | 58298 | 1 / 277 | 299,756 | 877,677 |

| Flow | Script / layout / style, ms | Instrumented parse, ms | Long tasks: count / total / max, ms | Live elements / CDP nodes | Bubbles | JS heap, MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| desktop: initial | 1911 / 4462 / 579 | 22.3 | 23 / 1394 / 89 | 3,740 / 9,335 | 324 | 7.6 |
| phone: initial | 8526 / 18187 / 2304 | 86.1 | 220 / 28007 / 286 | 3,780 / 7,877 | 324 | 19.9 |
| phone: warm_reopen | 165 / 488 / 58 | 0.0 | 1 / 711 / 711 | 3,780 / 22,808 | 324 | 21.1 |
| phone: reconnect | 10778 / 579 / 563 | 3.0 | 178 / 11402 / 95 | 3,755 / 8,693 | 324 | 11.8 |
| phone: reload_resume | 12793 / 27246 / 3435 | 3.3 | 270 / 49868 / 1191 | 3,553 / 7,678 | 307 | 13.2 |

HTTP encoded bytes come from Chrome `ResourceFinish` events, including
headers; WebSocket bytes are UTF-8 application payloads. TLS and WebSocket
framing overhead are excluded. Reload includes asset loading, unlike a tap
from the already-loaded list. The number of backlog frames can differ by
viewport because storage ACKs race the next batch, as in the real path.

JS heap is an instantaneous reading without forced GC. Live document element
count is separate from CDP Nodes, which includes retained/detached nodes.
Long tasks are main-thread tasks of at least 50 ms. Instrumented JSON parse
counts inputs larger than 1 KiB and is included in script time; do not add it
again. CDP script/layout/style durations and sampled source costs are different
views of the same work and must not be summed together.

## Where the time goes

| Cost or mechanism | Confirming code |
| --- | --- |
| Latest history page is bounded, not automatically drained | `hub-web/src/connection.ts:1740` clamps requests to 50; initial attach requests one page at `:1913`; `hub-web/src/main.ts:464` requests older pages only through Load earlier. |
| Daemon/store history selection | `cas-cli/src/ui/factory/protocol.rs:18` defines 50, `:21` defines 30 cross-session rows; `daemon/runtime/queue_and_events.rs:7400` requests limit+1; `crates/cas-store/src/prompt_queue_store.rs:8339` filters session/id and limits the query. |
| The missing stream: unprocessed replies bypass the history-page limit | `crates/cas-store/src/prompt_queue_store.rs:8311` selects pending operator rows, ordered by priority then id; `cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs:2779` selects ten/batch; `:7520` iterates them and `:7542` builds each live OperatorReply. `:7455` clears reply retry deferrals on a newest-page request. |
| Every frame triggers whole-view work | `hub-web/src/main.ts:900` receives each live reply; `:907` calls `updateConversationViews`; `:215` updates every retained view and persists arrival state. Journal success calls it again at `:3404`; failure also calls it at `:3411`. |
| Older backlog replies are appended as current live traffic | `hub-web/src/conversation-history.ts:823` receives using browser time and `:829` inserts with time placement; it lacks the durable placement used by history. Numeric tail samples show the bottom changing from reply 4207707 to older reply 4204926, eventually 4207073, while bubble count increases. |
| Layout/DOM work dominates cold drain and warm reopen | `hub-web/src/conversation-view.ts:619` derives the entire model, `:635` visits every item, `:1226` replaces every group's children even when keyed bubbles survive. Anchor reads at `:658` and `:734` follow writes and force layout. The sampled call stacks include native getBoundingClientRect under update/readingAnchor. |
| Whole-history scans grow with retained turns | `hub-web/src/conversation-history.ts:459` searches while hydrating sends, `:800` dedupes replies; `:556` waiting scans history and can scan its suffix for each blocker. `conversation-view.ts:984` invokes those scans while making turn signatures. `thread-model.ts:189` derives the entire model on each update. |
| Markdown contributes but is not the dominant cost | `hub-web/src/conversation-view.ts:1641` calls `markdown-renderer.ts:255`. Keyed bubble/signature reuse already exists at `conversation-view.ts:1205`; a proposed cache must preserve it, not assume all Markdown is reparsed on every update. |
| Retained browser replies can bypass the page bound too | `hub-web/src/commander-journal.ts:155` reads all reply rows, bounded to 400 at `:543`; `hub-web/src/main.ts:3371` restores every retained reply and `:3292` rehydrates them on journal invalidation. This capture's failed live writes retain only a small subset on disk; the in-memory warm view still has hundreds. |
| ConversationStore is metadata, not the paged history cache | `hub-web/src/conversation-store.ts:58` reads/serializes stored per-thread state; `:99` parses its namespace. `hub-web/src/main.ts:2617` persists arrival records during view updates. Source samples quantify it separately. |

The full queue-and-events path above is
`cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs`.
Read-only SQL page queries on the real store took less than 1 ms each; their
individual times are in each receipt. That excludes Rust conversion,
serialization, store contention and actual relay/network latency. Replay
request-to-response gaps are recorded separately in `replay_request_to_response_ms`.

Sampled inclusive source costs (ms; each column overlaps its callers):

| Flow | View | History scans | Thread model | Markdown | ConversationStore | Journal |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| desktop: initial | 6239 | 385 | 187 | 59 | 124 | 8 |
| phone: initial | 27784 | 2004 | 973 | 238 | 457 | 17 |
| phone: warm_reopen | 656 | 30 | 12 | 27 | 0 | 0 |
| phone: reconnect | 11316 | 4114 | 1452 | 204 | 6 | 14 |
| phone: reload_resume | 39424 | 1405 | 1758 | 343 | 16 | 231 |

The replay also found a concrete persistence defect, filed separately as
`cas-c641`: live `OperatorReply.operator_label` serializes `None` as null
(`cas-cli/src/ui/factory/protocol.rs:513`), whereas history skips the absent
label (`:230`). `hub-web/src/commander-journal.ts:68` rejects null. Then
`main.ts:3398` fails before the persistence ACK at `:3403`. Unacknowledged
old replies remain eligible to stream again. ACK totals in the receipts are
direct observations, not proof that null explains every failed write; the
throttled run also exposes fewer successful history writes than desktop.
Storage pressure/timeouts were not isolated in this measurement-only task.

## Proposed budgets and implementation order

| Gate, on the same real capture | Desktop | 4× phone |
| --- | --- | --- |
| Latest valid turn painted after list tap, cold and warm | ≤500 ms | ≤1,000 ms |
| Composer acknowledged after tap; independent of backlog completion | ≤500 ms | ≤1,000 ms |
| Longest individual main-thread task during open/catch-up | ≤50 ms | ≤100 ms |
| Appending an older page while preserving the reading anchor | ≤100 ms | ≤200 ms |
| Bubble DOM after arbitrarily many pages/streamed replies | ≤100, plus spacers | ≤100, plus spacers |
| Live document elements after catch-up | ≤2,000 | ≤2,000 |
| Retained JS heap after catch-up, measured after controlled GC in future comparison | ≤25 MiB | ≤25 MiB |

These are proposed acceptance budgets, not achieved results. Verify them over
at least 30 runs under comparable host load before making a p95 claim. Keep
history requests to one on initial open; byte cost should follow the latest
50 turns plus bounded metadata, not every older message body. Separately
measure actual hub transport before promising a real-network end-to-end SLA.

1. Correct the null optional-field/persistence-ACK contract without weakening
   credential fencing or acknowledging a reply before durable storage. Keep
   transport delivery and operator-read state separate.
2. Treat catch-up replies older than the current durable window as archive
   updates, not newly spoken bubbles. Keep receipt processing running, place
   turns by durable sequence, and avoid replacing the newest viewport with
   older live replay. Do not discard pending private replies just to be fast.
3. Apply the latest-first window to **every input**: initial page, live pending
   stream, in-memory reopen, journal restoration and inbox replay. The current
   50-row request limit already exists; adding pagination alone misses the bug.
4. Batch persistence-driven UI invalidations into one frame and update only
   views whose visible state changed. Read layout before DOM writes, then
   restore one anchor. Keep older pages on explicit scroll-up demand with one
   in-flight request and spacers/windowed DOM.
5. Preserve keyed Markdown nodes and selection/focus, cap any rendered-turn
   cache, and maintain incremental indexes for durable ids and unanswered
   asks/blockers. Reuse the same perf command and frozen capture to prove the
   change across cold, warm, reconnect and reload.

No implementation or Rust build is part of this delivery. Canonical HUB-J4
and other regression journeys are **NOT EXERCISED** by this measurement-only
change; the custom production-bundle opening/recovery paths have their own
Playwright receipts. Existing product source and bundle are unchanged in Git.

## Replay-persistence fix comparison

The supervisor requested a comparison against `b7eeeaa33` on
`factory/true-viper-13`, which canonicalizes a null operator label before
journal validation. That candidate changes only journal production code and
its existing test relative to the baseline product source. Its `hub-web`
tree and `docs/design/design-tokens.json` were extracted with `git archive`
into ignored `target/cas-d1b1-fixed`, then built using
`npm run build -- --sourcemap`. Initial archive build failed because the
design-token input was absent; the complete-input build passed. The candidate
source never replaced this worktree's product files.

From `hub-web`, the fixed replay command is:

```bash
node scripts/conversation-open-perf.mjs --capture "<task-artifacts>/final/private-history.json" --output "<task-artifacts>/fixed" --flow backlog --repeats 1 --dist "<candidate-build>/hub-web/dist" --bundle-revision b7eeeaa33
```

For persistence acceptance, add `--require-all-acked`: it asserts that every frozen pending notification ID was durably acknowledged before reconnect, rather than treating cumulative ACK counts as identity coverage. Each phase records expected/acknowledged/remaining IDs, replayed pending IDs and a timestamped ACK timeline. Baseline runs omit that flag so failed persistence can still be measured.

The supplied `--dist` must be built with source maps from the stated revision.
The probe revision and bundle revision are recorded separately in the receipt.
The frozen private capture is identical for baseline and candidate. The original
exploratory phone drain was **47.3 s**; the matched corrected-capture baseline
above is used for the comparison rather than treating that earlier single
sample as a stable median. No physical network timing is inferred.

| Flow | Matched baseline input, ms | Fixed input, ms | Baseline / fixed ACK totals | Baseline / fixed live reply frames | Fixed bubbles |
| --- | ---: | ---: | ---: | ---: | ---: |
| desktop: initial | 8682 | 7358 | 46 / 157 | 282 / 283 | 324 |
| phone: initial | 36829 | 47193 | 5 / 69 | 323 / 321 | 324 |
| phone: warm_reopen | 870 | 763 | 5 / 69 | 0 / 0 | 324 |
| phone: reconnect | 19371 | 19581 | 36 / 137 | 296 / 256 | 324 |
| phone: reload_resume | 58298 | 36018 | 41 / 156 | 277 / 199 | 307 |

ACK totals are cumulative observations at each phase, not unique-id counts or proof that all replies persisted. The null fix increases successful ACKs, but cold phone input is still **47.19 s**, essentially the original exploratory **47.3 s** experience. It does not meet the opening budget. The matched baseline/candidate runs each passed both named profile cases, but host one-minute load was 10.12 versus 20.53 on the same 32-logical-CPU machine, so these single samples do not establish a speedup/regression percentage. In particular, reconnect remains about 19.5 s, while hundreds of duplicate old frames still trigger view work. Remaining durable-write failures and transaction pressure need their own isolation; rendering/windowing remains required.

## Evidence

All paths below are relative to the assigned artifact root:
`/home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-d1b1/`.

- `delivery.log`: named Playwright result; `delivery/summary.json`: machine and numeric summary.
- `delivery/desktop-0/receipt.json`, `delivery/phone-0/receipt.json`: per-phase measurements, ACK counts, tail timeline, and source-mapped CPU samples.
- `delivery/{desktop-0,phone-0}/*.trace.json`: Chrome Performance traces; `*.cpuprofile`: CPU profiles. Phone has initial, warm_reopen, reconnect and reload_resume phases.
- `fixed.log`, `fixed/summary.json`, `fixed/{desktop-0,phone-0}/receipt.json`, traces and CPU profiles: candidate comparison. `fixed-build-complete.log` records its successful source-map build.
- `final/private-history.json`: private frozen input, excluded from publication.
- `typecheck-candidate-probe.log`: final probe TypeScript checks. Earlier `pilot*`, `baseline`, `measured`, `backlog-pilot`, and `final` directories retain exploratory/failure controls; they are not the delivered baseline.

`evidence.tar.gz` publishes the delivery receipts, traces, source map and this
report without conversation bodies. There are no trace DOM snapshots or real
authentication headers. Design voice: `DESIGN.md@b43940e43`, inheriting
`hub-web/DESIGN.md`.
