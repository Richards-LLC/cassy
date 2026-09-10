# Brief: Hub in your hand

## Single idea
A phone Hub should lead with the supervisor decision and let the terminal become a deliberate second step.

## Hero form
Before/after pair, drawn as a narrow rail giving way to one wide reading column: the shape demonstrates how a decision-first home can spend the phone's width on the operator's next move. The study begins with the actual current-state captures immediately below this compact argument.

## Emotional register
Calm, observant, practical — warm sandstone, a serif verdict and ruled image plates; indigo marks the proposed route and never implies a running session.

## Distinctive move
A continuous numbered field guide: each alternative is a paired phone and desktop plate with its own navigation silhouette, sharing the same two-system example so the layout carries the comparison.

## Deliberately omitted
No decorative hero, KPI tiles, tiny ten-card overview or invented performance claims. The decision is which navigation model to prototype, not which mock has the most polish. Images are authored SVG UI drawings; photography would make labels less precise. Effort and impact are ordinal design judgments, not measured results.

## Contract and scope
- Decision brief for Daniel, with ten candidates and a suggested first prototype. Source text: `index.md`; generated offline review surface: `index.html`.
- House tokens copied from `docs/design/design-tokens.json`, consistent with `hub-web/DESIGN.md` and the generated `hub-web/src/tokens.css`. System serif/sans/mono; no fetched fonts.
- Current state: Playwright against the checked-in `hub-web/dist` on localhost. One actual paired system if locally available. A second simulated system must be captioned as a fixture; it must never be represented as a second physical connection.
- Proposals: twenty distinct SVG drawings (390×844 and 1280×800), each adapted to light and dark. Sample names and counts are illustrative.
- Images open in an accessible dialog: width-fit reading, pinch/wheel zoom, drag pan, keyboard zoom and scrolling. Full screen applies to either the whole study or the image reader. Native Fullscreen API when supported; visible CSS fallback otherwise. Rotation preserves relative scroll position.
- JavaScript enhances interaction only. All proposals, captions, comparison and provenance remain in the static HTML. Print shows the whole study.
- Capture matrix: 390×844, 844×390 and 1280×800, light and dark. Additional checks: keyboard, blocked Fullscreen API, print, JavaScript disabled, reopening and network independence.
- Durable full QA evidence: `/home/pippenz/.cas/artifacts/cas-cc1b/`. Commit compact QA receipts under `qa/`.

## Critique
| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | A numbered paired field guide carries the sandstone/indigo/serif language; ten navigation silhouettes are visibly different. |
| Fit to argument | 4 | The first-screen split-width/full-column schematic makes the proposed shift visible, then the actual screenshots provide a fair baseline. |
| Hierarchy | 5 | The verdict and figure fit at 390×844 and 1280×800; navigation and comparison follow the argument. |
| Craft | 4 | Strict visual QA PASS, zero findings/allowlists; twenty SVGs have zero text overlaps or out-of-bounds labels. Desktop images intentionally need enlargement on phones. |
| Accessibility | 4 | Light/dark, keyboard, touch pinch, full-viewport fallback, JS-off, offline and print checks pass. Physical iOS/Android browsers remain untested. |

Scored by zen-otter-40 on 2026-09-10 after Chromium renders. Floor holds; supervisor supplies the independent review.

## Evidence and deliberate costs
- The HTML is approximately 1.16 MiB: the eight required real baseline PNG captures make the report skill's approximate 500 KiB size target impractical. The file still opens independently and issues zero network requests.
- Current-state provenance is in `qa/current-state.json`. One real read-only paired local Hub; the second is a transport fixture using the existing fixture session shape. A second physical machine was not connected. No HOME override or second real Hub was used.
- Native fullscreen uses the div inside the modal; browsers do not accept a dialog element itself as the Fullscreen API target. The CSS fallback is verified by deliberately rejecting the API.
- Rotation retains the previous scroll fractions until the new viewport is measured; browser resize scroll events cannot overwrite them early.
- Original proposal SVG text bounds were checked at 390×844 and 1280×800. The final gallery is checked at 390×844, 844×390 and 1280×800 in both schemes; strict visual QA checks both schemes and print/JS-off too.
- Source and generation: `index.md`, `drawings.mjs`, `study.css`, `reader.js`, `build.mjs`. Screenshot capture: `capture-current.mjs` against `serve-current.py`. Runtime HTML is entirely static plus progressive vanilla JS.

