# Token map: `hub-web/src/styles.css` `:root` → `docs/design/design-tokens.json`

Every custom property declared in the `:root` block of `hub-web/src/styles.css` at 3.17.3
(lines 1–106; 79 properties plus the `color-scheme` declaration), mapped to the house token it
becomes, or kept as a hub-only token with a reason, or retired. Unit 2 (cas-1aef) generates
`hub-web/src/tokens.css` from this table; Units 3–5 consume the names in the *Becomes* column.

Conventions in the *Becomes* column:

- `color.light.X / color.dark.X` — the house colour role, one value per scheme. Light and dark
  values are quoted from `design-tokens.json` 1.0.0.
- **keep** — stays a hub-only `--*` token, generated alongside the house tokens with the same
  value in both schemes unless a scheme pair is given. Every keep has a reason.
- **retire** — deleted; its consumers move to the named replacement in the unit that owns them.
- A **new** row at the end lists hub tokens this pass introduces.

Scheme note: `:root { color-scheme: dark; }` (line 2) becomes `color-scheme: light dark`, with
`prefers-color-scheme` selecting the value set and `html[data-scheme="light"|"dark"]` forcing
one (brief: *Light and dark policy*).

## Surfaces

| # | Property (3.17.3 value) | Becomes | Light | Dark | Why |
| --- | --- | --- | --- | --- | --- |
| 1 | `--bg-root` #101318 | `color.*.bg` | #F7F4EE | #12141A | page and the 8px gutter between shell regions |
| 2 | `--bg-panel` #151922 | `color.*.surface` | #FFFFFF | #191C24 | quiet chrome: rail, drawer, header, context panel, chips |
| 3 | `--bg-raised` #1B202B | **keep**, derived: `color-mix(in srgb, surface 96%, ink)` | ≈ #F5F5F5 | ≈ #21242C | the house has two surface steps; the console needs a third for rows, inputs and buttons on `surface`; deriving it from `surface`+`ink` keeps it one scheme-aware expression, not a literal |
| 4 | `--bg-terminal` #0C0E13 | **keep**, same value in both schemes | #0C0E13 | #0C0E13 | terminal wells, transcript, code wells and the connection log stay dark under a light UI; Ghostty's ANSI palette is unchanged and would not survive a light well (brief: *Not changing*) |
| 5 | `--bg-hover` #222836 | **keep**, derived: `color-mix(in srgb, surface 92%, ink)` | ≈ #EBEBEB | ≈ #2A2D36 | hover step above `--bg-raised`; same derivation, one step further |
| 6 | `--bg-active` #2A3142 | `color.*.verdict-soft` | #DDE1F7 | rgba(169,179,255,.16) | selection, the active tab, the active machine tile and the primary button are "the highlighted row" — the band the house puts behind the decisive interval |

## Lines

| # | Property | Becomes | Light | Dark | Why |
| --- | --- | --- | --- | --- | --- |
| 7 | `--line-subtle` #232936 | `color.*.line` | #DAD3C7 | #2B3040 | hairlines: inputs, ledger rules, the timeline hairline connectors |
| 8 | `--line-strong` #38415A | `color.*.line-strong` | #8F8371 | #6B7390 | focused-pane border, selected tab underline, the timeline spine, ledger sum rule |

## Text

| # | Property | Becomes | Light | Dark | Why |
| --- | --- | --- | --- | --- | --- |
| 9 | `--text-hi` #E8EBF2 | `color.*.ink` | #1B1D24 | #E9E6E0 | primary copy and every identifier the operator acts on |
| 10 | `--text-mid` #9AA3B5 | `color.*.ink-muted` (= `evidence`) | #5A5F6E | #A3A7B4 | labels, prose, eyebrows, metadata; 5.8:1 / 7.67:1 on `bg` |
| 11 | `--text-lo` #5C6577 | **retire** → `color.*.ink-muted` at `eyebrow`/`meta` size | — | — | 2.78:1 on `--bg-raised` is the largest single contrast class in the baseline (669 `time` nodes, 32 `.pane-last-activity`, 16 `.pane-role`); the house has no tertiary text colour — quiet text is smaller or muted, never below 4.5:1 |

## State

