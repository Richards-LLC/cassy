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

Socket catch-up schedules one presentation per animation frame. The journal owns
commit and ACK authority; its post-batch refresh must call the same scheduler.

## Validation remaining

Final frozen-thread budgets, final selected journeys, independent delivery QA and
the scored visual critique remain required before delivery. Current evidence is
in the task's artifact directory; this brief does not claim completion.
