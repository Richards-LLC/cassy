# Brief: Cassy Cloud supervisor conversations

## Single idea
Daniel should recognize the project, read the supervisor's actual words and send an addressed instruction in one continuous view.

## Hero form
A conversation list with prominent project labels leads directly into one readable thread; desktop keeps that same list beside the thread and a quieter context rail. The first meaningful content is the work and its supervisor, not a fleet metric figure.

## Emotional register
Recognizable, calm, direct — Cassy Cloud's system-serif wordmark, warm paper and one indigo mark, with clear sender/source labels instead of terminal chrome.

## Distinctive move
The project badge remains the visual anchor from the first thread row through the conversation header to the addressed Send action.

## Deliberately omitted
No synthesized replies, new message transport, worker-first overview, decorative dashboard, anonymous Send button, external fonts or status expressed by color alone. The authentic terminal stays an explicit alternate view.

## Component and state boundaries
- `cloud-brand.ts`: reusable inline mark + wordmark and project-badge label/markup. It owns presentation and safe catalog-path labeling only.
- `conversation-list.ts`: keyed supervisor-row DOM, preserving focus while freshness and connection fields update. Inputs are derived rows; selecting emits machine ID and session. Main remains the owner of catalog, selected thread and transport.
- Existing `TranscriptView` / `surface.transcript`: the evidence source for real pane text and scrollback. Do not reimplement terminal decoding or invent turns from text patterns.
- Existing `supervisor-message.ts` / sendControl path: validation, lease acquisition and verified operator attribution. Conversation presentation consumes the final operator reply/acknowledgment channel after its epic merge.
- New conversation presentation must separate actual server data, local drafts and derived display state. No 'acknowledged' state until a correlated receipt arrives; a reply is a separate event.

## Verification and budgets
- Exact pane text and operator messages/replies in scoped component/integration tests.
- Real default Hub build in Playwright: project badges; full-width phone thread; addressed Send; sent/acknowledged/replied distinction; terminal alternate; keyboard/draft preservation.
- 390×844, 844×390 and 1280×800, light/dark, plus reduced motion. Review each screenshot and record the verdict/change in `/home/pippenz/.cas/artifacts/cas-11b01/element-review.md`.
- Run Hub npm test, typecheck and an isolated-output build; supervisor owns committed dist regeneration per DESIGN.md unless explicitly instructed otherwise. Measure bundle delta; retain no new runtime dependency.
- UI critique and final evidence remain pending until the real default view is wired and inspected.

## Implementation and critique

The shell reuses existing pane mounts, composer handlers, dialogs and context
regions. `ConversationHistory` owns only browser evidence, keyed by machine and
session in main; `MessageQueued` remains owned by the existing channel. The live
pane is one explicitly labeled document that moves only when its actual text
changes. Redraws never become invented chat turns. Rejections have an Edit
message action; a rendered-thread guard prevents sending after stale selection.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Cassy Cloud serif lockup and indigo project anchors on warm paper/graphite |
| Fit | 5 | Project → supervisor → exact words → addressed reply is the primary flow |
| Hierarchy | 4 | One indigo Send action; quiet host/freshness and desktop context |
| Craft | 4 | Initial toast, tail-follow and terminal-header defects were reworked from screenshots |
| Accessibility | 4 | Named buttons, keyboard-safe drafts, local code scroll and two-scheme strict checks |

QA: 457 unit tests; typecheck and isolated build; six production-bundle viewport/
scheme journeys with controlled protocol fixtures; strict visual QA covers
15 fixtures × 2 schemes × 3 viewports. The per-element decisions and screenshot
reviews live in `/home/pippenz/.cas/artifacts/cas-11b01/element-review.md`; full
labelled evidence is in `LEDGER.md` beside it. Native paired-device delivery is
supervisor-owned integration verification (notification 28848), not claimed by
these browser fixtures. No new runtime dependency; bundle ~72.5KB JS / 12.5KB CSS
gzip. Physical-device interaction timings remain unmeasured.
