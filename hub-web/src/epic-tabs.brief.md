# Brief: epic tabs above the conversation

## Single idea

Every live epic has a visible place above the conversation, and the busiest one immediately shows who is doing the work.

## Hero form

Stat strip: a horizontally scrolling line of epic names, authoritative merged/total counts and one dot per live worker makes parallel work visible before opening Tasks & progress. The selected tab opens a ruled staffing ledger beneath it on desktop, and an accessible sheet on phone. The strip stays outside the conversation scroll area.

## Emotional register

Calm, close to the work: Commander Glass ink, violet selection rule, mono counts and green worker dots reuse hub-web/DESIGN.md rather than adding a palette. Task titles use normal prose and wrap; machine names make remote work legible.

## Distinctive move

The worker dots sit next to each epic's merge count, then resolve into named worker/task/state rows in the selected epic. A machine label follows each epic, including epics from a peer machine without visiting its conversation.

## Deliberately omitted

No manual factory focus, new status endpoint, guessed merged totals, animated counter or terminal feed. No independent polling timer: existing catalog, machine heartbeat and lifecycle events refresh session status. A tab choice is local and remains until its epic leaves the live catalog; a sheet can close without changing that choice.

## Component and state contract

progress-model.ts joins existing session snapshots, deduplicates epic/task/worker identity within project and machine scope, ranks in-flight work, and retains a valid explicit choice. epic-tabs.ts renders only that model. main.ts owns status reads and selection. Live epic means a reported nonterminal epic from a running session; ended sessions and closed/cancelled epics leave the strip. Missing progress reads Merge progress not reported; a failed machine keeps last-known rows with an explicit stale label and no live dots. Session/project identity prevents unrelated same-ID epics from colliding.

Desktop auto-opens the busiest epic; phone selects the busiest tab and opens its task sheet on tap so arriving peer status cannot interrupt the conversation or steal composer focus. Arrow keys/Home/End move between tabs, Escape closes the sheet and restores its opener. Selecting another tab overrides the automatic ranking until that epic closes. No new mutation controls.

The application requires JavaScript for signed machine status. Existing noscript and print behavior apply; the strip and selected ledger print in document flow. No images or motion are introduced. Keep the existing route budgets; measure the source production build with the affected journey.

## Validation

Model Vitest covers cross-machine aggregation, lifecycle states, authoritative counts, worker liveness and default/manual selection. An affected HUB-J26 journey walks the strip without opening Team or progress at desktop and phone, switches epics with the keyboard/touch, observes a lifecycle update and checks overflow, forced colors and reduced motion. Capture both widths and themes; record the rendered critique here before delivery.

## Critique

Scored on 2026-11-06 from production DOM captures at desktop/light and phone/dark, with an independent reader who did not see the worker scores. Strict inspection also passed both schemes at each viewport, including restored native modal state in the saved phone DOM; the earlier nonmodal serialization is excluded.

| Dimension | Worker | Independent | Evidence |
| --- | --- | --- | --- |
| Distinctiveness | 4 | 4 | Violet selected epic and green staffing dots resolve into named task rows, using Commander's Glass surfaces and mono counts. |
| Fit to argument | 5 | 5 | One line shows parallel epics across machines, and its selected ledger answers who is doing the work. |
| Hierarchy | 4 | 4 | Selection leads the quiet counts and machine captions; the phone scrim isolates the task sheet without adding another dashboard. |
| Craft | 4 | 4 | Strict visual QA PASS at 1280x800 and 390x844 in light and dark. The reader's cropped empty-thread monogram seam was corrected with token-sized empty-state spacing and a journey bound assertion. |
| Accessibility | 4 | 4 | Named tabs, arrow/Home/End navigation, text states, visible focus, Escape/Close return and reduced-motion/forced-colors checks pass. The independent reader assessed screenshots only; executable checks supply keyboard and print evidence. |

The floor holds. Task artifacts retain screenshots, strict JSON/Markdown receipts, journey traces and the actual source build manifest. The phone defaults to the busiest selected tab; task details open on tap, preserving focus when peer status arrives. Team retains its detailed metrics and scrolls to every worker below the persistent strip.

## Delivery boundaries and budget

No producer field, factory focus mutation, raw-output subscription, image or animation was added. Existing paired-machine heartbeat/catalog/lifecycle invalidation owns refreshes; one in-flight status request and one trailing invalidation per session bound bursts. Status failure exposes Retry epic status and last-known data removes live dots.

The reviewed source production bundle measured 220,919 gzip bytes of app JavaScript and 43,386 of CSS, using Python gzip compression. The checked-in baseline measured 218,479 and 42,654 with the same compression command; it was not rebuilt, so the difference is not a controlled source comparison. Both already exceed the inherited 200 KiB/30 KiB budget. This delivery deliberately keeps the existing entrypoint and reuses its progress model; it does not include a separate bundle-splitting project. The integration owner generates canonical dist at release preparation per CONTRIBUTING.
