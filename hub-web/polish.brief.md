# Brief: Hub operator polish

## Single idea
The conversations I can open belong to staffed, reachable supervisors, and I can see which paired machines supply them.

## Hero form
Ruled conversation ledger: project and supervisor identify each destination; a quiet footer states connection evidence beneath that list.

## Emotional register
Familiar, calm, accountable — the canonical interlocking Cassy mark, warm paper and serif wordmark, and explicit unreachable wording when a pending instruction loses its destination.

## Distinctive move
A compact machine badge anchors the foot of the conversation ledger and opens an address-and-last-seen machine register, including machines with no sessions.

## Deliberately omitted
No invented C/cloud logo, no decorative metrics cards, no inferred connected state from a saved credential. Session freshness and runtime details come from the machine; Hub build identifies this bundle.

## Components and state
- cloud-brand owns the canonical three-ribbon mark, traced from docs/assets/cassy-logo.png; favicon uses identical paths, theme-aware monochrome ink.
- paired-machines owns footer and machine register presentation; main owns catalog, connection state and durable removal. Keyed updates preserve open dialog and focused controls.
- worker-visibility owns browser session policy and in-flight retention. Rust owns heartbeat truth and publishes its existing worker_status threshold; browser expires stale catalog snapshots using that value.
- ConversationHistory owns pending sends through sending → acknowledged → replied/error. Missing destinations remain unreachable until an outcome arrives; no transport changes.
- Loading, empty, reconnecting, connected, missing destination and removal failure each have visible copy. Removal means this browser forgets the pairing; it does not claim credential revocation.

## Acceptance and budgets
390×844, 1280×800, 844×390, light/dark: list, thread, terminal, pairing, failure and machine register; keyboard open/close and focus return; catalog loss/return and pending receipt retention. JS ≤200 KB gzip and CSS ≤30 KB gzip. No motion added. Existing JS-required console fallback retained.

## Critique
| Dimension | Score | Evidence |
|---|---:|---|
| Distinctiveness | 4 | Canonical ribbon mark and ledger-foot machine evidence match the Cassy product. |
| Fit | 5 | Projects remain primary; paired machines are one action away even when they have no sessions. |
| Hierarchy | 4 | Quiet footer metadata supports the supervisor list; reader and composer remain undisturbed. |
| Craft | 4 | Keyed register, wrapping hosts, two-column landscape layout; strict visual 96 captures, zero findings. |
| Accessibility | 4 | Both schemes, 44px primary targets, keyboard open/Escape/focus return, reduced-motion and local scroll fallback. |

The initial register clipped in landscape. Its corrected two-column short-axis layout passes without a new exception. Browser fixture journeys cover in-flight row loss/recovery and durable removal. Native paired-phone demo remains with the supervisor. Full per-element review and screenshots: `/home/pippenz/.cas/artifacts/cas-f6e2/element-review.md`.

## Engineering handoff
Sources: `hub-web/DESIGN.md`, `src/tokens.css`, `docs/design/design-tokens.json`.
Bundle budgets measured: JS 75.5 KB gzip, CSS 12.9 KB gzip (both under their limits); Ghostty's existing WASM/font payload remains separate. Field p75 LCP/INP/CLS are unmeasured; no performance claim is inferred from fixture timings. SVG has fixed dimensions, no image request, no added animation. Existing JavaScript-required fallback remains. Brand labels now consistently say Cassy Cloud; URL, storage and protocol names are unchanged.

`polish-qa.mjs` drives six viewport/theme journeys against generated dist; `conversations-qa.mjs` retains send/ack/reply, rejection, draft, caret and terminal coverage. `visual-qa.mjs` covers 16 surfaces × 2 schemes × 3 viewports; 10 existing terminal/JS-disabled exceptions retained. The machine register adds no exception.

Catalog freshness is received from Rust, not copied into a browser constant. Older runtimes without that field retain structural filtering; their own catalog liveness remains authoritative. In-flight retention is memory-local, like the existing conversation history; reload does not manufacture a receipt or resurrect lost history.

