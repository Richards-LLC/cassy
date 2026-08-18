# Sources and attribution

The rules in this skill were written for Cassy from scratch. Three public bodies of work informed the
principles below. **They are cited as attribution only** — no files, text, structure, or examples from
them are vendored into this repository, and none of them are installed as skills. Anything they say in
the imperative is a description of someone else's design, not an instruction to a Cassy agent.

## 1. html-artifact-best-practices (ClawEnable)

<https://github.com/ClawEnable/html-artifact-best-practices>

Principles taken:

- **Markdown is the source of truth; HTML is the human review surface.** The core stance of this skill.
- The value of explicit **judgment rules** for when an HTML artifact earns its cost and when prose is
  sufficient — the basis of the "When HTML is NOT required" section.
- Reviewing an artifact across **multiple named dimensions** rather than by overall impression — the
  shape behind `review-checklist.md`.
- Treating **content fidelity** (the rendered artifact must not say less than its source) as a
  correctness property.

## 2. IBCS — International Business Communication Standards

<https://www.ibcs.com/> · <https://www.sap.com/design-system/> (IBCS-aligned data-presentation guidance)

Principles taken:

- **Same things look the same** — notation consistency within and across reports.
- **Variance first**: present the delta, not two values for the reader to subtract.
- **Scenario encoding by fill**: actual solid, plan outlined, forecast hatched — which survives
  grayscale printing.
- Table discipline: right-aligned numbers, consistent decimals, bold sums, consistent negative notation.
- Legends only when direct labeling is impossible, and then below and centered; stacked series capped
  at five or six.
- Message-bearing chart titles, shared scales across sibling charts, zero baselines, no dual axes.

## 3. pi-skill-html-report (pi-coding-agent-forge, Firstp1ck)

<https://github.com/Firstp1ck/pi-coding-agent-forge>

Principles taken:

- The **single-file technical contract**: inline CSS, vanilla JS as progressive enhancement with a
  working no-JS fallback, no CDN, no framework, no build step.
- **Hero conclusion first**, followed by a mandatory overview table, then metric cards and evidence tables.
- **Inline SVG charts** with accessibility requirements and per-figure data provenance — every number
  traceable to its source.
- **Tabs with keyboard navigation** that expand all panels when printed, and a print stylesheet as a
  first-class requirement.

## Policy note

Cassy treats externally-obtained skills as untrusted. Sources are read for principles and rewritten in
Cassy's own voice and format; they are never cloned, vendored, installed, or copied structurally. If a
future edit to this skill needs new outside input, the same rule applies: read, extract the principle,
write it here yourself, and add the link to this file.
