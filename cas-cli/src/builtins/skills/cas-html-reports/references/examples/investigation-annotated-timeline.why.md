# Why it works — investigation-annotated-timeline.html

Type: investigation / diagnostic. Audience: practitioner. Design language: Petrastella default.

## Concept brief

- **Single idea.** One configuration change caused the timeouts; removing it removed them.
- **Hero form and why.** An annotated timeline: seventeen nights on one axis with the timeout
  threshold, the deploy, and the revert drawn as vertical rules. A causal claim is a claim about
  *when*; the reader must see the symptom start and stop at the two marked moments without reading.
- **Emotional register.** Calm and evidentiary. The chart is the case; the prose confirms it.
- **One distinctive move.** The two annotations that matter ("18 min the night before the deploy",
  "18 min the night of the revert") sit below the series, symmetric around the timeout plateau, so the
  before-and-after reads as a pair. The peak annotation is set in the display italic.
- **Deliberately omitted.** An overview KPI row; a separate deploy table (the deploys are on the
  timeline); color as the only carrier of "timeout" (the points are also above the dashed rule and
  named in the evidence table).

## Decisions worth copying

- The verdict comes *after* the hero and restates what the figure already showed, with confidence and
  mechanism. A reader who stops at the figure has the answer.
- The evidence table is the figure's twin *and* carries a "what it proves" column, so the reasoning
  chain can stay short.
- Ruled-out causes live in a marginal note beside the evidence, not in a separate section the reader
  has to find.
- Vertical rules for events use the semantic roles (`--warning` for the change, `--action` for the
  fix); series points use `--evidence`; the threshold and the over-threshold points use `--verdict`.

## Critique (cas-ui-craft rubric, 1–5)

| Dimension | Score | Note |
| --- | --- | --- |
| Distinctiveness | 4 | Timeline with symmetric before/after annotations; display italic for the peak |
| Fit to argument | 5 | Insertion and removal of the cause are the two marked rules |
| Hierarchy | 4 | Figure → verdict → overview → evidence → chain → falsification → actions |
| Craft | 4 | Verified at 1280×800, 390×844, print; labels clear of marks; one y scale |
| Accessibility | 4 | Timeout points are above a labeled rule and listed in the table; contrast ≥ 4.5:1 |
