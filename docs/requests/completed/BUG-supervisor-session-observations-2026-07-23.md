---
from: ozer factory — session ozer-quiet-cobra-51, supervisor witty-puma-96
date: 2026-07-23
priority: P2
cas_task: (none)
---

# Supervisor session observations — 2026-07-23 (OnePass SFTP validation marathon)

Long solo-supervisor session (no spawned workers; heavy task/memory/coordination use, five staging merges, multiple sibling factory sessions active concurrently). Six issues, ordered by cost.

## 1. Task-lifecycle notifications echo back to the acting session — SHIPPED (`dc3d6e4`)

### Symptoms
Every `task close` I performed came back to me minutes later as a director-relayed `<task-lifecycle transition="task_closed">` teammate message — for my own action, verbatim, including my own close reason.

### Concrete evidence
Five occurrences in one session: cas-1462 (notification_id 212), cas-0896a (213), cas-32fc (214), cas-47a8 (219), cas-4c73 (263). Each carries `actor: witty-puma-96` — the same agent that received the relay. Timestamps 2026-07-23T01:02Z through 13:09Z.

### Workaround applied
Reply "lifecycle echo of my own close — nothing to action" and end turn. Burns a full model turn per close.

### Likely root cause
Queue fan-out for lifecycle events doesn't exclude (or mark) the transition's actor; the director relays to all supervisors indiscriminately.

### Proposed fix
- a) Suppress delivery when `actor == recipient` (leaned — the actor already has the close result synchronously).
- b) Deliver but flag `self: true` so harness/client can drop it without a model turn.
- c) Digest mode: batch own-action confirmations into the next unrelated wakeup.

## 2. Factory session logs cannot support post-session review — SHIPPED (`ebbef98`)

### Symptoms
Operator asked "review the supervisor and worker logs" — but `.cas/logs/factory-session-*.log` contains only a `session_end` header plus a full `git status` dump (114 entries of long-lived untracked files, identical across sessions). No assignments, messages, merges, closes, or errors.

### Concrete evidence
`factory-session-2026-07-22.log` = 355 lines: three session_end blocks, each ~115 lines of the same untracked-file listing. Grep for error/fail/stall across 7/22+7/23 logs matches only a filename containing the word "error".

### Workaround applied
Reconstructed session history from task notes + own context. Works only for the session doing the reviewing.

### Likely root cause
Session-end hook logs worktree state, and nothing else feeds the file.

### Proposed fix
- a) Append structured one-line events (assign/message/merge/close/reset/error) to the session log as they happen (leaned).
- b) Point reviewers at an existing queryable source (if the coordination DB already has this, a `cas factory log <date>` renderer suffices).
- c) Drop the git-status dump or cap it (it's ~97% of current log volume and repeats per session).

## 3. Codemap freshness gate evaluates the current checkout, not the canonical branch — SHIPPED (`90e2f60`)

Freshness now evaluates fetched `origin/<default>` alongside `HEAD`, accepts a
fresh result from either ref, and names the winning evaluation ref in
`cas codemap status`. Regression coverage reproduces the stale shared-checkout /
fresh canonical-branch factory shape.

### Symptoms
Immediately after regenerating CODEMAP.md and committing it to `staging` (the skill's own reset procedure), `cas codemap status` still reported "significantly out of date (217 structural changes)" with "Last updated: 2026-07-21".

### Concrete evidence
2026-07-23 ~13:2xZ: commit `d947abdb` ("docs: regenerate CODEMAP.md") on origin/staging; `cas codemap status` run from the main checkout — which a sibling session had pinned to an epic branch cut before the commit — reported stale. In multi-session factories the main checkout is rarely on the branch that received the regen.

### Workaround applied
Verified staging lineage manually and ignored the gate.

### Likely root cause
Freshness is computed from the git history of the working tree's HEAD; shared checkouts pinned to epic branches make that the wrong reference.

### Proposed fix
- a) Evaluate against `origin/<default branch>` (fetched ref, not checkout) and report checkout-vs-default divergence separately (leaned).
- b) Consider the codemap fresh if ANY of {HEAD, origin/default} contains a CODEMAP.md commit newer than the structural changes.
- c) Keep behavior but say explicitly "evaluated against branch X" so a false-stale is self-diagnosing.

## 4. Overlap-gate update-in-place buries fresh handoffs

**SHIPPED:** `9848a07` — `memory recent` now ranks and displays the later of creation and in-place update time, with regression coverage for resurfacing an updated old entry.


### Symptoms
The prior supervisor wrote a detailed session handoff. Because the overlap gate steers writers to "update the existing memory in place," it was appended to an entry created 2026-07-03. `memory recent` (created-sorted) didn't surface it; BM25 search for handoff-ish terms ranked it below stale entries; the incoming supervisor found it only because the human operator remembered "07-03-05 something".

### Concrete evidence
2026-07-23 ~00:1xZ: `memory recent limit=10` newest hit was 2026-07-22-7 (20:13); the handoff lived in 2026-07-03-5. `memory list sort=updated` does surface it — but nothing in the recall path defaults to that, and the same gate later forced two more of my own updates into that entry (score 5/5, "update in place" recommendation each time).

