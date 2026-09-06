# Why it works — benchmark-small-multiples.html

Type: comparison / benchmark. Audience: practitioner. Design language: Petrastella default.

## Concept brief

- **Single idea.** The right backend depends on corpus size, and the crossover is at 200 k documents.
- **Hero form and why.** Small multiples: four metric panels sharing one x axis, three candidates per
  panel drawn on the same scale. A benchmark is compared by eye; small multiples keep the eye honest
  and put the crossover in the same place in every panel where it exists.
- **Emotional register.** Precise and even-handed; the loser gets its section.
- **One distinctive move.** The crossover is drawn as a dashed vertical rule in the two panels where
  it happens and *named* in the title; the memory panel is annotated "where sqlite-fts still wins" so
  the trade-off is visible, not buried in a paragraph.
- **Deliberately omitted.** A single overloaded chart with twelve lines; a legend as the only key
  (line style and end labels identify candidates, the legend is a courtesy); bars for values that are
  really trends.

## Decisions worth copying

- Candidates keep the same color *and* line style in every panel and in print; the categorical
  palette was validated with the `cas-dataviz` script and its command is recorded in provenance.
- Each panel states its own y scale on its top tick; the caption says so, so a reader never compares
  heights across panels.
- The winner statement gives the margin, the metric, and the price (memory) in one sentence.
- "Where the loser wins" is a required section and is treated as one, with the unmeasured factors
  listed in a marginal note.

## Critique (cas-ui-craft rubric, 1–5)

| Dimension | Score | Note |
| --- | --- | --- |
| Distinctiveness | 4 | Crossover rule repeated across panels; trade-off annotated in the memory panel |
| Fit to argument | 5 | The argument is "it depends on size"; the x axis is size |
| Hierarchy | 4 | Multiples → winner → 400 k table → all sizes → harness → where the loser wins |
| Craft | 4 | Verified at 1280×800, 390×844, print; equal panel heights; end labels clear of lines |
| Accessibility | 4 | Line style plus end labels; per-panel `aria-label` carries every value; tables twin the figure |
