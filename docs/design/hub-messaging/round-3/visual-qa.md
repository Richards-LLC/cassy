# Visual QA — Pebble round 3

`scripts/visual-qa.mjs --strict` run per page per viewport on 2026-09-18, each
run covering light and dark. `--viewport` replaces the viewport list rather
than appending, so 1280×800 and 390×844 are separate invocations. Receipts:
`/home/pippenz/.cas/artifacts/cas-9f69/visual-qa/<page>-<viewport>/`.

**PAPER is the primary palette.** It is the hub token palette from
`hub-web/src/tokens.css` and the only scheme that keeps a light and a dark
variant; it carries the full state set. Graphite, Mono, Ember and Slate are
pinned palettes in `schemes.css` and carry the thread and list screens.

## Strict runs

| Page | Viewport | Result | Findings | Informational | Allowlisted |
| --- | --- | --- | --- | --- | --- |
| `thread-a.html` | 1280×800 / 390×844 | PASS / PASS | 0 | 0 | 0 |
| `thread-b.html` | 1280×800 / 390×844 | PASS / PASS | 0 | 0 | 0 |
| `list.html` | 1280×800 / 390×844 | PASS / PASS | 0 | 0 | 0 |
| `evidence.html` | 1280×800 / 390×844 | PASS / PASS | 0 | 0 | 0 |
| `empty.html` | 1280×800 / 390×844 | PASS / PASS | 0 | 0 | 0 |
| `pairs.html` | 1280×800 / 390×844 | PASS / PASS | 0 | 0 | 0 |
| `scheme-paper-thread.html` | 1280×800 / 390×844 | PASS / PASS | 0 | 0 | 0 |
| `scheme-paper-list.html` | 1280×800 / 390×844 | PASS / PASS | 0 | 0 | 0 |
| `scheme-graphite-thread.html` | 1280×800 / 390×844 | PASS / PASS | 0 | 0 | 0 |
| `scheme-graphite-list.html` | 1280×800 / 390×844 | PASS / PASS | 0 | 0 | 0 |
| `scheme-mono-thread.html` | 1280×800 / 390×844 | PASS / PASS | 0 | 0 | 0 |
| `scheme-mono-list.html` | 1280×800 / 390×844 | PASS / PASS | 0 | 0 | 0 |
| `scheme-ember-thread.html` | 1280×800 / 390×844 | PASS / PASS | 0 | 0 | 0 |
| `scheme-ember-list.html` | 1280×800 / 390×844 | PASS / PASS | 0 | 0 | 0 |
| `scheme-slate-thread.html` | 1280×800 / 390×844 | PASS / PASS | 0 | 0 | 0 |
| `scheme-slate-list.html` | 1280×800 / 390×844 | PASS / PASS | 0 | 0 | 0 |
| `index.html` (contact sheet) | 1280×800 / 390×844 | PASS / PASS | 0 | 0 | 0 |

Thirty-four runs, all PASS. No allowlist file was used, so every text/background
pair in every state of every palette was measured rather than suppressed. The
probe covers contrast (4.5:1 normal, 3:1 large), invisible text, clipped or
overflowing containers, horizontal escape, overlapping text, and JS-disabled /
print content loss.

Each pinned palette is exercised under **both** OS colour-scheme preferences by
every one of its runs, so "PASS" for Graphite means it passed with
`prefers-color-scheme: light` *and* `dark` — the palette does not flip.

## PAPER (primary) — contrast per role, light and dark

| Pair | Light | Dark |
| --- | --- | --- |
| operator bubble text on fill | 9.45 | 9.27 |
| supervisor bubble text on fill (3 machines) | 13.75–14.25 | 11.36–12.22 |
| coalesced-status text on supervisor fill | 5.20–5.39 | 5.89–6.34 |
| ask body text on amber fill | 8.52 | 9.32 |
| ask tray chip text (treatment A) | 16.8 | 13.7 |
| ask pick text inside the amber field (treatment B) | 8.52 | 9.32 |
| blocker body text on crit fill | 6.53 | 6.79 |
| blocker inset-window mono text (treatment A) | 16.8 | 12.6 |
| blocker outlined evidence line (treatment B) | 6.53 | 6.79 |
| attachment file name on its own surface | 16.8 | 12.6 |
| attachment size line on its own surface | 6.37 | 6.54 |
| "hash verified" mark | 6.70 | 7.33 |
| PDF plate lettering on the crit plate | 6.53 | 6.79 |
| evidence-table cell mono text | 16.8 | 12.6 |
| evidence-table header | 6.37 | 6.54 |
| evidence-table `pass` / `1 flake` | 6.70 / 6.55 | 7.33 / 7.96 |
| machine monogram on its avatar (3 accents) | 6.70–9.45 | 8.54–9.27 |
| unread count on its accent pill (3 accents) | 6.70–9.45 | 8.54–9.27 |
| waiting timestamp on a selected row | **5.05** | 7.42 |
| rail secondary text on a selected row (3 accents) | 6.14–6.52 | 7.40–7.91 |
| timestamps, day divider, captions on canvas | 5.80 | 7.66 |
| composer placeholder on the panel | 6.37 | 7.09 |

Tightest pair in Paper: the waiting timestamp (`#7F5504`) on the selected Atlas
row (`#DDE1F7`) at 5.05:1.

## Pinned palettes — contrast per role

Worst measured case per role within each palette, across all three machine
accents. Produced by `schemes.mjs`, which asserts every pair **before** it will
emit `schemes.css` and exits non-zero on anything under 4.5:1 — a palette
cannot be committed on taste alone. Its last run: *160 pairs checked across 4
pinned schemes, 0 under 4.5:1.*

