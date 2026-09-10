# Hub in your hand

Lead with the decision. Give the conversation room.

Ten directions for a phone Hub, each with a desktop companion. Start with **01 · Decision inbox**: it puts the operator's next action above the mechanics of the system. These are design proposals, not shipped screens or measured usability results.

## Current state

The current Hub is captured from the committed `hub-web/dist` at source revision `a616f6d506a738557cd8977d130db56afa520d0d` (dist last changed in `3afcccce`). The local server preserves `/commander/`. Phone captures are 390×844; desktop captures are 1280×800. The gallery includes both light and dark captures. Each capture's caption distinguishes a live paired system from a simulated second system. This is a view of the current build before the other changes in this epic.

The visual question is how quickly Daniel can identify the system, read the supervisor's update and respond with one hand. The phone should keep system identity and connection state visible while moving worker detail and terminal controls behind deliberate navigation. The desktop companion should spend its extra width on context and comparison.

## 01 · Decision inbox
Rationale: A single queue brings supervisor requests from both systems into a readable phone column. Daniel can inspect a request, see its source and send a response without opening a terminal. Desktop keeps the same queue beside the selected supervisor conversation and supporting evidence. This is the suggested first prototype because it serves the frequent check-and-respond visit directly.
Mobile: Requests first; system and supervisor on every row; one clear response action.
Desktop: Request list, full conversation and evidence in three readable columns.
Tradeoff: A reliable request state is required; unstructured output still needs the reader.
Effort: 4
Impact: 5
Best for: Quick decisions across systems
Silhouette: Prioritized queue

## 02 · One system at a time
Rationale: A large named system switcher replaces the phone's compressed machine rail. A persistent bottom bar offers Overview, Supervisor and Attention; the selected system stays visible on each screen. Desktop expands the switcher into a left-hand system list and keeps the familiar console. This is the smallest architectural departure.
Mobile: Named system picker, one scoped supervisor and a thumb-level navigation bar.
Desktop: Persistent named system sidebar with the selected supervisor and context.
Tradeoff: Cross-system problems require switching; an all-systems alert count must remain visible.
Effort: 2
Impact: 4
Best for: A fast, familiar improvement
Silhouette: Scoped workspace

## 03 · Projects span systems
Rationale: Organize the phone around projects instead of machines. Each project combines its supervisors, with a source label on every update so location is never ambiguous. Desktop exposes projects as rows and systems as columns, making distributed work easy to compare. The operator follows the work even when it moves between hosts.
Mobile: Project list followed by one project's cross-system updates.
Desktop: Project-by-system matrix with an inspector for the selected supervisor.
Tradeoff: Requires dependable project identity across systems and clear handling of duplicates.
Effort: 4
Impact: 4
Best for: Work distributed across hosts
Silhouette: Project directory

## 04 · Supervisor conversations
Rationale: Treat each supervisor as a conversation partner. The phone opens a familiar thread list, then a full-width conversation with an explicit Send button and the target supervisor always named. Desktop uses a conversation list, thread and context rail. Delivery acknowledgments distinguish a sent instruction from the supervisor's eventual response.
Mobile: Conversation list, readable turns and an addressed message composer.
Desktop: Thread list and conversation, with tasks and connection state alongside.
Tradeoff: Conversational output must preserve code and tool evidence; it cannot invent a clean reply from raw output.
Effort: 4
Impact: 5
Best for: Frequent back-and-forth
Silhouette: Message threads

## 05 · The daily brief
Rationale: The phone opens an editorial brief: what changed, what needs Daniel and what is still running. Short paragraphs link to the underlying supervisor output, with freshness shown beside the brief. Desktop places the same narrative next to its source timeline. This favors occasional check-ins over continuous monitoring.
Mobile: A dated reading page with decisions, changes and evidence links.
Desktop: Wide brief with a parallel source timeline and supervisor links.
Tradeoff: Summaries can be stale or incomplete; source and capture time must be explicit.
Effort: 5
Impact: 4
Best for: Returning after time away
Silhouette: Editorial brief

## 06 · Session deck
Rationale: Give each supervisor a full phone page: project, status, last update and next action. A visible Previous/Next control accompanies optional swiping so no essential navigation depends on a gesture. Desktop lays those same pages out as a comparison board, opening one to read more. System identity is repeated on every page.
Mobile: One supervisor per page with labeled previous and next navigation.
Desktop: Simultaneous supervisor previews arranged by system.
Tradeoff: Serial browsing hides the overall queue; the page count and attention markers must remain visible.
Effort: 3
Impact: 3
Best for: A small set of active supervisors
Silhouette: Paged deck

