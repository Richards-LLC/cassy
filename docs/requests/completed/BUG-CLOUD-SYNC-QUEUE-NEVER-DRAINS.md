# Bug: Cloud Sync Queue Never Drains in Normal Usage

**From:** Petra Stella Cloud team
**Date:** 2026-04-12
**Priority:** P1 — sync queues are backed up across all projects, cloud data is stale
**Affected code:** `cas-cli/src/mcp/daemon.rs`, `cas-cli/src/cloud/syncer/push.rs`

---

## Problem

The embedded daemon's cloud sync loop has two gates that must align for a push to fire:

```rust
// daemon.rs:387-388
_ = cloud_sync_interval.tick() => {
    if self.cloud_syncer.is_some() && self.activity.is_idle() {
```

1. **Timer gate:** fires every `cloud_sync_interval_secs` (default: 300s / 5 min)
2. **Idle gate:** `activity.is_idle()` requires `min_idle_secs` (default: 60s) with no MCP tool calls

In practice, the idle window almost never opens:

| Scenario | Outcome |
|----------|---------|
| Active Claude Code session | MCP tools called every few seconds | never idle | **no sync** |
| Factory session (supervisor + workers) | constant MCP traffic across agents | **no sync** |
| User pausing to think | must be 60s+ AND align with 5-min tick | **very rare** |
| Session ends | process exits immediately, no final push | **no sync** |
| Between sessions | no process running | **no sync** |

Additionally, when the daemon shuts down (daemon.rs:380-383), it just breaks out of the loop and cleans up — **no final sync attempt**.

## Evidence

Sync queues across all projects on 2026-04-12:

| Project | Pending items | Oldest item | retry_count | last_error |
|---------|--------------|-------------|-------------|------------|
| petra-stella-cloud | 353 | 2026-04-02 | 0 | NULL |
| cas-src | 101 | ~2026-04-03 | 0 | NULL |
| domdms | 35 | ~2026-04-09 | 0 | NULL |
| global (~/.cas) | 33 | ~2026-04-09 | 0 | NULL |
| gabber-studio | 1 | ~2026-04-10 | 0 | NULL |

All items have `retry_count = 0` and `last_error = NULL` — they were **never attempted**. The push logic itself works fine (confirmed by `last_push_at` timestamps showing occasional lucky pushes days ago).

---

## Required Changes

### Fix 1: Final sync on shutdown (critical)

**File:** `cas-cli/src/mcp/daemon.rs` — the `run()` method

After the main loop breaks on shutdown signal (line ~382), before unregistering and socket cleanup, force one final push:

```rust
// After the loop exits:

// Final cloud sync — drain the queue before we die
if let Some(ref syncer) = self.cloud_syncer {
    let cas_root = self.config.cas_root.clone();
    let syncer = Arc::clone(syncer);
    let _ = tokio::task::spawn_blocking(move || {
        let store = open_store(&cas_root).ok();
        let task_store = open_task_store(&cas_root).ok();
        let rule_store = open_rule_store(&cas_root).ok();
        let skill_store = open_skill_store(&cas_root).ok();
        if let (Some(s), Some(t), Some(r), Some(sk)) = (store, task_store, rule_store, skill_store) {
            let _ = syncer.sync_with_sessions(s.as_ref(), t.as_ref(), r.as_ref(), sk.as_ref(), &[]);
        }
    }).await;
}
```

This ensures every session drains its queue before exiting. Even if incomplete (timeout, network error), it's strictly better than the current zero-attempt behavior.

**Consideration:** The MCP server process may be killed with SIGTERM by Claude Code. Ensure the shutdown signal handler allows enough time for the final sync. A 10-second timeout on the spawn_blocking should be sufficient — typical push of 50 items takes <2 seconds.

### Fix 2: Push on session start (high priority)

**File:** `cas-cli/src/mcp/daemon.rs` — the `run()` method

