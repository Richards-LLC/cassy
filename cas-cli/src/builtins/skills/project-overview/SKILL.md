---
name: project-overview
description: Use when asked what a project is, to create or update PRODUCT_OVERVIEW.md, or when onboarding needs product intent after code structure is understood.
managed_by: cas
---

# Project Overview

Produce a **short, project-specific** domain snapshot at `docs/PRODUCT_OVERVIEW.md`. The goal is to replace reading the whole repo with reading 40–60 lines. Generic SaaS copy is the enemy; project jargon is the point.

**IMPORTANT: All file references use repo-relative paths** (e.g., `apps/backend/prisma/schema.prisma`), never absolute paths.

## What this skill is (and isn't)

- **IS:** a semantic read of the repo that names the *product*, the *users*, and the *core nouns/verbs* of the domain.
- **IS NOT:** an architecture doc, a codemap, a changelog, or a README replacement. Those already exist.

If the project has a CODEMAP, assume the reader has it. Don't restage file-structure facts here.

## Check the knowledge store first

`docs/PRODUCT_OVERVIEW.md` is a **view** over the project knowledge store, not an independent artifact. Before reading any source, ask the store what it already knows:

```bash
cas knowledge search "product overview domain personas"
```

- **A page comes back and its sources are current** (the docs and schemas it cites still exist and still say what it claims) — read it with `cas knowledge read <id-or-path>` and use it as your draft. You are reconciling a document, not writing one from scratch.
- **Nothing comes back, or the page cites paths that no longer exist** — the store has nothing usable. Do the full read order below.

Never skip writing `docs/PRODUCT_OVERVIEW.md` just because a page exists. The page is the distilled form; the file is the artifact humans and other agents open.

## Read order (highest signal first)

Read only what's needed to extract domain meaning. Stop once the picture is clear — do not exhaustively skim the repo.

