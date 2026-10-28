# Codex vs Claude factory supervisors, measured side by side

Spike for epic cas-b9d4 (Codex supervisor parity), 2026-10-27. Question: why does a Codex
supervisor feel like a "bumbling old man" next to a Claude supervisor?

## Answer

Codex supervisors are not deaf, and they do not pester the operator. They are slow and busy:

1. They loop on `sleep` and `inbox_poll` inside a turn.
2. They re-read the skills after every compaction.
3. They narrate before each tool call.
4. They reply to almost every worker update.

All four trace to the Codex supervisor contract at `crates/cas-pty/src/pty.rs:17` and how it
interacts with the Codex base prompt. Voice is a fifth, structural gap: the operator's user-level voice
instructions reach Codex at the lowest-precedence layer.

## Method and sources

- Read-only: local Codex rollouts and Claude transcripts, plus the projects' `.cas/cas.db`
  `prompt_queue` (opened `mode=ro`). No model calls and no credential files were read.
- Sample: every factory-supervisor session on this host (soundwave) from 2026-10-17 to 10-27. That is 9 Codex
  rollouts (40.6 h, 140 merges), identified by the developer message "You are the Cassy
  Factory Supervisor". It is also 14 Claude transcripts over 1 MB from 10-20 to 10-27 (124.8 h, 685 merges),
  identified by the SessionStart banner `**You**: <name> (supervisor)`. Only 3 Codex supervisor sessions exist in the last week, so
  the Codex window reaches back to 10-17.
- Not covered: the Mac. Its transcripts are not readable from this host.
- Scripts, manifest and raw per-session JSON:
  `~/.cas/artifacts/cassy-bbb8e755…/cas-c2b4/` (`analyze.py`, `aggregate.py`, `toolmix.py`,
  `probe2.py`, `manifest.tsv`, `results.json`, `per_session.json`).

Metric definitions:

- **Wake**: the transcript time a message is first seen, minus the `prompt_queue.created_at` of that message.
- **Signal→task action**: the time from an actionable inbound message (READY, MERGE REQUIRED,
  blocker, idle, QA dispatch) to the first tool call naming its task id.
- **Operator→final**: the time from an operator message to the last reply of that exchange. For Codex
  this is the `final_answer` phase.
- **Narration**: reply messages before the final one in an operator exchange.
- **Questions**: final replies that end in `?` or ask "should I / want me to / which".

## Pooled comparison (median of session medians; totals where noted)

| Measure | Codex (9) | Claude (14) | Read |
| --- | --- | --- | --- |
| Wake, queue → seen (s) | 17.8 (p90 136) | 20.3 (p90 188) | not a Codex problem |
| Signal → task action (s) | 14.4 | 7.2 | Codex 2x slower |
| Turn length (s) | 73.8 | 22.4 | Codex turns 3.3x longer |
| Inbound landing mid-turn | 1,286 of 1,418 (91%) | 1,872 of 3,115 (60%) | turns never end |
| Tool calls per merge | 50.4 | 14.5 | 3.5x |
| inbox_poll per hour | 7.6 | 0.2 | 38x |
| sleep/wait calls (total) | 151 | 29 | in-turn busy-wait |
| Messages sent per merge (total basis) | 9.8 | 2.6 | 3.8x |
| Skill/reference reads per hour | 8.1 | 0.8 | 10x |
| Skill re-reads (total) | 170 | 42 | |
| Compactions per hour | 0.57 | 0.056 | 10x |
| Operator → final answer (s) | 39.9 | 21.9 | 1.8x |
| Narration msgs per operator message | 1.5 | 0.4 | 3.75x |
| Final reply length (words) | 36 | 144 | Codex terse |
| Operator questions per 10 finals | 0.0 | 1.6 | Codex asks less |
| Final replies in the operator's requested voice | 0 of 235 | 50 of 92 (the one session with voice instructions loaded) | voice gap |
| Operator status nudges ("status?") | 6 in 40.6 h | 10 in 124.8 h | 1.9x per hour |

Ruled out:

- **Missed or late wakes.** Codex delivery is as fast as Claude's. Queue rows for Codex
  supervisors record `nudge_not_attempted` and `awaiting_ack` as their most common reasons.
  They come from the PTY transport ("typed payload is the wake; turn start unconfirmed"), not
  from lost messages. Only 3 Codex inbound messages carried a "not yet delivered" note.
- **Needless operator questions.** Claude asks more, usually with a recommendation. Codex
  asks almost never.

## Session-start load and precedence

