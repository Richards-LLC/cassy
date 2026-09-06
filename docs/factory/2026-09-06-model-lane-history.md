# Model lane history — measured per delivery, 2026-08-20 → 2026-09-06

Companion table to `2026-09-06-model-lane-rubric-review.md` (PART B of task cas-3372). Row-level data:
`2026-09-06-model-lane-history.csv` (288 rows: 280 worker task-segments, 8 supervisor sessions).
Retention horizon on this host starts 2026-08-20 (older Codex rollouts and Claude transcripts were purged).

## Definitions

- **Delivery** = one worker working one task in one session (a "task segment": the rollout between one
  `task start` call and the next). A task sent back and resumed by a new worker is two segments.
- **Send-back** = a `Decision: changes requested by supervisor` note on the task (the `request_changes`
  action, sanctioned since 2026-08-04). Counted once per task, on its first segment. CI reds fixed
  without a request_changes decision are not send-backs here, so the 2026-09-05 supervisor brief (which
  counted CI reds and message-only rejections: 5 on 19) reads higher than this table (3 on 20).
- **Urgent stops** = factory-log `coordination_message` events with `urgent=true` targeted at a worker
  name that appears in the lane's rows.
- **Cached input** = Codex `cached_input_tokens` (a subset of `input_tokens`); for Claude, cache-read
  plus cache-write tokens. **Uncached input** = the remainder.
- **Cost @ list** = shadow price at the vendor's Standard-tier list price on 2026-09-06 (see the rubric
  review PART A for citations); Claude cache writes priced at the 1-hour TTL rate because every cache
  write in the transcripts is `ephemeral_1h` (Opus 5 $10/M, Fable 5.1 $20/M). Both harnesses actually run on subscriptions (Codex account, Claude
  Max), so this is the price the work *would* carry on the API, not money spent.
- **Min to first push** = minutes from the `task start` call to the first `git push` tool call in the
  segment. Spike/no-code tasks without a push are excluded from the median.

## Per model × effort, whole horizon (workers only)

| Model / effort (harness) | Deliveries | Workers | Closed | Send-backs | Send-back rate | Urgent stops | Uncached input / delivery | Cached input / delivery | Output / delivery | Tool calls / delivery | Median min to first push | Cost / delivery @ list |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| gpt-5.6-luna / xhigh (codex) | 173 | 111 | 171 | 16 | 9% | 4 | 416,114 | 18,359,369 | 47,512 | 143 | 18.5 | **$0.51** |
| gpt-5.6-sol / high (codex) | 12 | 10 | 12 | 1 | 8% | 0 | 366,598 | 16,434,624 | 38,118 | 125 | 6.9 | **$8.80** |
| gpt-5.6-terra / high (codex) | 42 | 34 | 41 | 1 | 2% | 0 | 199,407 | 8,134,254 | 21,041 | 69 | 6.2 | **$2.28** |
| claude-opus-5 / high (claude) | 38 | 21 | 37 | 0 | 0% | 2 | 303 | 41,779,207 | 102,080 | 78 | 16.2 | **$26.52** |
| gpt-6-astra (any) | 0 | 0 | — | — | — | — | — | — | — | — | — | — |
| claude-haiku-4-5 (any) | 0 | 0 | — | — | — | — | — | — | — | — | — | — |
| claude-fable-5-1 as worker | 0 | 0 | — | — | — | — | — | — | — | — | — | — |

Luna distribution: output p50 45,076 / p90 74,368 / max 160,081 tokens; cost p50 $0.46 / p90 $0.87 / max $1.69.
Opus 5 distribution: output p50 85,551 tokens; cost p50 $19.82. Horizon totals at list: Luna $87.78 for
173 deliveries; Opus 5 $1,007.69 for 38; Sol $105.63 for 12; Terra $95.68 for 42.

Astra has **zero** cas-src worker or supervisor rollouts in the retained horizon. The only Astra session on
the host is a 2 h Penguinz session (2026-09-05 01:05Z, medium: 445K uncached in, 22.5M cached, 123K out,
205 tool calls). The spawn queue holds two Astra spawn requests (one high, one medium) with no matching
cas-src rollout. Haiku and Fable-as-worker were never spawned.

## Per period (rubric in force), workers only

Periods follow memory 2026-08-27-6 and the registry history: Terra/high was the standard route until
its 2026-08-27 suspension (in this horizon it only appears on 2026-08-20); Luna/xhigh standard with
Sol/high heavy from 2026-08-25; a Claude Opus 5/high burst on 2026-09-03→04 (taste route was Opus
5/high until 2026-09-04, then Astra/medium, then Fable 5.1/medium from 2026-09-05); Fable 5.1 as
supervisor from 2026-09-05 22:00Z.

