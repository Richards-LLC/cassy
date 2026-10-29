# Brief: Typed work rows

## Single idea

An operator can identify a read, search, command or edit from one quiet work row and open it for its scope and exact tool record.

## Hero form

An annotated activity ledger: operation-first summaries align with named file lists, search counts or command metadata in the expanded body. These shapes make unlike operations distinguishable without another colored card surface.

## Emotional register

Quiet, exact — existing Commander ink, rule and spacing tokens keep work subordinate to replies; failures use the existing critical tone plus words.

## Distinctive move

The disclosure changes its evidence layout with the operation: file scope for reads/edits, match scope for searches, command and working directory for execution. Unknown wrapper tools retain their original label and complete raw record without guessing their script's operations.

## Deliberately omitted

No new palette, icons, animation, dependencies, shell parsing or inferred edits. Structured metadata comes only from v2 fields; the complete raw detail stays agent-supplied text. Turn-start/completion notices share one collapsed Turn activity disclosure; failed notices remain visible.

## Boundaries and state

work-events.ts derives presentation facts from validated v2 data. work-rows-view.ts serves supervisor work and worker timelines; WorkPicture owns loaded detail and disclosure state. Signatures include v2 updates, allowing a stable event ID to finish without a stale row. Existing event IDs and lazy requests remain stable, and expanded bodies gain a focus key. Legacy tools keep their readable records. Loading, missing and failed-detail text remains explicit.

## Acceptance

Extend one renderer test for typed summaries/bodies, safe raw fallback, live metadata, focus/disclosure retention and collapsed lifecycle notices. Capture the production bundle at desktop/phone, light/dark, forced colors, keyboard, reduced motion and contrast. Design: hub-web/DESIGN.md; role sources: tokens.css/glass.css.

## Critique

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | The quiet activity ledger retains Commander Glass; opening a row reveals operation-specific evidence rather than another generic card. |
| Fit to argument | 4 | Read targets, search counts, command/cwd/exit and edit targets distinguish the four operations at a glance and on opening. |
| Hierarchy | 4 | Replies lead, then one collapsed work summary; failure words stand out without making all work equally prominent. |
| Craft | 4 | Strict visual QA PASS at 1280×800 and 390×844 in light/dark: zero contrast, clipping, overlap or overflow findings. Raw records wrap in the existing scrollable well. |
| Accessibility | 4 | Enter opens each detail, live metadata preserves detail focus, Escape returns from the timeline; forced colors, reduced motion and more contrast retain disclosure state. JavaScript is explicitly required by the application. |

Scored by crisp-tiger-26 on 2026-10-28 after reviewing production-bundle captures. All floors hold. The polish check inspects a captured production DOM with the exact dist CSS; live protocol and keyboard checks are in the runner trace. Source build b7a7e717d; evidence: task cas-d278, qa/bundle.json.
