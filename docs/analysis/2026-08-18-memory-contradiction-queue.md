# Memory-vs-observed-behaviour review queue (M4)

A memory is a claim about the world at the moment it was written. The store has
the levers to age one out — `valid_until`, importance/stability, and the
`opinion_reinforce` / `opinion_weaken` / `opinion_contradict` ops — but nothing
connected them to what was observed afterwards. The measurable consequence on
this machine: **3 of 1,383 live memories carry a `valid_until`**. A memory
saying "X is broken" keeps being retrieved at full confidence long after X was
fixed.

`docs/analysis/scripts/memory_contradictions.py` builds the queue that closes
that loop, taking M3's deployed-binary verdicts as its evidence of what actually
happened after a fix shipped.

```bash
python3 docs/analysis/scripts/memory_contradictions.py queue \
  --memories-db "$(git rev-parse --show-toplevel)/.cas/cas.db" \
  --seeds docs/analysis/v2.71.0-fix-wave-seeds.json \
  --verdicts ~/.cas/artifacts/cas-2332/v2.71-deployed-epoch-verdicts-lexical-only.json \
  --output   ~/.cas/artifacts/cas-2332/memory-review-queue.json

python3 docs/analysis/scripts/memory_contradictions.py apply \
  --queue ~/.cas/artifacts/cas-2332/memory-review-queue.json      # dry-run
```

## What it will and will not do

| Verdict | Claim kind | Proposal |
|---|---|---|
| `fixed` | defect assertion | `set_valid_until` at the clean-post boundary |
| `recurred` | defect assertion / prescription | `opinion_reinforce` |
| `insufficient-post-fix-data` | any | **nothing** |

The third row is the important one. "We did not observe a recurrence" and "we
observed no recurrence across adequate exposure" are different statements, and
only the second licenses ageing a memory out. Unobserved data produces a queue
entry that explains itself and proposes nothing.

**No automatic mutation, enforced rather than asserted.** `queue` opens the
store read-only (`PRAGMA query_only`, proven by a test that a write raises).
`apply` is dry-run by default; with `--execute` it still refuses any item that
is not `approved` by a named `approver`, and refusals are reported rather than
skipped quietly. The mutation itself runs through `cas memory`, so the memory
system's own audit trail records the change instead of this script writing
behind its back.

**Every row is auditable.** Each item carries the exact token that linked the
memory to the fix (`task_id`, `fix_commit`, or the matched phrase), the
verdict's epoch boundary and exposure counts, and the evidence card ids — so a
reviewer can reject the *link* rather than only the conclusion drawn from it.

## The incidental-link rule came from the data

Linking on task-id mention alone looked fine until it was run against the live
store: it matched a CI-timing note whose claim is about a 10-minute budget and
which merely cites three fix ids in passing. An item therefore carries an action
only when the asserting sentence both mentions the fix *and* shares at least two
content words with the fix's title/terms. Incidental links still appear in the
queue — flagged `link_is_incidental` with an explaining rationale — because
hiding them would hide the queue's own precision problem.

## Current run

Against the v2.71 wave: **6 linked pairs, 0 proposals.** Every pair is
incidental (no live memory actually claims one of these 20 defects), and every
verdict is `insufficient-post-fix-data` pending the semantic-labelling step
described in
[2026-08-18-v2.71-deployed-epoch-seed-run.md](2026-08-18-v2.71-deployed-epoch-seed-run.md).
Two independent reasons for silence, both correct. An empty action list here is
the honest output, not a missing feature.

## Still open

* **Precision measurement.** `evaluate --queue --labels` computes precision over
  a labelled sample and reports unlabelled items instead of assuming them
  correct. It cannot be run meaningfully until the queue proposes something,
  which needs the labelling step below. The queue must not be trusted before
  that number exists.
* **The demonstration case** ("a memory saying a defect exists is flagged
  fixed-post-epoch, with `valid_until` proposed") is implemented and unit-tested
  end to end, but has not yet run on a live `fixed` verdict for the reason above.
* **M3 contract gap.** M3 cannot currently express "evaluated, nothing matched":
  an empty semantic map reads as "not evaluated"
  (`deployed_epoch_verdicts.py:187`), so a fix whose candidates were all
  correctly rejected can never reach `fixed`. Until that is fixed, no v2.71 seed
  can produce a `fixed` verdict even after review.