The config `cloud.pull_on_start = true` triggers a pull at startup, but there's no corresponding push. Add a push attempt **before the first pull** in the startup sequence, right after the first `cloud_sync_interval.tick().await` skip:

```rust
// Initial sync: push stale queue, then pull fresh data
if self.cloud_syncer.is_some() {
    match self.run_cloud_sync().await {
        Ok(result) => {
            let mut status = self.status.write().await;
            status.cloud_items_pushed += result.total_pushed();
            status.cloud_items_pulled += result.total_pulled();
            status.last_cloud_sync = Some(Utc::now());
        }
        Err(e) => {
            tracing::warn!("Initial cloud sync failed: {e}");
        }
    }
}
```

This handles the common pattern: user opens project, has stale items from last session, needs to push them before pulling fresh data.

### Fix 3: Lower idle threshold for cloud sync (medium priority)

**File:** `cas-cli/src/mcp/daemon.rs` or `crates/cas-mcp/src/daemon.rs`

The current `min_idle_secs = 60` is appropriate for heavy maintenance tasks (memory consolidation, decay) but too conservative for a lightweight HTTP push. Two options:

**Option A (preferred):** Separate idle threshold for cloud sync. Add a `cloud_sync_idle_secs` config (default: 10s) and use it specifically for the cloud sync gate:

```rust
// In the sync branch of the select! loop:
_ = cloud_sync_interval.tick() => {
    if self.cloud_syncer.is_some() && self.activity.idle_seconds() >= self.config.cloud_sync_idle_secs {
```

10 seconds of idle is common during normal usage — user reading output, reviewing a diff, typing a response. This would catch most natural pauses.

**Option B (simpler):** Remove the idle gate entirely for cloud sync. Push is lightweight (gzipped HTTP POST), non-blocking (runs in spawn_blocking), and rate-limited by the 5-minute interval. There's no reason to also gate on idle.

### Fix 4: Reduce sync interval (low priority)

**File:** Config `cloud.interval_secs`

The 5-minute interval is fine for background sync but means even with the idle gate fixed, a newly queued item waits up to 5 minutes before push. Consider:

- Default to 60s (1 minute) instead of 300s
- Or add a "push soon" signal that reduces the next interval when items are enqueued (debounced at ~5s)

This is lower priority since Fix 1 (final sync) and Fix 2 (push on start) cover the critical paths.

---

## Implementation Order

1. **Fix 1 (final sync on shutdown)** — highest impact, catches the most common failure mode
2. **Fix 2 (push on start)** — catches stale queues from previous sessions
3. **Fix 3 (lower idle threshold)** — makes mid-session sync actually work
4. Fix 4 (reduce interval) — polish

Fixes 1 and 2 together solve the problem for the vast majority of cases: queue drains at session end, and anything missed gets picked up at next session start. Fix 3 makes it work during long sessions.

---

## Server-Side Changes Already Shipped

The following changes were merged to petra-stella-cloud on 2026-04-12 to support this:

- **cloud-d656:** Server push rejects NULL/empty `project_canonical_id` with 400 (prevents contamination)
- **cloud-f645:** Push response reports `{inserted, updated, skipped}` per entity type (enables client-side partial failure detection)
- **Data cleanup:** Deleted 10,450 orphaned NULL-project rows from the cloud DB

The CLI already sends `project_canonical_id` on every push (enforced in `push_sub_batch` and `push_sessions`). The sync queue items all have valid payloads. The only problem is the queue never gets a chance to drain.

---

## Testing

After implementing fixes, verify:

1. Start a Claude Code session, create a Cassy entry, confirm it appears in sync_queue
2. Exit Claude Code — check that sync_queue is now empty (Fix 1 drained it)
3. Manually add items to sync_queue while no session is running
4. Start a new Claude Code session — check queue drains within ~10s (Fix 2)
5. During an active session, pause for 15 seconds — check if sync fires (Fix 3)
