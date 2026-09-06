# Report types × audiences

Pick one **type** (what the report is) and one **audience** (who reads it first). The type fixes the
sections, the mandatory visuals, and the **hero figure** — the form that shows the argument above the
fold; the audience fixes the order of depth and the vocabulary. Every report also obeys the invariant
technical contract (`technical-contract.md`) and the presentation rules (`presentation-rules.md`),
and every report commits a concept brief (`cas-ui-craft`) that names its hero and the reason for it.

## The audience axis

The audience never changes the *data*. It changes what leads, how deep the page runs before detail is
relegated, and which words are allowed.

| Audience | Leads with | Depth policy | Vocabulary |
| --- | --- | --- | --- |
| **Executive** | The hero figure, then the selected type's one-sentence lead: a decision/number for evaluative work, a capability/system takeaway for explanatory work | One screen of substance; ≤5 KPI cards when the selected type calls for them; at most one chart or diagram above the fold; everything else in expandable or lower sections; methodology is present but LAST | Plain business language. No tool names, no file paths, no method jargon in the top third |
| **Practitioner** | The hero figure and the verdict, then the evidence that establishes it | Full depth inline. Evidence tables, reproduction steps, and provenance are primary content, not appendices | Precise and technical. Symbol names, file:line, commands, versions |
| **External** (client, partner, public) | The outcome and what it means for the reader | Full narrative depth, but internal mechanics abstracted to outcomes | No internal jargon, no ticket IDs, no team or agent names, no process narration |

**Audience rewrites the top of the page, not the bottom.** Every audience's report still contains the
evidence and the provenance — the executive version relegates them, it does not delete them.

## The hero figure per type

The hero is the first thing the reader sees and it shows the argument; a paragraph, a KPI row, or a
table of everything is not a hero. The default below is the form that fits each type's question; take
another from the `cas-ui-craft` vocabulary when the data argues for it, and write the reason in the
concept brief either way.

| Type | Default hero | Why that form |
| --- | --- | --- |
| Investigation / diagnostic | Annotated timeline | The verdict is a causal claim about *when*; the reader must see symptom, change, and fix on one axis |
| Metrics / mining analysis | Variance ladder or slope against baseline | The finding is a delta; the form makes the delta the mark |
| Decision brief | Ledger (claim vs evidence per option, or per row of the status quo) | The recommendation rests on a contradiction the reader must see side by side |
| Comparison / benchmark | Small multiples on one shared scale | Candidates are compared by eye; shared scale keeps the eye honest, the crossover is annotated |
| Incident / post-mortem | Annotated timeline with detection and mitigation marked | Duration and the gap between symptom and detection are the impact |
| Status / release summary | Was → now dumbbell or slope per workstream | The report is about change since last time |
| Financial report | Signed variance bars on a zero line, forecast hatched | The delta is the message and the scenario encoding is the discipline |
| Product / feature showcase | Capability map or product journey | The thesis is what is newly possible along a path |
| System / architecture explainer | End-to-end system flow with labeled arrows | The takeaway is what moves through the system |
| Executive / C-suite brief | The one figure that carries the conclusion, plus the ask as a closing figure | One screen, one argument, one decision |
| Board / stakeholder update | Recurring KPI small multiples with trend | Same metrics every edition; trend is the story |
| Client-facing deliverable | Outcome figure in the client's units | The reader wants the result, not the method |
| Research / market analysis | Thesis with evidence-for and evidence-against as a two-column ledger | Counter-evidence is required, so the form gives it equal space |

**Executive is an audience, not an automatic decision brief.** A product showcase or system explainer
for executives leads with its capability or system takeaway; it does not require an ask, a decision,
or KPI cards unless the selected type independently calls for them.

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

### 8. Product / feature showcase

Show what was built through the reader's experience, then prove that the capability is real. This is
not a release log or a decision brief.

1. `[R]` **Capability thesis** — what is newly possible for the reader, in one sentence, first.
2. `[R]` **Product journey or capability map** — the reader path or grouped capabilities, with the
   outcome at each step. This is the primary mandatory visual.
3. `[R]` **What changed and why it matters** — capability-by-capability explanation in reader language.
4. `[R]` **Capability-to-proof map** — a table pairing each capability with concrete proof (demo,
   screenshot description, test, measured behavior, or source) and its reader outcome.
5. `[R]` **Annotated experience walkthrough** — a concise, ordered walkthrough of the meaningful path.
6. `[R]` **Scope and boundary** — label each capability **current**, **planned**, or **not included**;
   never present planned work as available.
7. `[R]` **Evidence and provenance** — where the proof came from and when it was observed.

Mandatory visual: a product journey or capability map. The capability-to-proof table is mandatory
evidence, not a substitute for the visual. No decision, ask, or KPI row is required solely because
the audience is executive.

### 9. System / architecture explainer

Explain the operating system a reader is trying to understand, from inputs through outcomes. It is
not an architecture decision record.

1. `[R]` **System takeaway** — what the system does end to end, in one sentence, first.
2. `[R]` **End-to-end system flow or loop** — above the fold: inputs, transformations, outputs, and
   every labeled arrow. This is the primary mandatory visual.
3. `[R]` **Inputs and outputs** — who or what enters the system, what leaves it, and the interface or
   format at each boundary.
4. `[R]` **Components and layers** — each component's responsibility, owner/system boundary, and
   connections.
5. `[R]` **Data lifecycle** — creation, validation, storage, retrieval, retention/deletion, and any
   handoff to another system.
6. `[R]` **Annotated walkthrough** — follow one representative input through the flow in order.
7. `[R]` **Invariants and failure boundaries** — what must remain true, how failures are contained,
   and what the system explicitly does not guarantee.
