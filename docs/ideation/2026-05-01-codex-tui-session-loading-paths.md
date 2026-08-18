---
date: 2026-05-01
topic: codex-tui-session-loading-paths
focus: paths forward to load Codex CLI sessions in Cassy the way Claude Code sessions load today
scope: Codex + Claude Code as the only two supported harnesses (no OpenCode, no Cursor)
related: 2026-04-28-cas-harness-portability-ideation.md (different second-harness scope — OpenCode), 2026-05-01-codex-cli-empirical-reference.md, 2026-05-01-codex-session-format-spec.md
---

# Loading Codex Sessions in Cassy — Paths Forward

User-stated goal: "the preferred state is that it actually loads the Codex sessions in the TUI, like we do with Claude today."

**Scope:** Codex + Claude Code as the only two supported harnesses. No OpenCode, no Cursor.

The 2026-04-28 ideation explored OpenCode as the second harness — that scope has been **superseded** by this Codex-only choice. Its strategic ideas (versioned capability manifest, conformance suite, Cassy-canonical hook taxonomy, `.cas/` projections, Cassy-owned permission engine) remain relevant as patterns and inform sequencing here, but the specific OpenCode adapter work is out of scope. The existing `cas-mux/harness.rs:5-8` `SupervisorCli::{Claude, Codex}` enum already matches this final scope — no enum extension or removal needed.

This doc captures Cassy's current Claude-session surface, identifies the change-surface for Codex support, lays out three paths forward with tradeoffs, and recommends sequencing.

## What Cassy actually does with Claude sessions today

Important framing correction: **Cassy does not have a dedicated session-list TUI**. There's no "session viewer" the way the user's phrasing suggests. What Cassy actually does:

1. **Agents-as-sessions** (1:1 mapping): `crates/cas-types/src/agent.rs` carries `cc_session_id` per agent. The factory TUI (`cas-cli/src/ui/factory/`) renders agent status, heartbeat, role — not session content directly.

2. **Hook-driven ingestion**: `crates/cas-core/src/hooks/transcript.rs` parses Claude Code JSONL lines into `TranscriptEntry { entry_type, message, uuid, timestamp, version }`. Driven by `SessionStart`/`PostToolUse`/`SessionEnd` hooks Claude Code fires. Claude-specific fields are hardcoded: `uuid`, `timestamp`, `version` (e.g., `"2.1.14"`), `model` (e.g., `"claude-opus-4-7-20251101"`).

3. **On-demand transcript resolution**: `cas-cli/src/mcp/tools/service/factory_ops.rs:1059-1187` globs `~/.claude/projects/*/<session-id>.jsonl` to locate the JSONL for a given session UUID. Returns `TranscriptResolution::{Resolved, Synthesized, Ambiguous}`.

4. **Search index**: `crates/cas-search/` (BM25) + `cas-cli/src/hybrid_search/mod.rs` (BM25+semantic). Reactive: rebuilt by `cas-cli/src/daemon/maintenance.rs` periodic scan + on hook events. **No live file-watcher on Claude Code JSONL** — Cassy reacts to hook events at transaction boundaries, not file mutations.

5. **Render path**: `cas-cli/src/ui/markdown/renderer.rs` is a generic Markdown renderer (not Claude-specific). Tool-call rendering flattens `ContentBlock::ToolUse { id, name, input }` into Markdown.

6. **Partial harness abstraction (already exists, partially stale)**: `crates/cas-mux/src/harness.rs` has:
   ```rust
   pub enum SupervisorCli { Claude, Codex }
   pub struct HarnessCapabilities {
       pub supports_hooks: bool,
       pub supports_subagents: bool,
       pub supports_textbox_submit: bool,
       pub tool_prefix: &'static str,
   }
   ```
   Per `harness.rs:26-31`, `Codex` is declared `supports_hooks: false, supports_subagents: false, supports_textbox_submit: false` — **all three are wrong** for current Codex (see companion empirical reference doc, sections 4-5).

7. **Hardcoded "claude" surfaces in session pipeline:**
   - `agent_id.rs:6` — agent ID prefix `"cc-{ppid}-{hash}"` ("cc" = Claude Code).
   - `factory_ops.rs:1059-1187, 1185-1187` — hardcoded `~/.claude/projects` + Claude session ID UUID assumptions.
   - `hooks/transcript.rs:14-55` — Claude-specific JSONL parser fields.
   - `harness_policy.rs:29-44` — `parse_harness()` exists but downstream code assumes `Claude` semantics.

