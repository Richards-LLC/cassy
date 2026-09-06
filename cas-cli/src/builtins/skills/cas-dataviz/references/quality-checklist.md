# Chart quality checklist

Before shipping, verify each answer is yes.

0. Is the form named in the concept brief with a one-line reason, and is it the least-ink form for the reader's task (a ledger, slope, dot strip, timeline, or small multiples where those fit better than a bar or a table)?
1. Does the title make one defensible claim, with population, period, source, and unit clear?
2. Is the insight annotated on the relevant mark, not only explained in surrounding prose?
3. Does the form match the reader task, and would a table be more useful for exact evidence, many categories, or audit?
4. Are units, precision, locale separators, and percent versus percentage-point changes consistent?
5. Are uncertainty, forecasts, missing values, sample size, and baselines labelled where they affect interpretation?
6. Do small multiples share scale, ordering, and visual treatment? Is there exactly one axis per plot?
7. Is color applied by meaning, taken from the design tokens' semantic roles and `color.series` (or the project's `DESIGN.md`), and validated where categorical colors identify series?
8. Can a reader obtain every value through labels, the table, or text without hover or color perception?
9. Does the static HTML use inline SVG/CSS, avoid external dependencies, state figure provenance, and print without clipping?
10. Did you actually render and inspect desktop and `390×844` headless-browser screenshots in light and dark, plus print/PDF for a report, for collisions, clipping, contrast in situ, layout, and the 30-second argument? HTML/tag greps do not count as visual verification.
11. Does the figure score 4 or above on distinctiveness, fit to argument, and hierarchy in the `cas-ui-craft` critique, with the scores recorded in the concept brief?
