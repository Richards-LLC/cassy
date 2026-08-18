---
date: 2026-04-16
topic: cas-google-stitch-integration
---

# Cassy × Google Stitch Integration — Scope-Driven Screens & Visual Agent Loop

## Problem Frame

An agency runs $10K exploratory contracts where a client concept must be turned into visual screens for a pitch, then later graduated into the authoritative spec for the build phase. Today that means hand-authoring screens one-by-one in Stitch, then copy-pasting them into a deliverable disconnected from the eventual repo. In the build phase, frontend workers code UI blind — no visual spec, no way to verify their output matches the design, regressions ship. This integration closes both loops:

1. **Pitch phase:** concept → implied screens → interactive HTML preview, ready for the client
2. **Build phase:** same screens become Cassy tasks with `design_ref` attached; workers can render their implementation and visually critique it against the canonical screen, and task-verifier gates close on significant drift

## User Flow

```
Pitch phase
┌────────────────────────────────────────────────────────────────────┐
│  cas init  (same repo used for pitch AND eventual build)           │
│            │                                                       │
│            ▼                                                       │
│  cas stitch scaffold "<concept>"                                   │
│    ├─ auto-creates Stitch project via createProject()              │
│    ├─ writes .cas/stitch.toml (project_id, auth mode)              │
│    ├─ LLM expands concept into implied screen list (visible)       │
│    ├─ generates each screen via Stitch MCP                         │
│    └─ caches artifacts in StitchCacheStore                         │
│                                                                    │
│  cas stitch screens                            [list + status]     │
│  cas stitch regen <id> "<prompt>"              [iterate one]       │
│  cas stitch add-screen "<prompt>"              [add]               │
│  cas stitch drop <id>                          [remove]            │
│  cas stitch preview                            [serve local HTML]  │
│            │                                                       │
│            ▼                                                       │
│  Client reviews interactive HTML preview                           │
│  → if contract signed, continue to build                           │
└────────────────────────────────────────────────────────────────────┘

Build graduation
┌────────────────────────────────────────────────────────────────────┐
│  cas stitch lock                                                   │
│    ├─ snapshots current screen set as canonical build spec         │
│    ├─ creates one Cassy epic + one task per screen                   │
│    └─ populates design_ref {screen_id, path, thumbnail} per task   │
└────────────────────────────────────────────────────────────────────┘

Build phase (per task)
┌────────────────────────────────────────────────────────────────────┐
│  Worker picks up task (design_ref in task context)                 │
│  Worker writes code, can call stitch_visual_check(screen, route)   │
│    → returns structured diff; worker iterates                      │
│  Worker closes task                                                │
│    → task-verifier auto-runs visual check                          │
│    → drift above threshold blocks close; worker fixes              │
└────────────────────────────────────────────────────────────────────┘
```

## Requirements

**Scope → Screens Generation**
- R1. `cas stitch scaffold "<scope>"` is the entry point: accepts a natural-language concept, produces a Stitch project and an initial screen set.
- R2. Stitch project creation is fully automatic via `stitch.createProject()` — no manual web-UI step per engagement.
- R3. Stitch project ID and auth are persisted in `.cas/stitch.toml`; auth supports both API key and `gcloud auth application-default` credentials.
- R4. Scope expansion (concept → implied screen list) happens before generation and is visible to the user; the expanded list can be edited before screens are actually generated.
- R5. Each generated screen is cached in a project-scoped store keyed by prompt + parent screen, so regeneration against the same prompt does not burn quota.

**Refinement (CLI-driven)**
- R6. Refinement commands: `cas stitch screens` (list), `cas stitch regen <id> "<prompt>"`, `cas stitch add-screen "<prompt>"`, `cas stitch drop <id>`.
- R7. `cas stitch preview` serves a local interactive HTML preview of the current screen set, suitable for client presentation (multi-screen navigation, styled).
- R8. Quota is tracked per Stitch project with a soft-stop as the monthly cap approaches; `cas stitch budget` surfaces remaining standard/pro generations.

**Pitch → Build Graduation**
- R9. Pitch artifacts (screens, DESIGN.md, `.cas/stitch.toml`) live in the same repo that becomes the build repo — no export/import step.
- R10. `cas stitch lock` is the explicit pitch-to-build transition: snapshots the current screen set and creates one Cassy epic + one task per screen with `design_ref` populated.
- R11. Before `lock`, no build tasks exist. The pitch phase is design-only to avoid task pollution during iteration.

**Visual Agent Critique Loop**
- R12. New MCP tool `stitch_visual_check(screen_id, route_or_component)` is callable by any worker during task execution; renders the current implementation and returns a structured diff (pixel drift, token-level color/spacing/typography mismatches) against the Stitch screen.
- R13. task-verifier runs `stitch_visual_check` automatically at close for any task with a `design_ref`; drift above threshold blocks the close.
- R14. Drift threshold is configurable per project (field in `.cas/stitch.toml`) with a default tuned for agency work; workers can see the threshold before closing.

