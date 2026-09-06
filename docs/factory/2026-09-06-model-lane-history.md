# Model lane history — measured per delivery, 2026-08-20 → 2026-09-06

Companion table to `2026-09-06-model-lane-rubric-review.md` (PART B of task cas-3372). Row-level data:
`2026-09-06-model-lane-history.csv` (409 rows: 388 worker task-segments, 21 supervisor sessions).
Retention horizon on this host starts 2026-08-20. cas-src only; the cross-project pass is a sibling task.

## Definitions

- **Delivery** = one worker working one task in one session (a "task segment": the rollout between one
  `task start` call and the next). A task sent back and resumed by a new worker is two segments.
- **Send-back** = a `Decision: changes requested by supervisor` note on the task (the `request_changes`
  action, sanctioned since 2026-08-04). Counted once per task, on its first segment. CI reds fixed
  without a request_changes decision are not send-backs here, so the 2026-09-05 supervisor brief (which
  counted CI reds and message-only rejections: 5 on 19) reads higher than this table (3 on 24).
- **Urgent stops** = factory-log `coordination_message` events with `urgent=true` targeted at a worker
  name that appears in the lane's rows (logs exist from 2026-08-30).
- **Cached input** = Codex `cached_input_tokens` (a subset of `input_tokens`); for Claude, cache-read
  plus cache-write tokens. **Uncached input** = the remainder.
- **Cost @ list** = shadow price at the vendor's Standard-tier list price on 2026-09-06 (citations in
  the rubric review PART A); Claude cache writes priced at the 1-hour TTL rate because every cache write
  in the transcripts is `ephemeral_1h` (Opus 5 $10/M, Fable 5.1 $20/M). Both harnesses actually run on
  subscriptions (three Codex accounts, two Claude accounts), so this is the price the work *would* carry
  on the API, not money spent.
- **Min to first push** = minutes from the `task start` call to the first `git push` tool call in the
  segment. Spike/no-code tasks without a push are excluded from the median.
- **Sessions live in five homes**: `~/.codex`, `~/.codex-support@gabber.studio`, `~/.codex-pippenz@gmail.com`
  (Codex rollouts keyed by `cwd`), `~/.claude-daniel@petrastella.io`, `~/.claude-alt`, `~/.claude-pippenz@gmail.com`
  (Claude transcripts per project directory). The first pass of this task read only `~/.codex` and
  `.claude-daniel` and missed every Astra session; the `home` column names the source of each row.

## Per model × effort, whole horizon (workers only)

| Model / effort (harness) | Deliveries | Workers | Closed | Send-backs | Send-back rate | Urgent stops | Uncached input / delivery | Cached input / delivery | Output / delivery | Tool calls / delivery | Median min to first push | Cost / delivery @ list |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| gpt-5.6-luna / xhigh (codex) | 212 | 133 | 204 | 26 | 12% | 25 | 401,318 | 17,757,010 | 46,395 | 139 | 17.0 | **$0.49** |
| gpt-5.6-sol / high (codex) | 51 | 23 | 48 | 8 | 16% | 20 | 254,657 | 12,649,266 | 36,353 | 103 | 12.1 | **$6.81** |
| gpt-5.6-terra / high (codex) | 42 | 34 | 41 | 1 | 2% | 0 | 199,407 | 8,134,254 | 21,041 | 69 | 6.2 | **$2.28** |
| gpt-6-astra / high (codex) | 2 | 1 | 2 | 0 | 0% | 0 | 172,321 | 9,160,512 | 33,220 | 74 | 19.1 | **$12.54** |
| gpt-6-astra / medium (codex) | 1 | 1 | 1 | 1 | 100% | 1 | 147,945 | 9,859,456 | 26,733 | 95 | 11.8 | **$12.68** |
| claude-opus-5 / high (claude) | 52 | 26 | 51 | 2 | 4% | 13 | 283 | 40,020,008 | 93,633 | 73 | 16.4 | **$25.47** |
| claude-fable-5-1 / high (claude, worker) | 1 | 1 | 0 | 0 | — | 0 | 548 | 1,831,632 | 26,178 | 3 | — | $4.37 |
| claude-haiku-4-5 (any) | 0 | 0 | — | — | — | — | — | — | — | — | — | — |

Luna distribution: output p50 44,237 / p90 75,338 / max 160,081 tokens; cost p50 $0.45 / p90 $0.88 / max $1.69.
Opus 5 distribution: output p50 80,117 tokens; cost p50 $18.99. Horizon totals at list: Luna $104.11 for 212
deliveries; Opus 5 $1,324.55 for 52; Sol $347.07 for 51; Terra $95.68 for 42; Astra $37.76 for 3.

