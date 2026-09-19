# Visual QA — round-2 chat variants

`scripts/visual-qa.mjs --strict` run per variant per viewport on 2026-09-18. The
flag `--viewport` replaces the viewport list rather than appending, so 1280×800
and 390×844 are separate runs. Each run covers light and dark. Receipts:
`/home/pippenz/.cas/artifacts/cas-bdc0/visual-qa/<variant>-<viewport>/`.

| Render | Viewport | Result | Findings | Informational | Allowlisted |
| --- | --- | --- | --- | --- | --- |
| `v1-pebble.html` | 1280×800 | PASS | 0 | 0 | 0 |
| `v1-pebble.html` | 390×844 | PASS | 0 | 0 | 0 |
| `v2-stack.html` | 1280×800 | PASS | 0 | 0 | 0 |
| `v2-stack.html` | 390×844 | PASS | 0 | 0 | 0 |
| `v3-capsule.html` | 1280×800 | PASS | 0 | 0 | 0 |
| `v3-capsule.html` | 390×844 | PASS | 0 | 0 | 0 |
| `index.html` (contact sheet) | 1280×800 | PASS | 0 | 0 | 0 |
| `index.html` (contact sheet) | 390×844 | PASS | 0 | 0 | 0 |

No allowlist file was used, so every text/background pair in every state was
measured rather than suppressed. The probe covers contrast (4.5:1 normal, 3:1
large), invisible text, clipped or overflowing containers, horizontal escape,
overlapping text, and JS-disabled / print content loss.

## Contrast floors in the palette

`chat.css` restates each hub token as a literal because the probe's colour
parser cannot resolve `color-mix()` output, which would silently skip the pair.
Measured worst cases per role (both schemes):

| Pair | Light | Dark |
| --- | --- | --- |
| operator bubble text on fill | 9.45 | 9.27 |
| supervisor bubble text on fill | 14.0 | 11.4 |
| ask fill text (amber) | 8.52 | 9.32 |
| ask outlined field text (Variant 2) | 15.2 | 11.7 |
| ask fill text (terracotta, Variant 3) | 6.53 | 6.79 |
| rail secondary text on selected row | 6.40 | 7.40 |
| timestamp on thread canvas | 5.80 | 7.66 |
| attachment sub-line on inset card | 6.37 | 7.66 |
| "hash verified" mark on inset card | 6.70 | 8.59 |
| machine monogram on avatar (3 hues) | 8.37–9.45 | 8.54–9.27 |

## Overflow

`index.html` was additionally checked headless for standalone opening: 0
non-`file://` requests, 0 failed requests, 21/21 images decoded.

`render.mjs` reports `document.scrollWidth > clientWidth` for every render; no
render reported `XOVERFLOW`. Full-page capture heights: Pebble 920 / 1066,
Stack 804 / 921, Capsule 1123 / 1085 (desktop / mobile). Nothing is clipped —
no element in any variant uses `overflow: hidden`, and no preview line
truncates, so the strict probe found no `clipped-content`,
`content-overflow` or `truncated-container`.

## Token deviation on record

`--ask-bg` uses `color.state-warn`'s **dark**-scheme value (`#E2B14D`) in both
schemes. The light-scheme `state-warn` (`#7F5504`) only clears 4.5:1 as a
near-brown fill, which read as mud rather than attention; the bright amber
clears 8.52:1 against `text-hi`. The light-scheme value is retained as
`--ask-edge` for the 1–2px borders and the rail's needs-you dot, where it is
the correct weight. `--m-c` (`#5B3E8C` light / `#C0A3F0` dark) is a third
machine hue added at the same value as the existing avatar hues so a fleet of
three machines is distinguishable; it is the only colour here that is not
already a hub token value.
