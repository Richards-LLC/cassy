# Toast expiry

Single idea: A connected notice leaves the screen and the live region together when its 3.2-second lifetime ends.
Hero form: The existing Glass status toast above the phone conversation; its text announces the new connection while visible, then the empty status region waits for the next notice.
Emotional register: Calm, definite — preserve the existing notice, placement, timing and house tokens.
Distinctive move: The notice expires through the same retirement path used when a conversation or scoped state ends, so stale connection claims do not remain in accessibility output.
Deliberately omitted: New controls, copy, layout, colours and timing. This corrects the lifecycle of the existing status region.

## Critique

Scorer: watchful-octopus-75, 2026-10-27. Design: hub-web/DESIGN.md@ce0d8adc967a81387ec0701d4340b4e8e96ee872.

| Dimension | Score | Observed evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Existing Glass panels, violet action, machine monogram and house type remain intact in M01–M04; the change is the lifecycle of that house notice. |
| Fit | 5 | M02-active shows the machine's connection, while M02 and its ARIA readback remove the expired claim together. |
| Hierarchy | 5 | The phone toast sits below the full conversation header and above the composer; the desktop notice stays at the top right. |
| Craft | 5 | Native strict visual QA PASS, four modes, zero contrast, clipping, overlap, visibility or viewport findings. No layout or timing change. |
| Accessibility | 4 | The live region announces connection, becomes empty, then announces a subsequent cancellation and empties again in all four modes. Browser ARIA output is proven; assistive-technology audio was not exercised. The reviewed JavaScript application declaration is retained. |

No dimension is zero; distinctiveness, fit and hierarchy meet the floor. Visual appearance is unchanged (`visual_change: false`).
