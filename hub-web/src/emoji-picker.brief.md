# Brief: Commander emoji composer

## Single idea

An emoji belongs where the operator is already writing: choose one beside the mic, then continue the same draft at the same caret.

## Hero form

An annotated conversation thread: a compact search tray anchored above the composer, with recently used emoji first and a labelled grid beneath. The draft remains visible instead of becoming a separate editing screen.

## Emotional register

Light and familiar: use Commander's Glass surfaces, violet focus/selection, 44px touch targets and the existing Inter text scale. Emoji supply the color; the tray introduces no new palette or motion.

## Distinctive move

The tray gives the caret back. Choosing an emoji replaces the saved selection, updates the ordinary draft through its existing input event, and returns focus after that emoji without submitting the message.

## Deliberately omitted

No remote assets, automatic shortcode rewriting, new message format, giant emoji dependency in the first-paint bundle, or global replacement of the UI font. Existing code/terminal typography and message text remain intact.

## Component and state contract

- A small composer binding owns the trigger and lazy import. It captures the textarea selection before opening, rejects a stale/disconnected conversation, and uses the normal input event for draft persistence.
- A lazy picker owns a local searchable dataset and bounded recents. Native dialog/popover behavior supplies Escape and outside dismissal; labelled controls, a search status and roving arrow navigation provide keyboard and screen-reader access. Storage refusal leaves an in-memory picker usable.
- Emoji content rendering uses one color-emoji font stack for complete Unicode sequences in message bodies, conversation previews and the side panel, without changing their copied or spoken text. Native platform fonts are preferred; any needed fallback must be shipped locally with its license.
- Loading, no search matches, import failure/retry, selection, cancellation and reopening each have an explicit visible result. The tray scrolls within the visual viewport when a phone keyboard reduces available height.

## Acceptance and evidence

Capture the real before/after surface at 1280 and 390px in light/dark; retain the supervisor's earlier 1440px captures when practical. Test search, recents across reopening, replacement at a caret/selection, Escape/focus return, arrows/Enter, empty results and unavailable storage through the production picker. Run vitest, tsc and only source-impact-selected journeys. Record actual browser/OS identities: Chromium phone emulation does not prove physical Android Chrome or Linux font availability. No runtime asset request may leave the origin. The final ledger owns screenshots, ARIA snapshots, strict visual QA, measured lazy payload size, known platform gaps and the critique.

## Critique

Pending rendered implementation and strict light/dark desktop/phone evidence. No scores are claimed before rendering.

## Implementation

`emoji-composer` owns only the trigger, lazy import and current draft selection. `emoji-picker` owns search, in-page/persisted recents and the open tray; its local Emojibase tuples load in that same lazy chunk. `emoji-text` wraps complete sequences in sent/reply prose, previews and waiting questions without modifying source text or inline code. The phone draft gets its own line so the new 44px control does not shrink its text.

The tray uses an opaque Glass panel and existing ink, focus, line and elevation tokens. It anchors above the draft and clamps to the visual viewport; search and controls stay reachable through scrolling when the keyboard reduces available space. There is no animation. Failed imports expose retry on the same trigger; blocked storage preserves the pick and in-page recents. Conversation changes cancel delayed opens and dismiss stale trays.

## Critique

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | The tray stays directly above the same draft and returns its caret; conversation context stays visible. |
| Fit | 4 | Existing Glass surfaces, violet focus and native colorful glyphs; no new palette. |
| Hierarchy | 4 | Search, named emoji choices and optional recents precede the secondary paging action. |
| Craft | 4 | 44px controls, bounded scrolling, whole Unicode sequences and a full-width phone draft. |
| Accessibility | 4 | Named non-modal dialog, search/status, roving results, normal Tab escape, Escape dismissal and caret restoration. |

Initial 390px dark fixture inspection found the tray overlapping the draft; its anchor now uses the field's top edge. Browser evidence and final measurements are recorded in the task ledger. Native Linux/Android font coverage requires those actual platforms; viewport emulation is not platform proof.
