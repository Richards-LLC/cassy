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

No images or dependencies are added. The same gzip command on merged epic `2b491102e` and the combined delivered bundles measures app.js at 189,316 → 192,009 bytes (+2,693) and app.css at 35,472 → 36,205 bytes (+733). JavaScript stays below 200 KB; CSS intentionally retains the pre-existing whole-app budget exception. No new blur, polling, images or dependencies. Field p75 LCP/INP are not established by this local synthetic walk; production telemetry owns those measurements. The expanded spec makes singleton projects cards too, superseding the original plain-singleton requirement.

## Critique

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Glass carries one outlined register per project across machines, with compact harness marks and an amber decision section. |
| Fit to argument | 5 | Atlas and Borealis share one studio surface; pulse-card begins in its own bounded surface. |
| Hierarchy | 4 | Needs you leads the project register; project names outrank supervisor codenames and optional account lines. |
| Craft | 4 | Reviewed production light/dark phone and desktop renders keep project edges, rows and metadata aligned. Phone spacing was reduced after HUB-J16 measured five visible sessions; it now proves six with unchanged touch targets. Fresh strict visual QA is required before delivery. |
| Accessibility | 4 | Named machine chips speak online state; native filters and collapse expose pressed/expanded state. The production walk proves keyboard focus, persisted hidden rows, forced colors, no horizontal overflow and missing-field fallback. |

Scored by solid-leopard-96 on 2026-10-28. The public-surface floors hold; the fresh strict mechanical receipt remains a delivery gate. The app declares its JavaScript requirement and retains its existing noscript fallback. No new animation was added; reduced motion preserves state changes.

## Critique response and handoff

- Fixed real defects revealed by the expanded walk: six-session phone density, fractional End-confirmation clipping in a nested scroll container, a collapsed body whose flex display overrode the hidden attribute, and filter padding that ignored a landscape phone notch, and renderer recreation during search that doubled page-lifetime gesture listeners.
- Account and harness are only supplied catalog data. Older hubs retain a generic avatar and no blank account line. Real hub metadata production/auth remains the companion task's ownership; these browser receipts use a protocol double.
- The production HUB-J14 navigation part covers cross-machine grouping, optional identity, a decision shortcut, filter/search empty recovery, keyboard collapse across reload and footer filtering. HUB-J16 covers adjacent End-session confirmation/failure/focus and density. All selected affected journeys must pass before parking.
- Original narrower before/after pairs remain the consistent same-data comparison for the project-boundary defect. Additional full-navigation production captures show the expanded design at both widths and all three color modes; the ledger labels renderer-fixture mechanical checks separately.