Astra worker rows (all under the Astra supervisor on 2026-09-04/05, home `.codex-support@gabber.studio`):
fair-swan-9 / high cas-1939 (35.9K out, 69 tool calls, push at 20.3 min, $12.82), fair-swan-9 / high
cas-2226 (30.6K out, 78 calls, 17.9 min, $12.27), fierce-dragon-53 / medium cas-b8fc (26.7K out, 95 calls,
11.8 min, 1 send-back, 1 urgent stop, $12.68). Three deliveries are a sample, not a rate.

## Per period (rubric in force), workers only

Periods follow memory 2026-08-27-6 and the registry history. P1 = Terra/high standard (in this horizon
only 2026-08-20); P2 = Luna/xhigh standard, Sol/high heavy, Opus 5/high taste (2026-08-25 → 09-02);
P3 = the Opus 5 burst plus the Astra-supervised night (2026-09-03 → 09-04); P4 = 2026-09-05 onward.

| Period | Model / effort | Deliveries | Closed | Send-backs | Urgent stops | Output / delivery | Tool calls / delivery | Median min to first push | Cost / delivery @ list |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| P1 | gpt-5.6-terra / high | 42 | 41 | 1 | 0 | 21,041 | 69 | 6.2 | $2.28 |
| P1 | gpt-5.6-sol / high | 3 | 3 | 0 | 0 | 16,389 | 45 | 2.7 | $2.64 |
| P2 | gpt-5.6-luna / xhigh | 153 | 152 | 18 | 11 | 48,802 | 145 | 16.9 | $0.51 |
| P2 | gpt-5.6-sol / high | 10 | 10 | 0 | 2 | 34,061 | 110 | 6.5 | $7.91 |
| P2 | claude-opus-5 / high | 2 | 2 | 0 | 0 | 206,138 | 103 | 26.8 | $62.44 |
| P3 | claude-opus-5 / high | 42 | 41 | 2 | 13 | 92,835 | 75 | 16.4 | $23.06 |
| P3 | gpt-5.6-luna / xhigh | 32 | 30 | 4 | 10 | 40,016 | 121 | 18.0 | $0.43 |
| P3 | gpt-5.6-sol / high | 5 | 5 | 1 | 5 | 61,195 | 146 | 12.1 | $10.02 |
| P3 | gpt-6-astra / high | 2 | 2 | 0 | 0 | 33,220 | 74 | 19.1 | $12.54 |
| P3 | gpt-6-astra / medium | 1 | 1 | 1 | 1 | 26,733 | 95 | 11.8 | $12.68 |
| P4 | gpt-5.6-sol / high | 33 | 30 | 7 | 18 | 35,098 | 100 | 14.5 | $6.36 |
| P4 | gpt-5.6-luna / xhigh | 27 | 22 | 4 | 4 | 40,318 | 124 | 14.5 | $0.44 |
| P4 | claude-opus-5 / high | 7 | 7 | 0 | 11 | 57,481 | 51 | 22.2 | $28.43 |

Two labelled subsets cut across P3/P4:

| Subset | Model / effort | Deliveries | Workers | Send-backs | Urgent stops | Output / delivery | Tool calls / delivery | Median min to first push | Cost / delivery |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Astra-supervised, 2026-09-04 22:22Z → 09-05 16:20Z | gpt-5.6-sol / high | 36 | 11 | 7 (19%) | 18 | 37,631 | 101 | 13.9 | $6.46 |
| same | gpt-5.6-luna / xhigh | 4 | 3 | 3 (75%) | 3 | 35,866 | 114 | 14.4 | $0.37 |
| same | claude-opus-5 / high | 9 | 1 | 0 | 11 | 62,490 | 57 | 16.6 | $26.38 |
| same | gpt-6-astra / high + medium | 3 | 2 | 1 | 1 | 31,058 | 81 | 17.9 | $12.59 |
| Fable-supervised rubric-review window, 2026-09-05 16:20Z → | gpt-5.6-luna / xhigh | 24 | 13 | 3 (12%) | 2 | 39,639 | 122 | 18.9 | $0.44 |
| same | gpt-5.6-sol / high | 1 | 1 | 1 | 0 | 59,648 | 224 | 51.3 | $16.39 |

