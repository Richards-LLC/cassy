---
name: cas-html-reports
description: Ship every report deliverable as a self-contained single-file HTML artifact committed beside its markdown source, following a per-report-type presentation contract. Use whenever you are about to write an investigation or diagnostic write-up, a metrics/mining analysis, an audit, a decision brief, a comparison or benchmark, an incident post-mortem, a status or release summary, a financial report, an executive or board update, a client-facing deliverable, or a research/market analysis — in any domain, engineering or business. Trigger PROACTIVELY the moment a deliverable is going to outlive the conversation and be read by a human who was not in it.
managed_by: cas
---

# Reports ship as HTML

**Markdown is the source of truth. HTML is the human review surface.**

Every report you produce exists twice: a markdown file that holds the words and numbers (diffable,
greppable, the thing future agents read), and a single-file HTML artifact beside it that a human
opens and understands in one pass. Neither substitutes for the other. The markdown is never
generated *from* the HTML; the HTML is always generated *from* the markdown, and both are committed
in the same change.

The HTML must render correctly with **no network, no build step, and no external files**. Double-click
it from a checkout on a plane and it looks right.

## What counts as a report

A deliverable is a report when **all three** hold:

1. It is a **written conclusion**, not raw output — you analyzed something and are stating what you found.
2. It is **durable** — it is committed to the repo and meant to be read after this session ends.
3. It has a **reader who was not in the room** — a supervisor, an operator, a client, an executive,
   or a future agent with none of your context.

Canonical cases: investigation/diagnostic write-ups, metrics and mining analyses, audits, decision
briefs, comparisons and benchmarks, incident post-mortems, status/release summaries, financial
reports, executive and board updates, product and feature showcases, system explainers, client
deliverables, research and market analyses.

## When HTML is NOT required

Do not reach for HTML when the answer is short enough that formatting is overhead. Producing an HTML
artifact for these is an anti-pattern — it buries a two-sentence answer under 300 lines of markup:

- **Chat answers.** A reply in the conversation stays prose. Even a long one.
- **Task notes, progress notes, commit messages, PR descriptions.** These have their own homes and formats.
- **A short prose answer that happens to get written to a file** — a three-paragraph decision record with
  no numbers, no comparison, and no structure is markdown-only. Commit the `.md` and stop.
- **Machine-consumed output** — JSON, JSONL, CSV, logs, fixtures. Data files are data files.
- **Living documentation** — READMEs, architecture docs, runbooks, skills. These are read in the repo,
  edited continuously, and belong in markdown alone.

Rule of thumb: if the report has **no table, no comparison, no time series, no more than five findings,
and no numbers a reader must scan**, the markdown is sufficient. If you are unsure, ask whether a
reader would want to *scan* it (HTML) or *read* it top to bottom (markdown).

## The workflow

1. **Write the markdown first.** `docs/<area>/YYYY-MM-DD-<topic>.md`. Get the analysis right in plain text
   before any presentation decision. If the markdown is weak, the HTML is decoration on nothing.
2. **Pick your cell**: report type × audience. See `references/report-types.md`. The cell tells you the
   required sections, their order, and which visuals are mandatory.
3. **Render the HTML** beside it: same directory, same basename, `.html`. Follow
   `references/technical-contract.md` (structure, dependencies, accessibility, print, provenance) and
   `references/presentation-rules.md` (charts, tables, numbers, variance).
4. **Check it** against `references/review-checklist.md` before committing. Open it in a browser; print
   preview it; disable JavaScript and reload.
5. **Commit both files together**, in one change. An HTML artifact without its markdown source is a
   provenance failure; a markdown report whose HTML is stale is worse than no HTML at all.

## Pick your contract

Two axes, and you need both:

- **Type** answers *what is this?* — it fixes the required sections and their order.
- **Audience** answers *who leads?* — it fixes what goes above the fold and how deep the detail runs.

Same data, different lead. An executive gets the number and the decision; a practitioner gets the
evidence and the method; an external reader gets the outcome with no internal vocabulary at all.
The full matrix, with per-type required sections and mandatory visuals, is in
`references/report-types.md`.

### Explanatory routing

Choose **Product / feature showcase** when the reader asks *what did we build, what can it do, and
why is the experience compelling?* Choose **System / architecture explainer** when the reader asks
*how does it work end to end, what moves through it, and where are its boundaries?* These are
explanatory contracts, not disguised decision briefs: an executive audience does not add a required
ask, decision, or KPI row. When both questions matter, make the showcase primary and nest the
explainer after the capability-to-proof mapping, as `report-types.md` specifies.

## The invariant technical contract

These hold for every type, every audience, every domain. Details and rationale in
`references/technical-contract.md`:

- **One file.** No CDN, no framework, no build step, no sibling assets. Inline CSS in a single `<style>`,
  inline SVG for charts, data URIs or nothing for images.
- **Vanilla JS only, as progressive enhancement.** Every piece of content must be reachable with
  JavaScript disabled. Tabs collapse to stacked sections; collapsibles render expanded.
- **Semantic, accessible HTML.** Real headings in order, real `<table>` for tabular data, visible focus,
  contrast ≥ 4.5:1, charts carry a text alternative and a data table.
- **Print-ready.** A print stylesheet that expands every panel, shows link targets, and does not clip charts.
- **Provenance per figure.** Every number and every chart names where it came from — query, file, commit,
  time window — so a skeptical reader can reproduce it.
- **Copyable.** Numbers and tables must survive selection and paste into a spreadsheet or a message.

## Presentation rules

Consistency is the whole game: **same things look the same**, across figures and across reports. Show
the *variance*, not just the values. Actual, plan, and forecast are visually distinguishable by fill,
not only by color. Numbers are right-aligned, sums are bold, units are stated once and never mixed.
See `references/presentation-rules.md`; chart sections and wall-of-text visual-rhythm guidance follow the cross-harness `cas-dataviz` skill.

## Worked examples

Two reports written to this contract, both authored for this skill — open them in a browser and read
their source:

- `references/examples/engineering-investigation.html` — an investigation/diagnostic report for a
  practitioner audience (hero verdict, evidence tables, timeline, inline SVG chart, provenance footer).
- `references/examples/financial-quarterly-brief.html` — a financial report for an executive audience
  (KPI cards with variance, plan/actual/forecast encodings, variance-first bars, methodology last).
- `references/report-types.md` — a compact, reusable semantic HTML/SVG system-flow pattern with a
  visible text alternative and print treatment for explanatory reports.

## Sources

The principles here are drawn from three public bodies of work, cited as attribution only — no content,
structure, or file layout from them is vendored into CAS. See `references/sources.md`.
