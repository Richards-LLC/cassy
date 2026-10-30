# Brief: composer upload progress

## Single idea
A slow photo transfer shows measurable progress beside the files until the machine stores them.

## Hero form
Annotated bar: a native progress bar under the existing attachment strip shows bytes transferred, followed by a distinct Saving label while the machine stores the file.

## Emotional register
Patient, certain: the existing Glass composer and muted status keep a long transfer readable without an extra overlay.

## Distinctive move
The filename and percentage stay with the removable attachment cards, and a refusal replaces that one status instead of repeating below the composer.

## Deliberately omitted
No timer estimate or animated spinner: only browser byte events advance the bar, and upload completion never claims message delivery.

## Implementation
`ComposerUploads` owns local files, transfer fraction and the single refusal. `HubConnectionSupervisor` owns signing, deadline and auth retries; `uploadRequest` adapts browser byte events without owning credentials or retry policy. The existing conversation bubble owns Delivered.
Tokens: hub-web/DESIGN.md, src/tokens.css and src/glass.css. Native progress uses the action color and space-2 height, with no added motion.

## Critique

Scored 2026-10-30 against the production composer and its new fixture states.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Glass composer retains its floating pill and filename strip; byte progress sits immediately under its files. |
| Fit | 4 | The bar advances only from browser bytes; Saving remains distinct from message Delivered. |
| Hierarchy | 4 | One muted filename/percentage and one bar precede the ordinary text field; a refusal occupies that same status position. |
| Craft | 4 | Strict visual QA PASS for progress/refusal at 1440×800 and 390×844, light/dark; zero findings across eight renders. |
| Accessibility | 4 | Native labeled progressbar, numeric value and status text accompany the visual bar; retained files remain removable after failure. |

Production bundle: app.js 210.73 kB gzip and app.css 40.07 kB gzip exceed the generic frontend defaults; this small upload patch retains the existing bundle architecture. The lazy composer chunk is 1.84 kB gzip. Physical phone and native picker behavior remain assembly/real-device checks.