## 07 · Reader first
Rationale: Start directly in a supervisor's reflowed transcript, with a compact system breadcrumb and a reachable message action. Terminal becomes an explicit alternate view; code blocks retain their own horizontal scroll. Desktop provides a reading column beside the authentic terminal and task context. This builds on the Hub's existing transcript mode.
Mobile: Edge-to-edge readable transcript with source, freshness and a reply bar.
Desktop: Reflowed reader beside the terminal, with synchronized context.
Tradeoff: Long logs still demand search and a clear jump-to-latest control; exact terminal geometry remains a separate view.
Effort: 3
Impact: 5
Best for: Reading long supervisor updates
Silhouette: Reading column

## 08 · Attention timeline
Rationale: Begin with the events that changed the operator's understanding: a disconnected host, a recovered link or a supervisor needing help. The phone shows a chronological cross-system timeline with explicit severity words and grouped repeats. Desktop pairs that timeline with an incident inspector and transport details. Routine output stays one tap away.
Mobile: Time-ordered incidents, source labels and one relevant recovery action.
Desktop: Timeline with an incident inspector and affected-system context.
Tradeoff: A quiet timeline says little about progress; provide a visible route back to supervisors.
Effort: 3
Impact: 4
Best for: Checking health and interruptions
Silhouette: Event spine

## 09 · Work by stage
Rationale: Show outcomes moving through Needs you, In progress and Ready to review. The phone uses labeled stage tabs and a vertical work list; desktop shows the same stages as a board. Every item links to the responsible supervisor, while worker details remain inside the work item. This helps Daniel judge delivery without decoding panes.
Mobile: Stage selector with readable work items and named supervisors.
Desktop: Stage board with supervisor links and a selected-work inspector.
Tradeoff: Work state must match the real task system; a session can own several items and must not be counted as one task.
Effort: 4
Impact: 4
Best for: Reviewing delivery progress
Silhouette: Stage board

## 10 · Rotate into context
Rationale: Portrait uses a compact overview above one supervisor reader. Landscape turns the overview into a narrow persistent left column and keeps the conversation scrollable on the right. Desktop expands this into overview, reader and context. Rotation changes the arrangement while preserving the selected supervisor and reading position.
Mobile: Stacked portrait overview and reader; a two-column workspace in landscape.
Desktop: Persistent overview, reader and context with adjustable emphasis.
Tradeoff: The short landscape viewport needs restraint; the keyboard may temporarily force a single reading pane.
Effort: 4
Impact: 4
Best for: Longer visits and frequent rotation
Silhouette: Adaptive split

## Choosing a first prototype

Prototype Decision inbox first, and borrow Reader first for its detail screen. The deciding criterion is a short phone visit that ends with a clear response to the right supervisor. One system at a time is the strongest lower-cost alternative: it can improve navigation without requiring structured request extraction. Its cost is slower cross-system triage.

Effort and impact use a 1–5 ordinal scale. Effort: 1 = small presentation change, 3 = new responsive navigation or view, 5 = new synthesis and data behavior. Impact: 1 = narrow convenience, 3 = useful for one visit pattern, 5 = addresses reading and acting in common phone visits. Scores are design estimates by this study's author; no timings or user-test results are implied. Dependencies, accessibility and trustworthy state are included in effort. Reversing the recommendation means keeping the reader and switching the default home route; request-model work would be the sunk cost.

Keep the familiar terminal available in every direction. Before implementation, validate the inbox with Daniel using one live system and then two: can he name the source, read the latest update and respond without losing his place? That usability validation remains open.

## Reading the study

Tap any image to open the reader. Use pinch or wheel to zoom, drag to pan, or use the labeled zoom controls. Fit width makes a tall phone image readable by scrolling. Full screen fills the viewport; browsers that reject native fullscreen use an in-page full-viewport reader. Escape leaves fullscreen first and closes the image on the next press. The page-level Full screen control also supports reading the entire study. Device orientation changes preserve the relative reading position. Screen drawings depict proposed layouts; their buttons are illustrations, not live Hub controls. In the image reader: + / − zoom, 0 fits width. Arrow keys, Page Down and the scrollbar move the image; the mouse wheel zooms. The controls work with a keyboard. All content is present with JavaScript disabled and in print.

## Provenance

Source: `docs/design/hub-mobile/index.md`. Design language: `hub-web/DESIGN.md`, `hub-web/src/tokens.css` and `docs/design/design-tokens.json`. Drawings are authored SVG, with illustrative Atlas laptop and Forge desktop systems and three supervisors; labels and counts are sample content. All twenty proposal views have equivalent light/dark versions. Captures use Playwright and the locally served committed dist; exact capture receipts are under `qa/`. Full browser evidence lives under the task's artifact directory. Capture credentials and browser storage are temporary and are not part of the report. The HTML includes its assets and needs no network or build step to open.
