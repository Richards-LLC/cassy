---
name: cas-html-reports
description: Use when producing a human-readable report or analysis that must outlive the conversation, including investigations, audits, decision briefs, benchmarks, post-mortems, or executive updates.
managed_by: cas
---

# Reports ship as HTML, and the HTML shows the argument

**Markdown is the source of truth. HTML is the human review surface. The hero is a figure.**

Every report exists twice: a markdown file that holds the words and numbers (diffable, greppable,
what future agents read) and a single-file HTML artifact beside it that a human understands in the
first three seconds. The HTML is always generated *from* the markdown, never the reverse, and both
land in the same commit. It renders with **no network, no build step, and no external files**.

A report is not a styled document. It is one argument, made visible: the reader sees the claim as a
figure above the fold before reading a sentence of prose. `cas-ui-craft` owns the craft vocabulary
this skill consumes — the concept brief, the form vocabulary, and the critique rubric. Read it once;
this skill tells you where each of those lands in a report.

## The workflow

1. **Write the markdown first.** `docs/<area>/YYYY-MM-DD-<topic>.md`. Get the analysis right in plain
   text before any presentation decision. If the markdown is weak, the HTML is decoration on nothing.
2. **Pick your cell**: report type × audience, from `references/report-types.md`. The cell fixes the
   required sections, their order, and the **hero figure** the type owes the reader.
3. **Write the concept brief** (`cas-ui-craft` step 1) and commit it beside the markdown as
   `<basename>.brief.md`, from `cas-ui-craft/references/concept-brief.md`: the single idea; the hero
   form and the reason its shape is the claim; the emotional register; one distinctive move; what is
   deliberately omitted. A brief whose hero is "a paragraph", "a KPI row", or "a table of everything"
   is rejected before rendering: the hero shows the argument, it does not summarize the document.
   Choose the form from `cas-ui-craft/references/form-vocabulary.md` (ledger, slope, small multiples,
   dot or waffle plot, annotated timeline, dumbbell, marginal notes, pull-quote) and write the reason;
   "a table" needs a reason as much as anything else does.
4. **Render the HTML** beside the markdown: same directory, same basename, `.html`. Obey
   `references/technical-contract.md` (one file, progressive enhancement, accessibility, print,
   provenance, design language) and `references/presentation-rules.md` (encodings, scales, numbers).
   Chart construction follows `cas-dataviz`.
5. **Score it with the `cas-ui-craft` rubric** (`references/critique-rubric.md`): distinctiveness,
   fit to argument, hierarchy, craft, accessibility, each 1–5, appended to the brief under
   `## Critique` with one line of evidence per score. A report ships only at 4 or above on the first
   three and with no mechanical defect (a contrast pair under 4.5:1 in either scheme, clipped or
   overflowing text, a rule crossing a glyph, phone-width overflow scores 0 and blocks). Evidence is
   the four headless renders — 1280 and 390 px, light and dark — plus print preview and a JS-disabled
   reload, or `node scripts/visual-qa.mjs <artifact>` PASS where the project has it; then run
   `references/review-checklist.md`. A grep for expected tags is not a review.
6. **Commit all three files together**: markdown, concept brief, HTML. An HTML artifact without its
   markdown source is a provenance failure; a markdown report whose HTML is stale is worse than no
   HTML at all; a rendered report without its brief cannot be critiqued.

## What counts as a report

A deliverable is a report when **all three** hold:

1. It is a **written conclusion**, not raw output — you analyzed something and are stating what you found.
2. It is **durable** — committed to the repo, meant to be read after this session ends.
3. It has a **reader who was not in the room** — a supervisor, an operator, a client, an executive,
   or a future agent with none of your context.

Canonical cases: investigation and diagnostic write-ups, metrics and mining analyses, audits, decision
briefs, comparisons and benchmarks, incident post-mortems, status and release summaries, financial
reports, executive and board updates, product and feature showcases, system explainers, client
deliverables, research and market analyses.