| Period | Model / effort | Deliveries | Closed | Send-backs | Output / delivery | Tool calls / delivery | Median min to first push | Cost / delivery @ list |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| P1 08-20 (Terra standard) | gpt-5.6-terra / high | 42 | 41 | 1 | 21,041 | 69 | 6.2 | $2.28 |
| P1 08-20 | gpt-5.6-sol / high | 3 | 3 | 0 | 16,389 | 45 | 2.7 | $2.64 |
| P2 08-25→09-02 (Luna standard, Sol heavy) | gpt-5.6-luna / xhigh | 149 | 148 | 13 | 48,077 | 144 | 18.2 | $0.51 |
| P2 | gpt-5.6-sol / high | 7 | 7 | 0 | 42,679 | 138 | 12.3 | $10.03 |
| P2 | claude-opus-5 / high | 2 | 2 | 0 | 206,138 | 103 | 26.8 | $62.44 |
| P3 09-03→09-04 (Opus 5 burst) | claude-opus-5 / high | 36 | 35 | 0 | 96,300 | 77 | 16.2 | $24.52 |
| P3 | gpt-5.6-luna / xhigh | 4 | 4 | 0 | 41,436 | 144 | 19.5 | $0.48 |
| P3 | gpt-5.6-sol / high | 1 | 1 | 0 | 49,842 | 176 | 5.0 | $11.07 |
| P4 09-05 16:20Z→ (rubric-review window) | gpt-5.6-luna / xhigh | 20 | 19 | 3 | 44,525 | 138 | 18.9 | $0.50 |
| P4 | gpt-5.6-sol / high | 1 | 1 | 1 | 59,648 | 224 | 51.3 | $16.39 |

The Opus 5 burst's 0 send-backs on 36 deliveries is a recorded fact, not necessarily a quality fact:
the 2026-09-03→04 supervisor session (Fable 5.1/high, 976K output tokens) may have rejected work by
message instead of `request_changes`; the factory log shows 2 urgent stops on Opus workers in that window.

## Supervisor sessions (Claude, main checkout)

Every supervisor session in the horizon ran at **effort=high** — including all three Fable 5.1
sessions — while the registry's supervisor and taste lanes say `medium`. Claude Code's default is
high; nothing in the spawn path sets medium for the supervisor. The 2026-08-27 → 09-01 sessions ran
Fable 5 (price not researched, cost left unknown).

| Session start | Model / effort | Active minutes | Uncached input | Cache write | Cache read | Output | Tool calls | Cost @ list |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| 2026-08-27 12:47Z | claude-fable-5 / high | 427 | 970 | 1,209,737 | 176,702,744 | 241,041 | 153 | unknown |
| 2026-08-29 16:00Z | claude-fable-5 / high | 356 | 994 | 1,075,366 | 203,533,979 | 280,017 | 135 | unknown |
| 2026-08-30 15:38Z | claude-fable-5 / high | 129 | 536 | 724,349 | 68,202,993 | 139,297 | 77 | unknown |
| 2026-08-30 23:21Z | claude-fable-5 / high | 824 | 54,411 | 2,556,768 | 255,445,740 | 395,008 | 33 | unknown |
| 2026-08-31 13:34Z | claude-fable-5 / high | 274 | 11,119 | 1,203,403 | 64,611,429 | 131,645 | 26 | unknown |
| 2026-09-02 00:32Z | claude-fable-5-1 / high | 1,011 | 42,709 | 2,586,733 | 281,569,004 | 387,535 | 70 | $141.93 |
| 2026-09-03 17:00Z | claude-fable-5-1 / high | 1,001 | 92,272 | 4,619,897 | 568,043,654 | 976,282 | 150 | $284.15 |
| 2026-09-05 16:20Z | claude-fable-5-1 / high | 1,470 | 8,960 | 3,463,853 | 191,707,518 | 289,686 | 89 | $131.78 |

The 2026-09-05 supervisor day at list ($131.78) costs thirteen times the 20 Luna deliveries it reviewed
($10.05 in total). Every Claude cache write in the horizon is a 1-hour-TTL write (`ephemeral_1h_input_tokens`;
zero 5-minute writes), priced at $20/M for Fable 5.1: cache writes are 53% of that session's figure, cache
reads 36% even at Fable's 0.025× read price, output 11%.

## What each source could and could not supply

| Source | Supplied | Could not supply |
|---|---|---|
| `~/.codex/sessions/2026/08/20…09/06/*.jsonl` (424 files, 179 in cas-src) | model and `effort` per turn (`turn_context`), per-turn `input/cached_input/output/reasoning_output` tokens (`token_count` events), every tool call with arguments, timestamps, compactions | lane name (inferred from model×effort), merge outcome (from task notes), Astra data (none exist) |
| `~/.claude-daniel@petrastella.io/projects/-home-pippenz-Petrastella-cas-src*/` (35 transcripts ≥200 KB since 08-20) | model, `effort` per record, `usage` per assistant message (input, cache_creation, cache_read, output), tool_use blocks | reasoning tokens (not split out), `effort` for the 32 spawn-queued Opus workers is inferred from the transcript field (all high) |
| `.cas/cas.db` `tasks.notes` | send-backs (`Decision: changes requested by supervisor`), `Close rejected: MERGE REQUIRED` counts, status | message-only rejections; CI reds |
| `.cas/cas.db` `spawn_queue.worker_spec` (260 spawns since 08-20) | requested cli/model/effort per spawn: Luna/xhigh 140, Sol/high 44, Terra/high 33, Opus 5/high 37, Fable 5.1 3, Fable 5 1, Astra 2 | whether the spawn produced a rollout (13 Luna, 4 Sol, 2 Opus spawns are `failed`) |
| `.cas/logs/factory-session-*.log` (2026-08-30 →) | urgent stops per target, spawn stages, merges | anything before 08-30 |
| `task_lease_history` | not used: the rollout `task start` timestamp is the same instant and carries the worker identity | — |

Not in any source: per-request latency, rate-limit waits, and the actual subscription cost of a token.
