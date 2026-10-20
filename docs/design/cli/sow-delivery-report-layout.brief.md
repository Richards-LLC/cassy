# Brief: SOW delivery report layout

Surface: the canonical `cas-sow-delivery-report` report skeleton and its A4
output. This is a repair to the existing template, with its fictional Northwind
sample palette retained; it does not introduce a Cassy publication or a new brand.

## Single idea

The client can read the changed screen at the same scale as the other evidence,
and identify its round without the brand mark touching the label on a phone.

## Hero form

Keep the existing outcome strip: one marked promise per SOW outcome makes the
completion claim visible. The repaired evidence form is a before/after pair,
stacked at full content width so the reader can compare the same framing at a
readable scale on screen and paper.

## Emotional register

Clear and measured: retain the serif verdict, quiet metadata, original sample
tokens and captions; give each screenshot enough space to read its contents.

## Distinctive move

Before and after each use the complete evidence column, with their source and
round attached. A pair may continue onto another printed page, but an individual
image and its caption stay together. The after image carries the comparison
without an unverified decorative outline obscuring its column headings.

## Deliberately omitted

Omit the wide-screen and print two-column comparison because it halves the
screen text. Retain the template's raster-evidence exception: QA uses explicitly
synthetic, cropped local HTML screens, not a reconstructed customer history.
Keep the logo and round label on separate rows at phone width. Shared capture,
inlining and PDF utilities receive no SOW-specific layout or vocabulary rules.

## Critique

Final self-review and independent second-reader scores agree:

| Dimension | Self | Independent | Visible evidence |
| --- | --- | --- | --- |
| Distinctiveness | 4 | 4 | Serif verdict, outcome markers and blue evidence borders retain the template identity. |
| Fit | 5 | 5 | Full-width comparisons expose the changed columns at readable A4 scale. |
| Hierarchy | 4 | 4 | Verdict, outcome proof and changed screens follow a clear reading order. |
| Craft | 5 | 5 | Captions stay intact; no overlay crosses screenshot text; all four PDF pages have clean bounds. |
| Accessibility | 4 | 4 | Strict light/dark desktop and 390px renders pass without clipping or overflow. |

Scored by keen-eagle-97 and the read-only sow_visual_critique second reader on
2026-10-20; the floor holds.

Same-command PDF renders increase comparison-image width from 230–231pt to
474.75pt. The fixture's smallest 18px label therefore grows from about 5.2pt to
10.68pt. A wide client wordmark and round label have a measured 12px vertical
gap at 390×844, compared with a zero horizontal gap before the repair.

Evidence lives under the cas-9dee artifacts: `after.pdf`, `pdf-check/receipt.json`,
`after-qa/visual-qa.json`, `layout-measurements.json` and `pdf-measurements.json`.
The screenshots are explicitly synthetic local screens captured through the real
capture/inlining/print utilities; they prove template behavior, not a customer
application or a historical release. The second reader found an initial overlay
crossing glyphs and a split explanatory caption; both were corrected and the
final four-page render was inspected again.
