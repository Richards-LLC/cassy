---
source: [hub-web/src/tokens.css, docs/design/design-tokens.json]
inherits: petrastella
theme: dual
colors:
  bg: "--bg-root #F7F4EE / #12141A"
  surface: "--bg-panel #FFFFFF / #191C24"
  surface-raised: "--bg-raised color-mix(in srgb, var(--bg-root) 92%, var(--text-hi)) / color-mix(in srgb, var(--bg-root) 92%, var(--text-hi))"
  border: "--line-subtle #DAD3C7 / #2B3040"
  border-strong: "--line-strong #8F8371 / #6B7390"
  text: "--text-hi #1B1D24 / #E9E6E0"
  text-muted: "--text-mid #5A5F6E / #A3A7B4"
  primary: "--color-action #2E3A9F / #A9B3FF"
  accent: "--color-verdict #2E3A9F / #A9B3FF"
  focus: "--color-focus #2E3A9F / #A9B3FF"
  success: "--state-ok #226845 / #5FC492"
  warning: "--state-warn #7F5504 / #E2B14D"
  danger: "--state-crit #B3261E / #EF7B72"
  idle-mark: "--color-series-neutral #6B7280 / #9AA1AF"
  terminal: "--bg-terminal #0C0E13 / #0C0E13"
pebble:
  canvas: "--canvas #F7F4EE / #12141A"
  panel: "--panel #FFFFFF / #191C24"
  sheet: "--sheet-bg #FFFFFF / #1F232D"
  fold: "--fold #EAE5DB / #262B35"
  ink: "--ink #1B1D24 / #E9E6E0"
  ink-mid: "--ink-mid #5A5F6E / #A3A7B4"
  ink-soft: "--ink-soft #494E5C / #B4B8C4"
  you: "--you-bg #2E3A9F / #A9B3FF"
  you-text: "--you-fg #FFFFFF / #12141A"
  accent-default: "--accent #2E3A9F / #A9B3FF"
  accent-soft-default: "--accent-soft #DDE1F7 / #232838"
  accent-text: "--accent-fg #FFFFFF / #12141A"
  supervisor-default: "--sup-bg #E7EAF7 / #262B38"
  supervisor-text: "--sup-fg #1B1D24 / #E9E6E0"
  ask: "--ask-bg #E2B14D / #E2B14D"
  ask-text: "--ask-fg #1B1D24 / #12141A"
  ask-tray: "--ask-deep #7F5504 / #6E551C"
  warn-text: "--warn-text #7F5504 / #E2B14D"
  crit: "--crit-bg #B3261E / #EF7B72"
  crit-text: "--crit-fg #FFFFFF / #12141A"
  you-bubble: "--you-bubble-bg #2E3A9F / #3A46B0"
  you-bubble-text: "--you-bubble-fg #FFFFFF / #FFFFFF"
  ask-tint: "--ask-tint #E2B14D / #3A3020"
  ask-tint-text: "--ask-tint-fg #1B1D24 / #E9E6E0"
  ask-tint-tray: "--ask-tint-deep #7F5504 / #2B2418"
  ask-edge: "--ask-edge transparent / #E2B14D"
  tray-chip: "--tray-chip-bg #FFFFFF / transparent"
  tray-chip-text: "--tray-chip-fg #1B1D24 / #E9E6E0"
  tray-chip-line: "--tray-chip-line transparent / #E2B14D"
  pin-chip: "--pin-chip-bg #FFFFFF / #E2B14D"
  pin-chip-text: "--pin-chip-fg #1B1D24 / #12141A"
  pin-chip-hover: "--pin-chip-hover var(--bg-hover) / #EDC169"
  tray-focus: "--tray-focus #FFFFFF / #E2B14D"
  crit-tint: "--crit-tint #B3261E / #3A1F1E"
  crit-tint-text: "--crit-tint-fg #FFFFFF / #E9E6E0"
  crit-edge: "--crit-edge transparent / #EF7B72"
  lift: "--lift 0 1px 2px rgba(18,20,26,0.05), 0 6px 18px rgba(18,20,26,0.06) / 0 1px 2px rgba(0,0,0,0.32), 0 6px 18px rgba(0,0,0,0.34)"
  lift-strong: "--lift-strong 0 2px 4px rgba(18,20,26,0.08), 0 14px 34px rgba(18,20,26,0.10) / 0 2px 4px rgba(0,0,0,0.40), 0 14px 34px rgba(0,0,0,0.46)"
  lift-edge: "--lift-edge 10px 0 30px -18px rgba(18,20,26,0.22) / 10px 0 30px -18px rgba(0,0,0,0.60)"
  lift-head: "--lift-head 0 8px 20px -14px rgba(18,20,26,0.30) / 0 8px 20px -14px rgba(0,0,0,0.70)"
