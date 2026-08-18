# Memory-vs-observed-behaviour review queue (M4)

A memory is a claim about the world at the moment it was written. The store has
the levers to age one out — `valid_until`, importance/stability, and the
`opinion_reinforce` / `opinion_weaken` / `opinion_contradict` ops — but nothing
connected them to what was observed afterwards. The measurable consequence on
this machine: **3 of 1,384 live memories carry a `valid_until`** (measured
2026-08-18; the store grows, the 3 does not). A memory
saying "X is broken" keeps being retrieved at full confidence long after X was
fixed.

`docs/analysis/scripts/memory_contradictions.py` builds the queue that closes
that loop, taking M3's deployed-binary verdicts as its evidence of what actually
happened after a fix shipped.

```bash
python3 docs/analysis/scripts/memory_contradictions.py queue \
  --memories-db "$(git rev-parse --show-toplevel)/.cas/cas.db" \
  --seeds docs/analysis/epic-close-gate-seed.json \
  --verdicts ~/.cas/artifacts/cas-2332/epic-close-verdicts.json \
  --output   ~/.cas/artifacts/cas-2332/epic-close-queue.json

python3 docs/analysis/scripts/memory_contradictions.py apply \
  --queue ~/.cas/artifacts/cas-2332/epic-close-queue.json      # dry-run

python3 docs/analysis/scripts/memory_contradictions.py evaluate \
  --queue  ~/.cas/artifacts/cas-2332/epic-close-queue.json \
  --labels ~/.cas/artifacts/cas-2332/queue-labels.json
```

## What it will and will not do

| Verdict | Claim kind | Proposal |
|---|---|---|
| `fixed` | defect assertion, and the memory makes only that claim | `set_valid_until` at the clean-post boundary |
| `fixed` | defect assertion inside a memory carrying other claims | `opinion_weaken` |
| `recurred` | defect assertion / prescription | `opinion_reinforce` |
| `insufficient-post-fix-data` | any | **nothing** |

The last row is the important one. "We did not observe a recurrence" and "we
observed no recurrence across adequate exposure" are different statements, and
only the second licenses ageing a memory out. Unobserved data produces a queue
entry that explains itself and proposes nothing.

The second row came from the live store. Cassy memories are session notes, not
single claims: `2026-08-15-12` catalogues five separate defects, of which this
fix resolves one. An end date applies to the whole memory, so it may only be
proposed when the whole memory is the claim that was measured; otherwise the
evidence is real but its scope is one paragraph, and the additive, reversible
signal is the honest one. Every item reports `independent_claims` so a reviewer
can see which rule applied.

**No automatic mutation, enforced rather than asserted.** `queue` opens the
store read-only (`PRAGMA query_only`, proven by a test that a write raises).
`apply` is dry-run by default; with `--execute` it still refuses any item that
is not `approved` by a named `approver`, and refusals are reported rather than
skipped quietly. The mutation itself runs through `mcp__cas__memory`, so the
memory system's own audit trail records the change instead of this script
writing behind its back.

That last sentence used to say `cas memory`, and it was fiction. Measured on
cas 2.72.0, the binary exposes only `memory share` and `memory unshare`: there
is no `cas memory update` and no `opinion-*` subcommand, so an executed receipt
would have been a list of invocations exiting 2 — the "verification machinery
that is not itself verified" failure this queue exists to catch elsewhere. An
approved item now emits an `mcp__cas__memory` operation, and `--execute`
without an `--executor` that can reach that tool refuses and exits non-zero
rather than reporting a mutation it cannot perform.

**Every row is auditable.** Each item carries the exact token that linked the
memory to the fix (`task_id`, `defect_id`, `fix_commit`, or the matched
phrase), the verdict's epoch boundary and exposure counts, and the evidence
card ids — so a reviewer can reject the *link* rather than only the conclusion
drawn from it.

## Two linking rules, both forced by the data

**Memories name the ticket a defect was filed under, not the commit that fixed
it.** Every live memory asserting the epic-close false positive cites
`cas-b192`; none cites `cas-32ee`, the fix. Linking from the fix alone found
nothing but passing mentions. A seed therefore carries `defect_ids` — the seed
author's explicit, auditable statement of which ticket this fix closes out,
with the commits that justify it recorded alongside.

**The claim is read at the link, not at the top of the memory.** The first
version took a memory's first defect-flavoured sentence and asked whether the
fix appeared in it. On the live store that reads the wrong claim: a
five-instance defect-class memory whose *title* contains the word "defect" got
judged on its title while the sentence naming the fix sat four paragraphs down.
The queue now reads the paragraphs that mention the fix and takes the first
sentence there that both asserts something and is about the fix's subject.

"About the fix's subject" is measured against the fix *title's* words, plus any
whole symptom phrase. Symptom terms are not counted word by word, and that is
not fastidiousness: counting `delivery` from the phrase "delivery is not
accounted for" made a CI-timing note that mentions "a same-day delivery" look
like a claim about the epic close gate. Phrases are distinctive; their words
are not. Links that fail this test still appear in the queue, flagged
`link_is_incidental` with an explaining rationale, because hiding them would
hide the queue's own precision problem.

## Current runs

**The v2.71 wave** (20 fixes): **6 linked pairs, 0 proposals.** Every verdict is
`insufficient-post-fix-data` pending semantic labelling, and every link is a
passing mention — no live memory claims one of those 20 defects. Two
independent reasons for silence, both correct.

**The epic close gate** (`docs/analysis/epic-close-gate-seed.json`, one seed):
**7 linked pairs, 1 proposal.** This is the end-to-end demonstration of the
fixed-defect class named in the task, and it is the first `fixed` verdict
produced on this machine.

