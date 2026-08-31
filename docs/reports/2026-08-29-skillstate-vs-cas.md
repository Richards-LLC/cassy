# SKILL.state vs CAS: no runtime change, but it names our worker-notes problem

**Date:** 2026-08-29 · **Author:** factory supervisor session · **Audience:** practitioner (Daniel)
**Paper:** Badhe, Tiwari, Chung, *SKILL.state: Scalable Long-Horizon Agent Skills*, Google LLC + Purdue, arXiv:2608.26263v1 (2026-08-26) · Companion to the same-day WikiSkill evaluation (`2026-08-29-wikiskill-vs-cas.md`)

## Verdict

SKILL.state is a **runtime architecture** — it replaces append-only conversational history with a
structured, mutable execution state the model patches each step. CAS cannot adopt it wholesale:
our workers run inside conversational harnesses (Claude Code / Codex / Grok) whose append-only
history CAS does not own. But the paper's results bear directly on three CAS surfaces, and its
strongest lesson names a problem we live with daily: **worker task state is prose notes — an
append-only history — and workers burn context headroom exactly the way the paper predicts.**
Confidence: high on mechanism mapping; the benchmarks are synthetic/procedural, so magnitudes
won't transfer to open-ended coding work.

## The paper in one screen

Each step the model sees only `(P, Σ_t, O_t)` — immutable procedural spec, structured execution
state (JSON), latest observation. It emits reasoning (discarded after the step), a validated JSON
state patch (dictionary merge, null = delete), and one action. Prompt footprint is O(1); cumulative
tokens O(T) vs O(T²) for history runtimes. Schemas are authored once per domain, not per task.

| Result | Number |
| --- | --- |
| Long horizon (T=200, warehouse, Gemini-3-Flash) | 0.94 accuracy vs 0.74 ReAct / 0.88 LangGraph-style; 122k tokens vs 2.6M–6.2M |
| Token cut at T=100 | 16.2× vs stateful baseline |
| Noise (50 distractor events/turn) | ≥0.97 vs 0.53 ReAct — distractors are filtered at patch time and never re-enter context |
| External state drift recovery | 0 wasted turns vs 5–14 turns of "hallucination lag" for history runtimes |
| Budget-matched controls (same ~1,800-token budget) | truncation 0.18, summary 0.52, LLMLingua 0.22, SKILL.state 0.94 — **structure, not brevity, is the cause** |
| InterCode CTF / τ-Bench | best pass rates at 40–65% fewer tokens |
| Open-weight failure taxonomy | 68% premature state overwrite, 20% schema confusion, 12% JSON slips → constrained decoding |

Stated limitations: requires the state to be a *sufficient statistic* (fails when the schema can't be
known in advance, when an observation's relevance is recognized late, or **when the trajectory itself
is the target — auditing, debugging, provenance**); single-agent (concurrent writes need deterministic
merge semantics); malformed patches are rejected by the runtime, never corrupting state.

## What it does and does not touch in CAS

| CAS surface | Impact |
| --- | --- |
| Harness runtime (append-only chat history) | **Not ours to change.** Claude Code/Codex own the loop; CAS is hooks + MCP around it. No adoption path for the runtime itself. |
| Factory orchestration state (`epic_status`, `worker_status`, task board) | **Already state-centric.** These are structured, current-state surfaces, not history replays — the paper validates the design we have. |
| Task store as multi-agent merge operator | **CAS is ahead.** The paper punts on concurrent writes; CAS's leases + status transitions are exactly the deterministic merge semantics it says multi-agent needs. |
| **Worker task notes as the resume/continue surface** | **The real hit.** Notes are append-only prose; a resuming or context-cleared worker replays narrative history to reconstruct where it was, and mid-task workers report headroom dropping 95%→20% over one task. The paper's fix — a compact structured state patched as work proceeds, injected instead of history — maps exactly onto `task action=start brief=true` growing a real schema (phase, receipts collected, files touched, next step). Zero-recovery-step resume after `clear_context` is the paper's Table-3 result. |
| SessionStart context injection (rules/memories) | **Supports current design.** The noise result (irrelevant context collapses ReAct 0.68→0.53) endorses CAS's budgeted, scored injection; argues against ever widening it casually. |
| Verification / audit trail / provenance | **Explicitly out of scope for state-centric execution** — the paper's own limitation (3): when the trajectory is the deliverable (audits, provenance), history is the product. CAS's verification audit rows should stay history-shaped. |

## How it composes with the WikiSkill report (same week, complementary)

The two papers split the same raw material by consumer: SKILL.state says **discard** reasoning
traces from the *execution* context (they poison long horizons); WikiSkill says **retain** traces
durably for *learning* (the wiki maintainer mines them). Not a contradiction — a division of labor:

- Execution context: bounded, structured, current-state (SKILL.state) →
- Durable archive: immutable traces for later root-cause mining (WikiSkill; already tasked as cas-62a6) →
- Knowledge loop: gated promotion from mined experience (the cas-c845 epic in flight).

## Candidate follow-up (not tasked — awaiting operator call)

**Structured task execution state**: add a schema'd, patchable state blob to tasks (phase /
receipts / touched files / next action), updated by workers via small patches instead of only
prose notes, and made the primary payload of `start brief=true` and post-`clear_context` re-briefs.
Expected wins, per the paper's mechanism: slower context burn mid-task, lossless resume, and less
narrative replay in worker turns. Prose notes stay for humans and audit; the state blob becomes
the machine resume surface. Moderate effort (task schema + MCP action + worker skill guidance).

## Provenance

- Paper: `~/Downloads/2608.26263v1.pdf`, read in full (16pp), 2026-08-29.
- CAS-side claims from the same-day WikiSkill survey (working tree @ `f9692472`) and live factory
  session observations (worker headroom reports, task-notes flow).
- This report: `docs/reports/2026-08-29-skillstate-vs-cas.md` (source) + `.html` (render).
