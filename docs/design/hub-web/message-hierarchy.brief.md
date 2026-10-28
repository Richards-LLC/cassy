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

Pending rendered evidence.
