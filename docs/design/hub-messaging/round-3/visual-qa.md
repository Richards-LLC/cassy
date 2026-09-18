# Visual QA — Pebble round 3

`scripts/visual-qa.mjs --strict` run per state per viewport on 2026-09-18, each
run covering light and dark. `--viewport` replaces the viewport list rather
than appending, so 1280×800 and 390×844 are separate invocations. Receipts:
`/home/pippenz/.cas/artifacts/cas-9f69/visual-qa/<state>-<viewport>/`.

| Render | Viewport | Result | Findings | Informational | Allowlisted |
| --- | --- | --- | --- | --- | --- |
| `thread-a.html` | 1280×800 | PASS | 0 | 0 | 0 |
| `thread-a.html` | 390×844 | PASS | 0 | 0 | 0 |
| `thread-b.html` | 1280×800 | PASS | 0 | 0 | 0 |
| `thread-b.html` | 390×844 | PASS | 0 | 0 | 0 |
| `list.html` | 1280×800 | PASS | 0 | 0 | 0 |
| `list.html` | 390×844 | PASS | 0 | 0 | 0 |
| `evidence.html` | 1280×800 | PASS | 0 | 0 | 0 |
| `evidence.html` | 390×844 | PASS | 0 | 0 | 0 |
| `empty.html` | 1280×800 | PASS | 0 | 0 | 0 |
| `empty.html` | 390×844 | PASS | 0 | 0 | 0 |
| `pairs.html` | 1280×800 | PASS | 0 | 0 | 0 |
| `pairs.html` | 390×844 | PASS | 0 | 0 | 0 |
| `index.html` (contact sheet) | 1280×800 | PASS | 0 | 0 | 0 |
| `index.html` (contact sheet) | 390×844 | PASS | 0 | 0 | 0 |

No allowlist file was used, so every text/background pair in every state was
measured rather than suppressed. The probe covers contrast (4.5:1 normal, 3:1
large), invisible text, clipped or overflowing containers, horizontal escape,
overlapping text, and JS-disabled / print content loss.

## Contrast on every coloured bubble, both schemes

Worst measured case per role. Three machine accents exist, so rows that vary by
machine give the range across Atlas, Studio Mac and Bench.

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

The tightest pair in the whole set is the waiting timestamp (`--warn-text`
`#7F5504`) on the selected Atlas row (`#DDE1F7`) at 5.05:1 — above the 4.5:1
floor with margin to spare, and it is the only pair under 5.2 anywhere.

## Overflow

`render.mjs` measures `document.scrollWidth > clientWidth` on all 24 renders
and exits non-zero if any overflows; it reports none. The long evidence table
is the tightest case — eight rows of three columns inside a bubble at 390 —
and fits without a scroller or an ellipsis. No element in the whole surface
sets `overflow: hidden`, so the strict probe found no `clipped-content`,
`content-overflow` or `truncated-container` anywhere.

Full-page capture heights (desktop / mobile): thread-a 1157 / 1316, thread-b
1172 / 1373, list 800 / 844, evidence 800 / 844, empty 800 / 844, pairs 800 /
1336.

`index.html` was additionally checked headless for standalone opening: 0
non-`file://` requests, 0 failed requests, 30/30 images decoded.

## Token deviations on record

Both carried over from round 2 and unchanged:

1. `--ask-bg` uses `color.state-warn`'s **dark** value (`#E2B14D`) in both
   schemes. The light-scheme `state-warn` (`#7F5504`) only clears 4.5:1 as a
   near-brown fill; it is retained here as `--ask-deep` for the tray in
   treatment A, as `--warn-text` for table cells and the waiting timestamp, and
   as the rail's waiting dot, where that weight is correct.
2. A third machine accent is needed for a three-machine fleet: `--m-c`
   `#5B3E8C` light / `#C0A3F0` dark, set at the same value as the existing two.
   It is the only colour in the surface that is not already a hub token value.

Every colour is restated as a literal hex in `pebble.css` rather than
`color-mix()`, because the probe's colour parser handles only `#hex` and
`rgb()/rgba()`; `color-mix()` computed output returns null and the pair is
recorded as `unverifiable-contrast` instead of being measured.

## Shape checks that are not contrast

- Attention objects (ask and blocker) carry their outer radius on the object
  itself, not only on the inner body, so the drop shadow is cast from the
  object's own silhouette. An earlier revision cast a square shadow behind a
  rounded body; fixed before these renders.
- The dog-eared sheet uses two complementary `clip-path` triangles (canvas
  cut + fold) rather than `overflow: hidden`, so it adds no clipping findings.
- Treatment B's speech tail is a pseudo-element, invisible to the probe's
  element walk, and carries no text.
