# Factory efficiency audit, November 27, 2026

All times are Eastern. Code references are against origin/main `e2e2f49b52` (v4.59.1) unless stated. Every figure below comes from a source listed in section 12. Where two sources disagree, both are given.

## 1. Summary

**What was wrong.** On November 26 to 27 a Codex-supervised factory ran 10.75 hours with up to ten workers per machine. It closed one product issue, which it had filed against its own release that evening, plus one duplicate. About 71% of supervisor time went to getting Cassy's own releases past Cassy's own gates, and 3% went to already-open issues. Of 221 open issues, 62% were Cassy policing itself. Over the prior seven days the factory used 67.6B tokens, and 46% of all tokens were worker calls carrying more than 200K tokens of context.

**What changed.** Blocking enforcement is down to five rules plus eight safety rails. Everything else warns and is counted. Refusals no longer file GitHub issues. Widened rules and memories were rewritten or retired, 449 memories were archived, and the session-noise memory writers are being deleted. The supervisor auto-clear cap went from 500K to 300K on both machines.

**What's next.** The token burn comes from using the agent conversation as Cassy's message bus and database, so cost = calls × history. The approved redesign keeps storage; agents wake on one line and read a compact state snapshot. Order: one task per worker, supervisor context, no-op wakes, fixed preamble, verifier churn. Success is measured outside Cassy (proxy dashboard plus transcript totals), with a target per step.

## 2. The overnight failure

Supervisor: Codex `gpt-6.1-sol`, effort "high", on soundwave. The operator ended supervision at 9:46 AM and stopped all workers. The supervisor's own account is `SUPERVISOR-INCIDENT-2026-11-27.md`; the transcript forensics contradict parts of it.

