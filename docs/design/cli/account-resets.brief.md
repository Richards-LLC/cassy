# Subscription reset times

- Reader: the operator comparing Claude and Codex availability before launching or recovering a worker.
- Decision: see the exact local day/time for every known 5h and weekly reset beside its relative wait; missing or rolled-over usage stays explicit.
- Surface: one limits-layer formatter feeds account lists, picker details and reasons, Commander account rows, worker status and recovery notices. Inquire options use multiple physical lines and height-based paging. Existing Commander secondary text wraps in its account row.
- Machine output: stored and JSON reset instants remain UTC RFC3339. Launch profiles gain optional display-only usage text; no browser-side date formatter. Status uses already-known/local data and never fetches live usage.
- Constraints: 80 columns; reset values are not truncated. Per supervisor #4211474 the factory compact strip stays one line, shows only each account's most constraining window and absolute reset, orders by soonest reset and ends with +N more when full. Expired-window wording is unchanged.

## Commander account sheet

- Single idea: the operator can choose a subscription by the local day and time both usage windows free up.
- Hero form: selectable account rows, with the recommendation above two aligned window details; the selected row ties capacity to the launch action.
- Emotional register: calm and practical, using existing Glass secondary text and violet selection rather than urgency colours.
- Distinctive move: the same local reset grammar follows each account from terminal choice to Commander choice; the browser does not reinterpret time.
- Deliberately omitted: extra charts or badges for two values would compete with the account selection. This is an existing JavaScript application sheet; its reviewed JS requirement, overflow scroll and print handling remain in place.

## Critique

Scored by nimble-puma-8, 2026-10-27. Evidence under task cas-0dda artifacts.

Terminal receipts: `terminal-qa: PASS account-resets · 12 runs · 0 fail · 0 warn · 0 allowed`; `PASS limits-resets · 12 runs · 0 fail · 0 warn · 0 allowed`; `PASS compact-resets · 11 runs · 0 fail · 0 warn · 0 allowed`. Each covers 80/120 columns, four palettes, piped, C locale and NO_COLOR; list/limits also cover JSON.

| CLI dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Account state precedes the two reset rows; the compact strip puts the soonest freeing account first. |
| Fit | 4 | Separate window lines retain both dates; the factory strip stays one line and names omitted rows. |
| Craft | 4 | 35 gate captures have no overflow or split tokens; the same formatter supplies dates and relative waits. |
| Theme safety | 5 | Four palettes plus NO_COLOR retain identical readable date text. |
| Machine contract | 5 | Both JSON commands emit one document with RFC3339 instants. |

| Commander dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Glass account choices retain the product's violet selection and quiet secondary reset details. |
| Fit | 4 | The two window rows are inside the account being chosen, next to its recommendation. |
| Hierarchy | 4 | Account identity leads, recommendation follows, then the two reset rows; Start stays visible on phone. |
| Craft | 4 | Strict visual QA PASS, eight desktop/390px captures across light/dark including the scrolled selected account; zero findings. |
| Accessibility | 4 | Native radios and wrapping secondary text remain keyboard reachable; strict contrast, print and layout checks pass with the application's reviewed JavaScript declaration. |