| # | Property | Becomes | Light | Dark | Why |
| --- | --- | --- | --- | --- | --- |
| 12 | `--state-ok` #4CC38A | `color.*.good` | #226845 | #5FC492 | live connection dot, `live` phase text |
| 13 | `--state-warn` #E5B454 | `color.*.warning` | #7F5504 | #E2B14D | stale status, blocked chip, reconnecting/unreachable marks |
| 14 | `--state-crit` #E5645E | `color.*.danger` | #B3261E | #EF7B72 | critical attention, failed connection, `.danger` actions (Interrupt, remove machine) |
| 15 | `--state-info` #6CA7F2 | **split**: focus ring → `color.*.focus`; info-severity attention → `color.*.ink-muted`; running state → `color.*.good` | #2E3A9F / #5A5F6E / #226845 | #A9B3FF / #A3A7B4 / #5FC492 | the house has one accent (`action`/`verdict`) and it may not sit in a figure or a control rail as a status; blue as "info" is retired — info is evidence and reads muted |
| 16 | `--state-idle` #5C6577 | `color.*.series-neutral` for dots; `ink-muted` for text | #6B7280 / #5A5F6E | #9AA1AF / #A3A7B4 | the labelled "rest" bucket is exactly what an idle session is; as text it must clear 4.5:1, so idle labels use `ink-muted` |
| 17 | `--tint-warn` rgba(229,180,84,.09) | `color.*.warning-tint` | rgba(127,85,4,.10) | rgba(226,177,77,.14) | behind actionable warning content; `warning-on-warning-tint` 5.19 / 7.21 |
| 18 | `--tint-crit` rgba(229,100,94,.10) | `color.*.danger-tint` | rgba(179,38,30,.10) | rgba(239,123,114,.14) | behind critical cards/events; `danger-on-danger-tint` 5.06 / 5.58 |

## Overlay

| # | Property | Becomes | Light | Dark | Why |
| --- | --- | --- | --- | --- | --- |
| 19 | `--overlay-backdrop` color-mix(--bg-terminal 72%, transparent) | **keep**, derived: `color-mix(in srgb, bg 72%, transparent)` | from #F7F4EE | from #12141A | the modal backdrop must dim the page in the page's own scheme; deriving from `bg` instead of the terminal well keeps it scheme-aware |
| 20 | `--overlay-shadow-color` (same expression) | `elevation.overlay` colour component | rgba(18,20,26,.40) | rgba(18,20,26,.40) | the house overlay shadow is one value in both schemes |
| 21 | `--color-transparent` transparent | **keep** | transparent | transparent | the invariant test forbids literals outside `:root`; `transparent` is consumed 16 times through this token |

## Type

| # | Property | Becomes | Value | Why |
| --- | --- | --- | --- | --- |
| 22 | `--font-ui` Inter, ui-sans-serif, system-ui, sans-serif | `typography.family.body` | Inter, ui-sans-serif, system-ui, -apple-system, Segoe UI, Roboto, sans-serif | same face, the house stack |
| 23 | `--font-mono` "JetBrains Mono", "IBM Plex Mono", monospace | `typography.family.mono` | JetBrains Mono, IBM Plex Mono, ui-monospace, SFMono-Regular, Menlo, Consolas, monospace | same face, the house stack; Ghostty reads it at mount |
| 24 | `--fs-xs` .6875rem (11px) | `typography.scale.eyebrow` size | 12px / 16px | eyebrows, chips, pane chrome; 11px is below the house floor |
| 25 | `--fs-sm` .78125rem (12.5px) | **keep** as `--fs-meta` | 13px / 18px | session meta, pane header text; the house's smallest body step (14) is the base here, and the console needs one quieter mono step above the eyebrow |
| 26 | `--fs-base` .84375rem (13.5px) | `typography.scale.caption` | 14px / 20px | buttons, body, session names |
| 27 | `--fs-terminal` .8125rem (13px) | **keep** | 13px, `--line-terminal` 1.35 | Ghostty clamps 12–16 and owns the grid; not a house step |
| 28 | `--fs-md` .9375rem (15px) | `typography.scale.ledger` size | 15px / 22px, tabular | transcript reading view, panel and dialog titles, ledger rows |
| 29 | `--fs-lg` 1.125rem (18px) | `typography.scale.lede` size | 21px / 30px | the one `h1` (open session codename / "Fleet overview") and the pairing code line |
| 30 | `--weight-regular` 400 | `typography.scale.*.fontWeight` 400 | 400 | unchanged |
| 31 | `--weight-medium` 500 | `typography.scale.hero-number.fontWeight` | 500 | the pairing code and any hero-number figure; nothing else |
| 32 | `--weight-semibold` 600 | `typography.scale.heading.fontWeight` / `eyebrow.fontWeight` | 600 | headings and eyebrows; still the ceiling |
| 33 | `--tracking-label` .06em | `typography.scale.eyebrow.letterSpacing` | .08em | house eyebrow tracking |
| 34 | `--line-ui` 1.4 | **keep**, derived from `caption` (20/14 ≈ 1.43) | 1.43 | the scale states line-heights in px; the console keeps one unitless value for wrapped controls |
| 35 | `--line-terminal` 1.35 | **keep** | 1.35 | Ghostty grid metric |
| 36 | `--root-font-size` 16px | **keep** | 16px | the `rem` base; the house base of 17 is the body *size*, not the root |

