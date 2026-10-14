# Brief: communication-review.html

Source: `communication-review.md` (cas-447d, unchanged). Cell: **Investigation / diagnostic × Executive**
(the operator reads it first; the supervisor and fix owners read the detail). Design language:
no `DESIGN.md` in this repo, so `design-spec/references/tokens.css` (Petrastella) is pasted into
`:root` by name.

## Single idea
Cassy moves messages in 0.1–0.2 s, then records the wrong outcome: most "abandoned" messages
were read, and the operator's replies never arrive. (Rendered verdict, 17 words: "Cassy moves
messages in 0.1–0.2 s, then records the wrong outcome — and the operator's replies *never arrive*.")

## Hero form
**Stacked bar**, drawn as a two-row *receipt gap*: the top bar is what the queue recorded for this
session's 228 rows (one solid bar, labelled `abandoned`); the bar beneath it, on the same scale, is
what actually happened: 128 rows the worker had read and 100 supervisor→operator replies the sweep
threw away. The reader sees one label split into two different truths, neither of which is
"abandoned" — the gap between the bars is the claim. A third thin row restates the month (1,376 of
1,990 "abandoned" rows had a seen receipt) so it is not one bad night.
Investigation's default hero is an annotated timeline; it is refused because the verdict is not
about *when* something changed but about a record that disagrees with reality, which is a
composition-of-a-whole question.

## Emotional register
Calm, forensic, unarguable — sandstone hero, serif verdict with *never arrive* in the indigo
italic, the queue's own word set in mono and struck through; no status colour above the fold except
the single danger-labelled operator segment.

## Distinctive move
The queue's vocabulary is quoted and **struck through** wherever the record is wrong: the hero's
`abandoned` label, and once more below in the stage-distribution figure where the `abandoned` bar
is struck and annotated "128 of these were read". The reader learns one notation — struck mono =
what Cassy believes, serif = what happened — and meets it twice.

## Deliberately omitted
- No KPI card row or stat strip above the fold: "0.1 s transport" and "228 abandoned" as boxed
  numbers would read as a dashboard, not an argument.
- No table of contents or channel list before the verdict; the architecture table is relegated to
  an appendix because the operator does not need it to act.
- No timeline hero (see Hero form), no pie/donut for the stage distribution (design language bans
  them), no per-worker nudge bars (the source gives only young-newt-54's 67 and the 105 total; a
  per-worker split would invent numbers).
- No colour as the only carrier: every segment is labelled in text and the danger segment also
  carries a hatch, so the hero survives grayscale print.

## Section forms (reader's task → form)
| Section | Form | Why |
| --- | --- | --- |
| Hero | Verdict hero + stacked bar (receipt gap) | the split of one label is the claim |
| Overview | Evidence ledger (2-col) | the investigation type requires question/verdict/confidence/scope |
| What is broken / works | Callout rail beside a numbered list | five findings scanned, each pointing at its figure |
| Stage distribution | Sorted bars, `abandoned` struck and annotated | rank of final stages; the struck bar echoes the hero |
| Abandoned split across sessions | Stacked bar per session on one shared scale | composition (false-abandon vs operator sweep) per session |
| Channel volume / latency / gated | Evidence ledger with an inline gated-share bar | exact lookup across 7 channels; the gated share is the pattern |
| Triple-read and noise cost | Sorted bars, dashed (estimate encoding) | ranking estimated token costs; dashed = estimate |
| Nudge storms | Waffle (105 squares, 15×7) | share of a whole: one worker's 67, 23 in a single prompt |
| Lead verdicts | Evidence ledger with verdict chip | auditing 17 leads against evidence |
| Fix list | Heatmap-style impact × effort matrix, chips link to §7 task anchors | scanning where the cheap high-impact fixes sit |
| Architecture | Evidence ledger (appendix, `<details open>`) | lookup only |
| Provenance | Mono list + verbatim SQL | reproducibility |

No two adjacent sections share a form except ledgers separated by a figure.

## Critique

Rendered from `communication-review.md` by `build/render.py`; scored on the final HTML
(sha256 `f82af9d876da62fa6b7393e2742e791bfff5cc1f5e68016ab76cc577997841ec`).

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Sandstone verdict hero with the serif sentence and indigo italic *never arrive*; the struck mono `abandoned` appears in the hero and again on the stage-distribution bar, so the notation is learned once and reused. Not a 5: the lower sections are well-made ledgers and bars, but they carry no further house move. |
| Fit to argument | 5 | The hero is the claim: one solid bar labelled `abandoned` 228 sits over the same bar split into 128 "read by the worker" and 100 "operator replies swept unseen". The figure states both halves of the verdict without prose. |
| Hierarchy | 4 | Above the fold there is only the verdict, the receipt-gap figure, the 3px verdict rule and the provenance line: 1440×900 hero ends at 548 px, the figure at 516 px; 390×844 figure ends at 680 px and the hero at 795 px. Not a 5 because the overview ledger's banded verdict row is the next strong element and slightly repeats the hero. |
| Craft | 5 | visual-qa.mjs `--strict` PASS with 0 findings and 0 allowlisted, in light and dark at 1280×800 + 390×800 (default), 390×844 and 1440×900 (receipts in `visual-qa/`). Values sit at bar ends, there are no legends except the session figure's three-swatch key, and the matrix reflows to labelled blocks below 640 px. |
| Accessibility | 5 | 0 contrast or clipping findings in either scheme. Every figure has role=img with all values in its label or a twin table. Status is never conveyed by colour alone (hatch + label for the operator segment, struck text + label for the false claim). There is no JavaScript, and JS-off text is identical to JS-on (40,974 chars). All 14 `<details>` ship open. The print PDF (`screenshots/final-print.pdf`) has expanded content, repeating headers and light tokens. The 390 px page has no horizontal scroll (scrollWidth 390); only the channel ledger scrolls, inside its own region. |

