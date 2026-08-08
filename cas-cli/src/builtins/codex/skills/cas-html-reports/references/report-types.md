# Report types × audiences

Pick one **type** (what the report is) and one **audience** (who reads it first). The type fixes the
sections and the mandatory visuals; the audience fixes the order of depth and the vocabulary. Every
report also obeys the invariant technical contract (`technical-contract.md`) and the presentation
rules (`presentation-rules.md`).

## The audience axis

The audience never changes the *data*. It changes what leads, how deep the page runs before detail is
relegated, and which words are allowed.

| Audience | Leads with | Depth policy | Vocabulary |
| --- | --- | --- | --- |
| **Executive** | The decision or the single number that matters, in one sentence, above any scroll | One screen of substance; ≤5 KPI cards; at most one chart above the fold; everything else in expandable or lower sections; methodology is present but LAST | Plain business language. No tool names, no file paths, no method jargon in the top third |
| **Practitioner** | The verdict plus the evidence that establishes it | Full depth inline. Evidence tables, reproduction steps, and provenance are primary content, not appendices | Precise and technical. Symbol names, file:line, commands, versions |
| **External** (client, partner, public) | The outcome and what it means for the reader | Full narrative depth, but internal mechanics abstracted to outcomes | No internal jargon, no ticket IDs, no team or agent names, no process narration |

**Audience rewrites the top of the page, not the bottom.** Every audience's report still contains the
evidence and the provenance — the executive version relegates them, it does not delete them.

## The type axis

Each type below lists its required sections **in order**. `[R]` = required, `[O]` = optional.
"Mandatory visual" means the report is not complete without it.

### 1. Investigation / diagnostic

Answer a "why is this happening?" question with evidence.

1. `[R]` **Verdict** — the answer in one or two sentences, first thing on the page. Confidence stated.
2. `[R]` **Overview table** — question, verdict, confidence, scope examined, date, author.
3. `[R]` **Evidence** — one row per piece of evidence: observation, source (file:line, query, log window), what it proves.
4. `[R]` **Reasoning chain** — how the evidence yields the verdict; name what was ruled out and why.
5. `[O]` **Timeline** — when the behavior changed, correlated with commits/deploys.
6. `[R]` **What would falsify this** — the observation that would overturn the verdict.
7. `[R]` **Next actions** — with owner if known.
8. `[R]` **Provenance** — commands run, commit SHA examined, data window.

Mandatory visual: the evidence table. A chart only if a metric moved over time.

### 2. Metrics / mining analysis

Quantitative analysis of a corpus, a system's telemetry, or a mined dataset.

1. `[R]` **Headline finding** — the one number and its direction, with the comparison baseline named.
2. `[R]` **Overview table** — metric, current, baseline, delta, % delta, sample size.
3. `[R]` **Method** — what was measured, over what window, with what filters, and what was excluded.
4. `[R]` **Results** — one section per metric family; variance against baseline shown, not just values.
5. `[R]` **Threats to validity** — sample bias, confounders, measurement error.
6. `[O]` **Segment breakdown**.
7. `[R]` **Provenance** — query text or script path, dataset snapshot identifier, row counts.

Mandatory visuals: overview table plus at least one variance chart (delta vs baseline). Never present
two absolute bars and leave the reader to subtract.

### 3. Decision brief

Recommend one option among several.

1. `[R]` **Recommendation** — the option, in one sentence, first.
2. `[R]` **Decision context** — what must be decided, by when, and who decides.
3. `[R]` **Options table** — one row per option × columns for cost, risk, effort, reversibility, outcome.
4. `[R]` **Why this one** — the deciding criterion, named explicitly.
5. `[R]` **What we give up** — the strongest argument for the runner-up.
6. `[R]` **Reversal cost** — what it takes to undo if wrong.
7. `[O]` **Open questions**.

Mandatory visual: the options comparison table with a visually marked recommended row.

### 4. Comparison / benchmark

Measure alternatives against each other.

1. `[R]` **Winner and margin** — which, by how much, on which metric.
2. `[R]` **Overview table** — candidates × metrics, best value per column emphasized.
3. `[R]` **Harness** — hardware, versions, dataset, iterations, warm-up, what was held constant.
4. `[R]` **Results** — same scale across sibling charts; variance from the reference candidate shown.
5. `[R]` **Where the loser wins** — the conditions that flip the result.
6. `[R]` **Provenance** — exact commands, commit SHAs, raw-result location.

Mandatory visuals: comparison table plus one chart with a **shared axis scale** across all candidates.

### 5. Incident / post-mortem

What broke, why, and what changes.

1. `[R]` **Impact** — who was affected, how badly, for how long. Blast radius in numbers.
2. `[R]` **Overview table** — detection time, start, mitigation, resolution, duration, severity.
3. `[R]` **Timeline** — timestamped, from first symptom to resolution, marking detection and mitigation.
4. `[R]` **Root cause** — the causal chain, not the last thing touched.
5. `[R]` **Why it was not caught** — the missing test, alert, or gate.
6. `[R]` **Corrective actions** — each with owner and a verification method. Blameless language throughout.
7. `[R]` **Provenance** — logs, dashboards, commits, PRs.

