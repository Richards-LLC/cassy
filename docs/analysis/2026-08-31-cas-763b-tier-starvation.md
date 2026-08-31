# cas-763b — is the Helpful-Memories working set starved? Yes, and the knee is measurable

**Date:** 2026-08-31 · **Task:** cas-763b (spike, EPIC cas-8fac) · **By:** fast-robin-31
**Store read READ-ONLY** (`file:…/.cas/cas.db?mode=ro`, no copy, no writes).
**Harness:** `cas-cli/tests/retrieval_eval_test.rs` (cas-e0ed / cas-b06c), 56 judged
prompt-contexts over 189 real entries.

## Answer in one line

The working set is starved, the cause is **stability decay — not age, access, or
negative feedback**, and protecting `importance >= 0.9` recovers **100% of the
achievable precision while leaving 75% of the corpus archived**.

## Q1 — thresholds, and what actually fired

`cas-cli/src/daemon/decay.rs::apply_memory_decay` has six demotion branches and
**no promotion branch of any kind**:

| # | branch | condition |
|---|---|---|
| 1 | → archive | `is_expired()` |
| 2 | working → cold | `Observation` && `feedback_score() <= 0` |
| 3 | working → cold | `importance < 0.3` && `feedback_score() <= 0` |
| 4 | → archive | `feedback_score() < 0` |
| 5 | working → cold | `stability < 0.3` |
| 6 | cold → archive | `stability < 0.15` |

Stability itself is driven by `Entry::apply_decay` (`cas-types/src/entry/behavior.rs:379`),
called as `apply_decay(days_since_access / 30.0)` once **per daemon cycle**.

Measured on the live store (1,484 archive-tier / 33 working / **0 cold**, active rows only):

| attribution | count |
|---|---|
| archived with `stability < 0.15` (branches 5→6) | **1,441 of 1,484 — 97%** |
| archived by expiry (branch 1) | 5 |
| archived by negative feedback (branch 4) | **0** |
| demoted by `importance < 0.3` (branch 3) | **0** |
| never accessed | 1,319 |
| youngest archived entry | **11 days old** |
| mean age of archived entries | 96 days |

So the answer to "age vs access" is **neither, directly** — it is the stability
term, and 97% of the archive arrived through it. Branches 3 and 4, the two that
encode an actual quality judgement, never fired once.

Two structural notes fall out of the same numbers:

- **The cold tier is empty (0 rows).** It is a pass-through on the way to
  archive, not a resting place. Any policy phrased as "never demote *below
  cold*" is therefore a no-op for retrieval — see Q4.
- **An entry becomes invisible to Helpful Memories 11 days after creation** if
  nothing accesses it. That is the youngest archived row in the store.

## Q2 — are we archiving what the operator marked valuable?

Yes, and at scale.

| archive-tier, active | count |
|---|---|
| total | 1,484 |
| `importance >= 0.8` | **346** |
| `importance >= 0.9` | 63 |
| `helpful_count > 0` | 4 |
| `type = preference` | 36 |
| `harmful_count > 0` | **0** |
| `importance >= 0.8` **and** archived by the stability path | **326** |

326 entries the operator scored ≥ 0.8 were archived purely by decay. Not one
entry in the store has ever been marked harmful, so nothing reached archive
because it was judged bad — the corpus contains no negative signal at all.

## Q3 — does promotion ever fire?

**No, and the data proves it independently of the code.**

- There is no promotion branch in `apply_memory_decay`. The only way a tier
  rises is the explicit operator action `memory action=set_tier`.
- **74 archive-tier entries were accessed within the last 30 days and are still
  archive-tier.** They were retrieved, and stayed invisible to SessionStart.
- 176 entries have ever been accessed; 85 within 30 days.

Access is recorded and then ignored by tiering.

## Q4 — measured impact of each option

Each row materialises the fixture corpus with that policy's protected set as
`working` and everything else `archive`, then runs the **production**
Helpful-Memories selector (`helpful_memories_production`, seeded_task).

| option | eligible | P@5 | R@5 | hit | distinct |
|---|---|---|---|---|---|
| **A — today** (working only) | 14/189 | 0.0107 | 0.0096 | 2/56 | 43 |
| C6 — `helpful_count > 0` only | 16/189 | 0.0107 | 0.0096 | 2/56 | 43 |
| C5 — `importance >= 1.0` | 20/189 | 0.0286 | 0.0310 | 7/56 | 50 |
| C4 — `importance >= 0.95` | 27/189 | 0.0429 | 0.0441 | 9/56 | 50 |
| **C2 — `importance >= 0.9`** | **47/189** | **0.0500** | **0.0521** | **10/56** | 51 |
| C1 — `importance >= 0.8` | 120/189 | 0.0500 | 0.0521 | 10/56 | 51 |
| C3 — preferences + `>= 0.8` | 126/189 | 0.0500 | 0.0521 | 10/56 | 51 |
| B — no starvation (all working) | 189/189 | 0.0500 | 0.0521 | 10/56 | 51 |