## The demonstration, end to end

1. **The fix.** `6bb11c12` (cas-32ee) re-anchors an epic's child at the
   integration commit that accepted its squash, making
   `DeliveryContentPresence::Superseded` reachable from the unmerged branch —
   the exact root cause recorded in memory `2026-08-15-12`. The defect had been
   filed as `cas-b192`.
2. **The deployed boundary.** Built 2026-08-15T21:05Z, but the daemon did not
   start serving a binary carrying it until **2026-08-17T13:12:34Z**, with no
   mixed window. Two days of "merged" that were not "running".
3. **The evidence.** 1,453 clean-post observations against a threshold of 100.
   Zero clean-post lexical symptom matches; all 7 matching cards are clean-pre.
4. **The review.** 41 candidates labelled by hand — M2's top-25 hybrid pool for
   this seed, plus **all 16** of the 1,453 clean-post units carrying the close
   gate's vocabulary (`epic close`, `stranded`, `not accounted`, `blind-merge`,
   `anchor`, `squash`, `commit(s) not on`). All 41 negative, each with a written
   reason. The seven "commit(s) not on main" hits are the *child* close gate
   recording unattributable branch residue and clearing the close — the guard
   behaving correctly, not the symptom.
5. **The verdict.** `fixed`, "no symptom match in sufficient clean-post
   exposure", carrying the evaluation's provenance
   (`candidates_reviewed: 41`, reviewer, timestamp). This verdict was
   unreachable before the M3 contract change described below: an all-negative
   review was indistinguishable from no review.
6. **The proposal.** Memory `2026-08-15-10` ("Measure before following a tool's
   remediation") asserts, in the paragraph citing `cas-b192`, that *the epic
   close path never got the equivalent fix*. That claim is now false, and the
   fix has been observed clean. The queue proposes `opinion_weaken` — not
   `set_valid_until`, because the memory's other two claims (measure before
   running a destructive remediation; treat Cassy's own assertions as claims to
   verify) are untouched by this fix and would have been retired with it.
7. **The refusals.** Dry-run on the queue as built: one item, refused, "not
   approved", exit 1. Approved by a named approver: the `mcp__cas__memory`
   operation is planned and printed. Approved and `--execute`d without an
   executor: refused, exit 1. **No live memory was mutated by this work** —
   adjudication stays human, and the approval above is a sample, not consent.

The `set_valid_until` path is exercised by unit test rather than by this run,
because no live memory in this class is a single claim. That is itself a
finding: claim-level ageing wants claim-level storage, and today it has
memory-level storage.

## The M3 contract change this needed

M3 read `semantic.get(fix_id, {})` and treated an empty map as *not evaluated*,
so a fix whose candidates were all reviewed and correctly rejected could never
reach `fixed`. `--semantic-evidence` now also accepts a declared evaluation,
`{"evaluated": true, "candidates_reviewed": n, "reviewer": ..., "scores": {}}`,
while a bare empty map keeps its fail-closed meaning. A declared evaluation
must name a reviewer and account for at least as many candidates as it reports
positives; and a fix whose reviewer found a positive that maps to no evidence
unit is withheld entirely rather than published as "evaluated, nothing matched",
which would state the opposite of what the reviewer found.

`seed_evidence_inputs.py` gained a `window` command for step 4 above. M2 ranks
candidates over the whole corpus, so a top-N pool is dominated by the period
when a defect was being *discussed* — mostly before the fix served. A `fixed`
verdict is a claim about the clean-post window, so the reviewer is given that
window, with `units_in_window` and `selected` reported so the coverage of the
term filter is stated rather than implied (here: 16 of 1,453).

## Measured precision

Before this, the queue's precision was unmeasured and the doc said so. Now, on
13 labelled pairs across both runs
(`~/.cas/artifacts/cas-2332/queue-labels.json`, with a written reason per label
in `queue-labels-rationales.json`):

| Metric | Value | What it means |
|---|---|---|
| Action precision | **1.0** (1 of 1) | The one proposal made is the right one. n=1 — this is a floor, not a track record. |
| Decision accuracy | **12 of 13 (0.923)** | Every linked pair, including the ones held back: 6/7 on the epic-close run, 6/6 on the v2.71 run. |

Both numbers are reported because the queue makes two kinds of mistake and a
precision figure only shows one. A queue that proposed nothing would score a
perfect precision forever, so `evaluate` scores held-back items too.

**The one mistake is a miss, and it is instructive.** Memory `2026-08-15-12`
asserts "the epic close guard's remediation deletes shipped work" with
`cas-b192` marked OPEN — shipped, and observed clean, so it should have carried
`opinion_weaken`. It did not, because the claim classifier's defect vocabulary
has no entry for "deletes" or "destroys", so the sentence classified as `other`
and the link fell back to incidental. The vocabulary was deliberately **not**
extended to catch this one memory: tuning a classifier on the sample you are
about to measure moves the goalposts to wherever the ball landed. The gap is
recorded here instead, with its cost visible in the number.

## Still open

* **Recall is unmeasured.** Precision counts wrong proposals; nothing yet counts
  the memories that should have been flagged and were not. The single known
  miss above is a data point, not a rate.
* **The v2.71 wave still needs labelling.** Its 20 seeds can now reach `fixed`
  through the declared-evaluation contract, but only 64 of their 300 M2
  candidates map onto M1 units at all; the `window` command is the cheaper
  route to an honest review of each fix's clean-post window.
* **Claim-level storage.** Session-note memories carry many claims and one
  validity window. Until a claim can carry its own, `opinion_weaken` is the
  most precise instrument available for the common case.
