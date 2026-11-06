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