### Workaround applied
Operator-supplied ID fragment; thereafter I always used `list sort=updated`.

### Likely root cause
Recency surfacing keys on created_at; the overlap gate systematically converts new writes into updates, so the freshest content ages out of the recency window by design.

### Proposed fix
- a) `memory recent` ranks by max(created, updated) by default (leaned — one-line semantic fix).
- b) Overlap-gate acceptance bumps a `surfaced_at` timestamp used by recent/search boosting.
- c) A `handoff` entry-type pinned into next-session-start context for the same project, exempt from overlap-merging.

## 5. `.cas/` accumulates unbounded per-session sentinel files — SHIPPED (`341bd3f`)

### Symptoms
`.cas/` contains 166 `session_skills_seen_<uuid>` files (plus an empty-suffixed one), one per historical session, never cleaned.

### Concrete evidence
`ls ~/Petrastella/ozer/.cas | grep -c session_skills_seen` → 166, dating back months; also a bare `session_skills_seen_` (empty session id) suggesting one writer ran without a session.

### Workaround applied
None — cosmetic today, but directory listings and backup/sync of `.cas/` degrade linearly.

### Likely root cause
Skill-seen dedup sentinel is a file-per-session with no GC hook.

### Proposed fix
- a) Fold into cas.db (table keyed by session) — no files (leaned).
- b) GC sentinels for sessions older than N days in an existing cleanup pass (`agent_cleanup` / `gc_cleanup`).
- c) Single JSON map file instead of file-per-session.

## 6. AskUserQuestion reachable-but-fatal in factory mode

**SHIPPED:** `d5a8975` — Claude native factory supervisors and workers now start with `AskUserQuestion` excluded via `--disallowedTools`; the existing PreToolUse denial remains as defense in depth.

### Symptoms
Supervisor called `AskUserQuestion` (to clarify a vague bug report) and got a runtime error: "cannot reach the human in factory mode — it surfaces as a permission prompt on your own session and pauses the system."

### Concrete evidence
2026-07-23 ~00:2xZ, this session, first triage turn. The error text is excellent — but arrives only after a wasted call, and the failure mode it warns about (system pause) exists because the tool is offered at all.

### Workaround applied
Per the error: asked in plain text and ended turn for director relay.

### Likely root cause
Factory sessions inherit the standard tool surface; the interception happens at call time rather than schema time.

### Proposed fix
- a) Remove/deny-list AskUserQuestion from factory-mode sessions so the model never plans around it (leaned).
- b) Keep it but auto-convert the call into a director-relayed plain-text question instead of erroring.

## Evidence appendix for existing ticket cas-2cf9 (multi-epic concurrency / shared main dir)

Fresh, higher-stakes instances from 2026-07-23, offered as supporting evidence — not a new ticket:
- A sibling session switched the shared main checkout to its epic branch mid-turn; this session, mid-sequence, ran `vercel deploy` from it and **deployed the wrong tree to a production-project Preview** (caught by an immediately-preceding `rev-parse HEAD` echo; deployment deleted unused).
- `git push origin HEAD:staging` rejected non-fast-forward twice in one day because sibling epics merged between fetch and push.
- Branch switching in the shared checkout is also blocked by ~114 long-lived untracked files ("untracked working tree files would be overwritten") — the same listing that pads the session logs (issue 2).
Mitigation adopted session-side: all merges/deploys from detached `git worktree add` trees; never trust the main checkout across turns.

## Triage table

| # | Issue | Severity | Cost today | Affects |
|---|---|---|---|---|
| 1 | Lifecycle echo to actor | P2 | 1 wasted model turn per task close | every factory session |
| 2 | Session logs unreviewable | P2 | post-session review impossible from logs | operators, retros |
| 3 | Codemap gate wrong reference | P2 | false-stale nags + worker-dispatch blocks after a valid regen | multi-session factories |
| 4 | Overlap-gate buries handoffs | P2 | new supervisor missed the handoff until human intervened | session continuity |
| 5 | session_skills_seen litter | P3 | cosmetic; linear growth | .cas hygiene |
| 6 | AskUserQuestion in factory | P3 | 1 wasted call; pause risk if uncaught | factory sessions |

Happy to provide the full transcript (session `25fe20f1-a679-4da6-bf92-56df1fe02a25`, project `-home-pippenz-Petrastella-ozer`) or expanded log excerpts for any item.

---

## Resolution

**Resolved 2026-07-27 — shipped in cas v2.29.0 (`origin/main`).** Task: (6 items, individually tracked)

All six reported issues are marked SHIPPED inline: lifecycle echo (`dc3d6e4`), session log review (`ebbef98`), codemap freshness branch (`90e2f60`), overlap-gate handoff surfacing (`9848a07`), `.cas/` sentinel accumulation (`341bd3f`), and AskUserQuestion in factory mode (`d5a8975`). The last is independently corroborated: factory Claude sessions now launch with `--disallowedTools AskUserQuestion`.