Mandatory visual: the timeline. No blame, no individual names as causes.

### 6. Status / release summary

What shipped or where the work stands.

1. `[R]` **State in one line** — shipped / on track / at risk, and the date that matters.
2. `[R]` **Overview table** — workstream, status, owner, target date, change since last report.
3. `[R]` **Changes since last report** — as **was → now**, never as a raw activity log.
4. `[R]` **Risks** — each with likelihood, impact, owner, mitigation.
5. `[O]` **Next period plan**.

Mandatory visual: the status overview table. Status is encoded by text *and* shape/pattern, never by
color alone.

### 7. Financial report (P&L, budget, spend, forecast)

The strictest presentation discipline; this is where the IBCS-derived rules bind hardest.

1. `[R]` **Bottom line** — the result versus plan, in one sentence with the variance stated.
2. `[R]` **KPI row** — 3–5 cards: value, comparison base, absolute variance, % variance, direction.
3. `[R]` **Period statement** — the exact period, comparison period, currency, and units. Once, unambiguously.
4. `[R]` **Variance analysis** — the delta against plan and against prior period, decomposed by driver.
   Show the delta as its own chart, not two value bars side by side.
5. `[R]` **Line-item table** — right-aligned numbers, consistent decimals, **bold sums**, variance columns
   adjacent to their base, negatives in a single consistent notation (parentheses or minus — pick one).
6. `[R]` **Forecast** — hatched/patterned fill, explicitly labeled, with its assumptions named.
7. `[R]` **Assumptions and adjustments** — FX rates, one-offs, reclassifications, restatements.
8. `[R]` **Provenance** — source system, extraction date, close status (preliminary vs final).

Mandatory encodings (invariant across every chart in the report):

- **Actual** = solid fill. **Plan/budget** = outlined (no fill). **Forecast** = hatched fill.
- **Same scale** across sibling charts. A reader compares two charts by eye; unequal scales lie.
- Variance bars are signed and centered on zero; favorable and unfavorable are distinguished by
  direction and pattern, not by color alone.
- No 3-D, no truncated value axes on bar charts, no dual axes.

### 8. Executive / C-suite brief

Everything the executive audience rule demands, hardened into a type.

1. `[R]` **Hero conclusion** — the decision, the number, or the risk. One sentence, largest type on the page.
2. `[R]` **KPI cards** — 3–5, each with value, trend direction, and variance versus the relevant base.
3. `[R]` **One supporting chart** — at most one, above the fold. Choose the one that carries the conclusion.
4. `[R]` **So what** — the implication, in three bullets maximum.
5. `[R]` **The ask** — decision needed, by whom, by when. Explicit or the report has failed.
6. `[O]` **Detail sections** — collapsed or below the fold.
7. `[R]` **Methodology and provenance** — present, complete, and **last**.

Everything above the fold must fit one screen at 1280×800 with no scroll.

### 9. Board / stakeholder update

1. `[R]` **Where we are** — one paragraph, honest.
2. `[R]` **Narrative arc** — where we were → what changed → where we are going. In that order.
3. `[R]` **KPI table with trend** — the same metrics every period, never a rotating cast.
4. `[R]` **Risks** — plainly stated, each with owner, mitigation, and change since last update.
5. `[R]` **Asks** — explicit, addressed to the board, with the decision required.
6. `[O]` **Appendix**.

Mandatory visual: the recurring KPI table with period-over-period trend. Consistency across editions
is itself a requirement — same metrics, same order, same units.

### 10. Client-facing deliverable

1. `[R]` **Executive summary** — outcome-first, in the client's vocabulary.
2. `[R]` **Scope and period** — what was and was not covered.
3. `[R]` **Findings** — each with impact stated in the client's terms.
4. `[R]` **Recommendations** — prioritized, each with effort and expected benefit.
5. `[O]` **Appendix** — supporting detail.

Hygiene, non-negotiable: **no internal ticket IDs, no internal team or agent names, no tool or process
narration, no internal system codenames**. Neutral, unbranded styling. Provenance still exists — it
lives in the markdown source, which is internal, not on the client's page.

### 11. Research / market analysis

1. `[R]` **Thesis** — the claim, one sentence, with confidence stated.
2. `[R]` **Sources table** — source, date, type, reliability. Up front, not buried.
3. `[R]` **Evidence per claim** — every claim carries its citation inline; no claim without a source.
4. `[R]` **Counter-evidence** — the strongest case against the thesis, stated fairly. Required, never omitted.
5. `[R]` **Confidence and gaps** — what would change the conclusion, and what could not be verified.
6. `[O]` **Implications**.

Mandatory visual: the sources table. A claim whose citation is "general knowledge" is not a claim.

## Choosing when a deliverable spans types

Pick the type by the **question the reader is asking**, not by the work that produced it. A mining
analysis whose purpose is to pick a vendor is a decision brief with a metrics section, not a metrics
report. When two types genuinely apply, use the primary type's section order and nest the secondary
type's required sections inside it — never interleave two orders.
