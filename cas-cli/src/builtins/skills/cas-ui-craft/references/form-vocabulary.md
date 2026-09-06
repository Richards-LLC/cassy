# Form vocabulary

Choose the form by the reader's task, not by the data's shape. Tables and cards are the last
resort, not the default. Construction notes assume the Petrastella tokens; every figure also
carries a claim-title, source line, text alternative, and data table (`cas-dataviz`).

## Forms

| Form | Reader's task | Construction | Fails when |
| --- | --- | --- | --- |
| **Verdict hero** | get the conclusion | eyebrow → serif sentence ≤ 22 words → decisive figure → 3px verdict rule → provenance line; surface-hero | anything else sits above it |
| **Slope chart** | see one change between two states across categories | two vertical axes, one 1.5px segment per category, the decisive segment 2.5px verdict, others ink-muted; labels at both ends, no legend | more than ~12 categories; three or more states (use a bump chart or small multiples) |
| **Small multiples** | compare several similarly shaped series | 3–8 panels, shared scale and alignment, identical size, one claim-title over the grid, the panel that matters gets the verdict mark | panels use different scales; the grid exceeds two rows |
| **Dot plot** | compare magnitudes or positions with precision | sorted categories on the y axis, one dot each, a hairline from axis to dot, a verdict-soft band for the reference range | dots overlap (jitter or aggregate) |
| **Dumbbell** | before/after per category | two dots joined by a segment, the after-dot filled, the before-dot hollow; sort by delta | deltas are near zero for most rows (table them) |
| **Waffle plot** | show a share of a whole | 10×10 grid of 8px squares, filled squares in verdict, remainder in line; the count printed | more than three categories (stack a bar instead) |
| **Annotated timeline** | see sequence, gaps, and the decisive moment | vertical spine, mono timestamps left, events right, elapsed time printed on the spine, one verdict event with a marginal-note annotation | events are unordered or the gaps carry no meaning |
| **Evidence ledger** | audit exact values with their sources | ruled table, mono tabular numbers, sum row, last column is the source; the decisive row banded verdict-soft | it is the first thing on the page |
| **Stat strip** | glance at three to five current values | one ruled line of hero-number figures with caption units, separated by hairlines; no boxes, no icons | more than five values, or a value with no comparison |
| **Pull-quote** | hear the human voice in the evidence | serif italic at lede size on surface-hero, source in mono beneath | the quote restates a number already shown |
| **Marginal note** | understand a choice or a caveat without breaking the flow | 240px rail on wide screens, inline under its anchor below; caption type in evidence colour; a hairline connector to the anchored figure | notes outnumber paragraphs |
| **Callout rail** | see what changed since last time | a single column of eyebrow + one-line items on surface-hero beside the main figure | it becomes a second navigation |
| **Before/after pair** | judge a change in the artifact itself | same data, two renders stacked or side by side, each with its rubric scores | the "before" is a straw man |
| **Ruled pricing** | compare plans | a ledger where rows are capabilities and columns are plans; the recommended column carries a verdict left rule | it becomes three cards with buttons |

## Anti-defaults

These are what a template produces. Each needs a reason in the brief's *omitted* field when
refused and a reason in the critique when kept.

- **KPI card row** as the hero — four numbers in boxes show no argument.
- **Card grid** for anything other than genuinely parallel, equally weighted items.
- **Table wall** — three or more tables in a row with no figure between them.
- **Pie, donut, gauge, radar** — the design language excludes them.
- **Gradient hero with no figure**, glass panels, animated counters, icon-per-bullet.
- **Logo band, table of contents, or filter bar** above the verdict.