**Foundational Plumbing**
- R15. Stitch MCP server (`https://stitch.googleapis.com/mcp`) is registered as an upstream in `cas-mcp-proxy` with per-project auth forwarding.
- R16. Factory protocol (`cas-factory-protocol`) gains a `design_ref` field on the task message schema: `{screen_id, design_md_anchor, screenshot_path}`.

## Success Criteria

- An agency operator can go from a 2-sentence concept to an interactive HTML preview the client can navigate in under 10 minutes.
- Build-phase frontend tasks carry visual context automatically; workers never code UI without seeing the target.
- Visual drift is caught at task close without a human taking a screenshot — the verifier has teeth.
- Quota for a typical $10K engagement (estimated 10–20 screens, 2–3 refinement rounds, 30–50 build-phase visual checks) fits under the 350 gen/mo free tier; `budget` command surfaces headroom honestly.
- The same repo serves pitch artifacts and build code with zero bundling, zero export, zero handoff friction.

## Scope Boundaries

- **Not in scope:** Figma export pipeline — users can trigger it via Stitch MCP directly; no dedicated `cas stitch export figma` command.
- **Not in scope:** Code generation from screens. Workers still write code; `design_ref` is input, not output. Downstream tools like `stitch-kit` or `oogleyskr/stitch-mcp-server` may add this later as a layered capability.
- **Not in scope:** Client-facing collaboration in the preview (comments, approvals, sharing). Preview is read-only HTML; clients communicate outside the tool.
- **Not in scope:** Cross-engagement quota pooling, agency-wide dashboards. Quota is per-Cassy-project = per-Stitch-project. One project bears its own rate-limit fate.
- **Not in scope:** Shared Stitch project across engagements. Isolation (both quota and Stitch generation context) is a hard requirement.
- **Not in scope:** Stitch web-UI refinement as a primary workflow. Web UI remains optional for power users but the CLI is the canonical surface.

## Key Decisions

- **Same repo, pitch + build** — simpler than export/import, matches Cassy's "project = repo" assumption. Pitch artifacts persist, become build spec in place.
- **Fully-auto Stitch project creation** — validated April 2026 that `stitch.createProject()` exists as a first-class SDK method; removing the manual step is free.
- **CLI-driven refinement** — most Cassy-native; scriptable; keeps Stitch web UI optional, not required.
- **Both on-demand and gate-at-close visual critique** — gives workers self-correction capability during work AND verifier teeth at close. Neither alone is sufficient.
- **Explicit `lock` command to transition pitch → build** — prevents task pollution during design iteration. Pitch phase is "designs only, no tasks," build phase begins at lock.
- **Per-engagement Stitch project isolation** — quota isolation and avoiding generation-context cross-contamination from prior clients.

## Dependencies / Assumptions

- Stitch `createProject()` API remains publicly available (confirmed April 2026 via SDK README and release notes).
- Stitch rate-limit tier remains approximately 350 standard + 50 pro gen/mo on free; if a paid tier arrives, quota-budget logic must accept a configurable ceiling.
- Stitch remains a Google Labs experiment with no deprecation policy — integration should be feature-flagged so Cassy can operate if Stitch goes away.
- `cas-mcp-proxy` can inject per-project auth headers for upstream calls (to verify during planning against proxy internals).
- Worker rendering capability (for visual check) can be bootstrapped from existing Cassy infrastructure or a scoped headless-browser dependency.

## Outstanding Questions

### Resolve Before Planning
*(none — all product decisions made)*

### Deferred to Planning
- [Affects R3][Technical] Exact TOML schema for `.cas/stitch.toml` — field names, defaults, layering with global Cassy config.
- [Affects R12][Technical] Rendering backend choice for visual check — Playwright? puppeteer-core? a native Rust approach? Pick during planning based on existing Cassy dependencies.
- [Affects R12][Technical] Diff algorithm — SSIM, PSNR, or a structural-DOM diff as fallback for dynamic content (data-driven lists, dates).
- [Affects R14][Needs research] Drift-threshold default calibration — needs empirical validation on a handful of real UI tasks before a sensible default ships.
- [Affects R5][Technical] Cache invalidation rules — when does a cached screen become stale? On `regen` only, or also on Stitch-side edits detected via polling?
- [Affects R15][Needs research] Whether to also layer community upstream (`oogleyskr/stitch-mcp-server`) for framework-specific extraction (React/Tailwind tokens) — adds value but adds supply-chain surface.
- [Affects R4][Technical] Scope-expansion implementation — use an in-session LLM call (via Claude/Gemini in the worker), or does Stitch MCP expose a "suggest screens from scope" tool we can call directly?
- [Affects R10][Technical] `lock` idempotency and re-lock semantics — what happens if the user locks, generates new screens, then locks again?

## Next Steps

→ Hand off to planning (cas-supervisor or /plan)
