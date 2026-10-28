# Brief: conversation window

## Single idea

Opening a long conversation shows its latest exchange immediately, with older turns available as the reader moves upward.

## Hero form

Reading column: the latest supervisor bubble sits directly above the addressed composer, already at the bottom of the conversation.

## Emotional register

Immediate, settled — existing Pebble bubbles retain their positions and focused controls survive new replies.

## Distinctive move

The reading position belongs to a turn, so inserting an older page keeps that turn at the same screen coordinate.

## Deliberately omitted

No progress stream of historical bubbles on open; no additional decorative loading surface. Existing Load earlier and Jump to latest remain keyboard paths.

design: hub-web/DESIGN.md@ce0d8adc9

## Protocol and rendering contract

The initial request remains one page of at most 50 queue rows. A new daemon adds
`open_attention` to that page, independently of its cursor: the complete open
asks and blockers, using the existing reply shape. An empty array is authoritative;
the field is absent on older pages and older daemons. Delivery ACKs do not answer
questions. A linked accepted operator turn answers an ask; any later accepted
operator turn acknowledges a blocker. Session/kind and session/id indexes serve
these lookups. Older paired clients ignore the additive field.

The browser keeps all loaded events as its reading model, including decision
summaries, while mounting a measured window of at most 20 four-turn chunks and
caching 60 rendered items. The keyed turn anchor compensates for height changes;
native scroll anchoring is disabled so it cannot apply the same compensation.
Logical unconfirmed-send runs span visual chunks. Window navigation exposes cached
turns before fetching another page. Load earlier, Load later, Jump to latest and
question bookmarks remain keyboard paths in chronological order.

Socket catch-up and history share one presentation in the task after an animation
frame, separate from the shell's geometry measurements. Mount requests coalesce
before paint. Before measured row heights exist, text volume estimates keep long
Markdown replies from mounting many screens in the first frame. The journal owns
commit and ACK authority; its post-batch refresh uses the same scheduler.

## Validation remaining

Final frozen-thread budgets, final selected journeys, independent delivery QA and
the scored visual critique remain required before delivery. Current evidence is
in the task's artifact directory; this brief does not claim completion.

## Critique

Producer review, 2026-10-27. Existing Glass/Pebble visual language retained;
application JavaScript requirement is declared and reviewed. Strict component
polish: 12 renders at 1280×800 and 390×844, light/dark, zero findings. Every
render was inspected. This scores the presentation; numeric performance and
full journey acceptance are separate gates.

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Distinctiveness | 4 | Pebble grouped corners, machine accents and the question bookmark remain recognizable across both schemes. |
| Fit | 4 | The reading column ends at the addressed composer; older history uses the existing quiet control above the turns. |
| Hierarchy | 4 | Conversation text leads; loading earlier, optional work and context stay subordinate to the reply path. |
| Craft | 4 | Strict visual QA PASS across thread, loading-earlier and expanded-work renders; no clipping, overlaps or phone overflow. |
| Accessibility | 4 | Visible loading text, semantic log, keyboard paging/decision paths and light/dark contrast retained; real-device screen reader testing remains outside automated proof. |
