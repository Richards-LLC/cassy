---
from: Gabber Studio team (Daniel)
date: 2026-06-22
priority: P2
---

> Migrated to GitHub Issues: [#64](https://github.com/pippenz/cas/issues/64)

# Feature Request: `design-spec` skill — generate/maintain a `DESIGN.md` (UI/UX source of truth)

## What we need

A **framework-level CAS skill** that generates and maintains a single, self-contained `DESIGN.md` for a project — the design-system analog of what `codemap` is for code structure and `project-overview` is for the domain model. It captures the project's *visual* language (tokens + patterns + guardrails) as one machine- **and** human-readable file that FE workers and the design-reviewer all consume, so AI-generated UI stays on-brand and consistent across sessions and workers.

Prototype already built by hand in the Gabber repo: `apps/frontend/DESIGN.md` (see it for the target shape).

## Why

Right now CAS has no design counterpart to `codemap`/`project-overview`, and it shows:
- When we ran a multi-persona design review on a pricing page, the design-reviewer agent had to **grep the `.vue` + locale files to *reconstruct* the design intent** (tokens, which plan should be emphasized, the accent color, the do/don't rules). A `DESIGN.md` would have handed it all of that.
- Every FE worker re-derives the same things: which CSS token to use, what the selected-state pattern is, the mobile breakpoint, the framework gotchas (e.g. "use `--g-*` not Quasar `--q-*`; hardcoded-white surfaces render light on a dark theme"). These are project-stable facts that belong in one file.
- The existing hand-written `DESIGN_SYSTEM.md` (a `@nuxt/content` dev-onboarding doc) was **8 months stale** and documented tokens in prose, not as normative machine-readable values — so it had drifted from the live `app.scss` `--g-*` vars.

This is the same "document once, reuse everywhere, regenerate on drift" pattern `codemap` already proved.

## Prior art / format

Inspired by Google Labs' `design.md` spec (github.com/google-labs-code/design.md): **YAML token frontmatter (normative) + markdown prose (rationale)**, 8 sections: Overview, Colors, Typography, Layout, Elevation & Depth, Shapes, Components, Do's & Don'ts. Our Gabber prototype follows it.

## Proposed scope

A `design-spec` (or `designmd`) skill that:

### 1. Extracts live tokens (don't trust stale prose docs)
- Detect the design-token source: CSS custom properties (`--*` in a `:root`/theme SCSS), **Tailwind** `theme`/`tokens.json`, **Quasar** `quasar.variables.scss`, MUI/Chakra theme objects, Style Dictionary, Figma Tokens export.
- Pull the real values (colors by role, typography families/scale, spacing grid, radius, elevation/shadow, breakpoints) into the YAML frontmatter. The **code is the source of truth**, not any existing design doc.

### 2. Infers patterns from real components
- Read a few canonical components (modals, cards, primary buttons, inputs, badges, selected/hover states) to populate the **Components** section with concrete, project-specific patterns + the file they live in — not generic advice.

### 3. Captures project guardrails (Do's & Don'ts)
- Mine framework gotchas + prior corrections (CAS memories tagged design/CSS/UI, recurring review findings) into a Do's/Don'ts list. E.g. Gabber's: `--g-*` over `--q-*`, no pure-white-on-dark, Quasar overlay-drawer scroll-lock leak, carousel `height:auto` override, Nuxt auto-import silent-fail.

### 4. Freshness gate + memory pointer (mirror `codemap`)
- SessionStart/PreToolUse staleness signal when token files / theme config drift since `DESIGN.md` was last updated.
- Write a thin `project_<slug>_designmd` memory pointer.
- Preserve `<!-- keep -->` hand-edited blocks on regenerate.

### 5. Consumed by other skills/agents
- The design-reviewer / `cas-code-review` design persona should read `DESIGN.md` first instead of reconstructing intent.
- FE worker dispatch context can reference it so generated UI uses the right tokens/patterns by default.

## Acceptance

- One `design-spec` skill produces a `DESIGN.md` (token frontmatter + 8 sections) grounded in the project's *live* token source, for Quasar/Tailwind/MUI/etc.
- Freshness gate + memory pointer + keep-block preservation, matching `codemap`.
- The design-review persona/agent demonstrably consumes it (no more grepping to reconstruct design intent).

## Reference

Gabber prototype: `apps/frontend/DESIGN.md` (Quasar + `--g-*` dark-first theme, Playfair/Inter, 8pt grid). The skill should be able to regenerate something equivalent from the token source automatically.