| Layer (highest first) | Codex 0.163 supervisor (10-27, accounting) | Claude Code supervisor (10-27, cassy) |
| --- | --- | --- |
| Harness system prompt | `base_instructions` 21,866 B, including "Personality: As Codex …" and commentary-channel rules | system prompt snapshot 12,613 B |
| Factory contract | developer message, 2,605 B (`pty.rs:17`), no "do not poll" | first user turn, 2,571 B (`pty.rs:~297`), with "Act on the injected signal; do not poll" |
| Skills catalog | inside the same developer message, 16,326 B (includes OpenAI curated skills: imagegen, pages, sites, work-pets) | `skill_listing` attachment, 28,141 B |
| Multi-agent role | developer messages, 2,671 B | agent listing 3,714 B, deferred tools 14,854 B, MCP instructions 6,423 B |
| User and project instructions | user-role `# AGENTS.md` message, 3,460 B; today 5,079 B with the operator's user-level voice instructions (3,841 B) | `instructions` attachment, 9,207 B (CLAUDE.md, AGENTS.md, rules, memory), framed "These instructions OVERRIDE any default behavior" |
| Hook context | SessionStart and Bash PostToolUse via developer messages | SessionStart 2,274 B (10.3 KB spilled to a file), plus a supervisor reminder of 436 B |
| Skill bodies | not loaded; read with shell `cat` (cas-supervisor 7,017 B, checklist 7,073 B, references 132 KB) | loaded on demand by the Skill tool; 0 skill reads within 15 min after any of 7 compactions |

Precedence consequence: the factory supervisor prompt's rules govern content in both harnesses.
In Claude the operator's user-level voice instructions sit in an "OVERRIDE" system reminder
above the factory turn. In Codex they sit in a user-role message below two layers that pull the other way: the developer contract's
"Write in facts, not narration … Skip preamble", and the base prompt's "without flattery or
forced enthusiasm".

## Top 5 causes, ranked by measured cost

1. **Sleep-and-poll loop.** Measured cost: 7.8 extra tool calls per merge (polls, sleeps and
   the status lookups between them), about 60 minutes of in-turn sleep, and turns 3.3x longer.
   Mechanism: `CODEX_SUPERVISOR_INSTRUCTIONS` (`crates/cas-pty/src/pty.rs:17`) lacks "Act on the
   injected signal; do not poll", which the Claude (`pty.rs:313`) and Grok (`pty.rs:35`) contracts
   carry. The only assertion of it (`pty.rs:~7042`) checks Grok alone.
   Fix: add the sentence plus "end the turn; never sleep or loop on inbox_poll", and assert it
   for every harness. Task: cas-6e08.
2. **Replying to every update.** Measured cost: 7.2 extra messages per merge (9.8 vs 2.6;
   33.9/h vs 14.4/h). Excluding peer-supervisor threads, the session medians are 6.8 vs 2.5 local
   messages per merge, and the 10-27 accounting session, which had no peer traffic, ran 12.3.
   Each reply invites another wake. Mechanism: "… and reply via
   `coordination action=message target=<worker>`" in both contracts (`pty.rs:17`, `pty.rs:~304`);
   Codex follows it literally. Fix: reply only when the sender needs an answer, action or
   verdict. Task: cas-752f.
3. **Skill re-reads after compaction.** Measured cost: 1.8 extra reads per merge, 170 re-reads,
   and up to 3.7 MB of skill text re-read. 93 reads came within 15 minutes of a compaction, and Codex compacts 10x
   as often. Mechanism: the Codex contract says "Use skills cas-supervisor and
   cas-codex-supervisor-checklist" (`pty.rs:17`). The checklist
   (`cas-cli/src/builtins/codex/skills/cas-codex-supervisor-checklist.md`) fans out to about 10 references.
   Codex has no Skill tool, so nothing survives compaction. Fix: a resident core of at most 3 KB (a signal →
   action table) in developer_instructions, re-injected by the SessionStart `compact` hook. Task: cas-cbbb.
4. **Commentary narration.** Measured cost: +18 s operator → final answer, 718 commentary
   messages, and 1.5 narration messages per operator message. Mechanism: the Codex base prompt
   requires a commentary message before tool calls, an update at least every 60 s, and an
   announcement of each skill. "Skip preamble" in `pty.rs:17` never names the commentary
   channel. Fix: an explicit supervisor commentary policy. Task: cas-ae046.
5. **Voice precedence.** Measured cost: 0 of 235 Codex final replies use the operator's requested
   voice, and the voice is flat, with a median of 36 words. Mechanism: the load-order inversion described
   above. Caveat: the operator's user-level voice instructions reached the Codex AGENTS.md at 6:59 PM
   ET on 10-27, after the sampled Codex sessions, so this cause is predicted rather than observed. Fix:
   a developer-level clause saying that the operator's user-level instructions govern voice and style
   while the factory rules govern content. Task: cas-b748.

All five fixes edit the same constant at `pty.rs:17`, so land them in one lane or in sequence. None of them
touches wake or delivery code.