## Space

| # | Property | Becomes | Value | Why |
| --- | --- | --- | --- | --- |
| 37 | `--space-1` 4px | `space.1` | 4px | |
| 38 | `--space-2` 8px | `space.2` | 8px | |
| 39 | `--space-3` 12px | `space.3` | 12px | also the border-clearance minimum (`container.border-clearance`) |
| 40 | `--space-4` 16px | `space.4` | 16px | |
| 41 | `--space-5` 20px | **retire** → `space.6` (24px) | — | 7 uses; the house scale has no 20 |
| 42 | `--space-6` 24px | `space.6` | 24px | |
| 43 | `--space-8` 32px | `space.8` | 32px | |
| 44 | `--space-10` 40px | **retire** → `space.12` (48px) for section breathing, `space.8` for in-panel gaps | — | 18 uses; the house scale steps 32 → 48 |

## Shape and rules

| # | Property | Becomes | Value | Why |
| --- | --- | --- | --- | --- |
| 45 | `--radius-card` 6px | `radius.chip` for buttons, inputs and chips; `radius.ledger` (0) for rows and ledgers | 4px / 0 | a ledger is ruled, never boxed; controls take the chip radius |
| 46 | `--radius-pane` 8px | `radius.panel` | 8px | panes, dialogs, sheets, the state card |
| 47 | `--radius-pill` 999px | **keep** | 999px | status dots and count badges only; pills are not in the house set but dots need them; chips move to `radius.chip` |
| 48 | `--line-width` 1px | `chart.hairline` | 1px | every hairline: inputs, ledger rules, connectors |
| 49 | `--state-rule-width` 2px | **keep** | 2px | the critical left rule on attention events; the house's 3px is the verdict rule and must not be borrowed for danger |
| 50 | `--focus-ring-width` 2px | **keep**; colour → `color.*.focus` | 2px | the sole outline |
| 51 | `--shadow-overlay` 0 24px 80px var(--overlay-shadow-color) | `elevation.overlay` | 0 24px 80px rgba(18,20,26,.40) | dialog and `#toast` only; the invariant test counts two `box-shadow` declarations |

## Geometry (app chrome, on the 4px grid)

None of these has a house counterpart; they are the console's own measurements and stay hub-only unless marked.

| # | Property | Becomes | Why |
| --- | --- | --- | --- |
| 52 | `--machine-rail-width` 48px | **keep** | rail column |
| 53 | `--machine-drawer-width` 280px | **keep** | drawer sheet |
| 54 | `--context-panel-width` 320px | **keep** | context column |
| 55 | `--session-header-height` 44px | **keep** | header row |
| 56 | `--pane-secondary-min-width` 280px | **keep** | worker pane floor |
| 57 | `--pane-header-height` 32px | **keep** | pane eyebrow row |
| 58 | `--worker-collapsed-width` 240px | **keep** | collapsed worker bar |
| 59 | `--toolbar-height` 72px | **keep** | phone toolbar |
| 60 | `--button-height` 40px | **keep** | full-size control |
| 61 | `--button-compact-height` 28px | **keep** | pane-chrome control |
| 62 | `--dialog-width` 520px | **keep** | pairing dialog |
| 63 | `--terminal-state-width` 360px | **keep** | the connection state card; its content changes form (brief), its width does not |
| 64 | `--pair-detail-label-width` 140px | **keep** | ledger term column in `.pair-details` |
| 65 | `--mobile-drawer-max-height` 520px | **keep** | phone sheet |
| 66 | `--mobile-pane-min-width` 260px | **keep** | phone pane floor |
| 67 | `--mobile-attention-label-width` 200px | **keep** | landscape label column |
| 68 | `--fleet-card-min-width` 260px | **retire** | the session card grid is replaced by the hero figure and the ledger (brief: *Deliberately omitted*) |
| 69 | `--fleet-board-max-width` 1120px | `layout.container` | 1120px | the house container; already equal |
| 70 | `--mobile-header-chip-width` 72px | **keep** | phone header chips |
| 71 | `--mobile-context-pill-width` 152px | **keep** | phone bar pill (D7) |
| 72 | `--rail-item-min` 44px | **keep** | phone touch floor (D7) |
| 73 | `--landscape-attention-rail-width` 80px | **keep** | landscape phone column |
| 74 | `--browser-notice-height` 32px | **keep** | unsupported-browser line |
| 75 | `--attention-payload-max-height` 180px | **keep**; the rule that uses it already declares `overflow: auto` | satisfies `container.text-box-height` (a fixed height with a scroll strategy on the same rule) |

