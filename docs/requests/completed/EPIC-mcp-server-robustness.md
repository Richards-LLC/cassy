---
from: cas-src-crisp-dolphin-10 supervisor (subtle-bear-27)
date: 2026-04-16
priority: P0
related: BUG-team-memories-never-populate.md (blocked by this)
branch: epic/mcp-server-robustness
---

# EPIC: `cas serve` must survive handler failures

## Problem

`mcp__cas__search` deterministically crashes `cas serve`. Every non-search Cassy tool works; every search call kills the server. Claude Code auto-respawns the process, but the model sees `MCP error -32000: Connection closed` and eventually gets a `MCP server disconnected` system reminder — at which point Cassy task creation, memory recall, and skill lookup are unavailable for the rest of the session.

### Evidence

From `~/.cache/claude-cli-nodejs/-home-pippenz-Petrastella-cas-src/mcp-logs-cas/2026-04-16T14-45-04-974Z.jsonl`:

```
14:59:35  search         → Connection closed (1s)
14:59:49  search retry   → Connection closed (9s, "cleanly")
15:53:37  coordination   → 16ms  ✓
15:53:37  task ×2        →  7ms  ✓
15:53:37  search         → Connection closed (1s)
15:53:42  [auto-respawn]
15:53:44  search retry   → Connection closed (9s, "cleanly")
```

No coredump. No entry in `dmesg`. The "cleanly" part of the disconnect message suggests `std::process::exit()` somewhere in the search path, or a tokio task panic unwinding without reaching stderr.

### Scope of the fix

Two independent problems:

1. **Server dies on any handler failure.** A panic in any tool — not just search — takes down the whole process. Every other tool is one unwrap away from the same fate.
2. **The search crash itself.** Root cause unknown because crash output isn't preserved anywhere. Stderr reaches the Claude Code jsonl only up to the `[Cassy] Starting MCP server...` line; nothing after that.

## EPIC goal

`cas serve` survives any single tool-handler panic and returns a structured `INTERNAL_ERROR` to the client instead of exiting. The `search` regression is fixed and covered by a test that would have caught it.

## Out of scope

- MCP client-side auto-reconnect behavior (that's Claude Code; it already works).
- Broader supervisor/worker resilience (separate surface area).
- The team-memories bug (blocked by this EPIC — the EPIC planning session itself couldn't create Cassy tasks because search crashed).

---

## Subtasks

### A1 — Tee `cas serve` stderr to `~/.cas/logs/cas-serve-{date}.log`

**Why first:** B1 (reproduce crash) needs this to capture panic output. A2 (panic catcher) needs this to verify the catcher worked.

**Demo:** Run `cas serve` in a terminal, trigger any stderr write. File exists at `~/.cas/logs/cas-serve-2026-04-16.log` with the same content that reaches Claude Code's jsonl, plus timestamps + PID + agent_id (if set).

**Acceptance:**
- Log lines include `ts pid=N agent=<name> <message>` prefix
- Existing stderr→Claude Code path still works (init banner appears in both places)
- Rotates daily like `cas-YYYY-MM-DD.log` already does
- No regression in `cas serve` startup time

**Files likely touched:** `cas-cli/src/mcp/server/runtime.rs`, possibly a new `cas-cli/src/mcp/server/log.rs`.

---

### A2 — Panic catcher around every tool dispatch method

**Why:** A handler panic should become `McpError { INTERNAL_ERROR, message: "..." }`, not process death. Note: `std::panic::catch_unwind` does not cross `.await` points — use `tokio::spawn(...).await` and match `JoinError::is_panic()`, or install a panic hook that logs + aborts the task future.

**Scope:** All 11 top-level tool methods in `cas-cli/src/mcp/tools/service/mod.rs` — `search`, `task`, `coordination`, `memory`, `rule`, `skill`, `spec`, `system`, `team`, `verification`, `pattern`. Centralize via a helper, don't hand-wrap each.

**Demo:** Insert a test-only handler that calls `panic!("boom")`. Client receives `INTERNAL_ERROR: handler panicked: boom`. Server keeps serving.

**Acceptance:**
- One helper (`dispatch_with_catch` or similar) used by every tool method
- 10 consecutive panics do not kill the server
- Panic message + location appear in the A1 log file
- No new tokio version bump required

**Files likely touched:** `cas-cli/src/mcp/tools/service/mod.rs`, new helper module.

---

### A3 — Regression test for panic isolation

**Demo:** Test that spawns a `CasService` with a panic-injecting stub tool, issues a tool call, asserts (a) error is returned to caller, (b) next tool call succeeds against the same server instance.

**Acceptance:** Test lives in `cas-cli/src/mcp/tools/service/` test tree, runs in default `cargo test`, would fail if A2's catcher were removed.

**Depends on:** A2.

---

### B1 — Reproduce the search crash standalone

**Demo:** A script or documented command that starts `cas serve` with stdio wired to a shell, sends a `tools/call` JSON-RPC for `search` action=search, and reliably triggers the crash observed today. Artifact lands in `cas-cli/tests/regression/mcp_search_crash.md` or similar.

**Acceptance:**
- Reproducer runs pre-fix and produces the same "Connection closed after 9s cleanly" signature
- Output of A1 log captures the panic or exit cause
- Clear enough that a worker picking up B2 can start from it

**Depends on:** A1 (log must exist to see the crash cause).

---

### B2 — Fix the search crash root cause

**Scope:** wherever B1 points. Likely candidates (in order of suspicion):
1. `search.search_unified()` in `crates/cas-search/` — Tantivy query parse panic on some query shape.
2. One of the per-result `store.get()` calls in `cas-cli/src/mcp/tools/core/search.rs:90-216` — corrupted index row that doesn't deserialize.
3. Some code-symbol deduplication path introduced recently.

**Demo:** Reproducer from B1 exits normally, `search` returns results.

**Acceptance:**
- Root cause documented in commit message
- No unrelated changes bundled in
- A2's panic catcher remains — this fix should not rely on the catcher masking the bug

**Depends on:** B1.

---

### B3 — Automated regression test for the search crash

**Demo:** Fold B1's reproducer into an integration test under `cas-cli/tests/`. Runs in `cargo test`.

**Acceptance:**
- Test fails if B2 is reverted
- No network, no cloud auth required
- Test runtime < 30s

**Depends on:** B2.

---

## Dependencies & ordering

```
A1 ──┬── A2 ── A3
     └── B1 ── B2 ── B3
```

## Worker plan

- **Worker 1:** A1 → A2 → A3 (server robustness track)
- **Worker 2:** starts after A1 lands on epic branch → B1 → B2 → B3 (search fix track)

Or one serial worker A1→A2→B1→B2→A3→B3 if coordination overhead dominates.

## Done criteria for the EPIC

1. Any single handler panic returns `INTERNAL_ERROR` to the client; server process survives.
2. Today's search crash is reproduced in a test and no longer crashes on main.
3. `~/.cas/logs/cas-serve-*.log` exists and captures panics.
4. The team-memories EPIC (`BUG-team-memories-never-populate.md`) can proceed — CAS MCP is reliable again.
