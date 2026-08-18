---
from: ozer-strong-kestrel-49 (Ozer Health repo, solo supervisor session)
date: 2026-05-18
priority: P2
cas_task: (none)
---

# Session observations — CLAUDE.md duplication, ripple-check noise, deferred-tool friction

Report from a long-running Ozer Health session (codemap refresh → frontend-dev + backend-dev skill rewrite → CLAUDE.md restructure). Not a factory run; single-agent. Every issue below is minor in isolation; aggregated they cost ~10 min of session time on context bloat and false-signal triage. Ordered by impact.

The goal of this report is concrete evidence — file paths, log excerpts, exact user-visible behavior — for issues that are otherwise easy to dismiss as user error.

---

## 1. Cassy-managed CLAUDE.md block triple-duplicates across ancestor directories (P1)

### Symptoms

Session entered Ozer Health repo and loaded **three identical Cassy-managed blocks** into context:

- `/home/pippenz/CLAUDE.md` (14 lines, CAS:BEGIN…CAS:END)
- `/home/pippenz/Petrastella/CLAUDE.md` (14 lines, byte-identical)
- `/home/pippenz/Petrastella/ozer/CLAUDE.md` (14 lines, byte-identical)

All three contain the exact same `# IMPORTANT: USE Cassy FOR TASK AND MEMORY MANAGEMENT` body, wrapped in the same `<!-- CAS:BEGIN -->` / `<!-- CAS:END -->` markers.

### Concrete evidence

```bash
$ md5sum ~/CLAUDE.md ~/Petrastella/CLAUDE.md ~/Petrastella/ozer/CLAUDE.md
<same hash three times>

$ wc -l ~/CLAUDE.md ~/Petrastella/CLAUDE.md ~/Petrastella/ozer/CLAUDE.md
14 / 14 / 14 (42 lines loaded; 28 are pure duplication)
```

Claude Code loads CLAUDE.md from cwd up to home — so the same block gets read three times into the model's context every session. The repo also has 23 sibling projects under `~/Petrastella/` (`abundant-mines`, `cas-src`, `closure-club`, `country-liberty`, ...); the duplicate at `~/Petrastella/CLAUDE.md` applies to all of them.

### Workaround applied

User authorized dedup. Manual fix:
1. Kept `~/CLAUDE.md` (user-global; applies to all projects on the machine).
2. `rm ~/Petrastella/CLAUDE.md` (purely duplicate; ancestor-of-23 saved 14 lines × N sessions).
3. Stripped `<!-- CAS:BEGIN -->…<!-- CAS:END -->` from `~/Petrastella/ozer/CLAUDE.md`, kept project-specific content appended after the marker.

Result: 50 lines load per Ozer session (was ~79 with the dup).

### Likely root cause

Cassy injects the managed block via `cas hook UserPromptSubmit` (or session-start equivalent). The injection logic likely doesn't check whether an ancestor CLAUDE.md already contains the managed block before writing to the project-level file. The same logic ran when Cassy initialized `~/Petrastella/CLAUDE.md` and `~/Petrastella/ozer/CLAUDE.md` separately.

### Proposed fix

