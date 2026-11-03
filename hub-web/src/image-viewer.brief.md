# Inspect an image without leaving the conversation

Audience: the operator comparing screenshots sent together in one turn.
Decision: zoom into evidence, compare the next image, then return to the same place in the conversation.
Hero form: one expanded image on an opaque canvas, with its filename and position above it and a compact control row below.
Distinctive move: the existing thumbnail opens an in-app inspection surface; left/right stays within that turn’s images and closing restores the pressed card without scrolling the thread.
Omitted: editing, downloads, cross-turn galleries, HEIC/TIFF decoding and new visual tokens. Modified link clicks keep the existing open-in-tab behavior.

Design: hub-web/DESIGN.md, inherited Petrastella tokens and Glass dialog shell; existing provider/brand assets stay unchanged. Native dialog traps focus and makes the background inert. The image canvas has no motion beyond direct manipulation. Controls and text use existing semantic tokens; the viewport declares clipping because panning intentionally crops the zoomed image.

Boundary: image-viewer.ts owns only ephemeral image loading and inspection interaction. rich-attachment.ts owns the link entry point. The existing turn renderer supplies the gallery files and session-bound preview ticket source, so the viewer cannot roam other turns or mint arbitrary paths.

State: loading → ready or named failure with Try again; changing images resets zoom and ignores stale loads; close invalidates pending work, releases pointer state and restores focus and ancestor scroll positions. Local state is index, zoom and pan; names and gallery position derive from the selected artifact.

Failure recovery: a card has one unavailable note across repeated attempts, and opening the viewer clears its obsolete failure. At 390×844 the named image failure and Try again must remain visible without scrolling in both schemes; measure the viewport directly because a fixed-size recording can crop a taller phone page.

Inputs: touch pinch and drag, mouse wheel/drag, +/− zoom, 0 fit, left/right gallery, Shift+arrows pan, Escape close. All actions also have labeled controls. Focus begins at Close, Tab cycles within the dialog, and the canvas has a keyboard help description.

Performance: lazy-load the viewer on an unmodified image tap; reuse the existing short-lived preview ticket API. No new dependency or eager media bytes. Measure production chunks at delivery.

## Critique

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Glass dialog, violet focus and the existing evidence card form a continuous inspection path within the conversation. |
| Fit to argument | 5 | One image owns the canvas; its filename and turn position explain exactly what is being compared. |
| Hierarchy | 5 | Image first, filename next, compact gallery and zoom controls below; the 390px layout keeps every control visible. |
| Craft | 4 | Strict visual-qa PASS at desktop/phone in both schemes; 390/1440 production captures show no clipping or horizontal scroll. |
| Accessibility | 4 | Native modal plus explicit Tab boundaries; close restores focus and scroll; named loading/failure, keyboard controls and all three accessibility media modes exercised. JavaScript is required by Commander. |

Scored by ready-cardinal-28 on 2026-10-30. Four production renders and a separate strict production-component polish fixture were inspected. Protocol traffic is doubled; touch pinch uses synthetic PointerEvents rather than a physical phone. Evidence lives in the task's durable qa directory.

### Failure recovery follow-up

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Retains the existing card and Glass inspection dialog without introducing another error surface. |
| Fit to argument | 5 | Three failed opens retain one named note; a successful open clears the obsolete failure. |
| Hierarchy | 5 | The failure is readable above the controls, and Try again remains fully visible at 390×844 in both schemes. |
| Craft | 4 | Strict component-state inspection passed eight desktop/phone states with zero findings; native Commander captures confirm the card in context. |
| Accessibility | 4 | One retained status node avoids stacked announcements; native journey checks preserve keyboard, focus return and three accessibility media modes. |

Inspected on 2026-10-30 against da5310e1128e9a31aabbb4a191e6c8f3f0ab8f99. The strict fixture renders the actual components in isolation; its attachment card has no conversation container. Native source-bound Commander journeys supply the contextual evidence. The cropped original recording did not establish a phone layout defect: full viewport assertions passed before and after, so this follow-up changes no CSS. HUB-J22's four-worker desktop-dark thumbnail failure remains recorded separately with its passing one-worker control; batch QA round 7 owns that investigation.