8. **No `trait SessionStore` / `HarnessAdapter` for the session-pipeline side.** The mux/harness abstraction stops at "which CLI to launch"; the session-reader path is fully Claude-shaped below it.

## What "load Codex sessions in the TUI" means concretely

Three things, in increasing order of ambition:

**(a) Read** — enumerate Codex sessions, parse rollout JSONL into a Cassy-internal session representation, surface them alongside Claude sessions in whatever the factory TUI / `cas list` exposes. Read-only. Today's Claude session surface is already mostly read-only at the TUI layer (transcript_path resolution + render).

**(b) Hook-drive** — Codex hooks fire on `SessionStart`/`PostToolUse`/`SessionEnd` exactly like Claude (same wire format), so `cas hook` invocations from Codex would update the same agent registry, search index, memories, tasks. This requires Codex-side install of Cassy hooks (writing `~/.codex/config.toml` `[[hooks.SessionStart]]` blocks pointing at `cas hook SessionStart`).

**(c) Drive** — full factory orchestration on Codex: `cas factory` launches Codex panes (workers + supervisor), enforces the permission/rules surface, runs the EPIC workflow. This is the largest scope and depends on (a) and (b) plus capability-matrix correctness in `cas-mux`.

User's stated goal is closest to (a) with (b) as an obvious follow-on. (c) is the destination implied by the 2026-04-28 ideation but not necessarily blocking the user-visible feature.

## Three paths forward

### Path A — Minimal read-only adapter behind a feature flag

