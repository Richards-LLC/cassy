# Brief: reply heading hierarchy and callouts

## Single idea

A reader can scan a supervisor reply's answer, evidence and caveats without mistaking a written warning for a request that needs action.

## Hero form

Annotated prose: three descending heading sizes establish the reply's outline, while a short labeled margin rule introduces each caveat inside the same bubble.

## Emotional register

Calm, legible: existing body type, modest scale steps and unfilled callouts keep the thread readable without competing with the amber ask or critical blocker cards.

## Distinctive move

A Note, Warning or Caution label sits on a narrow semantic rule inside the prose, leaving filled attention cards exclusive to actionable factory state.

## Deliberately omitted

No callout buttons, alert/live roles, card flood, shadow or rounded container: model-authored prose carries no authority to create an actionable blocker.

## Implementation boundary

`markdown-renderer.ts` owns semantic h1–h3 elements and callout recognition; the existing inline renderer owns escaping and links. Callout bodies reuse supported blocks once, keeping nested callout markers literal. `styles.css` scopes presentation to rendered markdown, including the inbox and pinned-ask consumers. No server data or interaction state is added. Existing font, size, spacing and status roles come from `hub-web/DESIGN.md@ce0d8adc9` and its token sources.

## Acceptance

One existing unit case gains heading-role assertions. One browser regression reads h1–h3 and all three labeled callouts in light, dark and forced colors on desktop and phone, checks safe inline content and normal prose after the callout, and proves no attention card or action was created. Captures include reduced motion and more contrast. No motion, dependencies or assets are added.

## Critique

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Short labeled margin rules live inside the established supervisor bubble; no competing attention card is introduced. |
| Fit | 4 | Descending heading sizes expose the reply outline before the reader reaches the caveats. |
| Hierarchy | 4 | The answer leads at 21px, evidence at 15px, checks at 14px; all three exceed the labeled 13px caveats. |
| Craft | 4 | Eight real-bundle captures retain whole text; the browser assertions find no page overflow and a consistent unfilled left rule. |
| Accessibility | 4 | Semantic headings and labeled asides survive forced colors; the safe HTTPS link remains native, and no live role or action is created. |

Scored from the captured production-bundle reply on 2026-10-28. Strict mechanical QA is packaged with the final delivery; this table does not claim a completed mechanical run.