1. **README.md** — pitch, intended users, positioning (often 70% of what you need)
2. **docs/** — product docs, onboarding, architecture-overview style files (skip API references, auto-generated docs)
3. **Domain model:**
   - Prisma: `apps/backend/prisma/schema.prisma` or `prisma/schema.prisma`
   - SQL migrations: `migrations/`, `drizzle/`
   - Rust: top-level domain crates (look for `model`, `domain`, `entity` modules)
   - Any `types/`, `schemas/`, or DTO directories with named domain nouns
4. **Routes / pages / top-level components** — confirms user-facing journeys
   - `apps/*/src/pages/**`, `app/**`, `src/routes/**`
5. **Planning docs** — `docs/brainstorms/`, `docs/requests/`, `docs/plans/` — to pick up in-flight direction
6. **package.json / Cargo.toml / pyproject.toml** — name, description, deps hint at stack + category

**Skip** framework chrome: lockfiles, `node_modules`, `target/`, generated clients, migration up/down boilerplate, ESLint/Prettier configs, CI YAML, test fixtures.

## Output structure (fixed)

Write to `docs/PRODUCT_OVERVIEW.md`. Target **40–60 lines (~1.5KB)**. Hard cap 80 lines.

```markdown
# <Project Name> — Product Overview
> Auto-generated domain snapshot. Regenerate with `/project-overview` when the domain model drifts.

## Pitch
One or two sentences. What this project is, for whom, and why it exists. Uses the project's own vocabulary.

## Personas
- **<Role>** — what they do in the system, which surface they live in
- **<Role>** — ...
(2–5 roles. If only one persona exists, say so — don't invent.)

## Core Concepts
- **<DomainNoun>** — one-line definition using project terms
- **<DomainNoun>** — ...
(5–12 nouns. These are the words that appear in schemas, URLs, and UI labels. Prefer the project's spelling over industry-generic synonyms.)

## Primary Journeys
1. <Persona> does <verb> <noun> via <surface> → <outcome>
2. ...
(3–6 journeys. Each journey is one line. Name the surface: route, screen, CLI command, API.)

## Authoritative Sources
- Domain model: `<path>`
- Routes/pages: `<path>`
- Product docs: `<path(s)>`
- Planning: `<path>` (if active)
```

## Quality bar — zero generic-SaaS sentences

Every sentence must fail this test:
> "Could this paragraph be copy-pasted into any other SaaS starter's README?"

If yes, rewrite with project-specific nouns. Examples of what to cut:

- ❌ "A modern web application for managing users and data."
- ❌ "Empowers teams to collaborate efficiently."
- ❌ "Built with best-in-class technologies."
- ❌ "Scalable, secure, and extensible."
- ✅ "Tracks campaign performance per creator-brand pairing across TikTok, Instagram, and YouTube."
- ✅ "Ingests recording-studio takes, transcodes them with whisper, and routes generated clips to the gen-agent for image prompts."

When in doubt, **name something concrete from the schema or routes**. If you can't, you haven't read enough yet.

## Preserving hand-edited sections

If `docs/PRODUCT_OVERVIEW.md` already exists:

1. **Read it first.**
2. **Preserve any `<!-- keep -->` … `<!-- /keep -->` blocks verbatim.** These are user-owned; do not rewrite, reflow, or even re-whitespace them. Place them back in the same section they appeared in.
3. Everything outside keep-blocks is regenerated.
4. If a section header has `<!-- keep -->` on the line directly below it, preserve that entire section including the header.

Example:

```markdown
## Core Concepts
<!-- keep -->
- **Campaign** — a paid engagement between a brand and a creator, scoped to a single platform
- **Deliverable** — the post URL the creator submits as proof of work
<!-- /keep -->
- **Payout** — ...
```

The two bulleted lines and the `keep` markers survive re-runs.

## After writing the doc

### 1. Write a thin memory pointer

Invoke `mcp__cas__memory` with `action=remember` to create/update a pointer memory.

- **Name / title:** `project_<slug>_domain.md` (slug = lowercase kebab-case of project name)
- **Body:** ONE line only. A repo-relative link to the doc plus a single-sentence hook.
- **No content duplication.** Do not inline the pitch, personas, or concepts. The whole point is that search surfaces the pointer and the reader opens the doc.

Example:

```
See [docs/PRODUCT_OVERVIEW.md](docs/PRODUCT_OVERVIEW.md) — creator-brand campaign performance platform.
```

If a pointer already exists with the same name, update it. Do not create duplicates.

### 2. Seed the knowledge store

`docs/PRODUCT_OVERVIEW.md` is a distillable source, so one build turns the doc you just wrote into a knowledge page plus a source-ledger entry:

```bash
cas knowledge build --max-sources 5
```

Nothing else in the repo changed, so the ledger short-circuits every other source and this costs at most one model call. Confirm it landed:

```bash
cas knowledge search "product overview"
```

If the store is not initialized in this project, the command says so and there is nothing to fix — the doc on disk is still the artifact of record.

### 3. Clear the freshness counter

Run:

```bash
cas project-overview clear
```

This resets the pending-change counter that `SessionStart` uses to warn about drift. Skipping this step means the next session will keep nagging about staleness even though the doc was just refreshed.

### 4. Report back

Print two things to the user:

1. The file path that was written.
2. A 3-bullet summary of the pitch (one bullet each: what, for whom, why).

## When to run

- **First time:** no `docs/PRODUCT_OVERVIEW.md` exists → generate from scratch.
- **Drift warning:** `SessionStart` reports the domain model changed → regenerate and keep-blocks survive.
- **Manual:** user invokes `/project-overview` or asks for a product overview.
- **Periodic:** if the doc is more than ~8 weeks old and the domain model has moved, refresh it.

## Anti-patterns

- Reading the entire repo before writing anything. Stop when the picture is clear.
- Listing every model in the schema. Pick the 5–12 that drive the product.
- Copying the README verbatim. This doc is a distillation, not a rehash.
- Writing "the user" when you mean "the brand manager" or "the creator" or "the on-call engineer". Name the persona.
- Skipping the keep-block check on regeneration. Destroying hand-edits is a trust breaker.
- Forgetting to write the memory pointer or forgetting to run `cas project-overview clear`.
- Regenerating from scratch without checking `cas knowledge search` first. A current page is a draft you should be reconciling, not discarding.
- Forgetting `cas knowledge build` after writing the doc. The store then keeps serving a stale page to every agent that queries it.
