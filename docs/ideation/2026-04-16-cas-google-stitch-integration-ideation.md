---
date: 2026-04-16
topic: cas-google-stitch-integration
focus: integrate Cassy with Google Stitch — official APIs/exports vs community tooling
---

# Ideation: Cassy × Google Stitch Integration

## Grounding Summary

### Cassy extension surfaces (relevant to this topic)

- **`cas-mcp-proxy/`** — already aggregates upstream MCP servers (Figma, GitHub, etc.); one registration exposes N tools to every worker in every project
- **Skill sync** (`cas-cli/src/sync/skills.rs`) — DB↔`.claude/skills/*/SKILL.md` with YAML frontmatter; designed to mirror external skill libraries
- **Rule system** (`cas-cli/src/rules/`) — Draft→Proven lifecycle; canonical enforcement mechanism
- **Hook system** (`cas-cli/src/hooks/handlers/`) — SessionStart, PostToolUse, Stop, scored context injection
- **Store trait hierarchy** (`cas-cli/src/store/`) — consistent pattern across memory/tasks/rules/skills
- **Factory protocol** (`cas-factory-protocol`) — WebSocket supervisor↔worker messages
- **Slack bridge** (`slack-bridge/src/router.ts`) — inbound command routing
- **`cas codemap` / `cas project-overview`** — canonical context docs read by every new worker
- **Gap**: zero UI/visual artifact story; everything is text. No screen/component/design type.

### Google Stitch landscape (as of April 2026)

**Official surfaces:**
- MCP server at `https://stitch.googleapis.com/mcp` (auth via `STITCH_API_KEY` or OAuth)
- `@google/stitch-sdk` (npm, JS/TS) wrapping the MCP surface, integrates with Vercel AI SDK
- `google-labs-code/stitch-skills` — official agent skills library (same SKILL.md shape as Cassy)
- Exports: HTML/CSS (primary), PNG, DESIGN.md (portable Markdown design-system spec), Figma via community plugin
- Rate limits: **350 gen/mo standard + 50 gen/mo pro** on free tier — tight for iterative agents
- Auth: API key from Stitch Settings UI, or OAuth with Google Cloud project
- **No public creation API yet** — new projects/screens still require the web UI; SDK works only against existing projects
- Labs experimental status — no SLA, Google could kill it

**Community integrations:**
- `oogleyskr/stitch-mcp-server` — 25 tools including screen-to-React TSX, Tailwind config extraction, design tokens as CSS vars/SCSS/JSON, WCAG 2.1 audit
- `gabelul/stitch-kit` — 35 skills, 7 framework targets (Next.js, Svelte, React, React Native, SwiftUI, HTML), Claude Code plugin + Codex CLI agent
- `davideast/stitch-mcp` — CLI that builds a local site from Stitch projects, maps screens to routes, exposes HTML/screenshots via MCP
- `gemini-cli-extensions/stitch` — natural-language Gemini CLI extension for Stitch MCP
- Apify actor as HTTP proxy (ToS risk — not recommended)

**Dealbreakers / constraints:**
- Rate limits are real and low (350/50 gen/mo free tier)
- No creation API — cannot fully automate "prompt → design"
- Figma export is model-gated (Gemini 2.5 Flash standard mode only)
- Labs status means no deprecation policy
- Scraping violates ToS

### Integration vectors observable today

1. Stitch MCP + API key — the intended path (Antigravity codelab demonstrates this)
2. `stitch-sdk` programmatic generation against existing projects
3. DESIGN.md round-trip — portable Markdown, readable by any LLM
4. Stitch → Figma plugin → Figma-to-code bridges (Anima/Locofy/Builder.io)
5. HTML/CSS ZIP download → LLM conversion to React/Vue/Tailwind/SwiftUI

## Ranked Ideas

