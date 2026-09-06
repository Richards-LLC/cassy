---
name: design-spec
description: Use when the user asks to create or update a design spec, design-system documentation, or DESIGN.md, or before substantial UI work.
managed_by: cas
---

# Design Spec

Produce a **single, self-contained** design source of truth at `DESIGN.md` (repo root, or the frontend app root — e.g. `apps/frontend/DESIGN.md` — when the UI lives in one package of a monorepo). Front-end workers read this file *instead of* grepping components and theme files to reconstruct design intent; point every UI task at it.

**IMPORTANT: All file references use repo-relative paths** (e.g., `apps/frontend/assets/app.scss`), never absolute paths.

## What this skill is (and isn't)

- **IS:** the project's visual language captured once — real token values, real component patterns, real guardrails.
- **IS NOT:** a codemap, a product/domain doc, a component API reference, or a generic design-system tutorial. `codemap` covers structure; `project-overview` covers domain.

**The code is the source of truth, never an existing prose design doc.** Hand-written design docs go stale within months and describe tokens in prose. Read the live token source and copy real values.

## Read order (highest signal first)

### 1. Find the token source (required — do not guess values)

Probe in this order and stop at the first that exists:

- **CSS custom properties** — `:root {}` / theme blocks in `*.scss`, `*.css` (`app.scss`, `theme.css`, `globals.css`, `main.css`)
- **Tailwind** — `tailwind.config.{js,ts}` `theme`/`theme.extend`, `@theme` blocks in CSS (v4), `tokens.json`
- **Quasar** — `quasar.variables.scss`, `quasar.config.{js,ts}` `framework.config`
- **MUI / Chakra / Mantine** — `createTheme(...)`, `extendTheme(...)` theme objects (`theme/`, `src/theme.ts`)
- **Style Dictionary / Figma Tokens** — `tokens/**/*.json`, `style-dictionary.config.js`
- **CSS-in-JS / vanilla-extract** — `*.css.ts`, `styled-components` `ThemeProvider` value

Record the token file path — it becomes the freshness anchor and is cited in the Overview section.

**No token source found (greenfield):** the project inherits the Petrastella design language.
Copy the values in [references/design-tokens.json](references/design-tokens.json) into the
framework's native token form (CSS custom properties in the global stylesheet, `theme.extend`,
`quasar.variables.scss`, …), commit that file, and treat it as the token source from here on.
[references/petrastella-design-language.md](references/petrastella-design-language.md) explains
each role's intent and the type pairing (serif argument, sans reading, mono numbers) that
survives every override. Add `inherits: petrastella` to the frontmatter.

**Token source found:** add `inherits: petrastella | neutral | none` to the frontmatter and, in
Colors and Typography, name each Petrastella role the project overrides and what it keeps.
`neutral` is the design language's documented white-label fallback and requires the brand reason
in Overview; it is never chosen for lack of a decision. `cas-ui-craft` reads this file before
any render, so an override recorded here is an override every surface will honour.

### 2. Read canonical components (for the Components section)

Pick 5–10 real components that define the visual language, then name the file each pattern lives in:
modal/dialog, card/panel, primary + secondary button, text input, badge/chip, table row, nav item, the selected/hover/disabled states.

### 3. Mine guardrails (for Do's & Don'ts)

- Cassy memories and rules tagged design / css / ui / frontend (`cas__search` with `action=search`)
- Recurring UI corrections recorded in task notes
- Framework gotchas the repo has already tripped on (search for comments like `// don't`, `// override`, `!important`)

**Skip** `node_modules/`, `dist/`, generated CSS, vendor themes, snapshot files.

## Output structure (fixed)

Write to `DESIGN.md`: **YAML frontmatter (normative, machine-readable) + 8 markdown sections (rationale, human-readable)**. Target **120–200 lines**. Hard cap 300.

Frontmatter keys (omit a key only when the project genuinely has no such token — never invent values):