**Shape:**
- New crate `cas-session-codex` containing the ~150 LOC of serde structs from companion spec doc, plus `enumerate(codex_home) -> Vec<SessionSummary>` + `read(path) -> Result<SessionStream>` + tail-watcher.
- New `trait SessionStore` in `cas-core` with two implementations: `ClaudeSessionStore` (extracted from existing `factory_ops.rs:1059-1187` + `hooks/transcript.rs`) and `CodexSessionStore`.
- Common abstract event type (`HarnessEvent`-shaped, similar to idea #4 in 2026-04-28 ideation but scoped only to session-content events, not hook events).
- TUI surface: factory list shows sessions from both stores, identified by harness icon/badge. `cas-cli list` accepts `--harness claude|codex|all`.
- **Don't touch** the `cas-mux/harness.rs` capability matrix or hook handlers. Don't touch `agent_id.rs` prefix. Don't refactor `harness_policy.rs`.

**Pros:**
- Smallest scope. ~1-2 weeks.
- Proves the abstraction shape with two real backends before committing to a manifest design.
- Surfaces the user-visible feature (Codex sessions in TUI) on its own milestone.
- Doesn't touch the freshly-stabilized permission code (`959e69b`, 2026-04-27).
- Compatible with all five 2026-04-28 ideation paths — none of them are foreclosed.

**Cons:**
- Doesn't address the broader portability story; Cassy still has hardcoded Claude surfaces below the TUI layer.
- Two stores end up implementing similar logic before the eventual abstraction emerges from the design — risks getting locked in to wrong shape.
- Doesn't update the stale `cas-mux/harness.rs` capability matrix; readers downstream of that matrix will still see `Codex { supports_hooks: false }` and skip Codex hook integration.
- The "session content event" abstraction will partially overlap with the eventual `HarnessEvent` enum (idea #4 in 2026-04-28). Some rework when that lands.

**Risk class:** Medium. Real risk is locking in a session abstraction that fights the eventual hook-event abstraction.

### Path B — Sequenced via foundation work first; session loading is Phase 4

**Shape:**
- Defer all session-loading work until the foundation work lands. The 2026-04-28 ideation laid out a sequence (conformance suite → manifest → hook taxonomy → projections → permission engine) — borrow that sequencing pattern with Codex as the second harness target instead of OpenCode:
  1. Conformance suite + fake-harness binary (proves the Codex variant in `cas-mux` is real, not vaporware).
  2. Versioned capability manifest (the contract; replaces the stringly-typed `HarnessCapabilities` struct).
  3. Cassy-canonical hook event taxonomy (handlers stop being Claude-shaped).
  4. NEW: session loading lands here, on top of the manifest + hook taxonomy. The session abstraction declares its shape via the manifest (`session_format: claude_jsonl | codex_rollout`); the parser is a manifest-driven dispatch.

Since the harness set is closed at two (Claude + Codex), the manifest doesn't need to support arbitrary third parties — simplifies the v1 schema design relative to the 2026-04-28 framing.

**Pros:**
- Coherent end-state. No partial abstractions to refactor later.
- Forces the capability matrix and hook handlers to be honest before session loading depends on them.
- Conformance suite catches stale `cas-mux/harness.rs:26-31` claims automatically.
- Single canonical project doc emission story covers `AGENTS.md` natively.

**Cons:**
- Long calendar time before user-visible Codex session loading lands. The user just stated the preferred end-state; gating it behind 3-4 weeks of foundation work is a hard sell.
- Conformance suite could surface "Codex variant is vaporware" — pivot risk per idea #3's stated downside.
- Risks "ideal abstraction never ships" — over-engineering before the second backend is fully understood.

**Risk class:** Low technical, high schedule.

### Path C — Hybrid: ship Path A + concurrent capability-matrix audit

**Shape:**
- Path A as primary deliverable (read-only Codex session adapter behind a flag).
- In parallel, a focused PR that **only** updates `cas-mux/harness.rs:26-31` to reflect 2026 Codex reality (`supports_hooks: true, supports_subagents: true, supports_textbox_submit: true` — verify last with empirical test). Add explicit version-pinning comment citing Codex 0.128.0.
- Add a `cargo test` that exercises the matrix against fixture session files for both Claude and Codex — early seed for the conformance suite (idea #3 in 2026-04-28).
- Document the session abstraction shape (the new `trait SessionStore`) in a follow-up RFC marked as "v0, expected to evolve when manifest (idea #2) lands."

**Pros:**
- Ships user-visible feature on near-term timeline (~2 weeks).
- Closes the immediate stale-claim bug in `cas-mux/harness.rs` without taking on the full manifest refactor.
- Provides concrete fixtures + tests that feed directly into an eventual conformance suite.
- Documents the "this is v0" expectation explicitly so future refactor is uncontroversial.

**Cons:**
- Capability-matrix update is small but non-zero risk — could surface latent code paths that branched on `Codex { supports_hooks: false }` and silently skipped Codex hook integration. Audit needed before flipping.

**Risk class:** Medium. Recommend.

## Recommended sequencing

1. **Audit pass (1-2 days):**
   - Grep for every site that reads `HarnessCapabilities` in `cas-cli/` and `crates/`. Document which currently special-cases Codex due to the false `supports_hooks: false` claim.
   - Verify Codex's `supports_textbox_submit` empirically against a real Codex 0.128 process (this one wasn't researched).
   - Confirm the user is OK with the partial-abstraction shape implied by Path C (versus full Path B).

2. **Path C work (~2-3 weeks):**
   - **Week 1:** New crate `cas-session-codex`. Vendor the ~150 LOC of serde structs from companion spec doc. Implement `enumerate()` + `read()` against `~/.codex/sessions/`. Unit tests against captured rollout JSONL fixtures (commit a few real ones; run user's existing Codex sessions through them).
   - **Week 2:** Define `trait SessionStore` in `cas-core`. Move existing Claude session code (`factory_ops.rs:1059-1187`, parts of `hooks/transcript.rs`) behind the trait as `ClaudeSessionStore`. Implement `CodexSessionStore` against the new crate. TUI surface wiring (factory list, `cas list --harness`).
   - **Week 3:** Capability-matrix audit + correction. Flip `supports_hooks` and `supports_subagents` to `true` after auditing every reader. Add Codex-version pin comment. Add cargo test exercising matrix against fixture files. Documentation RFC for `SessionStore` v0.

3. **Defer to follow-on foundation work:**
   - Hook-driving Codex (Path B's "(b) Hook-drive" goal) — depends on a Cassy-canonical hook event taxonomy (per 2026-04-28 idea #4). Without that, every Codex hook handler is a fork of every Claude handler.
   - Full factory orchestration on Codex — depends on a versioned manifest + conformance suite (per 2026-04-28 ideas #2 and #3).
   - Permission engine work (per 2026-04-28 idea #5) — orthogonal, independent timeline.

## Open questions to resolve before starting

1. **Sub-agent representation.** Codex sub-agents = separate files with `parent_thread_id` linkage; Claude sub-agents = same file with `isSidechain: true`. Does Cassy's session abstraction expose "session" or "session graph" as the top-level entity? If "session" only, sub-agent sessions appear as siblings + a `parent_id` field. If "graph," parent-child relationships are first-class and the TUI renders trees. Recommendation: start with flat list + `parent_id` field, defer graph view until requested.

2. **Real-time vs reactive.** Cassy's existing Claude reader is reactive (hook-driven). Should the Codex reader follow the same pattern (require Codex hooks installed, react to events) or add live tail-watching of rollout JSONL? Tail-watching is straightforward (recorder flushes per line) but adds a daemon-style component. Recommendation: start with periodic enumeration on TUI refresh; tail-watching is follow-on if needed.

3. **Hook installation.** Even for read-only Path A/C, surfacing Codex sessions in the live agent registry probably requires Cassy hooks installed in Codex (`~/.codex/config.toml` `[[hooks.SessionStart]]` etc.) so that `cas hook SessionStart` registers the agent the same way Claude's SessionStart does. This blurs into Path B's "(b) Hook-drive" — the line between "read sessions" and "track sessions" depends on whether hooks are installed. Recommendation: separate the two surfaces. Read sessions from disk works without hooks. Live agent registration in factory TUI requires hooks. Path C delivers both at the same release, with a `cas integrate --harness codex` step writing the hook config.

4. **Tool-prefix policy.** `cas-mux/harness.rs:30` declares `tool_prefix: "mcp__cs__"` for Codex (vs `"mcp__cas__"` for Claude). Per 2026-04-28 idea #2 downsides, this swap is incomplete and conflicts with Codex's MCP namespacing. Recommendation: make the prefix uniform (`mcp__cas__`) and remove the Codex-specific variant. Codex doesn't need a different prefix; the existing rationale for the swap is unclear.

5. **AGENTS.md emission.** The `.cas/` projections idea (per 2026-04-28 idea #1) covers this generically. Path C should NOT take on AGENTS.md emission — defer to projection work. Cassy users on Codex can manually maintain `AGENTS.md` (or list `CLAUDE.md` in `project_doc_fallback_filenames`) for now.

## Files that will change

For Path C, the change-surface (commit-by-commit ordering):

| Crate / file | Change |
|---|---|
| New `crates/cas-session-codex/` | New crate. Serde structs + enumeration + tail-watcher |
| `crates/cas-core/src/session/` (new module) | `trait SessionStore`, common abstract event types |
| `cas-cli/src/mcp/tools/service/factory_ops.rs:1059-1187` | Refactor `~/.claude/projects` glob into `ClaudeSessionStore::resolve` |
| `crates/cas-core/src/hooks/transcript.rs:14-55` | Either generalize `TranscriptEntry` or move behind `ClaudeSessionStore` |
| `cas-cli/src/cli/list.rs` | Add `--harness` flag |
| `cas-cli/src/ui/factory/` | Render harness badge per agent; surface Codex sessions in list |
| `crates/cas-mux/src/harness.rs:26-31` | Capability matrix correction (after audit pass) |
| `cas-cli/src/integrate/` (or similar) | New `cas integrate --harness codex` writing TOML keep-blocks for `~/.codex/config.toml` MCP server + hooks |
| `tests/fixtures/codex-sessions/` | Captured rollout JSONL fixtures |
| `docs/architecture/session-loading.md` | RFC documenting `SessionStore` v0 and explicit "this evolves when manifest lands" stance |

## Cross-references

- What Codex actually exposes (extensibility surfaces, MCP, hooks, plugins): [2026-05-01-codex-cli-empirical-reference.md](./2026-05-01-codex-cli-empirical-reference.md)
- Parser-ready spec for Codex rollout JSONL: [2026-05-01-codex-session-format-spec.md](./2026-05-01-codex-session-format-spec.md)
- Prior portability framing (different scope — OpenCode as second harness, now superseded): [2026-04-28-cas-harness-portability-ideation.md](./2026-04-28-cas-harness-portability-ideation.md)

## Session log

- **2026-05-01:** Initial paths-forward doc. Three research streams executed: Codex extensibility surfaces, source-level reconnaissance, community/web ecosystem patterns — combined into companion empirical reference. Plus Cassy TUI session-loading audit + Codex session format reverse-engineering. User goal clarified: "actually load Codex sessions in TUI like Claude today." Scope clarified: **Codex + Claude only** (no OpenCode, no Cursor); 2026-04-28 OpenCode framing superseded. Three paths laid out (A: minimal read-only, B: gated on foundation work, C: hybrid). **Recommended Path C** with 2-3 week timeline. Five open questions documented. No path selected for execution yet — user picks up from here.