| Role | Graphite | Mono | Ember | Slate |
| --- | --- | --- | --- | --- |
| operator bubble text | 8.66 | 16.56 | 10.64 | 8.32 |
| supervisor bubble text | 13.89 | 14.99 | 13.69 | 13.23 |
| coalesced status on supervisor fill | 5.62 | 5.91 | 6.05 | 6.16 |
| ask body text on its fill | 9.45 | 7.93 | 12.29 | 12.63 |
| blocker body text on its fill | 6.86 | 7.13 | 8.52 | 8.67 |
| object text on its own surface | 17.43 | 18.58 | 14.02 | 13.34 |
| "hash verified" mark | 6.36 | 13.97 | 9.38 | 9.08 |
| machine monogram on its avatar | 6.53 | 5.17 | 9.28 | 9.44 |
| waiting timestamp | 4.73 | 5.47 | 10.90 | 10.95 |
| rail text on a selected row | 4.93 | 5.32 | 6.49 | 6.58 |
| muted text on canvas / panel | 6.18 | 6.65 | 6.78 | 6.80 |
| **tightest pair in the palette** | **4.73** | **5.17** | **6.05** | **6.16** |

Tightest pair anywhere in the set: Graphite's waiting timestamp on the selected
Bench row at 4.73:1. Mono's is its lightest machine avatar at 5.17:1 — that
accent was darkened from `#7A7D86` to `#6A6D76` because the assertion caught it
at 4.11:1.

## Palette notes

- **Paper** (primary) — warm paper, indigo operator, periwinkle/green/violet
  fleet, bright amber ask, terracotta blocker. Light and dark, token-backed.
- **Graphite** — cool slate, light-first. Steel-blue operator, a graphite fleet,
  amber ask, brick blocker.
- **Mono** — near-monochrome, light. The operator bubble is near-black, the
  fleet is a three-step neutral value ramp, and the *only* saturated hue in the
  whole scheme belongs to the things that want you: an orange ask and a burnt
  blocker. The receipt tick and the verified mark go neutral rather than green,
  on purpose.
- **Ember** — **dark-first**, saturated. Jade operator on warm black, warm
  brown supervisor wells, luminous gold ask, coral blocker, peach/mint/lilac
  fleet. Its hues were chosen for an emissive panel, not derived by inverting a
  light palette.
- **Slate** — cool dark. Periwinkle operator, barely-raised supervisor wells,
  bright amber ask, salmon blocker, ice/mint/lilac fleet.

## Overflow and palette pinning

`render.mjs` measures `document.scrollWidth > clientWidth` on all 39 renders
and exits non-zero if any overflows; it reports none. The long evidence table
is the tightest case — eight rows of three columns inside a bubble at 390 — and
fits without a scroller or an ellipsis. No element in the surface sets
`overflow: hidden`, so the strict probe found no `clipped-content`,
`content-overflow` or `truncated-container` anywhere.

`render.mjs` also screenshots every pinned-palette page **twice**, once under
each OS colour-scheme preference, and requires the two buffers to be
byte-identical before writing one copy. That check caught a real leak:
`pebble.css` declares `--lift`, `--lift-strong`, `--lift-edge` and
`--lift-head` once per scheme, and the first draft of `schemes.css` overrode
every colour but not the shadows — so all twelve pinned renders reported
`FLIPPED` because their shadows followed the OS while their colours did not.
`schemes.mjs` now emits shadows per palette (dark palettes cast black, light
palettes cast their own ink) and all twelve report `pinned=ok`.

Full-page capture heights (desktop / mobile): thread-a 1120 / 1261, thread-b
1148 / 1349, list 800 / 844, evidence 800 / 844, empty 800 / 844, pairs 800 /
1336, every scheme thread 1120 / 1261, every scheme list 800.

`index.html` was additionally checked headless for standalone opening: 0
non-`file://` requests, 0 failed requests, 45/45 images decoded.

## Token deviations on record

Both are Paper-only and carried over from round 2:

1. `--ask-bg` uses `color.state-warn`'s **dark** value (`#E2B14D`) in both
   light and dark. The light-scheme `state-warn` (`#7F5504`) only clears 4.5:1
   as a near-brown fill; it is retained as `--ask-deep` for the tray, as
   `--warn-text` for table cells and the waiting timestamp, and as the rail's
   waiting dot, where that weight is correct.
2. A third machine accent is needed for a three-machine fleet: `#5B3E8C` light
   / `#C0A3F0` dark, set at the same value as the existing two.

The four pinned palettes are not hub tokens at all and do not claim to be;
they are candidate palettes for the operator to choose between. Every colour in
all five palettes is a literal hex rather than `color-mix()`, because the
probe's colour parser handles only `#hex` and `rgb()/rgba()`; `color-mix()`
computed output returns null and the pair is recorded as
`unverifiable-contrast` instead of being measured.

## Shape checks that are not contrast

- Attention objects (ask and blocker) carry their outer radius on the object
  itself, not only on the inner body, so the drop shadow is cast from the
  object's own silhouette. An earlier revision cast a square shadow behind a
  rounded body; fixed before these renders.
- The dog-eared sheet uses two complementary `clip-path` triangles — a canvas
  cut plus a fold — rather than `overflow: hidden`, so it adds no clipping
  findings.
- Treatment B's speech tail is a pseudo-element, invisible to the probe's
  element walk, and carries no text.