- `source` — repo-relative path(s) of the token source
- `inherits` — `petrastella` | `neutral` | `none`; which base language the tokens descend from
- `theme` — `dark-first` | `light-first` | `dual`
- `colors` — by **role** (`bg`, `surface`, `surface-raised`, `border`, `text`, `text-muted`, `primary`, `accent`, `success`, `warning`, `danger`), value = the token name AND its resolved value
- `typography` — `families` (role → stack), `scale` (name → size/line-height/weight)
- `spacing` — base unit + the scale steps (e.g. 8pt grid)
- `radius` — name → value
- `elevation` — level → shadow value
- `breakpoints` — name → min-width

Then the eight sections, in this order:

1. `## Overview` — what the product looks/feels like in 3–5 lines; names the framework, the theme polarity, and the token source file.
2. `## Colors` — each role: when to use it, which token, what it must never be paired with.
3. `## Typography` — families by role (display vs body vs mono), the scale, and the weight/casing rules.
4. `## Layout` — grid unit, container widths, gutters, breakpoints, and the mobile rule.
5. `## Elevation & Depth` — the levels, what earns each one, and how depth reads on this theme.
6. `## Shapes` — radii by component class, border weights, icon sizing.
7. `## Components` — per canonical component: the pattern in 1–3 lines + the file it lives in (`apps/frontend/components/BaseModal.vue`).
8. `## Do's & Don'ts` — project-specific rules only, each as a ✅/❌ pair.

## Quality bar — zero generic design-blog sentences

Every line must fail this test:
> "Could this sentence appear in any design system's docs?"

- ❌ "Use color intentionally to create hierarchy."
- ❌ "Consistent spacing improves readability."
- ✅ "Surfaces use `--g-surface` (#161418); a hardcoded `#fff` panel renders as a light hole in the dark theme."
- ✅ "Selected plan card = `--g-accent` 1px border + `--g-surface-raised` fill; never a filled accent background (fights the price text)."

Every token value in the frontmatter must be **copied from the token source**, not remembered or inferred. If you cannot find a value, omit the key and note the gap in Overview.

## After writing the doc

Follow [../codemap/references/doc-hygiene.md](../codemap/references/doc-hygiene.md) for the three steps this skill shares with `codemap` and `project-overview`: keep-block preservation on re-runs, the thin pointer memory, and the commit. This skill's specifics:

- **Keep-blocks** most often wrap `## Do's & Don'ts` rules the team wrote by hand (`❌ Never use Quasar's --q-* variables; the theme only wires --g-*.`). Put them back in the section they came from.
- **Pointer memory title:** `project_<slug>_designmd` (slug = lowercase kebab-case of project name). Body example: `See [apps/frontend/DESIGN.md](apps/frontend/DESIGN.md) — Quasar dark-first `--g-*` theme, Playfair/Inter, 8pt grid.`
- **Commit** `DESIGN.md` so reviewers can diff it against the token source. Nothing in Cassy reads `DESIGN.md`: there is no hook, banner, or `cas` subcommand for it, and no automatic signal will tell anyone it went stale. Comparing the commit dates of `DESIGN.md` and its token source, by hand, is the only check there is.
- **Report back:** (a) the token source it was grounded in, (b) how many roles/components are documented, (c) any token the project is missing.

## When to run

- **Missing:** no `DESIGN.md` → generate from scratch before any significant UI work.
- **Drift:** the token source has commits newer than `DESIGN.md`'s last commit → regenerate; keep-blocks survive. You have to look; nothing announces it.
- **Manual:** user invokes `/design-spec` or asks for a design spec / DESIGN.md.
- **After a re-theme:** palette, type stack, or spacing unit changed.

## Anti-patterns

- Copying values from an existing prose design doc instead of the live token file. That doc is why this skill exists.
- Inventing plausible hex values or a "standard" type scale when the project's real values are unreadable. Omit and flag instead.
- Generic advice in Components ("modals should be dismissible"). Name the project's actual pattern and its file.
- Documenting every component. 5–10 canonical ones set the language; the rest follow.
- Skipping the keep-block check on regeneration. Destroying hand-edits is a trust breaker.
- Forgetting the memory pointer or forgetting to commit `DESIGN.md`.
