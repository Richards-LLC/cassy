---
name: cas-release-report
description: Use after every published version release, before its Slack announcement, to produce a standalone HTML release brief and a verified PDF for any project.
managed_by: cas
---

# Release report

1. Confirm the published version, commit and release assets. Read the project's
   CHANGELOG section, release-notes draft, closed issues and publication receipts.
   Keep exact source URLs, retrieval times and full asset digests; name missing
   evidence as unavailable. An unpublished version remains a draft.
2. Write `docs/release-reports/<version>.md` using the source contract in
   [references/exemplar.md](references/exemplar.md). Keep every user and developer
   change as Was → Now. Count verified closures separately from changelog entries;
   define the counting window and any latency endpoints.
3. Fill `<version>.brief.md` from
   [references/brief-template.md](references/brief-template.md) before rendering.
   Read `DESIGN.md` and `design-tokens.json` when present, including a token path
   named by DESIGN.md; map project roles to the renderer's token schema. Without
   project tokens, use the bundled Petrastella defaults. Record this choice.
4. Preserve the identity: a sandstone/serif verdict, closure stitches made of
   equal-area issue dots, a quiet numbered folio and one decorative stitch at
   install. Project tokens may adapt the palette and fonts; retain the motif,
   hierarchy and section order. Keep established product-surface order across
   releases. Dots count closures, never impact or severity.
5. Render from Markdown with the bundled stdlib Python script:

   ```bash
   python3 <skill-dir>/scripts/render.py docs/release-reports/<version>.md \
     --output docs/release-reports/<version>.html --project-root .
   ```

   Pass `--tokens <json>` for a mapped design file and `--emphasis <verdict text>`
   for an optional italic phrase. The script and
   [references/template.html](references/template.html) are reusable by CLI callers;
   no project name, release number or closure count is baked into the page.
6. Keep the fixed skeleton: verdict + change map; Release at a glance; What you
   can do now; Under the hood; Fixes ledger; Install; Evidence and scope. Preserve
   the template's `data-section` markers and documented map attributes. Keep the
   source table adjacent to its figure and open without JavaScript.
7. Render A4 and Letter using [references/pdf.md](references/pdf.md). Let sections
   flow continuously; keep cards and table rows together, repeat table headings,
   expand disclosures and print link destinations. Render every PDF page back to
   PNG, inspect page flow and verify bounds, links, commands and full digests.
8. Apply the brief's rubric to 1280×800 and 390×844 in both color schemes, with
   keyboard and JavaScript-disabled checks. Use the project's strict visual QA
   script when available. Record scores and proof paths in the brief; revise any
   clipping, contrast, overflow or half-empty pagination defect before delivery.
9. Commit the Markdown, brief, standalone HTML, chosen PDF and small QA receipt
   under `docs/release-reports/`. Carry these post-publication artifacts into the
   next release-prep commit; do not rewrite the published tag. Link the report
   and PDF in the release announcement draft before the authorized Slack post.

**Done when** the HTML opens offline with all source content, numbers and links;
PDFs pass both page formats; the brief's rubric passes; and committed report/PDF
paths are recorded in the release draft. Release-notes owns Slack wording and
posting authorization; this skill prepares artifacts and does not grant it.
