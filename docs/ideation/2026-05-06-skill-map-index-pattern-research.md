---
date: 2026-05-06
author: zen-lion-50 (factory supervisor) + daniel
status: research
topic: skill-listing token economics — index-with-fetch-on-demand pattern
relevance: claude-code-feedback, future-cas-skill-loading-design
---

# Skill-Map Index Pattern — Research Note

## TL;DR

Claude Code (and Cassy, by inheritance) currently front-loads every installed skill's full `description` into the system prompt at session start. For users with many skills (60+ in Daniel's case), this hits the configured budget cap (`skillListingBudgetFraction`, default 1% of context ≈ 4k tokens) and triggers crude truncation — full descriptions dropped entirely for cold-tier skills, with only the name surviving.

A better pattern, already proven in Cassy itself, is **index-with-fetch-on-demand** — load a tight `MEMORY.md`-style index of name + 1-line hook for every skill, and provide a tool to fetch the full description (frontmatter + trigger keywords + examples) when a skill looks relevant. This trades a fixed 4k-token cost for a smaller fixed index plus a variable per-session fetch cost that scales with actual skill use.

## Trigger event

Daniel's `/doctor` output 2026-05-06 surfaced the truncation warning:

> Skill listing will be truncated. 20 descriptions dropped (full descriptions kept for most-used skills) (1.8%/1% of context): fallow, fallow, cas-brainstorm, +17 more.

The `+17 more` plus visible duplicate listings (`fallow` and `cas-brainstorm` each appear twice in the loaded set) suggest both a budget pressure and a pre-existing dedup bug. The dedup bug is not just `fallow` and `cas-brainstorm` — verified 2026-05-06 that all 16 first-party Cassy skills are duplicated in the loaded skill list, byte-identical between `/home/pippenz/Petrastella/cas-src/.claude/skills/` (project) and `/home/pippenz/.claude/skills/` (user). The user-level copy is intentionally seeded by `cas update --user` (commit b35d0db) so worker worktrees can fall back to it when host projects gitignore `.claude/skills/`. Claude Code's skill loader does not dedupe by name across project + user source paths, so every Cassy skill appears twice in any session running inside cas-src. The Anthropic-shipped skills (`init`, `review`, `claude-api`, etc.) only appear once because they live in one location only — confirming the duplication is tied to "skill present in two source dirs," not a listing-pass bug.

Daniel's instinct: "with our skills we generally have really tight short top level and then link to sub resources, why can't we do something like that with a skills map?"

## Current architecture (Claude Code, ~Q4 2025)

- **System prompt assembly:** at session start, every installed skill's `name` + `description` from its frontmatter is concatenated into a system-reminder block.
- **Budget enforcement:** if total exceeds `skillListingBudgetFraction` of context, descriptions are dropped (not truncated) for the lowest-priority skills, ranked by recency-of-use.
- **Trigger evaluation:** Claude scans the loaded descriptions every turn to decide whether to fire a skill proactively. Trigger keywords, examples, and "use when" rubrics live inside the description body — so the description must be in-context for proactive trigger detection to work.
- **Caching:** the system prompt (including skill listings) is prompt-cached. Once a session starts, the 4k tokens of skill listings are essentially free for the rest of the session — they do not re-bill on each turn.

## Cassy's existing pattern (MEMORY.md)

Cassy auto-memory implements exactly the index-with-fetch-on-demand pattern Daniel intuited:

- **`MEMORY.md`** is always loaded into the system prompt — a flat list of `- [Title](file.md) — one-line hook` entries, ~150 chars each, max 200 lines (truncation enforced).
- **Individual memory files** (e.g. `feedback_prisma_relation_shadows_scalar_fk.md`) are NOT loaded eagerly. Claude reads them via the `Read` tool when the index hook flags them as potentially relevant to the current turn.
- **Result:** Daniel has 50+ persistent memories in his auto-memory, but the always-loaded cost is the tight ~5k-token index, not the full 50k+ of memory bodies. Cold memories cost nothing until needed.

This is the pattern Daniel is asking why skills don't use.

## Why Claude Code chose front-loading (the architectural trade-offs)

