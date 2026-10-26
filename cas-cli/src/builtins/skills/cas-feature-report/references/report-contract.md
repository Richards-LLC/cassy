# Feature report contract

A feature report is a `cas-html-reports` product showcase for one shipped
feature, written for the audience the request names. That skill's technical
contract, presentation rules and provenance rules all apply. This file adds
only what a feature report needs.

## Sections, in order

Each section keeps its `data-section` marker from
[report-template.html](report-template.html).

| # | `data-section` | Content |
| --- | --- | --- |
| 1 | `summary` | One sentence on what the reader can now do, in their words. Under it: who it helps, in one line. Then the hero (below) and a meta line: product, where it is available, date |
| 2 | `changes` | **What changed**: one row per change, as **Was → Now**, with who it helps. A row with an Example view links to it |
| 3 | `how-to` | **How to use it**: the reader's path as numbered steps, each naming the screen and the control they use |
| 4 | `examples` | **Example views**: one before/after pair per change that has one, in `changes` order, each captioned with what to notice. Omit the section when there are no pairs |
| 5 | `good-to-know` | Limits, settings, who has access, and anything planned or not included |
| 6 | `provenance` | Where and when the views were captured, the version shown, the report date, then `Markdown source:` and the markdown file name. The PDF check keys its footer-only last-page check on that phrase |

## Hero

The default hero is the clearest Example view: one pair, before then after,
captioned with the change it proves. With no pair, the hero is the
`how-to` path drawn as a short numbered strip of screens and controls. Never
lead with a paragraph, a stat row or a table of everything.

## Claims and scope

- Every change is labelled **available**, **planned** or **not included**.
  Only available work appears in `changes` and `examples`; planned and not
  included work goes in `good-to-know`. Never present planned work as
  shipped.
- State each change in what the reader sees or can do. Implementation
  detail belongs in an internal report only, and even there after the
  reader-facing statement.
- A number, a setting or a text claim stays HTML text, never a screenshot of
  it.

## Example views

- Use only complete pairs: the same view, viewport and framing, before and
  after. A change without a pair has no Example view, not an empty slot or a
  mock.
- Stack before then after at full content width, on screen and in the PDF,
  each with a caption: `Before` or `After`, the environment and the capture
  date. Keep each image with its caption.
- Keep image `src` values relative (`pairs/<id>.before.jpg`) and eager.
  `finalize` inlines them within the audience's budget and names any pair it
  had to leave out; it never keeps one side of a pair. Mark a pair's
  importance with `data-priority` on its `.sides` element (higher stays).
- At most one highlight per *after* view, drawn as an SVG overlay in the
  brand's `--verdict` colour, never painted into the pixels.

## Audience

- **Customer:** only the client's and the product's vocabulary.
  `finalize --audience customer` refuses ticket labels, issue and PR
  numbers, process and tool words, local or preview hosts, credentials and
  placeholders in the visible text. When a hit is the client's own word,
  pass `--allow <word>` and record why in the brief.
- **Internal:** the same structure. Task or PR references may appear in
  `provenance`, never in place of the reader-facing statement.

## Internal source map

For every change and pair, keep a `SOURCES.md` in the task artifacts
directory with the delivering task, the PR and the pair's capture record. It
answers "where did this come from?" without putting internal labels in a
customer's file. Do not commit it beside the report.