- **(a)** Before injection at a project-level CLAUDE.md, walk up to `$HOME` and detect if any ancestor already contains a `<!-- CAS:BEGIN -->` block. If yes, skip injection at the deeper level.
- **(b)** Or: provide a `cas config set hooks.managed_claude_md.scope = user|project|both` knob with `user` as the default (user-global is enough for the rule "use mcp__cas__task instead of TaskCreate" — it's not project-specific).
- **(c)** Or: a `cas claude-md doctor` subcommand that walks the CLAUDE.md ancestor chain, reports duplicates, and offers `--fix` to dedup.

**Cost today:** 28 wasted context lines per session × every CC session on this machine. Across daily use, non-trivial.

---

## 2. `mcp__cas__task` is a deferred tool — must be ToolSearch'd before first use (P1 UX gap)

### Symptoms

The Cassy-managed CLAUDE.md says repeatedly:

> **DO NOT USE BUILT-IN TOOLS (TodoWrite, EnterPlanMode) FOR TASK TRACKING.**
> Use CAS MCP tools instead:
> - `mcp__cas__task` with action: create

But `mcp__cas__task` is listed in the **deferred-tools** registry, not the eagerly-loaded tool set. To actually invoke it, Claude has to first run:

```
ToolSearch(query="select:mcp__cas__task", max_results=1)
```

…which returns the JSONSchema, registers the tool, and only THEN allows invocation. Net cost: one extra tool call before the first task action.

### Concrete evidence

Today's session hit this twice:
1. After deciding to track the codemap/skill-rewrite work via Cassy tasks, I called `ToolSearch(query="select:mcp__cas__task")` at ~14:30 UTC to load the schema before the first `mcp__cas__task action=create`.
2. Again after the ripple-check fired with task IDs, I needed `mcp__cas__task action=show` to look them up — but the tool wasn't loaded yet in that turn either (parent context vs branch context drift), required another ToolSearch.

### Why this matters

The whole point of the Cassy-managed CLAUDE.md block is to deflect Claude away from the harness's built-in TaskCreate/TodoWrite (which fires its own reminder; see #3). But the deflection-target tool is **harder** to reach than the built-in. Net friction:

| Path | Steps to first task created |
|---|---|
| Harness default (`TaskCreate`) | 1 (just call it) |
| Cassy-recommended (`mcp__cas__task`) | 2 (ToolSearch, then call) |

Combine that with #3 (built-in reminder firing every PostToolUse) and the friction gradient is the wrong direction.

### Proposed fix

This is mostly a Claude Code harness decision (eager vs deferred MCP tool loading), but Cassy can influence it:

- **(a)** Document in the Cassy-managed CLAUDE.md block the exact ToolSearch query Claude should run on first task-tracking intent: `ToolSearch(query="select:mcp__cas__task,mcp__cas__memory,mcp__cas__search")`. Saves Claude from inferring the syntax.
- **(b)** Provide a tiny eagerly-loaded shim skill (`cas-mcp-bootstrap`?) whose only job is to pre-fetch the CAS MCP tool schemas via ToolSearch at session start. Users could opt-in via `cas hook configure --eager-mcp`.
- **(c)** Upstream: file a Claude Code request for "pin MCP tools listed in CLAUDE.md to non-deferred" — but that's not Cassy's lane.

**Cost today:** ~2 extra ToolSearch calls in this session. Multiply by every session that does any task tracking.

---

## 3. Built-in Claude Code `TaskCreate` reminder fires repeatedly despite Cassy-managed CLAUDE.md telling Claude not to use it (P2 upstream, but Cassy-adjacent)

### Symptoms

Throughout the session, the Claude Code harness injected this system-reminder ~10 times:

> The task tools haven't been used recently. If you're working on tasks that would benefit from tracking progress, consider using TaskCreate to add new tasks and TaskUpdate to update task status (set to in_progress when starting, completed when done). Also consider cleaning up the task list if it has become stale. Only use these if relevant to the current work. This is just a gentle reminder - ignore if not applicable.

The reminder is **hardcoded in the Claude Code binary** at `/home/pippenz/.local/share/claude/versions/2.1.141`:

```
$ strings /home/pippenz/.local/share/claude/versions/2.1.141 | grep "task tools haven't"
The task tools haven't been used recently. If you're working on tasks
that would benefit from tracking progress, consider using ${QE} to add
new tasks and ${hI} to update task status ...
```

`${QE}` resolves to `TaskCreate`, `${hI}` to `TaskUpdate` — substituted at runtime. The harness doesn't read CLAUDE.md to decide whether to fire.

### Why this is a Cassy-adjacent issue (not just upstream)

The Cassy-managed CLAUDE.md block is a defection-rule for this exact reminder. Cassy users *expect* the rule to be load-bearing. Today every fire of the built-in reminder is also evidence that the rule isn't doing what users assume — Claude just sees the nudge and ignores it (correctly per CLAUDE.md), but the user sees the nudge **fire anyway** and assumes their config is broken.

### Workaround applied

Confirmed via grep that the reminder is harness-baked, not Cassy-emitted. No action taken; the behavior is upstream.

### Proposed fix

- **(a)** **Cassy-side** (defense in depth): the existing `cas hook PostToolUse` could pattern-strip these specific reminders from injected context. Risk: fragile against any wording change in CC releases; adds latency to every tool call. Likely not worth it.
- **(b)** **Upstream**: Cassy team files a Claude Code feature request for a settings.json knob like `disableBuiltInTaskReminders: true` or `taskTools: { provider: "mcp__cas__task" }` so the harness skips the reminder when an MCP provider claims the task surface. (Cassy reaching out to Anthropic on behalf of users is more leveraged than each user filing individually.)
- **(c)** **Documentation**: add a FAQ entry to `cas-src/docs/` explicitly explaining "you'll still see the built-in reminder; here's why, here's that it's harmless, here's the upstream request to follow."

**Cost today:** ~10 system-reminder injections × ~80 tokens each = ~800 tokens of context noise. Cosmetic, but erodes user trust in the Cassy rule.

---

## 4. Ripple-check fires on substring match across projects, false-positives on common filenames (P2)

### Symptoms

After editing `/home/pippenz/Petrastella/ozer/CLAUDE.md` (Ozer project), got this ripple-check reminder:

```
Ripple check: `CLAUDE.md` is referenced in task(s) cas-cd6a, cas-70e5, cas-4135.
Verify the parent epic/spec descriptions are still consistent with your changes.
```

All three referenced tasks are about the **cas-src repo's** CLAUDE.md, not Ozer's:

- `cas-cd6a` — "Open staging→main PR with internal changelog" (cas-src release PR, mentions "Explicit repo flag per CLAUDE.md")
- `cas-70e5` — "Empirical bisect of Ink crash pattern" (refers to "CLAUDE.md output-hygiene guidance + codemap SKILL rewrite" landed in cas-src)
- `cas-4135` — "Factory worktrees share CARGO_TARGET_DIR" (mentions "Documented in CLAUDE.md or CONTRIBUTING.md so humans know")

None of them reference the Ozer CLAUDE.md I was editing. The match is on the literal string `CLAUDE.md` regardless of which repo or which file.

### Why this is friction

- Forces the agent (me) to look up each referenced task to verify it's not actually impacted. ~3 tool calls of overhead per ripple-check fire.
- Cried-wolf erodes signal: next time a ripple-check fires legitimately, I'm primed to dismiss it.
- The fix would be lightweight — most filenames in task descriptions are referenced with a path prefix or repo context. Substring-only is the lazy match.

### Proposed fix

- **(a)** Make ripple-check **path-aware**: if a task body references `CLAUDE.md` without a path qualifier, assume it's the task's own repo (the task's `project_root` field or equivalent); only match if the edited file's repo matches the task's repo.
- **(b)** Or: scope ripple-checks to tasks within the **current project** by default. Cross-project ripple is a niche case (changes in shared infra) that should be opt-in.
- **(c)** Or: at minimum, surface the **repo** of each referenced task in the ripple-check message — `cas-cd6a (cas-src)`, `cas-70e5 (cas-src)`, `cas-4135 (cas-src)` — so the human / agent can dismiss cross-repo references instantly without a lookup.

**Cost today:** 1 ripple-check fire, 3 task lookups to confirm cross-project. ~30 seconds + token cost. Aggregate across daily use, real.

---

## 5. `cas codemap status` disagrees with SessionStart hook's freshness banner (P3)

### Symptoms

Session entered Ozer Health repo. SessionStart hook injected:

```
<codemap-freshness severity="high">
CODEMAP.md is significantly out of date (84 structural changes):
+docs/plans/2026-04-28-bundle-product-matrix.md,
+apps/frontend/components/provider/ProviderWorkCalendarGrid.vue,
+apps/frontend/tests/provider-work-calendar-grid.test.ts, ...
(+74 more). Run `/codemap` to update before assigning work.
</codemap-freshness>
```

But `cas codemap status` reported:

```
CODEMAP.md: /home/pippenz/Petrastella/ozer/.claude/CODEMAP.md
  Last updated: 2026-05-07 18:41:35 +0000
  Status: up to date
```

Two Cassy-emitted signals, conflicting interpretations of the same file state.

### Why the two paths differ (hypothesis)

- The SessionStart hook computes staleness from git diff since CODEMAP.md was last modified.
- `cas codemap status` reads `.cas/codemap-pending.json` — which is the *pending-changes ledger*, separate from git history.
- If `.cas/codemap-pending.json` was cleared (manually or by an earlier session) but commits landed after, the two sources disagree.

### Workaround applied

Ran `/codemap` to regenerate `.claude/CODEMAP.md`, then `cas codemap clear` to reset the pending counter. Both signals then agreed (`Status: up to date`, no banner on subsequent sessions).

### Proposed fix

- **(a)** Single source of truth: `cas codemap status` should compute the same staleness signal the SessionStart hook does, OR vice versa. Today they're divergent.
- **(b)** Auto-clear: if CODEMAP.md `mtime` is newer than the pending-changes ledger's first entry, `cas codemap status` could auto-reconcile.
- **(c)** Document the relationship between the two so users know which to trust when they disagree.

**Cost today:** ~2 min figuring out which signal was canonical and confirming the regen had taken effect.

---

## 6. The `cas codemap clear` step is easily forgotten after `/codemap` regen (P3)

### Symptoms

The `/codemap` skill at `~/.claude/skills/codemap/SKILL.md` documents the clear step at the very end:

> ### 2. Clear the freshness counter
> Run: `cas codemap clear`
> Skipping this step means the next session keeps blocking worker dispatch with "Run `/codemap` to refresh" even though the doc was just refreshed.

This is correct documentation. But "easy to skip" is observable: the prior session that produced the stale CODEMAP at 2026-05-07 (per `status` output) presumably ran `/codemap` but didn't clear, leading to today's banner-vs-status disagreement (#5).

### Proposed fix

- **(a)** Make `/codemap` (the slash command / skill) **idempotent**: after writing CODEMAP.md, automatically run `cas codemap clear`. The skill could shell out to the binary as its last step. Removes the human-forgot-step failure mode entirely.
- **(b)** Or: make `cas codemap status` detect "CODEMAP.md mtime is newer than the pending ledger" and self-clear with a log note.

Net effect either way: the user/agent should never have to manually invoke `cas codemap clear` — regen implies clear.

**Cost today:** N/A on this session (I ran the clear correctly), but the prior session's omission is what produced #5.

---

## Summary for triage

| # | Issue | Severity | Cost today | Affects |
|---|---|---|---|---|
| 1 | Cassy-managed CLAUDE.md triple-duplicates across ancestors | P1 | 28 wasted lines / session | every CC session on this machine |
| 2 | `mcp__cas__task` deferred-tool friction | P1 UX | ~2 extra tool calls | every session that tracks work |
| 3 | Built-in `TaskCreate` reminder fires anyway (upstream + Cassy-adjacent) | P2 | ~800 tokens cosmetic noise | every CC session, harness-level |
| 4 | Ripple-check substring-match cross-project false positives | P2 | ~30s + 3 lookups per fire | multi-repo workflows |
| 5 | `cas codemap status` disagrees with SessionStart banner | P3 | ~2 min triage | sessions inheriting stale ledger |
| 6 | `/codemap` regen doesn't auto-`cas codemap clear` | P3 | None today; produces #5 next session | every codemap regen |

**Total session friction:** ~5 min of confused triage, ~1000 tokens of context noise across 10+ system-reminder fires, and a permanent ~28-line context tax until the dedup workaround landed.

Most impactful single fix: **#1** (CLAUDE.md ancestor dedup). Quickest theoretical win: **#6** (auto-clear on regen).

Happy to clarify or produce additional evidence on any of the above — the session transcript is at `/home/pippenz/.claude/projects/-home-pippenz-Petrastella-ozer/42054b46-268d-493f-9e40-356ecfbf3a61/`.
