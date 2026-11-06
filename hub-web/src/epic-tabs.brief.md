# Brief: live epics beside the conversation (cas-0960)

## Single idea

The conversation owns the reading column; the open project's live epics sit to its right as a quiet ledger that says who is doing what, blocked work first.

## Hero form

A vertical tab list at the head of Tasks & progress in the context rail. Each card names the epic, then a mono merged/total (only when reported), live worker dots and machine labels. Under the list, the selected epic's work reads as ruled rows: task title, then worker · machine · state. The earlier sideways strip above the thread, with its 24vh inner scroll box, is gone.

## Emotional register

Calm, close to the work. It uses Commander Glass ink, `--bg-active` with a violet inline-start rule for the selected epic, green worker dots and a `--state-crit` word for blocked work, all from hub-web/DESIGN.md. No new palette.

## Distinctive move

The list is bounded by meaning rather than by height. The pane's own project shows first, and other projects wait behind one filter line. Blocked and live rows lead the selected epic, and parked or merged rows fold to a single "Show N parked" control. The rail scrolls as one region, and nothing in it has a fixed height.

## Deliberately omitted

This adds no new status read, polling timer, factory focus mutation or persisted filter. Unknown progress gets no placeholder count. A generic "EPIC:" prefix never stands in for a name. The phone sheet no longer has a nested dialog, because the epics live inside the existing Tasks & progress sheet.

## Component and state contract

`progress-model.ts` joins session snapshots by project and epic, carries `project` and a cleaned `title`, and orders rows Blocked → live → Ready → Parking/Merged. `epic-tabs.ts` renders only that model plus the counts and switches it is given. `main.ts` owns status reads, the selection, the pane-project filter (`cloud_project_id ?? project ?? project_dir`) and which epic's parked rows are open. The rail opens for epics even when the pane has no other progress.

Keyboard support is the same as before: Arrow keys (Up/Down and Left/Right), Home and End move between tabs. The parked control is a button with `aria-expanded`, and the filter is a button whose label states what it will show.

## Validation

Model Vitest covers project identity, title cleaning and row order. HUB-J26 (`team.journey.ts`, 1280 light and 390 dark) checks that the list sits right of the thread, a prefixed and an unreported epic, the other-projects filter, a peer epic with its parked row folded then expanded, a merge event, forced colors and overflow. Before/after captures at 1280×800 and 1680×1050 come from the journey production build.

## Critique

Scored on 2026-11-06 from the production-build captures in the task artifacts (`shots/before-*.png`, `shots/after-*.png`), against the operator's 1:13 PM ET screenshot.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | A violet-ruled selected epic and green dots resolve into named rows in Commander's own rail. It reads as Commander, not a generic tab bar. |
| Fit to argument | 5 | Who is doing what in this project is answered beside the conversation, and the chat no longer moves down. |
| Hierarchy | 4 | Selection, then blocked and live rows, then one fold line each for parked work and other projects. Release progress follows. |
| Craft | 4 | No clipped tabs, placeholder counts or inner scrollbar at 1280×800 and 1680×1050. Titles wrap to three lines, and the selected title shows in full. |
| Accessibility | 4 | Vertical tablist with arrow, Home and End keys, a text state beside each colour, an `aria-expanded` fold, forced-colors rules and visible focus. |