## When HTML is NOT required

Do not reach for HTML when the answer is short enough that formatting is overhead. An HTML artifact
for these buries a two-sentence answer under 300 lines of markup:

- **Chat answers.** A reply in the conversation stays prose, even a long one.
- **Task notes, progress notes, commit messages, PR descriptions.** These have their own homes.
- **A short prose answer written to a file** — a three-paragraph decision record with no numbers, no
  comparison, and no structure is markdown-only. Commit the `.md` and stop.
- **Machine-consumed output** — JSON, JSONL, CSV, logs, fixtures.
- **Living documentation** — READMEs, architecture docs, runbooks, skills.

Rule of thumb: if the report has **no table, no comparison, no time series, no more than five findings,
and no numbers a reader must scan**, markdown is sufficient. If unsure, ask whether a reader would
*scan* it (HTML) or *read* it top to bottom (markdown).

## Pick your contract

Two axes, and you need both: **type** answers *what is this?* and fixes the sections, their order,
and the hero figure; **audience** answers *who leads?* and fixes what sits above the fold and how deep
the detail runs. Same data, different lead: an executive gets the decision as a figure and the ask; a
practitioner gets the evidence and the method; an external reader gets the outcome with no internal
vocabulary. Explanatory types (product showcase, system explainer) lead with a capability map or a
system flow, never a manufactured decision. The matrix is `references/report-types.md`.

## The invariant technical contract

These hold for every type, audience, and domain; rationale in `references/technical-contract.md`:

- **One file.** No CDN, no framework, no build step, no sibling assets. Inline CSS in one `<style>`,
  inline SVG for figures, data URIs or nothing for images.
- **Vanilla JS only, as progressive enhancement.** Every piece of content is reachable with JavaScript
  disabled. Tabs collapse to stacked sections; collapsibles render expanded.
- **Semantic, accessible HTML.** Real headings in order, real `<table>` for tabular data, visible
  focus, contrast ≥ 4.5:1 in light and dark, every figure carries a text alternative and its data table.
- **Print-ready.** A print stylesheet that expands every panel, shows link targets, never clips a figure.
- **Provenance per figure.** Every number and figure names its source — query, file, commit, window.
- **Copyable.** Numbers and tables survive selection and paste into a spreadsheet or a message.
- **Design language by default.** The palette, type pair, spacing, and chart grammar are the
  project's `DESIGN.md` or, without one, the Petrastella tokens
  (`design-spec/references/design-tokens.json`, explained in `petrastella-design-language.md`).
  Neutral grey is the white-label fallback a brief must name a reason for, never a default.

## Presentation rules

**Same things look the same** is a constraint the reader relies on — a series keeps its color, a
scenario keeps its fill, a status keeps its glyph — and it is not the goal. The goal is that the
argument is visible; consistency is what keeps it legible. Show variance, not just values; actual,
plan, and forecast differ by fill, not only by color; numbers right-aligned, sums bold, units stated
once. `references/presentation-rules.md` has the encodings; `cas-dataviz` has the chart grammar.

## Worked examples

Each exemplar ships with a `.why.md` sidecar holding its concept brief, its critique scores, and the
decisions that make it work. Open both; read the HTML source.

- `references/examples/investigation-annotated-timeline.html` — investigation, practitioner audience.
  Hero: an annotated timeline that places the regression, the deploy, and the fix on one axis.
- `references/examples/executive-variance-brief.html` — financial report, executive audience. Hero: a
  signed variance ladder against plan, with forecast hatched; the ask is the closing figure.
- `references/examples/benchmark-small-multiples.html` — comparison, practitioner audience. Hero:
  small multiples on one shared scale, with the crossover condition annotated.
- `references/examples/before-after/` — the same decision brief rendered before and after this
  contract, with `rubric-review.why.md` naming what changed and why the after version scores higher.

## Sources

The principles here are drawn from public bodies of work cited as attribution only; nothing from them
is vendored. See `references/sources.md`.