Read it as a curve with a knee:

- **`importance >= 0.9` is the knee.** 47 of 189 entries eligible — a quarter of
  the corpus — and it reaches the same P@5/R@5 as making *everything* eligible.
  Widening to 0.8 (120 entries) or to everything (189) buys **nothing** at @5.
- Tightening past the knee costs real precision: 0.95 loses 14%, 1.0 loses 43%.
- **`helpful_count` alone is useless as a protection signal** — only 4 entries in
  the whole fixture carry it, so C6 is indistinguishable from today. That is a
  corpus property, not a code property, and it is the same signal-starvation
  that made cas-e979's feedback arm inert. It becomes viable only after
  cas-8f93's attribution populates helpful/harmful at scale.

**Honest limit:** C2/C1/C3/B are identical to four decimals, so `@5` cannot
separate them. C2 is therefore "the cheapest policy that reaches the ceiling on
this fixture", not "provably better than C1". A larger fixture or a deeper cutoff
could separate them; this one cannot.

## Recommendation

**Protect curated entries at the `working` tier, not at `cold`.** Specifically:
never auto-demote an entry below `working` while `importance >= 0.9` or
`helpful_count > 0`.

Three reasons it must say *working* and not *cold*:

1. `MemoryTier::is_active()` (`cas-types/src/entry.rs:183`) is
   `InContext | Working`. **Cold is not eligible for Helpful Memories.** A rule
   that floors curated entries at cold changes nothing a session can see.
2. The cold tier is empty in practice (0 rows) — it does not function as a tier.
3. The measurement above is of *eligibility*, and only working/in-context confer it.

Expected effect, from the table: **P@5 0.0107 → 0.0500 (4.7×), hit-rate 2/56 →
10/56**, with 142 of 189 entries still archived. This is the single largest
retrieval gain measured anywhere in this epic, and it is a policy change rather
than a ranking change.

Secondary, cheaper and independent: **promote on access.** 74 entries were read
in the last 30 days and left in archive. Promoting archive → working on access
costs nothing and is self-limiting. It could not be measured here because the
fixture deliberately carries no `last_accessed` (see the harness determinism
note), so it is recommended on mechanism, not on a number — flagged as such.

**Not recommended:** protecting on `helpful_count` alone (C6 — measurably worth
nothing today), or widening to `importance >= 0.8` (C1 — 2.5× the working set
for zero measured gain).

## Follow-up task spec (draft — not implemented here)

> **Title:** Curated memories must not auto-demote below `working`: floor
> `importance >= 0.9` / `helpful_count > 0` in decay.rs, and promote on access
>
> **Change:** in `cas-cli/src/daemon/decay.rs::apply_memory_decay`, guard the
> stability demotions (branches 5 and 6) so they cannot move an entry below
> `MemoryTier::Working` when `importance >= 0.9 || helpful_count > 0`. Expiry
> (branch 1) and negative feedback (branch 4) must still override — they are
> correctness boundaries, not decay. Separately, promote `Archive → Working` when
> an entry is accessed.
>
> **Threshold is configurable**, defaulting to 0.9, so the knee can be retuned
> without a code change when the fixture grows.
>
> **Acceptance:** the harness `live_tiers` row for `helpful_memories_production`
> seeded_task moves from P@5 0.0107 to ≈0.0500, re-baselined via the documented
> procedure with the reason in the commit message; a test asserts a curated entry
> survives a decay pass that archives an uncurated one; a test asserts an expired
> or harmful-marked curated entry is still demoted.
>
> **Watch for:** this raises the working set from 33 to a few hundred entries on
> the live store, which changes SessionStart token budget pressure. The
> Helpful-Memories section is capped at `limit = 5`, so the injected prompt does
> not grow — but `merge_entries` and the scorer now sort a larger candidate set
> per session. Measure the hook latency before and after.

## Method / caveats

- All store reads were `mode=ro` on the live DB; nothing was written or copied.
- Option rows were measured by materialising the committed fixture with each
  protected set, through the real production selector — not modelled.
- The fixture's 189 entries are a hand-picked topical slice of the live corpus,
  so its importance distribution (119 at ≥0.8, 40 at ≥0.9) is richer than the
  store's (346 of 1,484 at ≥0.8). The *shape* of the curve should hold; the exact
  eligible counts would differ on the full store.
- No decay behaviour was changed by this spike, per its brief.
