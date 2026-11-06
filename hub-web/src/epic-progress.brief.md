# Brief: Live epics in Tasks & progress

Replaces the epic tabs above the conversation (operator rejection, 2026-11-06 1:20 PM ET; design brief /mnt/rewind/cas/scratch/epic-progress-brief.md with amendments A–E).

## Single idea

The right pane answers "how far along is each epic of this project?" with the same open / in progress / merged split the factory TUI shows, and nothing competes with the conversation.

## Hero form

A ranked list of disclosure rows under the release header: title and mono merged/total on line one, a segmented bar on line two, the split in words and live worker dots on line three. The release header carries the same split and bar at larger size.

## Emotional register

Calm and countable. Commander Glass card rhythm, tabular mono counts, violet only for merged and in-progress ink; the critical tone appears only for blocked work.

## Distinctive move

The bar segments the epic as merged (solid), in progress (hatched) and open (the track), so a glance reads both progress and how much is moving right now; a row opens in place to its tasks grouped In progress, Open and Merged.

## Deliberately omitted

No strip above the conversation, no tabs, no second dialog on phone, no pace forecast (amendment C), no task ids (amendment D), no raw-id fallback for untitled epics, no "—/—" placeholder, no other project's epics, and no status reads for other projects' sessions.

## Component and state contract

- `progress-model.ts` `projectEpics(sources, projectKey, train)` keeps sessions whose `sessionProjectKey` (the conversation list's key) matches the selected session, joins epics by id across machines (newest snapshot first), strips "EPIC:" / "Epic —" prefixes, drops untitled epics and the train, and ranks blocked first, then most in flight. Open epics with no in-flight work or live worker only add to the quiet count.
- `epic-progress.ts` renders the section; disclosure and "Show more" state is browser-local and survives status re-renders. `projectSessions` selects which sessions' status is read.
- `main.ts` owns status reads: catalog arrivals, machine heartbeats, session events and opening a conversation read only the selected project's sessions.
- Counts come from the producer's `counts` field (session.rs, one pass over the loaded snapshot, the TUI's `epic_closed_counts` plus its ready and in-progress lists). Missing counts: no bar, in-flight words still shown.
- States: loading ("Waiting for project status…", "Checking prowl…"), empty (section absent), many (five rows, then "Show N more"), long titles (two-line clamp, full title in `title`), peer failed ("prowl last known 1:04 PM" plus Retry, no live dots).

## Validation

`progress-model.test.ts` (scope, dedupe, titles, train, quiet, sort) and `epic-progress.test.ts` (session selection, disclosure, Show more, missing counts). HUB-J26 project epics at 1280×800 and 390×844, light and dark, with forced colors and reduced motion; HUB-J17 for the train header scope.

## Critique

Scored 2026-11-06 from production journey captures at 1280×800 and 390×844, light and dark.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | The hatched in-progress segment between solid merged and the open track reads as Commander's own progress language. |
| Fit to argument | 5 | Each row answers open vs in progress vs merged in words and shape, for this project only. |
| Hierarchy | 4 | Release header leads, then blocked work, then the rest; counts sit right in mono. |
| Craft | 4 | Strict visual QA at both sizes and schemes; no overflow; counts update without layout shift. |
| Accessibility | 4 | Disclosure buttons with aria-expanded/controls, labelled bars, 44px phone targets, forced colors and reduced motion. |

Polish pass, three changes:

1. Collapsed rows really collapse: the panel's `display: grid` overrode `[hidden]`, so every epic printed its tasks and the pane read as a wall. A collapsed row is now three lines, and the journey asserts the panel is hidden before it opens.
2. "· 1 blocked" never splits across lines; it was orphaning "blocked" on its own line beside the dots at desktop width.
3. One rhythm: the Live epics section closes with the same hairline and spacing as the release header, and the Merged summary uses the card's space-1 padding at desktop (44px only on phone), removing the uneven gap under the task groups.
