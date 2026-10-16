//! Reclaim the allocator's freed pages between MCP bursts (GH #1247).
//! File-backed SQLite/Tantivy mappings are already reclaimable and shared.

use std::sync::Mutex;
use std::time::{Duration, Instant};

const IDLE: Duration = Duration::from_secs(30);
static CALLS: Mutex<(usize, Option<Instant>)> = Mutex::new((0, None));

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
    let calls = CALLS.lock().unwrap_or_else(|p| p.into_inner());
    if calls.0 == 0 && calls.1.is_some_and(|last| now.saturating_duration_since(last) >= IDLE) {
        // The regression is committed before enabling reclamation.
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
            .map(|line| line.split_whitespace().nth(1).unwrap().parse::<u64>().unwrap() * 1024)
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
        assert!(footprint() + 16 * 1024 * 1024 > retained, "active call was trimmed");
        drop(call);
        reclaim_if_idle(Instant::now() + IDLE);
        assert!(footprint() + 16 * 1024 * 1024 < retained, "idle server retained freed pages");
        std::hint::black_box(pins);
    }
}