### 1. Wire Stitch MCP as an upstream in `cas-mcp-proxy`
**Description:** Register `https://stitch.googleapis.com/mcp` in the proxy registry; forward `STITCH_API_KEY` per project (read from env or `.cas/stitch.toml`). Add `cas mcp add stitch` shortcut command. Optionally also register `oogleyskr/stitch-mcp-server` as a community variant for richer framework extraction (React/Tailwind/tokens).
**Rationale:** The proxy engine is designed exactly for this use case. One registration exposes 10+ official Stitch tools (plus 25+ community tools if layered) to every worker in every project that runs the proxy. Everything downstream in this ideation (context injection, token rules, screenshot verification) depends on tool availability.
**Downsides:** Auth key management per project; rate-limit quota is shared across all workers on the same key (addressed by #6); picking a default upstream (official vs `oogleyskr`) is an opinionated call.
**Confidence:** 95%
**Complexity:** Low
**Grounding:** `cas-mcp-proxy/`, `cas-cli/src/commands/mcp.rs`
**Status:** Explored — requirements captured in `docs/brainstorms/2026-04-16-cas-google-stitch-integration-requirements.md` (R15)

### 2. Import `stitch-skills` wholesale via Cassy skill sync
**Description:** Extend `cas-cli/src/sync/skills.rs` to mirror `google-labs-code/stitch-skills` into `.claude/skills/stitch/*` with `provenance: stitch-official` frontmatter so upstream updates flow on `cas sync`. Optionally also mirror `gabelul/stitch-kit` under a `.claude/skills/stitch-community/` namespace.
**Rationale:** Both ecosystems publish SKILL.md in the same YAML-frontmatter shape. The sync mechanism is the compounding primitive — one import authored once reaches every future session, every project. Dedupe against existing `.claude/skills/` and preserve upstream update path.
**Downsides:** Upstream schema drift risk; some skills may be irrelevant to non-UI projects (gate imports on project-config detection); supply-chain consideration if pulling community repos.
**Confidence:** 80%
**Complexity:** Low
**Grounding:** `cas-cli/src/sync/skills.rs`
**Status:** Unexplored

### 3. SessionStart hook injects design tokens + screen primer
**Description:** When a project declares a Stitch config (`.cas/stitch.toml` or detected local DESIGN.md), SessionStart parses the token table (colors, spacing, type scale) and top-3 canonical screens, injects them into the worker's first context window as a fenced `## Design Tokens` block. Falls back gracefully to no-op for non-UI projects.
**Rationale:** Every worker currently re-learns the design from scratch — invisible tax paid across dozens of spawns per day. One hook compounds across every task, every session. Gated injection keeps noise low for projects without design systems.
**Downsides:** Prompt-budget tax (small if scoped to tokens + 3 screens); stale tokens if not invalidated on DESIGN.md changes (needs file-watch or periodic refetch); initial Stitch fetch requires gen quota (addressed by #6 cache).
**Confidence:** 85%
**Complexity:** Low-Medium
**Grounding:** `cas-cli/src/hooks/handlers/session_start.rs`
**Status:** Unexplored

### 4. `design_ref` field on tasks (factory protocol extension)
**Description:** Add `design_ref: {screen_id, design_md_anchor, screenshot_path}` to the supervisor→worker task message schema. `cas task create` accepts `--screen <id>`. Worker prompt template renders the ref as a `## Design Reference` block. Optionally warn if a UI-shaped task is created without a `design_ref`.
**Rationale:** Solves task-level visual context delivery without touching every tool call (cheaper than ambient proxy injection). Complements #3 — session-level tokens + task-level screen specificity = full coverage.
**Downsides:** Schema migration for factory protocol; requires supervisor discipline to populate the field; "UI-shaped task" detection is heuristic.
**Confidence:** 75%
**Complexity:** Medium
**Grounding:** `cas-factory-protocol/`, `cas-cli/src/mcp/tools/core/task.rs`
**Status:** Unexplored

### 5. Token-drift rule (Draft→Proven) sourced from DESIGN.md
**Description:** New `cas-cli/src/sync/stitch_tokens.rs` parses DESIGN.md into a canonical token table. A Draft rule flags hardcoded hex colors, raw px/rem values, and non-token font sizes in `*.{tsx,jsx,vue,svelte,css,scss}` edits when the token table exists. Rule promotes to Proven after N clean closes; demotes on repeated violations.
**Rationale:** The Draft→Proven pipeline IS the canonical enforcement mechanism — uses it rather than inventing parallel linting. Kills silent design-system drift that today is caught only by eyeballing production.
**Downsides:** Regex false-positives (hex in non-color contexts, px in animation durations); DESIGN.md token extraction needs a resilient parser; rule-noise in projects without design tokens (gate on token-table presence).
**Confidence:** 75%
**Complexity:** Medium
**Grounding:** `cas-cli/src/rules/`, `cas-cli/src/hooks/handlers/post_tool_use.rs`
**Status:** Unexplored

### 6. Shared Stitch response cache + quota budget tracker
**Description:** New `StitchCacheStore` implementing the existing `Store` trait, keyed by `hash(prompt + project_id + screen_id)`. `cas-mcp-proxy` interceptor serves cache hits transparently before calling Stitch. Separate `stitch_budget` counter per project, per model tier (standard/pro), surfaces remaining gen quota to supervisor pre-dispatch with a soft-stop near cap. Optional `cas stitch budget` CLI to inspect.
**Rationale:** 350/50 gen/mo free tier is a HARD constraint confirmed by research. Parallel workers on the same project will double-spend quota on near-identical prompts. Cache + counter reuses existing store trait — minimal new infrastructure.
**Downsides:** TTL/invalidation semantics are non-trivial (when does a cached screen stop being valid — on DESIGN.md change? manual flush?); requires proxy middleware plumbing; budget counter needs resync if Stitch UI reports different numbers.
**Confidence:** 80%
**Complexity:** Medium
**Grounding:** `cas-mcp-proxy/`, `cas-cli/src/store/`
**Status:** Unexplored

### 7. Scope → implied-screens generator
**Description:** New `cas stitch scaffold "<scope>"` command (or MCP tool). Input: a natural-language scope like "recipe-sharing mobile app" or "SaaS admin dashboard for ops team." Pipeline:
1. LLM enumerates implied screens from scope (home, detail, profile, auth, empty states, errors) with acceptance criteria per screen.
2. For each screen, call Stitch MCP (`project.generate()` against a project, or multi-screen generation — up to 5 at once as of March 2026).
3. Cache screens in the `StitchCacheStore` (#6), emit one Cassy task per screen with `design_ref` (#4) pre-populated, create a parent epic tying them together.
4. Return a summary with thumbnail refs and quota consumed.

**Rationale:** This is the use case that makes #1 (proxy integration) actually valuable to operators — instead of pointing Cassy at a pre-made Stitch project, the user hands it a scope and gets back a decomposed task graph with designs attached. Sidesteps R3's "task queue pollution from design iteration" failure mode because scope expansion runs once upfront. Leverages Stitch's new multi-screen mode to batch gen-calls.

**Downsides:** Quota-burning — one scope could cost 8–15 generations (addressed by #6 caching + budget-check pre-flight). LLM-driven screen enumeration is lossy; user may need to edit the implied-screen list before dispatching. No public Stitch creation API means the target project must already exist (prompt user for project ID, or fall back to a Cassy-owned "scaffold" project shared across scope calls).

**Confidence:** 70%
**Complexity:** Medium-High (orchestration layer, but all components are in #1/#4/#6)
**Grounding:** Composes `cas-mcp-proxy/` (#1) + `cas-cli/src/mcp/tools/core/task.rs` + factory-protocol `design_ref` (#4) + `StitchCacheStore` (#6). New: `cas-cli/src/commands/stitch_scaffold.rs` or `cas-cli/src/mcp/tools/service/stitch_scaffold.rs`.
**Status:** Explored — requirements captured in `docs/brainstorms/2026-04-16-cas-google-stitch-integration-requirements.md` (R1–R16). Bootstrap model revised: Stitch `createProject()` API verified present April 2026, scaffold is fully auto.

### 8. Screenshot-based visual verification (phased)
**Description:**
- **Phase A:** Task-verifier agent attaches the Stitch-canonical PNG (via `screen.getImage()`) to the task-close payload as an artifact. No automated diff — humans see before/after in the close summary and Slack notification.
- **Phase B:** Add Playwright headless render of the implemented route + SSIM diff against canonical PNG. Gate close on threshold. Rule demotes on repeated red diffs.
**Rationale:** Explicitly addresses "Cassy is text-only, no way to show what a worker built" pain from grounding. One PNG per screen serves task-verifier + PR review + Slack bridge + docs + onboarding — multi-consumer artifact from a single generation.
**Downsides:** Phase B is High complexity (headless render, SSIM brittleness, threshold tuning, animation/theme flakiness). Don't couple Phase A and B — A has standalone value.
**Confidence:** 70% (Phase A) / 45% (Phase B)
**Complexity:** Medium (Phase A) / High (Phase B)
**Grounding:** `.claude/agents/task-verifier.md`, `slack-bridge/src/router.ts`, `cas-cli/src/mcp/tools/core/task.rs`
**Status:** Unexplored

## Rejection Summary

| # | Idea | Reason Rejected |
|---|------|-----------------|
| R1 | Screen-aware codemap (standalone) | Depends on manual `@screen:` bindings nobody will maintain. Merge the useful slice into #3 (inject a "Screens" block from `project-overview`) |
| R2 | Visual diff via Slack bridge (standalone) | Duplicates #7 — Slack is one consumer of the same PNG artifact, not a separate mechanism |
| R3 | DESIGN.md watcher → auto-task decomposition | High risk of task-queue pollution from design-iteration churn; granularity of "new component" is hard to diff-detect. Revisit after #3/#5 stabilize |
| R4 | Slack `/stitch` command router | Niche convenience for Slack+Stitch-active teams; doesn't compound across workers. Follow-up once foundation ships |
| R5 | Stitch screens as first-class Cassy artifact type | Speculative architectural change (new schema, new sync, new semantics) ahead of confirmed friction from #3/#5 |
| R6 | Reverse-generate Stitch designs from existing code | Depends on unconfirmed Stitch code→design multimodal capability; not in documented surface |
| R7 | Designer-worker role that outputs Stitch prompts | Blocked by Stitch's lack of a public creation API (April 2026); premature role specialization |
| R8 | Stitch screen URL as acceptance criteria (standalone) | Covered by #7 Phase B — this is how the verifier attaches to a task, not a separate feature |
| R9 | DESIGN.md as source of truth, code hydrated | Too ambitious inversion; rewrites developer workflow for no proven pain; hand-edit fences always leak |
| R10 | Round-trip: code edits regenerate Stitch previews | Same blocker as R6 (code→design path); quota-burning on every commit |
| R11 | Screen IDs as memory keys / spatial memory | Schema complexity outweighs value; tagging prose memories with screen IDs achieves 90% of the benefit |
| R12 | Pre-batched Stitch generation during idle factory time | Speculative quota burn on guessed needs; #6 cache serves real demand deterministically |
| R13 | `cas stitch export figma` one-shot bridge | Low unique value once #1 ships (users trigger Figma export via Stitch MCP directly); export is model-gated anyway |
| R14 | Designer-engineer canonicality handshake (revision lockfile) | Depends on non-existent Stitch webhooks; polling is fragile and quota-costly |

## Implementation Notes

**Phasing:** #1–#3 form a minimal viable foundation (all Low/Low-Med complexity, high compound value). #4–#6 are Phase-2 depth. #7 is the frontier — high value, high burden, worth phasing A/B.

**Dependency graph:**
- #1 unblocks #3, #4, #5, #6, #7 (they all rely on Stitch MCP being reachable from workers)
- #6 (cache) should land before #3 (SessionStart fetches tokens) to avoid quota burn during onboarding
- #2 (skill import) is independent and can ship in parallel with #1

**Operational notes:**
- Stitch rate limits make a **shared cache mandatory**, not optional, for factory use
- Stitch's Labs-experimental status argues for **loose coupling** — keep integration behind a feature flag; don't let Cassy hard-depend on Stitch APIs
- Auth-key-per-project is the right scope boundary; rate limits are per-project anyway

## Session Log

- 2026-04-16: Initial ideation — 41 raw ideas generated across 4 frames (operator pain, inversion/automation, assumption-breaking, leverage/compounding), merged/deduped to 21, filtered to 7 survivors. Focus: integrate Cassy with Google Stitch — compare official + community options.
- 2026-04-16: Selected idea #1 (Wire Stitch MCP as upstream in `cas-mcp-proxy`) for brainstorming.
- 2026-04-16: User added scope-driven usage requirement → ideation extended with idea #7 (Scope → implied-screens generator). Survivor count now 8. Both #1 and #7 selected for combined brainstorm (scope generator is the use case that gives #1 its shape).