glass:
  source: "hub-web/src/glass.css overrides these generated roles; values are light, dark"
  root: "light #EFECFB, dark #0E0C20 (role --bg-root)"
  raised: "light #F7F5FF, dark #23203E (role --bg-raised)"
  material: "light rgba(255,255,255,0.6), dark rgba(22,20,44,0.54) (role --look-glass)"
  material-strong: "light rgba(255,255,255,0.78), dark rgba(26,23,52,0.74) (role --look-glass-strong)"
  row-open: "light rgba(91,63,224,0.14), dark rgba(168,151,255,0.16) with a 2px --color-action inset edge (role --look-row-open)"
  blur: "blur(16px) saturate(160%) for the four chrome panels and dialogs only, never per message; 26px and stacked shadows measurably slowed timing-sensitive journeys (role --look-blur)"
  ink: "light #1A1830, dark #EEEBFF (role --ink)"
  ink-mid: "light #4A4762, dark #B8B3D9 (role --ink-mid)"
  action: "light #5B3FE0, dark #A897FF (role --color-action)"
  send: "#7656FA to #5B3FE0 to #3E5BEA with white text (role --look-send); hover #6847F2 to #4E33D2 to #3550DC (role --look-send-hover)"
  you-bubble: "light #6C4DF5 to #3651D8, dark #7656FA to #4766E6, white text (role --look-you)"
  ask: "light #FFD983 to #FFA184, dark #FFCF73 to #FF8F6E, text #2A1A05 (role --look-ask)"
  ask-tray: "light #A8471D to #9B3A2C, dark #7A3410 to #6E2A22 (role --look-ask-tray); hovered answer pill #F3EEFF (role --tray-chip-hover)"
  aurora: "violet, teal, pink and blue corners over a pale (light) or near-black (dark) field; source gradients --look-aurora-source, painted as a rendered 320x200 image (role --look-aurora)"
  rail: "382px on desktop, so the 8px-inset floating list keeps 374px of content; 374px on a phone (role --conversation-rail-width)"
typography:
  families:
    display: "--font-display \"Iowan Old Style\", \"Palatino Linotype\", Palatino, \"Book Antiqua\", Georgia, \"Times New Roman\", serif"
    body: "--font-ui Inter, ui-sans-serif, system-ui, -apple-system, \"Segoe UI\", Roboto, sans-serif"
    mono: "--font-mono \"JetBrains Mono\", \"IBM Plex Mono\", ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"
  scale:
    eyebrow: "--fs-xs 12px / 16px / 600"
    meta: "--fs-meta 13px / 18px / 400"
    caption: "--fs-base 14px / 20px / 400"
    ledger: "--fs-md 15px / 22px / 400"
    lede: "--fs-lg 21px / 30px / 400"
    verdict: "--fs-verdict clamp(24px, 3vw, 34px) / 1.1 / 400"
    terminal: "--fs-terminal 13px / 1.35 / 400"
  weights: "--weight-regular 400, --weight-medium 500, --weight-semibold 600"
  tracking: "--tracking-label 0.08em"
  control-line-height: "--line-ui 1.43"
spacing:
  base: "4px"
  steps: "--space-1 4px, --space-2 8px, --space-3 12px, --space-4 16px, --space-6 24px, --space-8 32px, --space-12 48px, --space-16 64px"
radius:
  control: "--radius-card 4px"
  panel: "--radius-pane 8px"
  dot: "--radius-pill 999px"
elevation:
  overlay: "--shadow-overlay 0 24px 80px rgba(18,20,26,0.40)"
geometry:
  rail: "--machine-rail-width 48px"
  conversation-rail: "--conversation-rail-width 374px"
  button: "--button-height 40px"
  dialog: "--dialog-width 520px"
  phone-rail-target: "--rail-item-min 44px"
breakpoints:
  phone: "(max-width: 53rem), (max-height: 30rem) and (pointer: coarse)"
  landscape-phone: "(max-height: 30rem) and (pointer: coarse)"
  compact: "(max-width: 53rem)"
  narrow: "(max-width: 500px)"
---
## Overview

Cassy Commander is a plain TypeScript client whose only surface is Conversations: a list, thread and context rail in the **Glass** look (light and dark). The supervisor's raw terminal output is available read-only, on request, in a dark Raw output drawer.
Glass (`hub-web/src/glass.css`, imported after `styles.css`) is the only look; there is no look switch. It re-colours the generated roles and adds depth to existing surfaces; it changes no markup. `hub-web/src/tokens.css` is generated from `docs/design/design-tokens.json` by `hub-web/scripts/generate-tokens.mjs`; `docs/design/hub-web/token-map.md` records each mapping and retained console measurement.
The scheme follows the OS with light as the fallback; `commander.scheme` stores `system`, `light` or `dark`, and `hub-web/src/scheme.ts` applies `html[data-scheme]`.
This document records the token foundation and the conversation surface built on it; Conversations is the only surface.
Ghostty's ANSI palette stays in `hub-web/src/terminal/ghostty-adapter.ts`; it is independent of the application palette. Ghostty now runs only as the hidden emulator behind the Raw output drawer.

