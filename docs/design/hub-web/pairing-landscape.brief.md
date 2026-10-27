# Pairing on a sideways phone

- **Audience:** the operator reading or sharing a pairing code on a phone held sideways.
- **Argument:** every pairing instruction and action stays reachable when the screen is short.
- **Form:** retain the existing bounded glass dialog and its scrolling field list; allow the dialog itself to scroll any remaining overflow.
- **Distinctive move:** the large pairing code remains the focal point, with Copy command and Send the code in their established order.
- **Omitted:** no shorter copy, hidden controls, smaller touch targets, new tokens or layout mode.

Design source: `hub-web/DESIGN.md`, Overlay level3 and phone layout invariants. The dialog keeps its viewport bound and the existing focus-ring gutters, typography and light/dark surfaces. Scrolling is the declared overflow strategy; content is never discarded to meet the height bound.

Regression: the production pairing-code fixture at844×390 must fit or scroll its content, and Copy command, Send the code and Cancel must each receive focus wholly inside the dialog.

## Critique

Scored2026-10-27 by subtle-pelican-6 and a blind second reader; the change retains the existing design.

| Dimension | Root | Blind | Evidence |
| --- | --- | --- | --- |
| Distinctiveness | 4 | 4 | Cassy's serif identity, violet rule and large monospace code remain intact. |
| Fit | 4 | 4 | The bounded dialog allows overflow to scroll while keeping every pairing action. |
| Hierarchy | 4 | 4 | The code leads; instructions and sharing actions follow in their established order. |
| Craft | 4 | 4 | Strict visual QA:378 renders,0findings,0suppressed; desktop and phone spacing stays aligned. |
| Accessibility | 4 | 4 | Both schemes remain legible; native browser checks cover keyboard Copy/share fallback, focus reachability and matching accessibility preferences. |

The blind review covers screenshots; the native interaction trace supplies scrolling, clipboard and preference evidence. Token consistency, spacing, typography, copy, focus, contrast, overflow and motion pass for this scoped change.
