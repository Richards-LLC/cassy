# Designer review: bundled `dataviz` skill

Review basis: the complete Claude Code bundled skill captured at `/home/pippenz/.cas/artifacts/dataviz-reference/` on 2026-08-11, including its references and runnable palette validator.

## Preserve

- **Preserve — color-last procedure.** Selecting form before palette correctly prevents cosmetic color choices from deciding the argument.
- **Preserve — computable palette validator.** Contrast and color-vision checks are measurable, so a script is more reliable than visual confidence and belongs in distribution.
- **Preserve — form heuristic.** The explicit “is it even a chart?” test usefully steers headline values and crowded categories to stat tiles or tables.
- **Preserve — anti-pattern catalogue.** Named failure modes such as dual axes, recoloring after filters, rainbow scales, tooltip-only values, and clipped labels make review concrete.
- **Preserve — one-axis rule.** A single scale avoids invented correlations and forces an honest comparison through facets, shared baselines, or indexed series.

## Missing

- **Missing — message-first design.** The source chooses a form by data job, but does not require a single reader takeaway or a claim-title, so a technically correct chart can still have no argument.
- **Missing — annotation practice.** It supports selective labels but does not require marking the insight on the mark itself, leaving readers to hunt between prose and geometry.
- **Missing — fuller chart-choice tree.** It lacks explicit guidance for uncertainty intervals, distribution/relationship questions, audit-first tables, and the shared-scale discipline of small multiples.
- **Missing — number and unit discipline.** It mentions a few tick examples but not consistent precision, unit placement, percentage-point versus percent change, or sample-size labeling.
- **Missing — uncertainty display.** The procedure does not direct estimates, forecasts, missing values, or confidence intervals to be visually distinguished from observed values.
- **Missing — small multiples guidance.** It recommends faceting as an escape hatch without specifying common scales, alignment, and per-panel annotation.
- **Missing — data-density judgment.** CAS reports are evidence documents, yet the source treats a table largely as accessibility fallback rather than a primary, often superior, audit surface.
- **Missing — print/PDF behavior.** The interactive-by-default posture omits page-break, clipping, grayscale, and annotation retention checks required for report artifacts.

## Deliberate default inversions for CAS

- **Deliberate inversion — interaction default.** Hover remains available as an optional layer, but static labels, tables, and accessible text lead because self-contained report HTML must survive GitHub embeds, PDF, print, and JavaScript-disabled reading.
- **Deliberate inversion — design-system reference palette.** CAS keeps the validator and a minimal local palette instead of prescribing a comprehensive branded theme, so evidence artifacts can inherit local report tokens while validating the colors they actually use.
- **Deliberate inversion — dashboard-oriented filters.** Filters remain documented when a live surface needs them, but are not a default for committed evidence documents.

The CAS skill therefore retains the disciplined form/color/accessibility core, adds claim-title and annotation requirements, and treats static SVG + table + print behavior as first-class output.