## Motion

| # | Property | Becomes | Value | Why |
| --- | --- | --- | --- | --- |
| 76 | `--attention-motion-duration` 150ms | `motion.reveal` | 200ms, easing `motion.easing` | a new event revealing in the timeline is a reveal |
| 77 | `--chrome-motion-duration` 120ms | `motion.chrome` | 120ms | hover, focus, toggle |
| 78 | `--connection-spin-duration` 800ms | **retire** | — | the spinner goes with it; the house forbids looping animation and the connecting card states its outcome instead |
| 79 | `--connection-log-max-height` 60dvh | **keep**; its rule declares `overflow: auto` | 60dvh | the connection log ledger scrolls inside the dialog |

## New hub tokens this pass introduces

| Property | From | Value | Consumer |
| --- | --- | --- | --- |
| `--font-display` | `typography.family.display` | Iowan Old Style, Palatino Linotype, Palatino, Book Antiqua, Georgia, Times New Roman, serif | the fleet verdict sentence; the connection outcome sentence |
| `--fs-verdict` | `typography.scale.title`, clamped for the console | clamp(24px, 3vw, 34px) / 1.1, tracking −.015em | the two display slots above |
| `--fs-meta` | replaces `--fs-sm` | 13px / 18px | row 25 |
| `--color-verdict` | `color.*.verdict` | #2E3A9F / #A9B3FF | the one ring on the hero figure; the marked event on the attention timeline; the ledger row that needs you |
| `--color-action` | `color.*.action` | #2E3A9F / #A9B3FF | links and the primary button text; never inside the figure |
| `--color-focus` | `color.*.focus` | #2E3A9F / #A9B3FF | focus outline (row 50) |
| `--color-series-neutral` | `color.series-neutral.*` | #6B7280 / #9AA1AF | idle dots (row 16) |
| `--rule-verdict` | `chart.mark-decisive` | 2.5px | the ring stroke on the hero figure |
| `--rule-hero` | design language §6 | 3px | the rule under the verdict sentence |
| `--space-12`, `--space-16` | `space.12`, `space.16` | 48px, 64px | section breathing on the fleet board (row 44) |

Tokens carried over with the same name and value in both schemes keep their name so the 886
`var()` consumers in `styles.css` and the 28 in TypeScript do not all churn in Unit 2; only the
retired rows (11, 41, 44, 68, 78) and the split row (15) require consumer edits, and those land
in the unit that owns each consumer's surface.

## Search manifest

Commands run on 2026-09-07 against `hub-web/` at 8c3ec339 (3.17.3) to enumerate the declared
tokens and their consumers, so this table can be checked for completeness:

| Command | Hits | Meaning |
| --- | --- | --- |
| `sed -n 1,106p hub-web/src/styles.css \| grep -c "^  --"` | 79 | custom properties declared in `:root` — the 79 rows above |
| `grep -c "^  --" hub-web/src/styles.css` | 79 | no custom property is declared outside `:root` |
| `grep -o "var(--[a-z0-9-]*" hub-web/src/styles.css \| wc -l` | 886 | `var()` consumers in the stylesheet |
| `grep -o "var(--[a-z0-9-]*" hub-web/src/styles.css \| sort -u \| wc -l` | 78 | 78 of the 79 declared tokens are consumed in the stylesheet; no undeclared token is consumed |
| `comm -23 <(declared) <(consumed)` | 1 | `--fs-terminal` — consumed only from TypeScript (`getPropertyValue` at terminal mount), not from any CSS rule |
| `grep -rho "var(--[a-z0-9-]*" hub-web/src/*.ts hub-web/src/terminal/*.ts hub-web/index.html \| wc -l` | 28 | `var()` reads from TypeScript (`getComputedStyle` at mount: fonts, sizes, rail width, line-strong, state colours) |
| `grep -n "box-shadow" hub-web/src/styles.css` | 2 | the two overlay shadows the invariant test counts |
| `grep -n "prefers-color-scheme" hub-web/src/styles.css` | 0 | no light scheme exists at 3.17.3 |
| `grep -n "@media print" hub-web/src/styles.css` | 0 | no print stylesheet exists at 3.17.3 |
| `grep -n "prefers-reduced-motion" hub-web/src/styles.css` | 2 | reduced-motion blocks exist (lines 1509, 1639) |
