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