Scored by jolly-falcon-2 (cas-6abe) on 2026-10-14. The floor holds: distinctiveness, fit and hierarchy are each ≥ 4, and no dimension is 0.

Round history: r1 had values detached from bar ends, a collapsed waffle key, a channel ledger scrolling at 1440 px, a hero bar-height rule overridden by cascade order, and the §2.1 intro duplicated. r2 fixed the hero. Final fixed the rest, plus mobile matrix reflow and the palette (below). Screenshots for every round are in `screenshots/`.

## Review checklist (cas-html-reports/references/review-checklist.md)

Every item has been resolved. Where an item needed a decision, the decision is noted.

- **0 Brief and critique.** The brief has all five fields plus a section-form table. The hero is above the fold at 1280×800, 1440×900 and 390×844 (measured above). The critique is above. Renders exist for 1280, 1440 and 390 in light and dark (`screenshots/final-*`). The strict visual-QA PASS is in `visual-qa/{default,390x844,1440x900}/visual-qa.md`. The `:root` tokens are `design-spec/references/tokens.css`, pasted verbatim; the repo has no `DESIGN.md`.
- **1 Content fidelity.**
  - `build/check_numbers.py`: the HTML and the markdown carry the same set of 315 numbers, with none extra and none missing.
  - `build/check_text.py`: 674 of 684 markdown units (paragraphs, list items, table cells, SQL lines) appear verbatim. The other 10 are deliberate presentation changes:
    - 9 headings were renumbered or retitled. "Executive summary" became "What is broken" plus the "What works" rail, §8 became "What would overturn this", and section numbers appear as a `§n` prefix.
    - The §2.1 row label "worker/self-reminder → named agent (…)" is split into the label and a sub-line in the channel ledger.
  - No conclusion was softened.
  - The HTML links `communication-review.md` relatively, and the markdown, brief and HTML are committed together under `docs/reports/2026-10-14-agent-communication-review/`.
- **2 Information architecture.** The order is Investigation × Executive:
  1. hero
  2. overview (question, verdict, confidence, scope, date, author)
  3. findings
  4. evidence (§2, §3)
  5. reasoning chain (§4 verdicts, §5)
  6. what would falsify (§8)
  7. next actions (§6 matrix, §7)
  8. architecture appendix
  9. methodology and provenance last
  The type's optional timeline is omitted because the verdict is not about *when*.
- **3 Semantic HTML.** There is one `<h1>` (the verdict) and headings descend h1→h2→h3. Tables have thead, scoped th and a caption. The landmarks are header, main, sections and footer. `lang`, `charset`, viewport and a dated `<title>` are present, and there is a skip link.
- **4 Responsive.** There is no page-level horizontal scroll at 390 px. Only the 9-column channel ledger and the SQL blocks scroll, inside focusable regions. The bar figures are CSS percentage bars and the waffle is a viewBox SVG.
- **5 Accessibility.** Contrast is covered by visual-QA in both schemes. Each figure has role=img with an aria-label or SVG title carrying every value, and a twin table: the stage and session tables are verbatim from the markdown; the token-cost and waffle values are all printed as text.
  - Keyboard: every control is a native link, summary or focusable scroll region, with a visible focus outline.
  - Palette: `validate_palette.js "#4C5CA8,#B3261E" --surface "#FFFFFF"` gives ALL PASS. In dark, `"#7083D1,#EF7B72" --mode dark --surface "#191C24"` passes every check except the lightness band, for `danger` #EF7B72 at L 0.713.
  - Accepted exception: `danger` is the status token, not a series slot. It marks a failing lane, always carries the 135° hatch and a direct text label, and clears 3:1 on the surface. The grey "other" segment uses `--series-neutral`, which is the sanctioned rest bucket.
- **6 Interaction.** There is no JavaScript, so JS-off loses nothing: the text is identical, 40,974 chars, with all 14 details open. There are no tabs and no motion beyond the 120 ms link-colour transition, which `prefers-reduced-motion` removes.
- **7 Dependencies.** A grep finds no `http(s)://`, `src=`, `@import` or `cdn`. The 1280 px render made 0 external requests. There is one inline `<style>`, no `<script>`, system font stacks only, and the file is 105 KB.
- **8 Copyability.** All data is in real `<table>`s. SQL sits in `<pre><code>` without line numbers. Numbers carry no non-breaking spaces inside digit runs; `&nbsp;` is used only between a value and its unit.
- **9 Print.** `screenshots/final-print.pdf` was checked. Details are expanded and summaries hidden, `thead` repeats, figures, rows and chips don't break, h2/h3 keep with the next block, and the hatch survives with print-color-adjust. Light tokens are used (the dark block is `screen`-only). There are no external links to expand.
- **10 Visual emphasis.** The receipt-gap figure is the most prominent element. There is one verdict mark per figure: the 128 segment, the struck `abandoned` bar, the repeats bar and the 23-nudge squares. The matrix uses no verdict, because it holds links. Bars start at zero and siblings share a scale: the session figure has one 743-row scale. Estimates are dashed.
- **11 Provenance.** Every figure has a source line naming the query or extractor and the 11:51Z snapshot. The footer carries the markdown Sources verbatim, all four `queries/*.sql` verbatim, the render script and the token source. Transcript token counts are labelled as estimates (chars ÷ 4). The client-facing hygiene rule does not apply, because this is an internal operator report.
