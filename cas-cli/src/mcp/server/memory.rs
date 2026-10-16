//! Reclaim the allocator's freed pages between MCP bursts (GH #1247).
//! File-backed SQLite/Tantivy mappings are already reclaimable and shared.

use std::sync::Mutex;
use std::time::{Duration, Instant};

const IDLE: Duration = Duration::from_secs(30);
static CALLS: Mutex<(usize, Option<Instant>)> = Mutex::new((0, None));

pub(crate) fn configure_allocator() {
    // Do this before Tokio starts its workers: the cap cannot remove arenas
    // already created. Two arenas retain allocator concurrency without a
    // separate high-water footprint for every CPU on a large factory host.
    // SAFETY: mallopt is glibc's process-wide allocator configuration API.
    unsafe { libc::mallopt(libc::M_ARENA_MAX, 2) };
    CALLS.lock().unwrap_or_else(|p| p.into_inner()).1 = Some(Instant::now());
}

pub(crate) async fn reclaim_when_idle() {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        // Scanning arenas must not occupy an async scheduler thread. The
        // server wrapper owns this future, so disconnect cancels the loop.
        let _ = tokio::task::spawn_blocking(|| reclaim_if_idle(Instant::now())).await;
    }
}

/// Lives inside the handler task, including work continuing past its deadline.
pub(crate) struct Call;

impl Call {
    pub(crate) fn started() -> Self {
        CALLS.lock().unwrap_or_else(|p| p.into_inner()).0 += 1;
        Self
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        let mut calls = CALLS.lock().unwrap_or_else(|p| p.into_inner());
        calls.0 -= 1;
        calls.1 = Some(Instant::now());
    }
}

fn reclaim_if_idle(now: Instant) {
    let mut calls = CALLS.lock().unwrap_or_else(|p| p.into_inner());
    if calls.0 == 0
        && calls
            .1
            .is_some_and(|last| now.saturating_duration_since(last) >= IDLE)
    {
        // Hold the activity lock through the trim so a new handler cannot
        // start between the idle check and reclamation. Live allocations,
        // SQLite connections and index mappings remain valid.
        // SAFETY: glibc releases only unused allocator pages; no pointers are
        // passed, and malloc_trim is safe with other allocating threads.
        unsafe { libc::malloc_trim(0) };
        calls.1 = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn footprint() -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmSwap:"))
            .map(|line| {
                line.split_whitespace()
                    .nth(1)
                    .unwrap()
                    .parse::<u64>()
                    .unwrap()
                    * 1024
            })
            .sum()
    }

    #[test]
    fn idle_serve_returns_free_pages_without_trimming_active_calls() {
        let call = Call::started();
        // Small live allocations pin the arena between freed blocks, matching
        // a long-lived server. No RSS ceiling depends on the runner's baseline.
        let blocks: Vec<_> = (0..512)
            .map(|_| (vec![42u8; 64 * 1024], Box::new(42u64)))
            .collect();
        std::hint::black_box(&blocks);
        let pins: Vec<_> = blocks.into_iter().map(|(_, pin)| pin).collect();
        let retained = footprint();
        reclaim_if_idle(Instant::now() + IDLE);
        assert!(
            footprint() + 16 * 1024 * 1024 > retained,
            "active call was trimmed"
        );
        drop(call);
        reclaim_if_idle(Instant::now() + IDLE);
        assert!(
            footprint() + 16 * 1024 * 1024 < retained,
            "idle server retained freed pages"
        );
        std::hint::black_box(pins);
    }
}
