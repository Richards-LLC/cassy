# Brief: End session failure

## Single idea

An operator whose End session request fails can find the same session and deliberately retry from the keyboard.

## Hero form

Annotated row: the failure sentence sits beside the session it belongs to, with its End session control as the next action.

## Emotional register

Calm, actionable — name the session and machine, keep the row, and explain the retry without exposing a request path.

## Distinctive move

Return focus from the in-flight status to that row's End session control, with the error as its accessible description.

## Deliberately omitted

No page-wide modal or automatic retry: ending a session must remain a deliberate confirmed action. Preserve the existing DESIGN.md tokens and layout.

## Critique

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Recovery stays attached to its Cassy conversation row, with the existing machine accent and visible focus ring. |
| Fit | 4 | The sentence names the failed session and machine, then the same End session control offers a confirmed retry. |
| Hierarchy | 4 | Session identity leads; the alert sits below its focused action and above the next row at 390 and 1280 pixels. |
| Craft | 4 | Strict inspection of the actual error renderer has no clipping, overlap, overflow or contrast finding in light/dark desktop/phone. |
| Accessibility | 4 | Keyboard failure restores the described control; leaving during the request preserves the operator's new focus. Forced colours, reduced motion and more contrast keep the alert readable. |

The real app and error-renderer captures use local source builds. The strict app report retains four baseline JavaScript-required fallback findings (cas-1f04); the delivery adds none. Production dist is preserved until the serialized landing turn.
