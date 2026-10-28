# Brief: The whole picture in the thread (cas-ff22)

## Single idea

Between two replies the operator can see what the supervisor did: what it thought, which tools it ran, and which tasks moved. A worker's work is one tap away from the fleet list, without a terminal.

## Hero form

- **Show work** is a switch in the conversation header's overflow (and in the command palette). It is stored on this device (`commander.showWork`). Off is the default and shows replies only, exactly as today.
- With it on, each stretch of work between two conversation turns becomes one quiet **work block**, keyed and in time order:
  - a muted summary line, set in `--ink-mid` at `--fs-meta`, e.g. "Thought, ran 6 tools". The block is collapsed by default. Its disclosure button opens one row per event.
  - **task chips** (merged, parked, blocked, closed, reopened, started) stay visible while the block is collapsed. They are small pills with the task id in mono. Status tone shows in the chip's text colour and dot only (D8), never violet.
  - an expanded row is one line: a "Thinking" line, "Read limits.rs", "Bash: Run the tests", or a todo or worker notice. A failed tool shows a `--state-crit` dot and the word "failed". A row with detail is a disclosure. Its detail loads on first open through `WorkEventDetailRequest` and shows as plain text in a scrolling `pre`.
- **Worker timeline**: each worker row in Tasks & progress gets a "Timeline" button that opens `dialog#worker-timeline`. On desktop it is a 640px right sheet, and on a phone a bottom sheet, the same frame as Raw output. It shows the worker's state chip and current task, then its newest rows first, live. A "Raw output" link stays as the terminal fallback.

## Emotional register

Quiet and trustworthy. Work is context, not conversation: it never competes with a reply or an ask. Nothing moves under the reader, and nothing heavy loads until it is asked for.

## Distinctive move

The work block reads as part of the thread's rhythm, not as a log dump. One line is collapsed by default, chips carry the facts that matter at a glance, and the detail is one tap deeper. The live timeline is pushed: the daemon sends one tiny `WorkEventsAppended` notice when the stream gains events, so no client polls a quiet machine.

## Data and safety

- Agent-supplied text (labels, details, thinking) is only ever set as `textContent`. No work-event string reaches `innerHTML`, a URL or an attribute that parses markup.
- Pages come through the existing cursor API: the newest page on open, older pages only when an older stretch of thread is on screen, and detail only on expand. A thread with Show work off sends no work-event request.
- The push notice carries no content, only the newest event id. The hub relays it only to sockets that have read a work-events page. The daemon checks for new events on its blocking lane, at most every 2 seconds and only while a subscriber is connected. It never calls a model.

## Deliberately omitted

The following are left out:

- No streaming of partial thinking.
- No in-thread terminal.
- No editing or acting from a work row.
- No search across work events.
- No new colours. Work blocks use existing tokens only.

## Render contract

- Work items enter `threadModel` as one optional input. With no input, the model is byte-identical to today's (pinned by test).
- A work arrival repaints only the changed keyed items through `ConversationView.update()`, never a shell render. The windowed rendering of the speed task (cas-f48b) owns how many items are mounted, and work items are ordinary keyed items within it.
- A disclosure toggles its own node only.

## Critique

Scored against cas-ui-craft after the first real render. The evidence is the journeys `show-work` and `worker-timeline`, at phone and desktop sizes, in light and dark.