**1. Trigger detection wants the full description in context.**
Skills are designed to fire *proactively* — Claude reads trigger keywords and examples in the `description` field and decides "this matches, fire it." If only a 1-line hook were loaded, the model would need a two-stage trigger:
   1. Skim index → "this might match"
   2. Fetch full description → re-decide
This adds a tool round-trip per candidate skill and risks missing subtler triggers buried inside descriptions (e.g. `fallow`'s 90-framework laundry list, `claude-api`'s `imports anthropic` heuristic, `cas-brainstorm`'s "trigger PROACTIVELY when..." language). The current design assumes that complete trigger context per skill is more valuable than the token overhead.

**2. Prompt caching makes front-loading nearly free after session start.**
The 4k tokens of skill descriptions get cached once and ride for the whole session. On-demand fetching defeats prompt caching for that content — it would re-bill per fetch and add latency from extra tool round-trips. The front-loaded design optimizes for "many turns per session, infrequent skill firing" — which is the typical pattern.

**3. Truncation as crude tier-2.**
The current `skillListingBudgetFraction` truncation is implicitly already doing tier-1 (hot, full descriptions) vs tier-2 (cold, dropped) — it's just *dropping* the cold tier instead of replacing with index entries. So the architectural primitive (recency-ranked tiering) already exists. The improvement Daniel suggests is replacing "drop" with "demote to index."

## Where the index pattern wins (the actual case)

For users with **a long tail of rarely-used skills**, front-loading is wasteful:

- Daniel's hot-tier (fires daily): `cas-search`, `cas-task-tracking`, `cas-memory-management`, `cas-supervisor`, `cas-worker`, `cas-code-review`, `cas-brainstorm`, `fallow` — call it ~10 skills.
- Daniel's warm-tier (fires weekly): `cas-ideate`, `cas-supervisor-checklist`, `cas-playwright-debug`, `cas-seo-expert`, `cas-servers`, `codemap`, `db`, `update-config` — ~10 skills.
- Daniel's cold-tier (fires <monthly): `claude-api`, `init`, `review`, `security-review`, `keybindings-help`, `simplify`, `fewer-permission-prompts`, `loop`, `project-overview`, plus the inbox of plugin-shipped skills he hasn't pruned — 20+ skills.

For the cold tier, paying ~200 tokens per session per skill description that fires <5% of sessions is pure deadweight. A two-tier design — full descriptions for hot/warm, name + 1-liner index for cold, with a `SkillDescribe` (or similar) tool to fetch on demand — would be strictly Pareto-better:

- Total system-prompt cost drops by ~3-5k tokens for power users.
- Prompt caching still works for the always-loaded hot tier.
- Cold-skill discoverability stays intact via the index hooks.
- The fetch-on-demand cost is paid only when a cold skill plausibly matches — i.e. ~the same frequency the skill itself fires.

## Implementation sketch (if Anthropic shipped this)

```
┌─ system prompt (always loaded) ───────────────────┐
│ # Skills                                          │
│                                                   │
│ ## Hot tier (full descriptions)                   │
│ - cas-search: <full description>                  │
│ - fallow: <full description>                      │
│ - ... (8 more)                                    │
│                                                   │
│ ## Warm tier (full descriptions)                  │
│ - codemap: <full description>                     │
│ - ... (8 more)                                    │
│                                                   │
│ ## Cold tier index (use SkillDescribe to expand)  │
│ - claude-api — Build/debug Claude API + SDK apps  │
│ - init — Initialize CLAUDE.md                     │
│ - review — Review a pull request                  │
│ - ... (17 more)                                   │
└───────────────────────────────────────────────────┘

┌─ on-demand tool ──────────────────────────────────┐
│ SkillDescribe(name: "claude-api")                 │
│   → returns full frontmatter description body     │
│     so model can decide whether to invoke         │
└───────────────────────────────────────────────────┘
```

Tier assignment could be:
- **Recency-based** (current truncation heuristic, but inverted: keep cold-tier as index entries instead of dropping).
- **User-configured** (settings.json `skillTiers: { hot: [...], warm: [...] }`).
- **Auto-promoted** (a cold-tier skill that fires gets promoted to warm for the rest of the session).

The simplest first cut is **recency-based with the existing primitive** — change the truncation behavior from "drop" to "demote to index." Zero new config, zero new tools required *if* Claude is willing to invoke an existing tool (e.g. `Skill` itself, with a hypothetical `describe` action) on cold-tier matches.

## Cassy implications

Cassy itself currently has the same constraint — every Cassy skill (`cas-supervisor`, `cas-worker`, `cas-search`, etc.) ships a full frontmatter description that lands in the system prompt. As Cassy adds more skills (current count: ~15 first-party + plugin skills), the same budget pressure will hit eventually.

The design lessons from this research:
1. **Cassy skill-listing should adopt the MEMORY.md index pattern early**, before hitting Claude Code's truncation cliff. Sketch: `cas-skills.md` index always loaded, individual skill bodies fetchable via `mcp__cas__skill action=describe`.
2. **Cassy already has the on-demand-fetch primitive** — `mcp__cas__skill` exists as an MCP tool. The only missing piece is documenting that skill descriptions can be fetched lazily, and adjusting the harness's skill-loading pass to emit index entries instead of full bodies for cold skills.
3. **Trigger keywords matter.** If Cassy adopts the index pattern, the 1-line index hooks must encode enough trigger signal that Claude knows when to fetch the full description. The hook is essentially a recall query against the full description — same dynamic as the auto-memory MEMORY.md hooks, where good hooks lead to good recall.

## Recommendation

**For Claude Code (Anthropic):** file as feature request via `/feedback` or github.com/anthropics/claude-code. Framing: "skill-map index pattern, mirror how MEMORY.md works in auto-memory." Cite the existing MEMORY.md primitive as proof that the harness already supports the pattern internally.

**For Cassy:** keep an eye on this as Anthropic ships (or doesn't). If they do — adopt their convention. If they don't — design Cassy's skill-listing around the index pattern proactively, since Cassy users (especially Daniel) will hit the same wall sooner than typical Claude Code users.

**Side observation — already tracked upstream, no new ticket needed:** verified 2026-05-06 that the duplication is exactly the "loader does not dedupe across multiple `.claude/` source dirs" bug, already filed multiple times against Claude Code:

- **anthropics/claude-code#27069** — "Skills/commands appear duplicated when using git worktrees." Most general framing; their suggested fix ("Deduplicate skills by name when loading from multiple `.claude/` directories") is exactly right for our scenario. Worktrees are not required to repro — plain project + user skill dirs trigger the same bug.
- **#43003** — `~/.claude/skills/` + `~/Library/Application Support/...` variant (macOS skill-creator path).
- **#51008** — single-source dedup variant on WSL; appears to be a separate underlying bug (no second source needed).
- **#34831** — closed-stale older variant.

All open with `area:skills`, `bug`, `has repro` labels. Filing a fifth report would be noise. If the dedup fix lands upstream, it reclaims roughly half the per-session skill-listing budget for any project that uses `cas update --user`, which materially weakens (but does not eliminate) the urgency of the index-pattern proposal — a user with 60+ unique skills still trips the budget cap.

## Status

Research only. No Cassy code change proposed at this time. Pattern documented for future reference and external feedback to Anthropic.

## 2026-05-06 verification addendum

Prior to drafting any external feedback, the dup claim and the index-pattern motivation were verified end-to-end:

1. **Dup is real and self-inflicted:** all 16 Cassy skills are byte-identical between project `.claude/skills/` and user `~/.claude/skills/`. Source: `cas update --user` seeds the user-level copy so worker worktrees have a fallback when host projects gitignore the project-level dir.
2. **Already filed upstream four times** (#27069, #43003, #51008, #34831). No new ticket warranted. The fix is on Anthropic's side: dedupe skills by name during loader pass.
3. **Index-pattern proposal still stands** but with reduced urgency once #27069 lands — a fixed loader gives back roughly 2k tokens for any cas-update--user user, postponing the truncation cliff for typical Cassy users by maybe 15-20 skills' worth of headroom.

Net: the strongest immediate ask to Anthropic is "fix #27069." The index-pattern proposal is a follow-on once that's resolved and a real long-tail-skills user actually hits the new (post-dedup) cap.
