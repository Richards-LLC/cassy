# Why it works — executive-variance-brief.html

Type: financial report. Audience: executive. Design language: Petrastella default.

## Concept brief

- **Single idea.** The operating-income beat is real, but $138 K of it leaked through gross margin,
  and one pricing decision determines whether Q3 keeps the rest.
- **Hero form and why.** A signed variance ladder against plan, decomposed by driver, with the Q3
  forecast hatched at the bottom. The message is a delta; the form makes each delta a bar on a zero
  line so the leak (the one bar pointing left that matters) is visible at a glance.
- **Emotional register.** Composed, slightly sober; the leak is named without alarm.
- **One distinctive move.** The single display-italic annotation on the COGS bar ("margin fell
  61.0 → 59.4 %: the leak") is the only editorial voice in the figure. The ask is a *closing figure*,
  two option panels on one scale, with the recommendation stamped.
- **Deliberately omitted.** Two absolute bars (actual vs plan) for the reader to subtract; a
  five-card KPI row above the fold (four cards sit beside the hero and none competes with it); a
  gradient or a gauge.

## Decisions worth copying

- Scenario encoding is by fill: actual solid, forecast hatched via an SVG `<pattern>`, so the
  forecast stays distinguishable in grayscale print and the closing figure repeats the same hatch.
- Favorable and unfavorable are distinguished by direction first, color second, and stated in the
  axis note.
- The line-item table follows the ladder and reconciles to it in one sentence, so an executive can
  stop after the hero and a finance reader can audit the decomposition.
- Methodology and provenance are present and last.

## Critique (cas-ui-craft rubric, 1–5)

| Dimension | Score | Note |
| --- | --- | --- |
| Distinctiveness | 4 | Ladder with one editorial annotation; the ask as a two-panel closing figure |
| Fit to argument | 5 | The leak is the one leftward bar; the decision is the hatched Q3 bar |
| Hierarchy | 4 | Hero → KPI cards beside it → so-what → ask → line items → assumptions → provenance |
| Craft | 4 | Verified at 1280×800, 390×844, print; labels in their own column, no collisions |
| Accessibility | 4 | Direction and hatch carry meaning; every bar is labeled; contrast ≥ 4.5:1 |
