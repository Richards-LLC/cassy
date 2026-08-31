# WikiSkill vs CAS: what the paper validates, and the three mechanisms it says we're missing

**Date:** 2026-08-29 · **Author:** factory supervisor session · **Audience:** practitioner (Daniel)
**Paper:** Tang et al., *WikiSkill: Compiling Agent Experience into Persistent Knowledge for Skill Evolution*, Google Research, arXiv:2608.27454v1 (2026-08-27) · **CAS surveyed at:** working tree of `cas-src` @ `f9692472` (main)

## Thesis

WikiSkill independently validates the architecture CAS already has — a three-layer pipeline from raw
experience to compounding knowledge to executable skills — and CAS is *ahead* of the paper on pruning
and knowledge-page provenance. But the paper's measured results indict exactly the three mechanisms
CAS lacks: **measured validation gating** on skill/rule changes (CAS promotes on a single self-reported
tool call), **immutable trace retention** (CAS hard-deletes raw experience at 30 days), and
**experience→skill provenance** (CAS's link is a free-text tag no code reads). Confidence: high on the
CAS side (code survey with file:line evidence); the paper's numbers are benchmark results whose
transfer to open-world dev work is argued, not proven.

## The paper in one section

WikiSkill organizes an agent workspace into three layers and runs an evolutionary loop over them:

- **Raw layer** (`raw/`) — immutable, write-once execution traces of every rollout.
- **Wiki layer** (`wiki/`) — compounding structured knowledge: pattern pages, an evolution log
  (`logs.md`), and a programmatically-maintained per-skill impact tracker (`skill-impact.md`).
  **Never reset, never rolled back.**
- **Skill layer** (`skills/`) — the active skills, each with a `PURPOSE.md` mapping it back to the
  wiki patterns that motivated it.

Four components per iteration: an **Inference Agent** (runs tasks; sees skills but is *denied* wiki
access), a **Wiki Maintainer** (root-cause analysis over a stratified sample of ≤8 traces — ≤5
failing, ≤3 passing, 15k chars each — producing patch-based pattern-page edits), a **Skill Proposer**
(multi-turn ReAct agent reading the wiki index + skill-impact + task outcomes; one atomic skill
proposal per iteration), and a **Gating & Rollback** mechanism (proposal accepted only if measured
validation score improves; skills roll back, the wiki never does).

Headline numbers:

| Result | Number | Where |
| --- | --- | --- |
| WikiSkill vs best competing skill-evolution method (avg across 5 benchmarks) | +3.3 to +12.0 pts per model | Table 1 |
| Ablation: giving the Skill Proposer wiki access (i.e. persistent knowledge) | 48.7% → 63.7% (**+15.0**) | Table 3 |
| Ablation: giving the *Inference Agent* wiki access during rollouts | 63.7% → 60.9% (**−2.8**, hurts) | Table 3 |
| Skills evolved by one model, run by another | frequently beats self-evolved; e.g. Qwen-27B skills lift Gemma-31B LiveMath 33.9→73.7 | Table 2 |
| Optimizer cost per iteration (full-batch) | O(1): 1 + T_ReAct LLM calls | Appendix D |
| Late-stage skill refinement (iterations 5–7) still accepted | 4–28% of accepted updates | Table 5 |

Stated limitations: no skill retrieval/triggering evaluated (skills are fully injected), strict gating
discards neutral proposals, **no wiki pruning mechanism**, benchmarks don't cover very long-horizon work.

## Layer-by-layer mapping onto CAS

| WikiSkill mechanism | CAS analogue | Status |
| --- | --- | --- |
| Raw layer: immutable write-once traces | `entries` (observations), `events`, `recordings`; Claude Code transcripts parsed but never retained | ✗ Mutable in place; `events`/`recordings` hard-deleted at 30 days (`cas-cli/src/daemon/maintenance.rs:219-247`) |
| Wiki pattern pages compounding from experience | Memories/learnings (tiers, decay, consolidation) + knowledge pages | ◐ Knowledge pages genuinely compound with real provenance — but read **only the filesystem**, never experience (`cas-cli/src/knowledge/sources.rs:143-157`) |
| Evolution log (`logs.md`) | None — no history/versions table for rules, skills, or memories; all updates destructive in-place | ✗ |
| Per-skill impact tracker (`skill-impact.md`), programmatically maintained | `rules.surface_count` — schema-only, **never incremented anywhere**; `helpful_count`/`usage_count` are agent self-report | ✗ |
| Skill→knowledge provenance (`PURPOSE.md`) | Free-text tag `from_learning`; `Rule.source_ids` populated only by dead code (`cas-cli/src/rules/mod.rs`, zero non-test callers); `Entry` and `Skill` have no provenance field at all | ✗ |
| Gating: accept skill change only if measured validation score improves; rollback otherwise | `Skill.validation_script`/`preconditions`/`postconditions` exist in schema (m073/m074) and are **never executed**; `cas_skill_create` writes + syncs to disk in one shot | ✗ |
| Wiki Maintainer (root-cause analysis over sampled traces) | `learning-reviewer`/`duplicate-detector` — prompt-only agents, threshold-triggered, judging the *text* of learnings, not traces | ◐ |
| Skill Proposer (reads impact history to avoid repeating rejected proposals) | `learning-reviewer` prompt table maps phrasing → outcome ("Always X" → rule); no impact history exists to read | ◐ |
| Wiki pruning | Decay/tiering (`daemon/decay.rs`), consolidation, dedup, archive | ✓ **CAS is ahead** — the paper lists this as an open limitation |

## The five sharpest findings

1. **One self-reported call promotes a rule to Proven.** `cas_rule_helpful`
   (`cas-cli/src/mcp/tools/core/rules.rs:39-75`): a single `rule action=helpful` flips
   Draft→Proven and immediately syncs the rule into `.claude/rules/` for injection into every
   future session. No threshold, no second party, no measured effect. `harmful` increments a
   counter and **never demotes** (`rules.rs:191-216`). WikiSkill's equivalent decision requires a
   measured validation-score improvement, and its ablation puts +15pts on doing this right.

2. **The skill validation schema is inert.** `Skill.validation_script`, `preconditions`,
   `postconditions` (`crates/cas-types/src/skill.rs:204-218`) survive migration, storage, and
   round-tripping — and are never executed or evaluated anywhere in the tree. The gate WikiSkill
   says is essential exists in CAS as three dormant columns.

3. **CAS's only true measurement loop is firewalled from every decision that matters.**
   `retrieval_outcomes` (used/helpful/ignored/corrected/harmful, append-only, privacy-hashed) is
   well-engineered — and by explicit design (`crates/cas-store/src/retrieval_store.rs:1-6`)
   "never mutates entry/rule counters or global search weights." Its entire effect is a
   ±0.20-clamped nudge to ambient-recall ranking (`cas-cli/src/ambient_recall.rs:2178-2191`).
   Promotion, demotion, rule injection order, and learning review consume none of it.

4. **No rollback exists for any knowledge artifact.** Rules, skills, and memories update via
   destructive in-place `UPDATE`; rule "archive" in the reviewer prompt is a hard `DELETE` in code
   (`rules.rs:219-232` vs `.claude/agents/rule-reviewer.md:28`). WikiSkill's loop is only safe to
   run autonomously *because* every skill change is reversible and every proposal outcome is logged.

5. **Provenance is destroyed at every merge point.** Consolidation (`cas-cli/src/daemon/decay.rs:129-166`)
   archives source memories and writes a fresh merged entry with no link back. Learnings derived
   from observations carry no source ids (`daemon/observation.rs:52-59`). The paper's case study
   shows why this matters: the Skill Proposer avoided repeating a rejected proposal *because* the
   audit trail said it had been tried.

## Where CAS is ahead of the paper

- **Pruning and lifecycle** — decay curves, tier demotion, consolidation, dedup: the paper names
  wiki growth as an unsolved limitation; CAS has a working answer.
- **Knowledge-page provenance engineering** — per-fragment source markers with a forgery threat
  model (`cas-cli/src/knowledge/merge.rs:232-268`), cascade deletes, tombstones, locked pages.
  Better than anything in the paper — it just points at files, not experience.
- **A real, immutable gate exists — for task closure.** The verification subsystem
  (`crates/cas-store/src/verification_store.rs`, external gates, audit rows) is exactly the
  gating discipline WikiSkill wants; it's aimed at task delivery rather than knowledge artifacts.
- **Cross-harness skill distribution.** The paper's cross-model transfer result (skills evolved by
  one model helping another, Table 2) empirically supports CAS shipping identical skills to
  Claude/Codex/Grok — with the caveat that model-specific workarounds can transfer negatively.

