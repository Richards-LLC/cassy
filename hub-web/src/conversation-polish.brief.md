# Brief: Conversations operator questions and progress (cas-6e3a)

## Single idea

The operator can answer the supervisor's actual question in one thread and see the named workers' current work without interpreting internal events.

## Hero form

An annotated conversation thread: one full ask in its place among messages, with a compact waiting pointer that lands on it; the question and its declared choices carry the decision.

## Emotional register

Calm and direct: existing Pebble paper/graphite surfaces, readable prose and human state labels, no new overlay covering the conversation or composer.

## Distinctive move

The stepped ask object is the only answer surface; a slim waiting bookmark jumps to that object instead of rendering a second card above the composer.

## Deliberately omitted

No invented Yes/Hold choices, no repeated ask body, no raw activity-note feed or guessed healthy worker state. Missing current work says that it has not been reported. No changes to Interrupt, Raw output, the hidden pane host or Terminal removal.

## Component and state contract

- attention-objects owns the ask's declared options and answered/retired treatments. Absent or empty options produce no quick replies. The latest waiting pointer can focus the composer for a free-text question without stealing focus on arrival.
- conversation-view owns the single full in-flow ask and compact waiting bookmark. Clicking the bookmark scrolls/focuses the corresponding thread turn (or composer for the latest free-text reply); dismissal and acknowledged/answered retirement keep existing history ownership. The bookmark never expands into a second ask.
- context-rail owns markdown-clean text excerpts and jumps. Use the existing safe Markdown renderer's plain text representation before truncation; do not create a second unsafe HTML path.
- A focused progress model owns human task/agent labels and workers derived from the selected session's catalog roster. Per-worker metadata/task titles are joined only by exact worker name/task ID. Missing details stay unknown/unreported. No last-note content is promoted to current work or synthetic control authority.
- main.renderStatus uses that model; controls continue to require real reported agent capability/generation. Rail min-width/word wrapping prevents long IDs, names and task titles from forcing horizontal scroll.

## Shared seams and base

Integrated Commander base `7b0138f3f` includes cas-0546 Terminal removal and released 3.46.0. Preserve its waitingOnOperator asks/blockers-only rule and removal of task-lifecycle attention notices. Its header actions, hidden/inert pane host and stage, Interrupt/Raw output wiring and width token are outside this task. Second lander rebases, rather than hand-merging main.ts.

## Acceptance and evidence

Before/after actual built-dist captures at1280/390 light/dark for C1–C5. A red-capable journey drives a no-options ask and markdown previews, asserts zero invented choices, one full ask, bookmark keyboard navigation, six roster workers with truthful current task/unknown data, human statuses and no rail overflow. The fleet operations journey seeds the catalog roster from its status workers and updates both after spawn/stop; it checks the human Active/Held labels. Include explicit-options, answered/retired, empty roster and long text transitions; final ARIA state and receipt trace. Playwright at most2 workers, no concurrent display matrices; Vitest maxWorkers2 and one typecheck/build at a time. Current product tokens/colors/motion stay in place. Strict visual QA against base plus independent QA, and actual Android emulator proof (supervisor seam) required. Final acceptance is recorded in the commit-bound QA ledger.

## Critique

The scoped critique reviews the implemented ask and progress surfaces in actual built-dist light/dark desktop/phone captures; final integration captures and strict findings are attached in the QA bundle.

| Dimension | Score | Evidence and remaining limit |
| --- | --- | --- |
| Distinctiveness | 4 | The stepped ask stays the single answer surface; a slim bookmark connects it to the composer. |
| Fit | 5 | Declared choices and free text match supervisor intent; worker task titles come from exact reported joins. |
| Hierarchy | 4 | Full questions remain in the thread, with one compact pointer and prose status labels. |
| Craft | 4 | Markdown-clean excerpts, wrapped identifiers and stable bookmark targets survive short phone layouts. |
| Accessibility | 4 | Named buttons, keyboard focus, 44px phone targets, and tested forced-colors/reduced-motion/contrast states; native Android keyboard captures supplement browser emulation. |

The report scores this change. Existing header findings are tracked separately against the base; independent QA remains supervisor-owned.