8. `[R]` **Component map** — a visual grouping components/layers and their interfaces.
9. `[R]` **Current versus planned boundary** — visibly mark planned components, interfaces, and flows;
   do not connect them visually as though they already operate.
10. `[R]` **Evidence and provenance** — source files, commands, traces, commits, or observations that
    establish the explanation.

Mandatory visuals: an end-to-end system flow or loop plus a component map. A system diagram is a
reading aid only when arrows are labeled and its full meaning is available in text. No decision, ask,
or KPI row is required solely because the audience is executive.

### 10. Executive / C-suite brief

Everything the executive audience rule demands, hardened into a type.

1. `[R]` **Hero conclusion** — the decision, the number, or the risk. One sentence, largest type on the page.
2. `[R]` **KPI cards** — 3–5, each with value, trend direction, and variance versus the relevant base.
3. `[R]` **One supporting chart** — at most one, above the fold. Choose the one that carries the conclusion.
4. `[R]` **So what** — the implication, in three bullets maximum.
5. `[R]` **The ask** — decision needed, by whom, by when. Explicit or the report has failed.
6. `[O]` **Detail sections** — collapsed or below the fold.
7. `[R]` **Methodology and provenance** — present, complete, and **last**.

Everything above the fold must fit one screen at 1280×800 with no scroll.

### 11. Board / stakeholder update

1. `[R]` **Where we are** — one paragraph, honest.
2. `[R]` **Narrative arc** — where we were → what changed → where we are going. In that order.
3. `[R]` **KPI table with trend** — the same metrics every period, never a rotating cast.
4. `[R]` **Risks** — plainly stated, each with owner, mitigation, and change since last update.
5. `[R]` **Asks** — explicit, addressed to the board, with the decision required.
6. `[O]` **Appendix**.

Mandatory visual: the recurring KPI table with period-over-period trend. Consistency across editions
is itself a requirement — same metrics, same order, same units.

### 12. Client-facing deliverable

1. `[R]` **Executive summary** — outcome-first, in the client's vocabulary.
2. `[R]` **Scope and period** — what was and was not covered.
3. `[R]` **Findings** — each with impact stated in the client's terms.
4. `[R]` **Recommendations** — prioritized, each with effort and expected benefit.
5. `[O]` **Appendix** — supporting detail.

Hygiene, non-negotiable: **no internal ticket IDs, no internal team or agent names, no tool or process
narration, no internal system codenames**. Neutral, unbranded styling. Provenance still exists — it
lives in the markdown source, which is internal, not on the client's page.

### 13. Research / market analysis

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
type's required sections inside it — never interleave two orders. For a report that must both
showcase a product and explain its mechanics, make **Product / feature showcase** primary: place the
**System / architecture explainer** after the capability-to-proof map, using the explainer's full
section order. Make the reverse choice only when the reader's primary question is how the system works;
in that case, nest a concise capability-to-proof map inside the explainer's annotated walkthrough.

## Reusable accessible system-flow pattern

Use this compact, self-contained pattern for the required flow diagram. Replace the labels and text
alternative with the report's real system; do not use it as decorative art. The diagram is static, so
it introduces no keyboard interaction. If a report adds clickable nodes or controls, each must be
reachable and operable by keyboard, with visible focus.

```html
<style>
.flow { max-width: 48rem; border: 1px solid #555; padding: .75rem; }
.flow svg { width: 100%; height: auto; }
.flow .node { fill: #eef4ff; stroke: #174ea6; stroke-width: 2; }
.flow .planned { fill: #fff; stroke-dasharray: 6 4; }
@media print { .flow { break-inside: avoid; border-color: #000; } .flow .planned { fill: none; } }
</style>
<figure class="flow" aria-describedby="flow-summary">
  <svg viewBox="0 0 720 160" role="img" aria-labelledby="flow-title flow-desc">
    <title id="flow-title">Current request-processing flow</title>
    <desc id="flow-desc">A request is validated, stored, then returned. A dashed planned analytics
      component receives an optional copy after storage.</desc>
    <defs><marker id="arrow" markerWidth="8" markerHeight="8" refX="7" refY="4" orient="auto"><path d="M0,0 L8,4 L0,8z"/></marker></defs>
    <rect class="node" x="20" y="52" width="150" height="48" rx="6"/><text x="95" y="81" text-anchor="middle">Request</text>
    <rect class="node" x="285" y="52" width="150" height="48" rx="6"/><text x="360" y="81" text-anchor="middle">Validate + store</text>
    <rect class="node" x="550" y="52" width="150" height="48" rx="6"/><text x="625" y="81" text-anchor="middle">Response</text>
    <path d="M170,76 H280 M435,76 H545" stroke="#174ea6" stroke-width="2" marker-end="url(#arrow)"/>
    <text x="225" y="45" text-anchor="middle">valid request</text><text x="490" y="45" text-anchor="middle">stored result</text>
    <rect class="node planned" x="285" y="118" width="150" height="30" rx="6"/><text x="360" y="138" text-anchor="middle">Planned analytics</text>
    <path d="M360,100 V114" stroke="#174ea6" stroke-dasharray="6 4" marker-end="url(#arrow)"/><text x="447" y="135">optional copy (planned)</text>
  </svg>
  <figcaption id="flow-summary">Current path: request → validate and store → response. Planned path:
    an optional post-storage copy to analytics; it is dashed and not part of the current system.</figcaption>
  <details open><summary>Text alternative and boundary notes</summary><p>Validation rejects malformed
    requests before storage. Storage is the current durability boundary. Analytics is planned, so no
    current request depends on it.</p></details>
</figure>
```