## Caveats — the case against acting on this uncritically

- WikiSkill runs on benchmarks with ground-truth scoring and train/val/test splits. CAS operates on
  open-world dev tasks with no oracle; "measured validation" must be proxied (verification verdicts,
  test outcomes, `friction_score`, retrieval outcomes), and proxies can be gamed by the same LLM
  self-report problem CAS already has.
- Full skill injection (no retrieval) is the paper's stated confound-avoidance choice; CAS's
  injection is budgeted and scored, so their acceptance numbers don't transfer directly.
- The −2.8pt result for Inference-Agent wiki access is about *skill-development* trajectory quality,
  not about whether injecting memories helps the immediate task — it does not argue against ambient
  recall as such.

## Recommended changes, prioritized

| # | Change | Paper evidence | Anchor |
| --- | --- | --- | --- |
| 1 | Replace one-call Draft→Proven with an outcome-threshold fed by `retrieval_outcomes` + verification verdicts; add a demotion path from `harmful`/`corrected` | +15pt gating ablation | `rules.rs:39-75`, `retrieval_store.rs:1-6` |
| 2 | Add `rule_versions`/`skill_versions` history + make delete a tombstone (code already has `RuleStatus::Retired`, unused) | Rollback is what makes autonomous evolution safe | `rules.rs:219-232` |
| 3 | Execute `validation_script` as a create/update gate on skills (schema already shipped) | Gating & Rollback §3.2.4 | `skill.rs:204-218` |
| 4 | Populate provenance: `Entry.source_ids`, revive the dead `Rule.source_ids` path, preserve links through consolidation | Case study: audit trail prevented repeated failed proposals | `decay.rs:129-166`, `cas-cli/src/rules/mod.rs` |
| 5 | Make `surface_count` real (increment at injection) and correlate with session outcome — a `skill-impact.md` analogue | Skill Proposer reads impact history | `build_start.rs:436-505` |
| 6 | Append-only archive for traces past 30 days (compressed, or hash-chained summaries) instead of hard delete | Raw layer is write-once | `maintenance.rs:219-247` |

Each row is a candidate factory task; none has been created — awaiting operator sign-off.

## Provenance

- Paper: `~/Downloads/2608.27454v1.pdf`, read in full (28pp), 2026-08-29.
- CAS evidence: read-only code survey of `cas-src` working tree @ `f9692472`, 2026-08-29,
  ~90 tool calls; all file:line references verified at survey time.
- This report: `docs/reports/2026-08-29-wikiskill-vs-cas.md` (source) + `.html` (render).
