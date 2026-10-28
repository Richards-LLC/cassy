# Brief: project-first Commander navigation

## Single idea

The operator sees what needs a decision first, then finds every supervisor under its project across machines.

## Hero form

Grouped register: a compact Needs you section leads a stack of closed project surfaces; the project header carries machine chips and the supervisor rows carry identity and current work.

## Emotional register

Calm, directed — opaque Glass surfaces separate projects, amber marks an operator decision, and violet stays reserved for selection and actions.

## Distinctive move

One project owns one card even when its supervisors run on different machines; clickable machine chips connect project context to the same filter bar.

## Deliberately omitted

No invented account, harness, status or performance figures. Missing optional identity fields leave a clean generic avatar and no blank email line. No collapse animation or additional blur. Metrics appear only when the catalog supplies known counts.

## Component map

- `ProjectNavigation` owns project composition, pinned decision shortcuts, machine filters and per-device collapsed projects.
- `ConversationList` remains the keyed row/control renderer, preserving pointer intent, end confirmation, ended notices and focus through catalog updates. Optional row metadata supplies the harness mark, email and supervisor state.
- `main.ts` owns catalog/history data and navigation actions; it passes `harness`, `account_email`, `project` and `machine_online` unchanged from the additive hub lane.
- The shell supplies filter and navigation landmarks. The existing paired-machine dialog gains filter choices so footer filtering retains machine-management access.

## State and acceptance

| State | Owner | Visible result and recovery |
| --- | --- | --- |
| Catalog and identity | Hub/history | Project cards, known worker counts and optional harness/email; missing values never imply an identity. |
| All / Needs you / machine | Navigation | Pressed filter chip, matching cards and explicit no-results state; All clears it. Search intersects the filter. |
| Waiting answer, merge or blocker | History/attention | Needs you shortcut points to the existing conversation; hidden when empty. |
| Collapsed project | Device local storage | Header stays, rows hide, toggle reopens; unavailable storage keeps in-memory interaction working. |
| Conversation selection | Main | One selected row and the addressed conversation; shortcut opens the same thread. |
| Missing/offline machine | Hub connection | Named machine chip with honest online/offline state; no successful connection inferred from stale data. |

## Tokens, accessibility and budgets

Use `hub-web/DESIGN.md@ce0d8adc9`, existing Glass `--bg-raised`, `--ink`, `--ink-mid`, `--color-action`, state colors, 4px spacing and pane radii. Project boundaries use `--ink-mid` for at least 3:1 against their surface. Inter and mono identifiers inherit the existing stacks and brand mark. Native buttons expose pressed/expanded state; machine state also has text, every row remains keyboard reachable when expanded, and toggles retain focus. Phone heading spacing is compact enough to leave the project register useful. Forced colors use system colors; reduced motion adds no animation.

No images or dependencies are added. Baseline app.js is 189.18 KB gzip and app.css 35.75 KB gzip; the CSS baseline already exceeds the generic 30 KB target. Measure final delta and avoid adding new blur or polling. The expanded spec makes singleton projects cards too, superseding the original plain-singleton requirement.

## Critique

Pending the expanded implementation and fresh renders. Original narrower group-boundary evidence is retained as a baseline and does not claim completion of this redesign.