## Brand

The Hub's product name is **Cassy Cloud**. `public/favicon.svg` is the single
brand asset: the canonical three interlocking Cassy ribbons traced from
`docs/assets/cassy-logo.png`, white, pale aurora blue (#E6ECFF to #A9C2FF) and
aurora teal (#B8F5EA to #6FE0D0), on a 512px rounded tile (radius 114) filled
with the `--look-send` violet gradient (#7656FA to #5B3FE0 to #3E5BEA) under a
soft white sheen. The tile is the same in light and dark. The white ribbon
measures 4.7–6.5:1 on the tile; the tile measures 4.0–5.6:1 on the light glass
field (#EFECFB) and 3.0–4.1:1 on the dark one (#0E0C20). The old mark was a
flat ink, pale #A9B3FF in dark: high luminance contrast but almost no chroma,
which is what read as washed out on the violet aurora.
`hub-web/src/cloud-brand.ts` renders that file as a 32px `<img>` (a data URI,
so each copy keeps its gradient ids private) beside the serif wordmark in
`--font-display`, medium weight, `--fs-lg` (21px), with `--space-2` separation.
`hub-web/scripts/render-brand-icons.mjs` paints the PNGs from the same file:
`favicon-16.png` and `favicon-32.png`, `icon-192.png` and `icon-512.png`
(rounded tile), `apple-touch-icon.png` (180, full-bleed square; iOS rounds it)
and `icon-maskable-192.png`/`icon-maskable-512.png` (full-bleed, ribbons at
scale 0.95 inside the 80% safe circle). `index.html` links them and
`public/manifest.webmanifest`; the hub embeds every one (`server.rs`). Rerun
the script after changing `favicon.svg`. No downloaded font is required.

Installed app: the manifest names the app **Cassy** (`name` and `short_name`),
`display: standalone`, `start_url` and `scope` `./` (the `/commander/` base),
`background_color` #0E0C20 (Glass dark `--bg-root`, the splash behind the
icon) and `theme_color` #5B3FE0 (Glass light `--color-action`); `scheme.ts`
still sets the live `theme-color` meta per scheme. iOS reads
`apple-touch-icon.png` and the `apple-mobile-web-app-*` meta (title Cassy,
capable, `black-translucent` status bar; the shell already pads for
`safe-area-inset-top`).

Rules:

- Never recolour the mark with `--color-action` or `currentColor`, or draw it
  as flat ink; the tile and its three ribbon fills are the brand.
- The tile never sits on another violet fill (a `.primary` control, the
  `--look-you` bubble); on glass, on either field and on launcher backgrounds
  it needs no plate or outline.
- Minimum sizes: 16px (tab), 32px (sidebar lockup and pairing dialog), 180px
  apple-touch, 192px and 512px install icons. Never render the mark below 16px.
- Maskable icons keep every ribbon inside the centred 80% safe circle; check a
  circle crop after any geometry change.

Place the complete brand at the top of the thread list on phone and desktop;
the list screen is the only place the phone shows the lockup. The phone
conversation header omits it so the header stays one row (about 56px): a 40px
"‹" back target, a 36px avatar, the supervisor codename (ellipsised) beside a
13px project badge, the host line beneath, then Interrupt (the word, no icon)
and an icon-only Raw output. The buttons keep their full accessible names
("‹ Conversations", "Interrupt the <project> supervisor", "Raw output").
The desktop context rail repeats a quiet 14px wordmark with a 24px mark.
The main wordmark is 21px with a 32px mark; compact minimums are 14px / 24px.
Reserve 8px around the mark. The vector is decorative beside readable text,
never an unlabeled navigation control. A project badge names the work, not the
product: use `--bg-active` and `--color-action`, 15px semibold, allow wrapping,
and derive its name from catalog `project_dir` with an honest missing state.

Conversations are the default presentation at every width. Desktop places a
floating 382px glass list and the glass context rail around a flexible reading column. Phone and
short-axis touch layouts use list → full-width thread with an always addressed
composer. `conversation-shell.ts`, `conversation-list.ts`, `conversation-view.ts`
and `conversation-history.ts` separate layout, catalog rows, the real pane,
and correlated channel events. There is no other presentation.

The conversation header carries the session controls. **Interrupt**
(`#conversation-interrupt`, `--state-crit` outline) sends `InterruptPane` to the
supervisor pane, taking the session's lease implicitly the way a send does; when
another device held control the toast says "Took control from <device>", so the
takeover is never silent. When it cannot act (outage, not yet connected, no
`pane-interrupt` permission, no supervisor pane) it stays visible with
`aria-disabled` and its reason in `#conversation-interrupt-reason`. **Raw output**
(`#conversation-raw-output`) opens `dialog#raw-output`: a read-only
`TranscriptView` of the supervisor pane on `--bg-terminal`, a 640px right sheet
on desktop and a bottom sheet on a phone, following the tail; Escape or Close
returns focus to the button. It takes no lease and sends no input.

The conversation mounts into `#pane-grid`, which holds two siblings: the visible
`.conversation-thread-slot` and `.pane-host`, the supervisor pane's emulator
surface. `.pane-host` is `hidden`, `inert`, `aria-hidden` and `display: none
!important`: it paints nothing, takes no pointer or focus, and the thread is never
inside or behind it (`conversation-stage.test.ts`). It exists only so the
attach keeps the pane's text for Raw output and its activity for the working
line.
The generated dark-well selector excludes `.conversation-active` so reading
inherits the chosen page scheme; code scrolls locally without clipping prose.

## Colors

- Glass overrides `--bg-root` with its own field (light #EFECFB, dark #0E0C20) under the aurora; `--bg-panel` is opaque white (light) or #1A1830 (dark) and is the opaque fallback for every glass panel under more contrast, reduced transparency or forced colours.
- The house `--bg-raised`/`--bg-hover` derivations are overridden by Glass: `--bg-raised` is an opaque lavender-white (#F7F5FF) or deep indigo (#23203E) card, so worker cards, the paired-machines row and dialog footers never read as grey slabs on glass; `--bg-hover`/`--bg-active` are violet washes. Header controls (‹ Conversations, Raw output, Pair a machine in the list header) are text buttons: transparent, `--ink`, `--bg-hover` on hover. These are the console's two derived overrides; selection uses house `verdict-soft` through `--bg-active`.
- `--text-hi` inherits `ink`; `--text-mid` inherits `ink-muted`. There is no tertiary text token; timestamps and pane roles use the readable muted value.
- `--color-action` is for controls and links; `--color-verdict` is for the decisive figure mark. Both inherit the house accent; neither is a running-status colour.
- `--color-focus` supplies the sole focus outline. `--state-ok`, `--state-warn` and `--state-crit` inherit `good`, `warning` and `danger`; info text is muted evidence.
- `--state-idle` and `--color-series-neutral` inherit `color.series-neutral`; idle text uses `--text-mid`, while dots use the neutral mark value.
- `--tint-warn` and `--tint-crit` inherit the corresponding house tints for actionable warnings and critical events. `.danger` actions remain text on a normal control surface.
- Message delivery states (cas-ca7f): a message known **not sent** is critical (`--crit-bg` outline, glyph and label); a message only **not confirmed** — it may well have arrived — is caution (`--warn-text`), in the thread and on the dismissed-messages chip alike. The chip turns critical as soon as it counts any message known not sent; a settled record (the supervisor replied since) drops to `--ink-mid`.
- Reply receipt facts (cas-e6d2): **Forwarded · not stored on this device** means this browser received the reply but has not committed it to its IndexedDB journal. **Stored on this device** follows the durable commit and authorizes the authenticated application receipt. Neither label asserts that the operator read the reply. Shared read state and cloud replay cursors remain separate contracts.
- Kept commands use per-item IndexedDB transactions, scoped to the exact hub URL, installation device and session. Tabs claim a client reference before its socket write; a sending or unconfirmed claim has no automatic expiry/reclaim that could execute it twice. Held messages retain the existing two-minute wait and explicit cancellation. Reload restores waiting and uncertain states. A storage failure keeps the existing recovery copy and grants no application ACK.
- The direct delivery journal supports legacy or explicitly un-enrolled installations only. Future account enrollment fails closed until authenticated account/feed identity and projection clearing are implemented. Local reply payloads expire after 90 days, including on a quiet reopen; the reply cap is 400 and kept-send scopes/count/bytes stay bounded. Remove/revocation deletes private payloads and fences stale writers.
- `--bg-terminal` stays #0C0E13 in both schemes. The generated dark-well scope supplies `color.dark.*` and `color.series-neutral.dark` to the Raw output transcript, search/dialog inputs, pairing code and log/payload `pre` elements.
- Dark-well descendants inherit matching dark control surfaces, lines and foregrounds; the generated scope repeats the derived surface expressions so they resolve against its own dark roles.
- `--overlay-backdrop` derives from page `bg`; `--overlay-shadow-color` is extracted from `elevation.overlay`. Neither borrows an ANSI colour.

## Typography

- Display (`--font-display`) inherits the Iowan/Palatino/Georgia house serif for the connection verdict sentences; body (`--font-ui`) inherits Inter/system sans; identifiers (`--font-mono`) inherit JetBrains/IBM Plex/system mono.
- `--fs-xs` is the house 12px eyebrow, `--fs-base` the 14px caption, `--fs-md` the 15px ledger and `--fs-lg` the 21px lede. Pane/session metadata uses the retained 13px `--fs-meta` console step.
- `--fs-verdict` clamps the house title to 24px–34px at 3vw; the brief gives that slot 1.1 line-height and −.015em tracking. It is available for the screen units; existing headings still consume `--fs-lg` or `--fs-md`.
- `--tracking-label` inherits the house .08em eyebrow tracking. Weights are 400/500/600; the 500 hero-number weight is available for pairing code; ordinary copy stays 400 and headings top out at 600.
- `--line-ui` is caption line-height divided by size (20/14 → 1.43). Ghostty keeps `--fs-terminal` 13px, `--line-terminal` 1.35 and its 12–16px runtime clamp.
- Codenames, IDs, paths, timestamps, phases, scope names and JSON use mono even inside prose. Session codenames are never uppercased.

## Layout

- Spacing inherits the house 4px grid: 4/8/12/16/24/32/48/64px. Retired 20px gaps move to 24px; retired 40px control dimensions use `--button-height` so touch geometry stays 40px.
- Desktop is the conversation shell: the floating 382px glass list, a flexible reading column open to the aurora, and the glass context rail (collapsing to its 48px track).
- The phone query is `(max-width: 53rem), (max-height: 30rem) and (pointer: coarse)` in both CSS and `hub-web/src/viewport.ts`; landscape uses the same short-axis query so rotation keeps the phone layout.
- A phone shows list → full-width thread; every touch target uses `--rail-item-min` 44px. Tasks & progress and Waiting on you open as sheets; Raw output is a bottom sheet.
- Interior regions scroll within the shell's `100dvh`; the Raw output transcript scrolls inside its sheet.

## Elevation & Depth

- **Glass materials.** One aurora (`--look-aurora`) sits behind the whole shell on a fixed layer and never moves: any animation behind glass makes every frosted panel re-blur each frame, which measured 30 fps instead of 59 and timed out journeys. Its colours are defined once as gradients (`--look-aurora-source`), but it is painted from a 320×200 JPEG that `hub-web/scripts/render-glass-aurora.mjs` renders from them into `--look-aurora`. Live gradients behind see-through panels repainted on every update inside a panel and timed out the event-flood and session journeys; the stretched image is indistinguishable and cheap. Rerun the script after changing a source gradient. The conversation list, conversation heading, context panel and composer float over it as frosted panels: `--look-glass` with `--look-blur`, a 1px `--look-glass-edge` drawn as an inset shadow (never a border, so geometry matches the panels underneath), `--look-shadow-float`, 20–26px radii and an 8px inset from the window edge (the desktop rail is 382px so its content keeps 374px). On a phone the list is full-bleed glass and the conversation heading hangs from the top edge with rounded lower corners, keeping the 56px phone header. The middle column is open to the aurora. Reading surfaces stay nearly opaque: supervisor bubbles are `--sup-bg` (0.9) with no blur of their own (a blur per bubble cost a quarter of a long thread's frame rate), the operator's bubble is the `--look-you` gradient (a refused or unconfirmed message never takes it: it keeps the base's dashed, unfilled record in `--ink` on `--look-glass-strong`), and dialogs are `--look-glass-strong` over a blurred `--look-scrim`. The open conversation's row carries a violet `--look-row-open` wash and a 2px `--color-action` edge all round (5.7:1 light, 7.5:1 dark against the list), never a left bar; a hovered row gets only the faint `--bg-hover` wash with no edge, so hover can never look open. `glass.test.ts` pins the edge contrast and that hover draws no edge.
- **Colour, used sparingly.** Violet is the one action colour: Send, the compose button and every `.primary` action carry the `--look-send` gradient and its glow; on hover they deepen to `--look-send-hover` and are never brightened (a `brightness(1.08)` filter put white under 4.5:1). An open question is the warmest object on screen, an amber-to-coral `--look-ask` card over an ember `--look-ask-tray`; its answers are opaque pills that stay opaque on hover (`--tray-chip-hover`), because the translucent `--bg-hover` wash lets the ember through. Every interaction state of a control on a gradient or the tray is measured in `glass.test.ts`, which resolves the winning background, colour and filter from both stylesheets. Machine accents keep each avatar's hue but no longer tint bubbles or the selected row.
- **Sheets, actions and confirmations.** A dialog's sticky action bar is a light frost (`--look-glass-bar`) with a hairline above and a 12px gap before it; the installations sheet keeps its bar as a footer outside the scroller (transparent, hairline only), so its last Revoke shows whole in the default view at 390; its plain buttons (Close, Back) are outlined pills. Full-screen phone sheets (the launch sheet and the Attention sheet) are opaque `--bg-panel` and square, so the page never ghosts through. In Paired machines, Browser installations and Remove sit 8px apart and Remove is a `--crit-bg` outlined pill. The toast is a solid `--bg-panel` card that appears at once (it still slides, but never fades through the page). The operator's delivery line (Sending…, ✓ Delivered) is full white on the violet bubble, and under more contrast the bubble is the flat `--you-bubble-bg`.
- **Text on glass.** Text sits on a known colour: opaque cards, ≥ 0.6-alpha panels, or (timestamps and meta lines only) the aurora's pale middle in light and its near-black field in dark. The vivid corners lie under the panels. `glass.test.ts` measures every token pair, composited over the aurora's stops, at ≥ 4.5:1; each delivery also measures real rendered pixels.
- **Accessibility.** Forced colours: all Glass decoration sits inside `@media (forced-colors: none)`, so the system palette and the forced-colours rules in `styles.css` apply untouched. `prefers-contrast: more`, `prefers-reduced-transparency: reduce` or forced colours: panels, bubbles and sheets become opaque `--bg-panel` with no blur and `--line-strong` edges. More contrast also darkens `--ink-mid` toward `--ink`, makes subtle lines strong, and replaces the aurora with flat `--bg-root`. Motion: Glass adds none, so `prefers-reduced-motion` needs no Glass override; the house motion rules apply.
- `#toast` in `hub-web/src/styles.css` consumes house `elevation.overlay` through `--shadow-overlay`, identical in both schemes; Glass dialogs use `--look-shadow-float` instead. Phone drawer and attention sheets stay shadowless.
- The Pebble conversation surface (EPIC cas-cac1, `docs/design/hub-messaging/round-3/`) uses elevation instead of hairlines: the rail edge is `--lift-edge`, the selected row and calm bubbles are `--lift`, attention objects and the compose FAB are `--lift-strong`, the thread header is `--lift-head`. All four are generated per scheme (ink-cast in light, black-cast in dark); `invariants.test.ts` pins every `box-shadow` to a token.
- Pebble tokens live under `pebble:` above and are the contract every Pebble child consumes: `--canvas`/`--panel`/`--sheet-bg` surfaces, the `--ink` ramp, the constant operator pair `--you-bg`/`--you-fg`, the ask and blocker pairs, and the per-machine set `--accent`, `--accent-soft`, `--accent-fg`, `--sup-bg`, `--sup-fg`. The machine set is generated as `.machine-accent-N` (indigo, green, violet) and assigned by `hub-web/src/machine-accent.ts` from the machine id — FNV-1a into a jump consistent hash, so a fourth accent appended to `generate-tokens.mjs` recolours no existing machine. `machine-accent.test.ts` measures every rendered pair at ≥ 4.5:1 in both schemes.
- Dark tints, never floods (P9): in dark an in-thread ask is `--ask-tint` #3A3020 in ink with a 4px `--ask-edge` gold edge and outlined tray chips; a blocker is `--crit-tint` #3A1F1E in ink with a 4px salmon `--crit-edge`; the operator's bubble is `--you-bubble-bg` #3A46B0 with white text. A compact waiting bookmark points to the single in-thread question; it has no duplicate gold card or reply chips. Send keeps the bright accent. `--you-bg` stays the operator's action colour (FAB, focus fallbacks); the bubble has its own pair. In light every tint token resolves to the flood it replaced, and the edges are transparent, so paper is unchanged. `machine-accent.test.ts` pins the pairs (text ≥ 4.5:1, edges and chip outlines ≥ 3:1).

## Shapes

- `--radius-card` now inherits house chip radius 4px for controls; `--radius-pane` inherits panel radius 8px for panes/dialogs. Current rows still consume these until their screen units introduce ruled ledgers.
- `--radius-pill` stays 999px for dots and existing count/chip shapes; it is a console exception, not a new house radius.
- Hairlines use house `chart.hairline` through `--line-width` 1px. Critical rules stay 2px; `--rule-verdict` inherits `chart.mark-decisive` 2.5px and `--rule-hero` is the brief's 3px verdict rule.
- Focus uses `--focus-ring-width` 2px and `--color-focus`. Motion is house chrome 120ms/reveal 200ms with `--motion-easing`; the retired connection spin duration and animation declaration are gone.

## Components

- Shell: `render()` in `hub-web/src/main.ts` builds the conversation shell (`hub-web/src/conversation-shell.ts`) plus the palette and pairing dialogs. `setScheme()` serves the command-palette Appearance entry.
- Conversation header: Interrupt and Raw output (`conversationActionsMarkup`, `syncConversationActions`); see Brand above.
- Raw output: `dialog#raw-output` in `hub-web/src/conversation-shell.ts` with `hub-web/src/transcript-view.ts`; the transcript is a dark well and its controls inherit dark control roles.
- Connection surface: `hub-web/src/connection-state-view.ts`; existing connecting/failed/retry markup remains. Its log `pre` is a dark well; the screen unit owns the outcome sentence and attempt timeline.
- Attention: `hub-web/src/attention-view.ts` renders the conversation's own Attention section; critical/warning cards use semantic tints, info uses muted evidence, and payload `pre` remains dark. Only asks and blockers are "waiting on you" (`waitingOnOperator()` in `hub-web/src/context-rail.ts`); awaiting-merge and lifecycle events never count as needing the operator.
- Workers/tasks and composer: Tasks & progress in the context rail (`renderStatus` in `hub-web/src/main.ts`, `hub-web/src/fleet-ops-view.ts`); mono identifiers, muted supporting copy and green in-progress status use the new roles; Send stays explicit.
- Pairing dialog: `pairDialogMarkup()` in `hub-web/src/main.ts` and cancel semantics in `hub-web/src/pairing-dialog.ts`; inputs and code wells get dark foregrounds, while the dialog and detail terms follow the page scheme.
- Buttons and inputs: `hub-web/src/styles.css`; full controls retain 40px height and compact pane controls 28px. Keyboard focus uses the house focus role; disabled copy uses muted text.
- Toast: body-level `#toast` in `hub-web/src/main.ts`; raised surface and the house overlay shadow, above the phone composer.
- New session: body-level `#launch-dialog` owned by `LaunchSheet` in `hub-web/src/launch-session.ts`, outside `#app` so a shell rebuild never replaces it mid-choice. On a wide screen (≥62rem) and a landscape phone the form is two columns: projects with Workers under them on the left, Supervisor and Account on the right, and the summary with Cancel/Start as one sticky row across the foot; the whole form fits above Start at 1280×800 with four accounts. A 600px sheet between 40rem and 62rem puts Workers beside Supervisor with Account under both; a phone is one full-screen column (Supervisor, Account, Workers). Only the supervisors POST /v1/sessions accepts are offered: Claude, Codex, Grok. The Account step (cas-9666) lists every account for Claude or Codex from GET /v1/launch/profiles, the machine default preselected with a "Default" pill; a logged-out account is shown disabled with its `cas <cli> login <name>` command and Copy; names wrap anywhere; Grok and a CLI that is not installed have no step; a failed check says so with Try again and the launch falls back to the default account. Every open resets supervisor, account, workers and filter to the machine defaults, and the summary names the account and any workers. Rows are `--panel` on the raised dialog; the chosen project and supervisor carry `--bg-active` with a `--color-action` edge (an `.is-checked` class, not `:has()` alone); Attach and Start are the only `.primary` controls. Progress is a `--rule-hero` verdict rule with a quiet elapsed count, never a spinner; a refusal is a `--state-crit` edge with the machine's own words in a disclosure. The list header shows "New session" beside Pair a machine only when a machine grants `session-launch`; otherwise "Allow new sessions" opens the grant path.

### Operator questions and progress (cas-6e3a)

The full stepped ask lives once in the conversation thread. Quick replies are only the options declared by that ask; absent or empty options leave free-text reply to the composer. The waiting bookmark jumps to the thread question instead of repeating an expanded card above the composer. Excerpts use the safe Markdown renderer's plain text before truncating, so preview punctuation never leaks formatting markers.

The context progress rail uses human labels such as “In progress” and “Awaiting merge”, wraps long names and task titles within its width, and lists the selected session's actual worker roster. Each worker shows its reported current task/title; absent details remain unreported. Raw last task notes are not current work. Worker controls still require reported capability and generation, never synthesized from catalog names. Task lifecycle notices do not become operator questions. Implementation/evidence contract: `src/conversation-polish.brief.md`.

## Do's & Don'ts

- ✅ Change house values in `docs/design/design-tokens.json`, update the mapping in `hub-web/scripts/generate-tokens.mjs`, and run `npm run tokens`; commit the generated CSS.
- ❌ Never hand-edit `tokens.css`, add a handwritten `:root` palette to `styles.css`, or restore the retired token aliases; `tokens.test.ts` checks drift and every CSS/TypeScript consumer.
- ✅ Glass values live only in `glass.css`'s token blocks (`:root, html[data-scheme="light"]` plus the dark media block and `html[data-scheme="dark"]`, which must stay identical). Glass surface rules consume those tokens; `glass.test.ts` pins the pairs and the accessibility media rules.
- ❌ Never put a gradient, blur or animation outside `@media (forced-colors: none)` in `glass.css`, animate anything behind glass (the aurora included), or reintroduce a look switch (`data-look`, `?look=`).
- ✅ Add a generated dark-well scope when retaining a dark background beneath a light shell; source foregrounds from `color.dark.*`.
- ❌ Never combine light-scheme ink with `--bg-terminal` or wire Ghostty ANSI entries to application state tokens.
- ✅ Use `applyScheme()` at boot and `setScheme(system|light|dark)` for Appearance; storage denial still permits a page-local choice.
- ❌ Never put scheme state in `shellSignature()` or remount the hidden pane surface merely to change chrome colours.
- ❌ Never render conversation content inside `.pane-host`, give the host a visible size, or reintroduce a second presentation beside Conversations.
- ✅ Keep machine text mono, focus outlines visible and every shadow a token consumer (`--shadow-overlay` for overlays, `--lift*` for Pebble objects).
- ❌ Never redefine a Pebble contract token in a component or put a machine accent on the compose FAB: compose is the operator's action and stays `--you-bg` at `:root`.
- ❌ Never use accent as an info status, add a looping connection animation, or restore a low-contrast tertiary text step.
- ✅ Let the integration owner rebuild `hub-web/dist` once; validate lane builds with a separate worktree output directory.
- ❌ Never hand-edit or commit generated `hub-web/dist` output from a factory lane.

## Behavioural constraints
<!-- keep -->

These are engineering decisions the visual system sits on. They survive redesigns.

**Render model.** `render()` chooses one of three paths via `renderDecision()` in `hub-web/src/render-model.ts`: *regions* (default, the only path a heartbeat may take — `renderRegions()` writes into nodes already on screen), *shell* (full rebuild, only when `shellSignature()` changed), or *defer* (signature changed while a form control has focus; flushed by `DeferredRenderScheduler` in `hub-web/src/deferred-render.ts` after focus leaves and the pointer gesture has delivered its click — a macrotask, because the click is dispatched in the same task as pointerup). If a value appears in shell markup it belongs in `shellSignature()` or `applyLiveRegions()`; per-heartbeat data must never enter the signature; `applyLiveRegions()` writes only into existing nodes; anything a region re-creates (conversation list rows, rail sections, header actions) binds its own handlers; lease identity is deliberately structural.

**Browser installations.** Each Paired machines row opens an installation inventory for that named hub. The current browser leads with “This browser” and its full device ID; a ruled ledger shows generation, first paired, last use, origin, signing-key fingerprint and account enrollment. Origins and fingerprints wrap on phone; the body scrolls and Close remains reachable. Revoke confirms the exact device ID and ends its hub access; Remove from this browser is a separate local action. Other installations require hub-admin permission. Account enrollment currently displays Un-enrolled; display names never imply verified account identity.

Re-pairing proves the retained origin-local signing key and updates the same installation. Prepare, catalog stage, server commit and catalog activation are separate boundaries. Cancel confirms remote abort before restoring prior browser access. Uncertain cleanup stays visible with Retry cleanup and quarantines the affected hub until recovery is confirmed. Key replacement additionally proves the prior key. Tabs share a hub-scoped Web Lock and reload accepted generations from the same-origin catalog; broadcasts carry only invalidation, never secrets. Lost storage enrolls a distinct device and requires explicit revoke consent to replace an old one. The browser never merges devices by their display labels.

**Pairing cancellation.** Cancel discards the invitation: `cancelPendingPairing()` invalidates the in-flight operation, clears the pending store and resets the draft; reopening offers the create-code flow and says pairing needs a fresh URL from `cas hub pair`. A pairing invitation is a one-time capability and Cancel is the operator saying the request must not proceed — including when the link went somewhere it should not have. The dialog closes only once the page can vouch that the cancellation is durable; otherwise it stays on a cleanup step with a retry that never resumes the invitation (`cancellationOutcome()` in `hub-web/src/pairing-cleanup.ts`). A cancellation owns that step through `PairingCancellationTracker` in `hub-web/src/pairing-cancellation.ts`: a rollback that rejects after Cancel still lands on the step, retries run one at a time and report rejection inside the dialog, and any replacement flow supersedes the cancellation so a late result can never close or rewrite it. Browser cancel blocks this browser only; it does not revoke the machine's invitation.

**Raw output text (D15).** The hidden surface never resizes the PTY: with no box it keeps the pane's reported grid (`fitHidden()` in `hub-web/src/terminal/ghostty/surface.ts`), so text wraps the way the supervisor's terminal does. The transcript (`hub-web/src/transcript.ts` model, `hub-web/src/transcript-view.ts` DOM) reflows the emulator's logical lines from `GhosttySnapshot.rowData` in the browser — no hub-side projection, no new endpoint. Its history is the emulator's scrollback; reaching the top pages the viewport back and "Jump to latest" returns. Canvas paint is always skipped; the transcript is read-only and never focuses a pane input.

**Phone layout invariants.** The phone rule keys on the short axis and pointer, not width alone (D5); only the supervisor pane is attached, and its host never paints; the header keeps one row with Interrupt and Raw output reachable, and there is exactly one bottom bar (the composer) — never stacked bars; severity is carried by text colour and the dot, never by a fill only some severities receive (D8).
<!-- /keep -->


## Connection evidence and stable Details

An open Connection log shows the measured machine cause while that machine is Unsteady, including when its session attachment remains live. Session-specific causes lead when the machine is healthy. Retry and last-success evidence use the connection's measured state.

NetworkInformation throughput, RTT and effective-type estimates do not count as a changed route. The four missed-heartbeat rule still owns half-open status; actual offline/online, wake and underlying transport-type changes retain their recovery behaviour.

An Attention notice with unchanged identity and copied payload retains its open Details and Copy controls when the panel's outage explanation changes, including shell rebuilds within the same conversation and unchanged roster. The panel follows its view scope, so switching conversations never carries another conversation's panel forward. Its current Copy callback is refreshed. A changed payload or roster replaces the control and restores the corresponding notice's focus.
