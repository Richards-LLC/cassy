# Brief: Source and work at one reading rhythm

## Single idea

An operator can inspect a diagram, patch or source block with the same header, then scan quieter work without losing failure or live state.

## Hero form

A shared reading frame above a ruled activity ledger: source and preview occupy the same bounded reading area, and each work label starts after one fixed icon gutter.

## Emotional register

Commander Glass stays quiet and exact: regular ink for the operation, muted ink for time, existing critical ink and words for failure.

## Distinctive move

Source stays selectable in the frame that drew its preview. Copy always uses the literal input; switching views never rerenders it. Only a tool with explicit unfinished lifecycle facts can shine.

## Deliberately omitted

No new colors, dependency, overlay, or inferred live state from a timestamp. Inline Expand preserves context and keyboard focus. Phone controls keep 44px targets; desktop rows use the existing 24px spacing step.

## Components and state

createCodeFrame owns local wrap, expansion and preview visibility; setCodeFramePreview attaches a sanitized renderer result without removing source. rich-code enhances markdown; work-rows-view wraps tool-reported diffs through the same API. WorkPicture retains disclosure/detail state. Lifecycle start/phase without outcome/ended_at marks live rows; unknown/legacy events stay still. Source errors retain existing fallback text and Copy. Use tokens.css/glass.css and hub-web/DESIGN.md@7d93fc693; the existing favicon and application voice remain in the shell.

## Acceptance

Extend the existing real Markdown test for Source/Preview and Expand, and existing work coverage for lifecycle and gutter alignment. Measure identical work inputs before/after, capture desktop/phone light/dark, and prove keyboard, reduced motion and forced colors on the production scratch build. JavaScript is required by this application; print expands reading content and hides reading controls.

## Critique

Design: hub-web/DESIGN.md@7d93fc693; token sources: tokens.css/glass.css.
Reviewed implementation1bf91637f at desktop1280×800 and phone390×844 in light/dark. The rendered fixture source is the real application renderer with fixed input; transport acceptance comes from the separate production journeys.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Commander Glass, mono labels and one quiet reading header unify diagrams, patches and code. |
| Fit | 4 | Source/Preview retains the same literal input and Copy; inline Expand keeps the reading context. |
| Hierarchy | 4 | Work sits beneath replies; regular labels align on a16px gutter, timestamps remain secondary and failures retain words plus a critical chevron. |
| Craft | 4 | Strict visual QA PASS:12 after renders,0findings, matching12 baseline renders. Five desktop rows228→120px; phone234.67→226.67px with44px targets. |
| Accessibility | 4 | Native keyboard Source/Preview passes at390px in forced colors, reduced motion and more contrast; no page overflow. Source stays selectable. Production work journey checks live/end transitions and animation media gates. |

No new palette, image or dependency. The initial application already exceeds the generic frontend budget: baseline JS211772gzip bytes/CSS40471; treatment212199/40639 (+427/+168). Lab LCP/INP/CLS were not measured. Print removes the frame height cap and hides action controls; long diffs retain their existing lazy disclosure and must be opened to print their preview. JavaScript is explicitly required by Commander.
