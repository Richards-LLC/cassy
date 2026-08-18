---
from: Petra Stella pulse-card team
date: 2026-04-14
priority: P2
---

# Feature Request: `project-overview` Skill + Freshness Automation

## Summary

Add a Cassy skill that generates a **PRODUCT_OVERVIEW.md** for any project — a lean,
domain-focused document describing what the project *is*, who uses it, and its core
concepts — plus lightweight automation that keeps it from going stale.

Today a cold-starting agent (or a new engineer) opens a Petrastella repo and hits
generic template marketing in README.md. Nothing in the codebase says "this is a
creator-brand campaign performance platform." CODEMAP helps with navigation but not
with product intent. We want a standard place for that intent to live, and a standard
way to produce and refresh it.

---

## Problem

1. **Template boilerplate dominates the entry points.** Most Petrastella apps are built
   on the `pulse-card` full-stack template. The template's README is the first thing
   anyone reads — agents included — and it describes the template, not the product.
   The actual product identity (Pulse, Surge, Depth, Campaigns, Shortlists, Brand
   Deals…) is diffused across the Prisma schema, dashboard components, and planning
   docs. No single artifact pulls it together.

2. **Cassy has no product-domain layer.** Cassy memories capture architecture, conventions,
   incidents, and preferences, but there is no convention for *product context*. Agents
   spun up in a new session guess at the domain from file names.

3. **When overviews do exist, they drift.** Hand-written product docs go stale within
   weeks of a schema change or a persona-split refactor. No one notices until an agent
   hallucinates a feature that no longer exists.

---

## Proposed Solution

### Part 1 — `project-overview` skill

A new skill (name TBD: `project-overview`, `product-overview`, or `cas-overview`) that:

- Reads the repo in a defined priority order (README → docs/ → schema/domain model →
  routes/pages → planning docs). Explicitly skips framework chrome and template
  marketing.
- Produces `docs/PRODUCT_OVERVIEW.md` with a fixed structure:
  - one-paragraph pitch
  - personas (tied to route/component dirs)
  - 5–10 core domain concepts (each tied to a schema model or component dir)
  - 2–4 primary user journeys
  - authoritative-source pointers (no content duplication)
- Target ~40–60 lines, ~1.5KB — lean enough to always load.
- Strips anything that could apply to any generic SaaS ("authentication," "file
  upload," "multi-tenant workspaces" → out). Favors project-specific jargon.
- On completion: prints the file path and a 3-bullet summary.

The draft prompt that produced a good result on `pulse-card` is included in the
appendix below and can seed the skill body.

### Part 2 — freshness automation

Model this on the existing CODEMAP freshness hook:

- **SessionStart hook** compares mtime/structure of `docs/PRODUCT_OVERVIEW.md`
  against:
  - the domain-model file(s) (e.g. `apps/backend/prisma/schema.prisma`)
  - top-level component/page directories that the overview references
- If either has seen material change since the overview was last updated (N commits or
  M days — tunable), surface a freshness warning similar to:
  `> PRODUCT_OVERVIEW.md is significantly out of date (N structural changes). Run /project-overview to refresh before assigning work.`
- `.cas/project-overview-pending.json` captures the change set, same pattern as
  `.cas/codemap-pending.json`.

### Part 3 — optional, stretch

- A **rule** auto-surfaced on domain-model edits: "You are editing the core domain
  model — consider whether `docs/PRODUCT_OVERVIEW.md` needs an update in the same PR."
- A **memory-pointer convention**: when this skill runs, it also writes (or updates) a
  tiny Cassy memory named `project_<slug>_domain.md` that just points at the doc. Gives
  Cassy search a first-class hit without duplicating the doc's contents in memory.

---

## Why Skill + Doc, Not Skill + Memory-Only

Two layers is deliberate:

- **Doc in git** — versioned, reviewable in PRs, visible to humans without Cassy, survives
  across branches/worktrees, and is the natural place to update alongside schema
  changes.
- **Thin Cassy memory pointer** — ensures Cassy surfaces the doc during semantic search,
  without Cassy holding a stale copy of its contents.

Memory-only drifts silently; doc-only is invisible to Cassy search. The pointer pattern
gives us both without the maintenance cost of keeping two copies in sync.

---

## Acceptance Criteria

- [ ] Running the skill on a repo with no existing overview produces
      `docs/PRODUCT_OVERVIEW.md` matching the structure above.
- [ ] Running it on a repo where the overview exists updates in place without
      losing manual edits to sections marked as hand-curated (TBD escape hatch —
      could be a `<!-- keep -->` marker).
- [ ] On a representative Petrastella project (pulse-card, petra-stella-cloud,
      gabber) the output is under 60 lines and contains no sentence that applies
      equally to a generic SaaS starter.
- [ ] SessionStart surfaces a freshness warning when the domain model has drifted
      significantly from the overview.
- [ ] A thin project memory pointer is created/updated pointing at the doc.

---

## Out of Scope

- Generating the broader docs/ tree (ARCHITECTURE, RUNBOOK, etc.) — this skill is
  product identity only.
- Translating the overview to non-English locales.
- Auto-commit/auto-PR on stale detection — surface the warning, let the human decide.

---

## Appendix — Draft Prompt

```
Read enough of this codebase to write a PRODUCT_OVERVIEW.md that captures what makes
*this project specifically* what it is — not what makes it a generic web app.

Consult in this order:
1. README.md, docs/, any ARCHITECTURE or OVERVIEW files
2. The database schema or domain model (prisma, SQL, type defs, core structs)
3. Top-level route/page structure and primary UI components
4. Any planning / roadmap / inventory / port-map docs
5. Skip: boilerplate framework docs, generic auth/billing/template chrome

Produce a single docs/PRODUCT_OVERVIEW.md with:
- One-paragraph pitch — what the product does, for whom, in plain language
- Personas — who uses it and how their UI/experience differs
- Core domain concepts — the 5–10 terms this project invented or repurposed
- Primary user journeys — 2–4 numbered flows, ~1 sentence each, with route paths
- Authoritative sources — point to live files; do NOT duplicate their contents

Rules:
- Strip anything that could apply to any SaaS starter. If the sentence works for a
  generic multi-tenant CRUD app, delete it.
- Favor project-specific jargon over generic nouns. If the codebase uses "Pulse" or
  "Burst" or "Deal," use those words — don't flatten them into "metric,"
  "notification," or "transaction."
- Target 40–60 lines, ~1.5KB.
- Do not invent features. If something is ambiguous, leave it out and note the source
  file to consult.
- When finished, print the file's path and a 3-bullet summary of the pitch.
```

---

## Motivating Example

When applied manually to `pulse-card`, this prompt produced ~45 lines covering:
personas (creator / brand / admin), the Project=Campaign duality, Item vs SocialPost
separation, the Pulse/Surge/Depth/Snapshot metric vocabulary, and three user
journeys — all grounded in `schema.prisma` and `components/dashboard/` + `components/p/`.
Template chrome (auth, workspaces, billing, Capacitor, i18n) was deliberately excluded.
The resulting doc is the kind of thing a cold agent or a new engineer could read once
and ship a useful first PR from.