| Measure | Value |
|---|---|
| Duration | 10.75 h (11:00 PM to 9:46 AM): 159 turns, 1,188 tool calls |
| Supervisor time in a turn / idle between turns | 6.0 h / 4.7 h |
| Product issues closed | 1 (#2563, filed by the supervisor at 8:36 PM against v4.59.0, which it shipped at 8:26 PM) |
| Duplicates closed | 1 (#2575) |
| Previously open issues closed | 0 |
| Issues opened after bedtime | 11 (2 closed, net +9) |
| Recovery release v4.60.0 | Not published: release-content proof refused it twice; full CI passed 14,281 tests both times |
| Main | Owned by the train from 12:45 AM; frozen from 4:19 AM for 5 h 27 m, blocking non-repair merges and staffing |

### Where supervisor time went (328 attributed tool-minutes)

| Stream | Minutes | Share |
|---|---:|---:|
| v4.60 release-proof repair | 112 | 34% |
| v4.59.1 QA, close, report | 65 | 20% |
| Status, staffing, memory | 59 | 18% |
| Prowl native QA chain | 56 | 17% |
| New self-filed defects | 25 | 8% |
| Already-open backlog | 11 | 3% |

**Workers.** Of 24 Linux status snapshots, 9 showed zero workers executing and 22 at most one (peak 4, mean 0.8). Four workers were spawned overnight; two waited 49 and 39 minutes on the supervisor. Sensors put soundwave workers in a turn for 451 of 3,912 alive minutes (11.5%).

### Where the transcript contradicts the self-report

| Self-report | Transcript |
|---|---|
| "1–3 active repair lanes" | 0–1 executing in 22 of 24 snapshots |
| 18 new reports, one product fix | The supervisor filed the fix's issue and 12 of the 18 reports; all 18 came from the factory account |
| Timeline skips 4:19 AM | The freeze that held merges and staffing for 5.5 h began at 4:19 AM |
| Cites a 53-minute supervisor delay | Omits the workers left idle for 49 and 39 minutes |
| Prowl QA chain was scope creep | The supervisor approved that chain in 39 directive messages |
| Omits the forecast | Its own ranking capped the night at 10 issues, after promising "about sixty" at 7:50 PM |

**Root cause.** Issues close only when a release ships, so the night hung on one cut, and Cassy's release-content proof refused it. The supervisor made repairing that gate the job; each repair met the proof and review loop, which produced new self-filed defects that became the next tasks. The train, then the freeze, blocked every other merge.

**The harness made it worse.** The Codex supervisor waited about 45 s on each idle wake (about 49 min total) because the daemon has no transcript evidence for it; drained one message per turn (145 of 161 turns) because Codex has no UserPromptSubmit hook; saw mid-turn mail only after shell calls; and, with no context reset, ran at about 445K of a 475K window through seven compactions between 7:39 PM and 9:51 AM (four inside the overnight window). Delivery to it: median 73 s, p90 668 s, against 0 s and 21 s for the November 23 Claude baseline (harness audit; the sensors audit, on a different message set, gives 80 s and 625 s). Fixes are in section 11.

## 3. The backlog: what the 221 issues were

Method: titles and last three comments for all 221, about 15 bodies in full, plus git history; per-issue rows in `issue-classes.csv`.

| Kind | Raw | With duplicates folded into their root kind |
|---|---:|---:|
| Own gate refuses legitimate work | 46 | **105** |
| Real malfunction | 56 | 81 |
| Wish or nit | 27 | 29 |
| Duplicate or recurrence | 86 | 0 |
| Agent confusion | 3 | 3 |
| Fixed or stale | 3 | 3 |

- **138 of 221 (62%) police Cassy itself**: gates, guards, proof, receipts, verification, QA ledgers, release attribution. Product-visible bugs are rare (3 Commander UI, 1 Slack, about 4 Cloud sync).
- **About 115 root causes.** The top 20 roots hold 101 issues.
- **Gate share of new issues is rising**: 45% (Nov 6–12), 64% (Nov 13–19), 67% (Nov 20–27), 77% on Nov 24.
- **Refiling churn.** Focused proof (G-61): 139 filed, 113 closed over its lifetime; shell guards (H-xx): 110 filed. Each patch narrows one false refusal and the next shape gets filed.
- **The loop.** The bug-routing rule turned refusals into issues: 684 of 999 issues opened since November 1 carry the automatic-report footer. New gates draw issues within a day; eight open issues ask for more gates.

Five changes close about 94 issues, and four of the five are deletions:

| Change | Closes |
|---|---:|
| Make focused proof advisory. Lane CI plus named tests are the proof. | ~30 |
| One delivery identity: the pushed branch tip, resolved by git | ~24 |
| Shell guards allow what they cannot parse. Literal secret and root checks stay. | ~18 |
| Delete the single-owner train lock | ~10–12 |
| Fold the 12 Playwright skill tips into one skill edit | 12 |

## 4. Gates: census and the five rules

| Census item | Count |
|---|---:|
| Registered codes (G 164, H 45, C 40, W 69, N 101) | 419 |
| Codes that can refuse (G, H, C) | 249 |
| Refusal reasons outside the registry (release attribution and others) | 7 |
| Recommended: keep blocking / keep as plain error / merge / warn / delete | 67 / 11 / 11 / 99 / 68 |
| Kept blocking rows that are agent gates (the rest are CI and release jobs) | 36 |
| Deletable lines, ±30% (production about 33K, tests about 45K) | about 75–80K |
| Overnight refusals | 187, about 8.5 per epic merge (22 merges, 1 main landing) |

Refusals are decided at 527 call sites in 75 files; central observation runs after the decision and cannot undo it. Every pre-tool hook deny passes through `handle_pre_tool_use_inner` (`pre_tool.rs:548`), so one switch demotes all H codes. G gates need one helper plus about 210 call-site edits.

**Decision, 1:59 PM.** Five blocking rules:

| # | Rule | Mechanism kept |
|---|---|---|
| R1 | CI green on the exact tree before main | Merge-queue CI jobs, G-111, G-140, H-23 |
| R2 | One independent review | G-104, with G-154, G-156 and G-19 keeping verdicts unforgeable |
| R3 | Delivery = pushed branch tip from git | G-59. Receipt, anchor and rebind codes are deleted. |
| R4 | Release contents = git log between tags | Release pipeline jobs. Release attribution is deleted. |
| R5 | Never overwrite unsaved work or read a real secret | G-136 to G-139, G-143, H-09, H-16, H-22, H-29 and others |

Eight safety rails stay blocking:

| Rail | Codes |
|---|---|
| Production database writes | H-17 |
| Destructive Cloud purge | G-144, G-145 |
| Policy, config and role anti-tamper | H-11, H-12, H-14, H-41 |
| Urgent stop | G-36 |
| Project hold | G-92 |
| Build and worker caps | H-02, H-03, H-04, G-135 |
| Slack only via Violet | H-06 |
| Release signature | C-28 |

Everything else warns and is counted. A new blocking gate needs operator sign-off or the deletion of one. Nothing owns main; a freeze blocks only its release; merges happen automatically on green plus review.

## 5. Rules and memories audit

**Standing rules.** After deduplication the audit covered 44 standing rules: 15 had been widened beyond the operator's recorded words and 14 conflicted with the November 27 decision. The most damaging:

| Rule as practiced | Operator's actual ask | Outcome |
|---|---|---|
| Every refusal, apology or workaround is filed as a Cassy bug | Downstream supervisors report real Cassy defects | Rewritten: refusals are counted locally and never filed |
| Park needs exact-tip receipts per crate and five passes of changed tests | Stop discovering flaky tests at release time | Rewritten: green lane CI on the exact tip is the proof |
| Every lesson becomes a Cassy change, ideally a gate row | Fix friction at the source ("kaizen") | Rewritten: default, auto-fix or warning. A new gate needs sign-off. |
| Every acceptance criterion needs a park checklist | Don't drop bounced work | rule-048 shortened. The checklist gate warns. |

The November 6 "fuck rules" memory had been widened into "enforce all discipline at tool boundaries"; it was rewritten. rule-175 ("every change ships") was retired; rule-026, rule-048 (shortened) and rule-172 were kept. The red-first requirement was deleted outright ("we own the code base"); its producers and readers are being removed.

**Memories.**

| Action | Count |
|---|---:|
| Cassy memories archived (memory audit) | 449 |
| Cassy memories rewritten (memory audit) | 22 |
| Rules-audit Cassy memory updates / archives | 14 / 6 |
| Local memory files rewritten / retired | 8 / 2 |
| Noise rows on the soundwave snapshot, before → after the purge migration | 2,360 → 0 |
| Total entries on the soundwave snapshot, before → after | 4,044 → 1,684 |

383 of 487 working-tier rows (79%) were automatic session noise (files created, build success, captured prompts), and SessionStart reads that tier. Both writers, Stop-hook synthesis and prompt capture, are deleted rather than gated, and a migration purges their rows on every install.

## 6. Token economics

Window: transcripts from November 20 to 27 on both machines.

| Measure | Value |
|---|---:|
| Total tokens | 67.6B over 270,567 calls |
| soundwave / prowl | 52.7B (78%) / 14.9B (22%) |
| Cache reads | 66.5B (98%) |
| Output | 135.8M |
| Proxy dashboard, same seven days | about 15B tokens, $4,923; Opus and Fable averaged 255–280K input per call |

The two counters measure different things and the sources do not reconcile them; each step measures against both.

The proxy dashboard (about 15B tokens, $4,923) covers only soundwave traffic routed through CPA Manager Plus starting November 23 at 1:56 PM ET. Claude traffic stopped routing through it after November 25. The transcript total (67.6B tokens) covers both machines and all harnesses.

### By role

| Role | Tokens | Share | Mean context per call |
|---|---:|---:|---:|
| Worker | 43.9B | 64.9% | 246K |
| Supervisor | 17.5B | 25.9% | 406K |
| Verifier/reviewer | 3.4B | 5.1% | 128K |
| Subagent | 2.7B | 3.9% | 127K |

**Cost model.** Every call re-reads resident history. Supervisors woke on about 420 wake-minutes a day (2,938 in seven days), each costing at least one call at about 220–250K tokens. The overnight Codex supervisor averaged 248K per call over 2,383 calls (591M input). Wake count and history size dominate; notice size does not.

### Ranked waste, biggest first (approved order of attack)

| # | Source | Measured |
|---|---|---|
| 1 | Long-lived workers | Worker calls above 200K context: 31.4B, 46% of all tokens. Some workers lived 7–15 h across 2–21 tasks, re-reading up to 967K per call; "one task per worker" was only a memory. |
| 2 | Supervisor context | Calls at 400K+ context: 12.0B. Top three sessions 0.9–1.2B each at 510–530K per call. |
| 3 | No-op wakes | 435 of 2,938 supervisor wakes (14.8%) were state-only, about 100M input tokens a week. One merge can cause three wakes. Overnight: 46 of 160 turns no-op. |
| 4 | Fixed preamble | Claude supervisor: 38K tokens on call one, about 60K by call two. Codex re-sends about 57K characters of instructions per resume or compaction (793K in one rollout). Calls after worker startup briefs: 4.4B. |
| 5 | Verifier churn | 1,686 sessions, 3.4B tokens. Overnight: 39 approved, 5 rejected, 28 errors. |
| Later | Tool output | Bash: 250M of 442M tool-result characters. Codex shell averages 4.6K (max 45K); task show up to 48K. |

## 7. Why past fixes failed

Earlier attempts (wake policy, no-op wake filters, auto-clear, a daemon spike, concise skills) failed for three reasons.

| Failure mode | Evidence |
|---|---|
| Filtered message kinds instead of changing how messages are delivered | Filtered rows still arrive as digest lines. Messages are still typed into the pane, and a whole history is still re-read per wake. |
| Added chatter of their own | The auto-clear "skipped" notice wrote 2,133 rows in seven days, 1,123 of them to a single supervisor. Overnight, auto-clear was skipped 97 times and never fired, because harness compaction always arrived first. The Stop-hook "no wake source" reminder fired 26 times in 75 minutes. |
| No measured target | `supervisor_message_wakes.started_at` has been null for every supervisor since November 7. 7 of 15 scorecard sensors need attention. No step had a before/after token number. |

## 8. Decisions and rationale (November 27)

| Time | Decision | Rationale |
|---|---|---|
| 1:59 PM | Five blocking rules plus eight safety rails. Everything else warns and is counted. No new blocking gate without sign-off or a deletion. Nothing owns main, a freeze blocks only its release, and merges happen automatically on green plus review. | 105 issues are gates refusing legitimate work. A freeze held main for 5.5 h overnight. |
| Same day | Refusals stop filing GitHub issues automatically. They go to a local ledger, and an issue is filed after 5 recurrences in 7 days. | The filing rule was the loop that fed the backlog. |
| Same day | Rules and memories cleaned as in section 5, including the red-first deletion | Widened rules generated gates; 79% of the working tier was noise; "we own the code base" |
| Same day | Supervisor auto-clear cap 500K → 300K on both machines; operator floor 300K | Supervisor calls averaged 406K. Both project configs read 300000; `supervisor_auto_clear.rs:471` reads the key. |
| 2:53 PM | Agents wake on one line and read a state snapshot. Status becomes state. Tools return answers plus handles, later. | Cost = calls × history. Storage is kept. |
| Same day | Order: one task per worker, supervisor context, no-op wakes, fixed preamble, verifier churn | Ranked by measured tokens (section 6) |
| Same day | The operator and the supervisor jointly own codebase health and optimization | Optimization had no owner |
| Same day | Success is measured outside Cassy (proxy plus transcript totals), with a target for each step | Earlier fixes had no measured target (section 7) |

## 9. Work in flight

Status as of 3:08 PM.

| Task | Title | State |
|---|---|---|
| cas-80610 | Shrink the factory to five blocking rules and measure efficiency (epic) | open |
| cas-d224 | Gate refusals stop auto-filing GitHub issues; count them locally instead | awaiting merge |
| cas-a2ea | One hook switch: non-kept pre-tool denies warn and let the call proceed | in progress |
| cas-63e0 | Nothing owns main: retire the train-owns-main lock and make a freeze block only its release | in progress |
| cas-09b9 | Skills, launch text and park checks follow the five-blocks direction | in progress |
| cas-29e7 | Delete the session-noise memory writers and purge their rows on every install | in progress |
| cas-8eac | Purge synced session-noise memories from Cloud and confirm zero on prowl after install | open |
| cas-8d7a | Workers end with their task: Cassy shuts a worker down at task close and recycles it at ~200K context | in progress |
| cas-ba9e | Design brief: agents wake on one line and read state, not an ever-growing chat (supervisor first) | open |
| cas-8b8a | Nightly factory scorecard from recorded events: lead time, busy share, refusals per merge, wake latency, main-blocked time | in progress |
| cas-4118 | Scorecard time-sink sensors: phase waterfall, worker minute buckets, refusal recovery cost, supervisor turn reasons, CI/review/peer latency | open |
| cas-8ff0 | Codex supervisor gets turn-start delivery, all-pending drain, transcript-aware wakes and a context reset | open |
| cas-06b1 | Later: Cassy tool outputs return answers plus handles, not dumps | open |
| cas-22f6 | Capped worker-check logs submitted tip while compiling later working-tree inputs | open |
| cas-4248 | Rule projection reconciliation retires every project rule when the projected files are briefly absent | open |

## 10. Sensors and the scorecard

Cassy records most raw data, but all ten efficiency metrics are only partially covered.

| Metric | Overnight baseline | Main gap |
|---|---|---|
| Issue-to-ship lead time | #2563: 6 h 19 m, including 2 h 28 m from release to close | No close time; no created time for issues filed outside Cassy |
| Worker busy share | 11.5% in a turn, about 24% holding a lease while idle, about 64% unassigned | No blocked/held intervals; 151 duplicate Codex turn rows |
| Refusals per merge | 187 refusals, about 8.5 per epic merge | No actor; uncoded refusals dropped (`gates/facts.rs:204`), at least 22 |
| Wake latency | Codex supervisor: median 80 s, p90 625 s | `started_at` has been null since November 7 |
| Supervisor decision latency | Median 2.4 min, p90 6.6 min (n=29 linked replies) | No link from a blocker to the supervisor action |
| Main-blocked time | Frozen 5 h 27 m | Freeze is one overwritten label; no interval table |
| Rework | 5 reopens, 54 restarts, 5 rejected verifications, 28 errored | No reopen event or CI attempt number |
| Context health | Peak 379,049 input against a 380,000 threshold. 4 compactions. | Context size appears only in digest prose. |
| Cost | 853M input tokens, 35 closed tasks, about 24M tokens per closed task. 554 CI wall-minutes. | 74% of turns have no task; CI minutes not kept |
| Self-generated backlog | 12 filed, 2 closed | Agent and human filings look the same on GitHub |

Slice one fixes the three sensor bugs, adds actor to refusal facts and adds `cas scorecard --since/--until`. Slice two adds a phase waterfall, worker minute buckets, refusal recovery cost, supervisor wake reasons, and CI, review and peer latency. Token targets are checked against the proxy and transcript totals, not Cassy's own counters.

## 11. Open items for later

| Item | State |
|---|---|
| Tool outputs | Shell, task show, worker_status, remind_list and epic_status return the answer, what changed and a handle. After worker lifetime and the supervisor snapshot. |
| Codex supervisor fixes | (a) Transcript evidence for wakes, so idle wakes use the 2 s bar. (b) UserPromptSubmit hook, or drain all pending in one injection. (c) PostToolUse covers MCP tools. (d) Handoff and restart at about 80% of window. Optional: effort "medium". Codex should not supervise unattended until these land. |
| Branch and worktree pruning | soundwave: 3,620 local branches, 4,254 remote-tracking, 260 worktrees, 11 rescue refs; main checkout 6,956 commits behind origin/main. Keep the incident's retained train, prepare and repair refs until adjudicated. |
| Cloud noise purge | Synced noise memories need a supported Cloud deletion. It is a kept safety rail: operator confirms the measured count first. |
| Harness audit correction | It says no Rust code reads the supervisor input-cap key; `supervisor_auto_clear.rs:471` does. The Codex gap is the missing context reset. |

## 12. Sources

Scratch paths are under `/mnt/rewind/cas/scratch/`.

| Source | Content |
|---|---|
| `SUPERVISOR-INCIDENT-2026-11-27.md` (repo root) | Supervisor self-report |
| `nits-audit/transcript-forensics.md`, `codex-harness.md` | Overnight time split, refusals, workers; Codex defects |
| `nits-audit/issue-taxonomy.md`, `issue-classes.csv` | 221-issue classification |
| `gate-census/census.md`, `census.csv` | Gate census and recommendations |
| `sensors/sensors.md` | Sensors and overnight baselines |
| `rules-audit/rules.md`, `rules.csv`, `apply-log.txt` | Rules audit and applied changes |
| `memory-audit/memories.md`, `archive-log.txt`, `rewrite-log.txt` | Memory audit and applied changes |
| `token-spike/tables.md`, `deep.txt` | Seven-day token totals |
| `state-not-chat/research.md` | Delivery seams, wake volume, preamble sizes |
| `.cas/cas.db` (read-only) | Epic and child tasks, rule states |
