# Brief: Commander, alive at a glance

Phase 1 · cas-675e · 2026-10-05 · silent-viper-51. **Proposed, awaiting operator approval.** Static illustrative data; no product implementation or live-state proof. Visual baseline: Conversations-only epic `357e7cb523f64735b4c26f27371b6cdd05f73460`; proposal branch also includes its whitespace-only successor `7b0138f3f` (no visual/source-token changes).

## Single idea

Commander makes the one decision waiting for you obvious while the rest of the work keeps quietly moving.

## Hero form

**Verdict hero with an annotated timeline:** “One decision needs you.” sits above living conversation rows, and the selected row opens into the actual question; the eye travels from status to a decision without scanning a dashboard.

## Emotional register

Alert, composed, tactile — warm paper or three graphite planes, a serif question, readable indigo action, and disciplined mono timestamps make work feel present without a wall of warnings.

## Distinctive move

A narrow indigo decision seam travels from the selected conversation into its peek: the row's short question becomes a large serif question, and its answer occupies the same visual track.

## Deliberately omitted

No KPI cards, rainbow machine fills, giant brand banner, generic Yes/Hold choices, repeated waiting overlay, Terminal view, fake progress percentage, perpetual connection spinner, or confetti. Semantic warning/error colors remain available for real causes; the vivid house indigo is reserved for selection, real attention and action. Machine identity remains text/monogram rather than another accent competing with the operator.

## First screen and responsive composition

- Desktop 1280×800: a small global chrome line, the verdict, a 360px conversation list, and a raised peek to its right. Working, Needs you, Done and connection trouble have distinct words; they do not demand equal attention. The selected conversation stays in the list while its question is readable in the peek. Open conversation enters the full existing thread.
- Phone 390×844: two compact living rows lead into the selected peek in normal flow. Its question and declared choices remain above the fold; other conversations continue below. This is a focused peek state, not an artificially shrunken desktop. Exactly one bottom composer region; its More control contains secondary actions. No duplicated sticky action bar, ask overlay, or sheet above an unrelated composer.
- The phone snapshot shows a peek opened by tapping a named Peek affordance. Close returns to the list and the initiating row. Hover may preview on a fine pointer, but never opens a reply form or moves focus automatically; Space opens the deliberate keyboard peek, Enter opens the conversation. A focused reply draft prevents hover replacement. Escape closes, restoring focus. Modified chords and all shortcuts are suppressed in editable controls; Interrupt requires deliberate activation and existing capability authority.

## State and status tiers

The verdict is a live summary, never permanent copy: zero real asks uses “Work is moving” only with reported ongoing work, otherwise “Your conversations”; multiple asks prints the real count. Missing or filtered observations never fabricate a count.

1. **Ambient:** a quiet dot and explicit state word on every row; the word carries meaning without color.
2. **Glanceable:** the most recent reported current action plus elapsed age, e.g. “Running tests · 2m”. Elapsed is age of that report, never a guessed ETA. Use exact session/worker/task joins; missing current work says “Current action not reported”. Plain-text previews reuse the safe Markdown path. Truncate only the excerpt, never the state/cause.
3. **Decision:** Needs you means an unanswered real ask or operator-actionable blocker, not task lifecycle, Awaiting merge, reconnect delay, or routine activity. One full ask surface at a time: peek temporarily owns the selected ask; the full-thread mode owns it after navigation. The list holds a short excerpt/bookmark, never a second expanded form. Choices come exclusively from the ask's declared options; a free-text ask has only the reply field.
4. **Receipt:** Done requires an explicit completion event, shown as a quiet summary with timestamp. Stale observations never imply done, healthy or offline. Network connection cause and task activity are separate axes; “Offline” is only a known offline condition. Reconnecting/permission denied/unreachable retain their named causes, retry timing and last-confirmed observation. A Needs-you ask survives a connection loss, with sending unavailable and its reason visible.

Pending and reply semantics from e6d2 are inherited: forwarded, stored on this device, and read remain separate. Uncertain sends say Not confirmed and never auto-retry. A fresh activity event alone may pulse; pause the pulse when freshness expires or the tab is hidden. No synthetic liveness from a timer. Async updates preserve focus/drafts and use existing regional rendering; no heartbeat shell rebuild.

## Proposed visual language

Read live `hub-web/src/tokens.css` plus `hub-web/DESIGN.md` at the baseline. The mockup embeds those generated tokens verbatim, then derives composition-only surfaces; it does not modify the generator or production stylesheet.

- Keep `--font-display`, `--font-ui`, `--font-mono`, 4px spacing grid and existing text/foreground pairs. Serif is for the verdict and the actual operator decision, not every status label.
- Root plane uses `--bg-root`; list plane uses `--bg-panel`; peek uses `--sheet-bg`, `--lift-strong` and a fine `--line-strong` edge. Dark surfaces therefore remain #12141A / #191C24 / #1F232D; light remains warm canvas / paper / raised paper, with depth earned by shadow and edge. No transparent glass text surface.
- One vivid house accent: existing #2E3A9F on light / #A9B3FF on dark, using matched `--you-fg` foregrounds. A thin attention edge and one main choice carry it; no accent flood behind the question. Semantic success is a small mark only, with the row's word in ordinary ink.
- Phase 2 will expose LCH seed inputs in the token generator rather than hand-write component palettes. Convert the existing source RGB values to LCH first; preserve measured text ≥4.5:1 and marks/control edges ≥3:1. This proposal authorizes no unreviewed recoloring of semantic states.
- Consistent 20px outline icons, 1.8px currentColor strokes, round caps/joins. Icon-only controls have visible affordances and accessible names. No emoji status vocabulary.

