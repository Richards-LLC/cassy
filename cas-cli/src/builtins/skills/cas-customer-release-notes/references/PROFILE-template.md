---
customer: <<client display name, exactly as the client writes it>>
prepared_by: <<who the notes come from, as the client knows them>>
design: <<repo-relative path of the DESIGN.md the notes wear, e.g. apps/frontend/DESIGN.md>>
mark: <<repo-relative path of the client logo (SVG preferred), or none>>
fonts: <<repo-relative .woff2 files for the display and body faces, or none>>
customer_channel: <<where the operator sends the notes, e.g. #client-updates (CHANNEL_ID)>>
allow: <<words that are the client's own and pass hygiene, e.g. Epic, or none>>
---

# Customer deliverables profile

> Canonical Cassy template for `cas-customer-release-notes`. Copy it to
> `docs/deliverables/PROFILE.md` in the client's product repository and fill
> every double-angle-bracket value with the operator. These are business
> facts, so code cannot supply them.

- `customer` and `prepared_by` appear on the notes exactly as written here.
- `design` names the `DESIGN.md` that `cas deliverable brand` reads first. It
  takes precedence over the root `DESIGN.md` and over any nested one.
- `mark` is the logo the notes carry when `DESIGN.md` names no `brand-mark`.
- `fonts` closes the gap `cas deliverable brand` reports when a brand face
  has no local `.woff2`; without it, the notes print in the named fallback
  stack.
- `allow` lists client words that customer hygiene would otherwise refuse,
  such as a product named "Epic". Give each word's reason in the brief.
