---
name: cas-customer-release-notes
description: Use when release notes, a product update or a changelog are for customers or a client ("customer release notes for X", "send the client the October update"); builds a client-branded HTML/PDF with real before/after screenshots.
metadata:
  managed_by: cas
---

# Customer release notes

Customer release notes tell one client's users what changed in their product,
in their words, wearing their brand. The product's `DESIGN.md` decides how the
notes look. This skill decides only what they must say, and
`cas deliverable finalize --audience customer` makes the file safe to send.

Team release notes for Slack belong to `cas-release-notes`; a version's
internal report belongs to `cas-release-report`; an SOW completion report
belongs to `cas-sow-delivery-report`. Nothing reaches the client, or the
client's channel, without the operator's explicit go.

## Inputs

| Input | Source |
| --- | --- |
| Profile | `docs/deliverables/PROFILE.md` in the client's product repo (step 1) |
| Brand | The `DESIGN.md` the profile names ([DESIGN.md first](../design-spec/references/design-md-first.md)) |
| Changes | The product's `CHANGELOG.md` section, merged release notes and closed work for the period |
| Screens | Before/after pairs from the delivering tasks' QA artifacts; never a lone after shot |

## Steps

1. **Ensure the profile exists.** Read `docs/deliverables/PROFILE.md` in the
   product's repo. If it is missing, copy
   [references/PROFILE-template.md](references/PROFILE-template.md) there and
   ask the operator for every `<<...>>` value. Never guess the client's name,
   brand or channel. Commit the file. Done when
   `grep -nE '<<[^>]+>>' docs/deliverables/PROFILE.md` prints nothing.
2. **Resolve the brand.** Run
   `cas deliverable brand --root . --audience customer --json`. Exit 1 means the
   product has no usable `DESIGN.md`: run `design-spec` first, because customer
   output never wears the default brand. Read its `type` entries: a font with
   no local `.woff2` falls back, and the profile's `fonts` closes that gap.
   Done when it exits 0 and you have recorded its `design` path and revision.
3. **Collect the period's changes.** Take every user-visible change since the
   previous customer notes. Drop internal-only work: refactors, CI, tooling and
   anything the client's users cannot see. Done when each kept change names the
   user who notices it.
4. **Write the source** as
   `docs/deliverables/customer-notes/<date>-<slug>.md` to
   [references/notes-contract.md](references/notes-contract.md): one Was → Now
   item per change, written in the client's vocabulary, with its pair when one
   exists. Done when every kept change appears exactly once.
5. **Shape the page from `DESIGN.md`.** Read its Overview, Layout, Components
   and Do's & Don'ts, then compose the draft HTML from
   [references/notes-skeleton.html](references/notes-skeleton.html). The
   skeleton fixes the content order and the brand markers only. Hierarchy,
   density, imagery, component shapes and voice come from the client's file.
   Another client's notes are never the layout. Write the `cas-ui-craft`
   brief first; it names the design move this client's file argues for. Done
   when the draft keeps the `/*cas:brand*/` marker and every section in the
   contract.
6. **Finalize.** Run
   `cas deliverable finalize <draft.html> --audience customer --root . --out <notes.html> --pdf a4,letter --author <prepared_by> --check-dir <artifacts_dir>/pdf`,
   adding `--allow <word>` for each word in the profile's `allow` list. It
   fills the brand, embeds the screenshots within the customer budget and
   names any pair it had to leave out. It also refuses ticket labels, PR or
   issue numbers, process words (branch, commit, agent, worker, supervisor,
   epic, sprint, ticket), tool names, local URLs, credentials and home paths.
   Then it prints A4 and Letter and checks both PDFs. A refused deliverable is
   not written. Fix the source, never the output. Done when it exits 0.
7. **Critique and view every page.** Run the strict visual QA from the
   `cas-ui-craft` critique rubric on `<notes.html>` and view every page PNG in
   `<artifacts_dir>/pdf`. This is a public surface: distinctiveness, fit and
   hierarchy each score at least 4. Done when visual QA prints PASS and you
   have viewed every page.
8. **Commit and hand off.** Commit the source, brief, HTML and A4 PDF beside
   it. Draft the message to the client in the profile's `customer_channel`
   with the PDF attached, and stop for the operator's go. Post only through
   `violet` once the operator says so; never through a personal Slack route.
   Done when the operator has the draft and the committed paths.

Record `design: <path>@<revision>` from step 2 in the task's close reason.
