# Customer release notes contract

The notes are a `cas-html-reports` client-facing deliverable for the external
audience. This file fixes **what** the notes say and in which order. **How**
they look comes from the client's `DESIGN.md` (SKILL step 5): its type,
colour roles, components, density and voice. A section here is content, not a
layout. Render it as the client's own components render that kind of content.

## Sections, in order

The skeleton marks each with `data-section`. Keep the markers and their order.

| # | `data-section` | Content |
| --- | --- | --- |
| 1 | `headline` | The period or version, the product name, and one sentence saying what is better for the client's users, in their words. The profile's `customer` and `prepared_by` |
| 2 | `changes` | One item per user-visible change, grouped the way the client's product groups its features. Each item has a short title, **Was** (what users met before) and **Now** (what they meet today), and its before/after pair when one exists |
| 3 | `good-to-know` | Only when needed: what users or admins must do, what moved, and any limits that still apply. Each point is one action or fact |
| 4 | `provenance` | The period covered, the product and environment the screens come from, the capture date, the preparation date, then `Markdown source:` and the notes' file name. The PDF check keys its last-page test on that phrase |

Do not add a section to match another client's notes. When the client's
`DESIGN.md` defines a hero or summary component, the headline may use it;
otherwise the headline stays text.

## Writing

- Use the client's vocabulary: their feature names, their user roles and
  their tone from `DESIGN.md`'s Overview and Do's & Don'ts.
- Every change is Was → Now. "Improved" and "enhanced" without a Was are not
  a change.
- Each sentence states what a user can now do or no longer has to do. Never
  write how the work was organised, who did it, or which tool helped.
- No numbers without a source: a measured speed-up names what was measured.

## Before/after pairs

- A pair shows the same screen, at the same viewport and framing, before and
  after the change. Wrap both images in one element with class `captures`, so
  `finalize` keeps or drops them together. Caption each side `Before` or
  `After` with its capture date.
- No *before*, no pair: describe the change in text and show no lone after
  shot. A mocked or reconstructed *before* misstates history.
- Mask the client's personal data in every screenshot.
- Put `data-priority="1"` on the pairs that carry the period's main change;
  over the customer budget, `finalize` drops the lowest priority first and
  names what it dropped.