## Motion and action bar proposal

Motion is not running in these static mockups. On approval: working dot breath 1800ms only for fresh work; new messages enter with 120ms opacity/4px translate and a maximum 3×40ms stagger; peek 200ms reveal with a restrained backdrop blur; state change 160ms cubic-bezier(.2,.8,.2,1), with no bounce on text. Done gets one 160ms mark reveal, never confetti. Reduced motion sets all these to zero, pulse off, blur off; forced colors retains visible borders and labels. A CSS spring-like curve avoids a new animation dependency.

Desktop contextual bar shows Peek (Space), Open (Enter), Raw output and Jump to waiting; Interrupt remains an explicit named action and never a bare destructive letter shortcut. Bind chords only after checking the existing shortcut map; the mockup invents no Raw output/Interrupt keyboard chord. Toasts use the same bar slot without obscuring the reply field and expire without focus theft. Phone merges context actions into the single composer/More area. Raw output remains the existing read-only transcript drawer, not a Terminal view.

## Coordination and approval boundary

Grounding: `commander-qa-2026-10-05/INSPIRATION.md` (Linear hierarchy/LCH, Raycast contextual bar, Rauno depth, event-based activity, four-tier status, Claude agent peek). This is a design synthesis of that supplied research, not a new empirical claim about those products.

Read cas-6e3a's `conversation-polish.brief.md` at `12dcf7679`: preserve declared options, one ask, safe previews, exact roster joins and human labels. Its possible Send-for-review contrast defect is not copied; the main choice here uses the inherited paired foreground/action tokens. cas-2b3a5 peer #3541038 supplied CONNECTION-STATE-CONSTRAINTS.md at `095c7895b`: connection labels are Live / Unsteady / Connecting / Reconnecting / Needs pairing / Unreachable, separately from task Working / Idle / Done. `lastSuccessAt` is connection success, never activity freshness. Heartbeats are 5s, Unsteady after 2 misses, reconnect after 4. An unknown fetch retains `network_or_browser_policy_unknown`, never proves pairing loss; permission knowledge and hub authorization remain separate. A live attachment cannot conceal an Unsteady machine, and stopped-upstream is session-specific. The mockup uses Live/Unsteady as connection subtitles and preserves missing-observation wording; implementation must consume the exact final contracts.

The operator should approve or revise the decision seam, serif question, single-accent treatment and phone peek composition. Supervisor relays this package. **No product code, DESIGN.md update, browser-journey claim, or task completion before approval.** Phase 2 must update DESIGN.md from actual generated tokens, implement in slices, and prove real built-dist state/keyboard/draft/connection transitions, reduced-motion matchMedia, all journeys and independent QA.

## Critique

Scored by silent-viper-51 on 2026-10-05 after inspecting all four exact-size mockups and both print schemes. These are **self-critique scores for the static proposal**, not independent QA or completed product acceptance.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | The indigo row-to-question decision seam and serif question are visible in both schemes; no machine rainbow competes with them. |
| Fit to argument | 4 | One Needs-you row resolves into the full question and its two declared choices; Working is quiet but still carries an actual action line. |
| Hierarchy | 4 | The decision and main choice dominate; the smaller Send control is a second action in the same track, worth revisiting after live interaction. |
| Craft | 5 | Strict visual QA PASS: 4 renders, 0 findings, 0 informational, 0 allowlisted. Exact captures put desktop bar bottom at 791.7/800 and phone composer bottom at 812.4/844, with no horizontal overflow. |
| Accessibility | 4 | JS-disabled/no-network captures preserve the content; primary text contrast ≥9.27:1, decision edge ≥7.91:1; labeled 44px controls and two print PDFs. App keyboard behavior and screen-reader traversal remain Phase 2 proof. |

Total 21/25; distinctiveness, fit and hierarchy each meet the ≥4 floor. `visual-qa.md` records the strict static receipt. `capture-receipt.json` measures first-fold geometry and contrast. Minimum primary contrast is the measured main-choice pair, not a claim about every future component.

Initial render critique: the desktop contextual bar was below 800px and the phone composer slightly below 844px; tightened list rhythm and shortened the illustrative question body. Final four native viewport PNGs verify the question, choices and composer fit, plus the desktop bar. Secondary phone rows remain below the fold, rather than being hidden. The print table continues on page 2, preserving all four conversations.

## Review package

- `commander.html`: self-contained responsive static proposal; follows the browser scheme, no external assets/scripts.
- `mockups/light-desktop.png`, `mockups/dark-desktop.png`: exactly 1280×800.
- `mockups/light-phone.png`, `mockups/dark-phone.png`: exactly 390×844.
- `mockups/light-print.pdf`, `mockups/dark-print.pdf`: print inspections, all conversation content retained.
- `visual-qa/visual-qa.md`, `visual-qa/visual-qa.json`: strict diagnostic receipt and full-page captures.
- `capture-receipt.json`: JS-off, zero external requests, reduced-motion static captures and measured geometry.

PNG/PDF/JSON evidence lives in the durable cas-675e artifact directory; the brief, single-file HTML and small strict receipt are committed together under `docs/design/commander-refresh-phase1/`. Product code and `hub-web/DESIGN.md` are untouched.