The Astra-supervised night carries the horizon's worst review outcomes on every lane it touched (Sol 19%
send-backs and 18 urgent stops on 36 deliveries; Luna 3 of 4 sent back; 11 urgent stops on one Opus worker,
watchful-lark-16). The rows cannot separate "Astra reviewed badly" from "the night's tasks were hard";
they do show that Sol's whole-horizon 16% send-back rate is mostly that night (P2 Sol: 0 on 10).

## Supervisor sessions (main checkout, all homes)

**Named finding — the 2026-09-05 stall.** The supervisor that held finished workers from ~05:56Z to 15:10Z
(agent loyal-crane-48, factory session cas-src-daring-badger-54, cas-20a3 note 15:10Z) was **Codex GPT-6
Astra**: rollout `~/.codex-support@gabber.studio/sessions/2026/09/04/rollout-2026-09-04T18-22-16-01a06e83…jsonl`,
cwd `/home/pippenz/Petrastella/cas-src`. Its `turn_context` records: Sol/high for the first 17 minutes
(22:22Z), Astra/**high** from 22:39Z, Astra/**medium** from 00:54Z (the `~/.codex/config.toml` default)
for the rest of the session. The rollout has **no events at all between 05:56Z and 11:48Z (352 min)**,
resumes 11:48–13:13Z, and a continuation session (13:16–16:16Z, Astra/high then medium) has further
34- and 65-minute gaps before the 15:10Z operator intervention. Session totals: 3.24M uncached input,
273.0M cached input, 197K output (33K reasoning), 1,345 tool calls, 9 compactions, 891 active minutes —
**$315.31 at Astra list** ($273 of it cached input at $1/M), plus $34.65 for the continuation. No agent
row survives in `cas.db` (cleaned), which is why the first pass found nothing.

**Named finding — supervisor effort.** Every Claude supervisor session in the horizon (17 sessions, three
homes) ran at **effort=high**; the registry's `supervisor` and `taste` lanes say `medium`, and 3.17.2
shipped that default. Claude Code's default is high and the spawn path never sets `effortLevel`, so the
lane default that shipped in 3.17.2 is not what produced any of today's results. The transcripts cannot
price medium vs high: no Fable supervisor session at medium exists (AA's Fable 5.1 ladder puts the whole
low→max span at 11× output tokens, so the difference is material but unmeasured here).

| Session start | Harness / home | Model / effort | Active min | Uncached in | Cache write | Cache read | Output | Tool calls | Cost @ list |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|
| 2026-08-20 11:38Z | claude / pippenz@gmail | fable-5 / high | 414 | 1,900 | 1,124,881 | 529,840,046 | 430,412 | 313 | unknown¹ |
| 2026-08-20 19:08Z | claude / pippenz@gmail | fable-5 / high | 89 | 19,448 | 294,333 | 24,642,437 | 88,729 | 34 | unknown¹ |
| 2026-08-25 14:13Z | claude / pippenz@gmail | fable-5 / high | 206 | 528 | 568,915 | 55,338,338 | 101,702 | 75 | unknown¹ |
| 2026-08-27 12:47Z | claude / daniel | fable-5 / high | 427 | 970 | 1,209,737 | 176,702,744 | 241,041 | 153 | unknown¹ |
| 2026-08-29 16:00Z | claude / daniel | fable-5 / high | 356 | 994 | 1,075,366 | 203,533,979 | 280,017 | 135 | unknown¹ |
| 2026-08-30 15:38Z | claude / daniel | fable-5 / high | 129 | 536 | 724,349 | 68,202,993 | 139,297 | 77 | unknown¹ |
| 2026-08-30 17:50Z | codex / pippenz@gmail | gpt-5.6-sol / high | 330 | 1,971,986 | 0 | 59,527,808 | 99,363 | 418 | $33.69 |
| 2026-08-30 23:21Z | claude / daniel | fable-5 / high | 824 | 54,411 | 2,556,768 | 255,445,740 | 395,008 | 33 | unknown¹ |
| 2026-08-31 13:34Z | claude / daniel | fable-5 / high | 274 | 11,119 | 1,203,403 | 64,611,429 | 131,645 | 26 | unknown¹ |
| 2026-08-31 18:25Z | claude / pippenz@gmail | fable-5 / high | 1,035 | 806 | 826,298 | 101,438,657 | 160,471 | 95 | unknown¹ |
| 2026-09-01 11:58Z | claude / pippenz@gmail | fable-5 / high | 341 | 37,925 | 1,756,629 | 227,106,836 | 369,887 | 42 | unknown¹ |
| 2026-09-01 18:01Z | claude / pippenz@gmail | fable-5 / high | 373 | 30,641 | 664,137 | 93,900,491 | 199,605 | 21 | unknown¹ |
| 2026-09-02 00:32Z | claude / daniel | fable-5-1 / high | 1,011 | 42,709 | 2,586,733 | 281,569,004 | 387,535 | 70 | $141.93 |
| 2026-09-03 12:46Z | claude / pippenz@gmail | fable-5-1 / high | 79 | 4,039 | 446,985 | 13,352,358 | 60,832 | 6 | $15.36 |
| 2026-09-03 14:08Z | codex / pippenz@gmail | gpt-5.6-sol / high | 158 | 384,895 | 0 | 30,713,216 | 54,711 | 216 | $14.92 |
| 2026-09-03 17:00Z | claude / daniel | fable-5-1 / high | 1,001 | 92,272 | 4,619,897 | 568,043,654 | 976,282 | 150 | $284.15 |
| 2026-09-04 02:48Z | claude / pippenz@gmail | fable-5-1 / high | 172 | 25,073 | 806,301 | 48,669,310 | 158,863 | 14 | $36.49 |
| 2026-09-04 13:55Z | claude / pippenz@gmail | fable-5-1 / high | 501 | 11,884 | 3,534,653 | 291,133,973 | 354,040 | 123 | $161.30 |
| **2026-09-04 22:22Z** | **codex / support@gabber** | **gpt-6-astra / medium** (high until 00:54Z) | 891 | 3,241,336 | 0 | 273,028,096 | 197,328 | 1,345 | **$315.31** |
| 2026-09-05 13:16Z | codex / support@gabber | gpt-6-astra / high→medium | 180 | 696,496 | 0 | 25,990,784 | 33,930 | 182 | $34.65 |
| 2026-09-05 16:20Z → (live) | claude / daniel | fable-5-1 / high | 1,484 | 9,604 | 3,528,408 | 222,642,894 | 321,841 | 94 | $142.42 |

¹ Fable 5 (the pre-5.1 model) price was not researched; its rows are left unpriced rather than guessed.

The live Fable 5.1/high supervisor day ($142 at list) is 13× the $10.5 of the 24 Luna deliveries it
reviewed; 50% of it is 1-hour-TTL cache writes at $20/M, 39% cache reads at $0.25/M, 11% output. The
Astra supervisor's night ($315) is 2.2× that, and 86% of it is cached input at Astra's $1/M — Astra's
cache-read price is 4× Fable 5.1's, which is the whole difference for a coordinator whose context is
re-read every turn.

## What each source could and could not supply

| Source | Supplied | Could not supply |
|---|---|---|
| Codex rollouts in `~/.codex`, `~/.codex-support@gabber.studio`, `~/.codex-pippenz@gmail.com` (`sessions/2026/08/20…09/06`, 243 cas-src sessions ≥100 KB) | model and `effort` per turn (`turn_context`), per-turn `input/cached_input/output/reasoning_output` tokens (`token_count`), every tool call with arguments, timestamps, compactions, activity gaps | lane name (inferred from model×effort), merge outcome (from task notes) |
| Claude transcripts in `~/.claude-daniel@petrastella.io`, `~/.claude-alt`, `~/.claude-pippenz@gmail.com` (`projects/-home-pippenz-Petrastella-cas-src*`, 59 files ≥200 KB since 08-20) | model, `effort` per record, `usage` per assistant message (input, cache_creation 5m/1h split, cache_read, output), tool_use blocks | reasoning tokens (not split out) |
| `.cas/cas.db` `tasks.notes` | send-backs (`Decision: changes requested by supervisor`), `Close rejected: MERGE REQUIRED` counts, status | message-only rejections; CI reds |
| `.cas/cas.db` `spawn_queue.worker_spec` (260 spawns since 08-20) | requested cli/model/effort per spawn: Luna/xhigh 140, Sol/high 44, Terra/high 33, Opus 5/high 37, Fable 5.1 3, Fable 5 1, Astra 2 | which home the spawn ran in (`requester_config_dir` is empty for every row) |
| `.cas/logs/factory-session-*.log` (2026-08-30 →) | urgent stops per target, spawn stages, merges, supervisor agent name and factory session | anything before 08-30; the supervisor's model (never logged) |
| `.cas/cas.db` `agents` | the live supervisor only | the stalled supervisor's row (already cleaned) |
| `task_lease_history` | not used: the rollout `task start` timestamp is the same instant and carries the worker identity | — |

Not in any source: per-request latency, rate-limit waits, and the actual subscription cost of a token.
