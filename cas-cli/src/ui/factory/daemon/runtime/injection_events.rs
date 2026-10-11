//! Collapse repeated `supervisor_injected` events (cas-e193).
//!
//! The queue loop records one `supervisor_injected` event per injection
//! attempt. A retried delivery repeats the identical row: a missing pane
//! produced 120 `error` rows per prompt over ten minutes, and one re-injection
//! storm wrote ~28,000 `ok` rows for each of 428 prompts. On the cassy store
//! that was 799,578 rows, and collapsing identical repeats leaves 39,254.
//!
//! Readers need only the first row of each outcome. The ack waiters
//! (`wait_for_supervisor_ack`, `cas factory message --wait-ack`) look for the
//! prompt's `ok` row, and the activity feed shows one line per outcome. A
//! repeat of the same (prompt, recipient, status, error) adds nothing, so it is
//! skipped. A changed status or error text is a new outcome and is recorded,
//! and the tracing log still carries every attempt.
//!
//! The memory is per process and bounded: [`INJECTION_EVENT_DEDUPE_CAPACITY`]
//! keys, oldest evicted first. A daemon restart forgets it, which costs at
//! most one extra row per in-flight outcome.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

/// Outcomes remembered at most; the oldest go first.
pub(crate) const INJECTION_EVENT_DEDUPE_CAPACITY: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct InjectionOutcome {
    /// The `.cas` directory whose events table receives the row; prompt ids
    /// are only unique within one store.
    scope: PathBuf,
    prompt_id: i64,
    target: String,
    status: String,
    error: Option<String>,
}

/// Bounded first-seen set of injection outcomes.
#[derive(Debug)]
pub(crate) struct InjectionEventDedupe {
    seen: HashSet<InjectionOutcome>,
    order: VecDeque<InjectionOutcome>,
    capacity: usize,
}

impl InjectionEventDedupe {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            seen: HashSet::new(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    /// True the first time an outcome is seen (record it), false for an
    /// identical repeat (skip it).
    pub(crate) fn first_occurrence(
        &mut self,
        scope: &Path,
        prompt_id: i64,
        target: &str,
        status: &str,
        error: Option<&str>,
    ) -> bool {
        let outcome = InjectionOutcome {
            scope: scope.to_path_buf(),
            prompt_id,
            target: target.to_string(),
            status: status.to_string(),
            error: error.map(str::to_string),
        };
        if self.seen.contains(&outcome) {
            return false;
        }
        if self.order.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.seen.remove(&oldest);
            }
        }
        self.seen.insert(outcome.clone());
        self.order.push_back(outcome);
        true
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.order.len()
    }
}

static RECORDED_INJECTIONS: LazyLock<Mutex<InjectionEventDedupe>> =
    LazyLock::new(|| Mutex::new(InjectionEventDedupe::new(INJECTION_EVENT_DEDUPE_CAPACITY)));

/// Process-wide check used by the queue loop before it writes a
/// `supervisor_injected` event. A poisoned lock records rather than drops.
pub(crate) fn should_record_injection(
    scope: &Path,
    prompt_id: i64,
    target: &str,
    status: &str,
    error: Option<&str>,
) -> bool {
    match RECORDED_INJECTIONS.lock() {
        Ok(mut dedupe) => dedupe.first_occurrence(scope, prompt_id, target, status, error),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_retries_record_once_and_new_outcomes_record_cas_e193() {
        let here = Path::new("/project-a/.cas");
        let mut dedupe = InjectionEventDedupe::new(16);
        let missing = Some("No such file or directory (os error 2)");
        assert!(dedupe.first_occurrence(here, 7, "worker-a", "error", missing));
        for _ in 0..119 {
            assert!(!dedupe.first_occurrence(here, 7, "worker-a", "error", missing));
        }
        // A different error text, the eventual success, another recipient
        // and another prompt are each new outcomes.
        assert!(dedupe.first_occurrence(here, 7, "worker-a", "error", Some("pane gone")));
        assert!(dedupe.first_occurrence(here, 7, "worker-a", "ok", None));
        assert!(!dedupe.first_occurrence(here, 7, "worker-a", "ok", None));
        assert!(dedupe.first_occurrence(here, 7, "worker-b", "ok", None));
        assert!(dedupe.first_occurrence(here, 8, "worker-a", "ok", None));
        // The same prompt id in another store is a different prompt.
        assert!(dedupe.first_occurrence(Path::new("/project-b/.cas"), 8, "worker-a", "ok", None));
    }

    #[test]
    fn memory_is_bounded_and_evicts_oldest_first_cas_e193() {
        let here = Path::new("/project-a/.cas");
        let mut dedupe = InjectionEventDedupe::new(3);
        for prompt_id in 0..3 {
            assert!(dedupe.first_occurrence(here, prompt_id, "w", "ok", None));
        }
        assert!(dedupe.first_occurrence(here, 3, "w", "ok", None));
        assert_eq!(dedupe.len(), 3);
        // Prompt 0 was evicted, so it records again; prompt 3 is remembered.
        assert!(dedupe.first_occurrence(here, 0, "w", "ok", None));
        assert!(!dedupe.first_occurrence(here, 3, "w", "ok", None));
    }
}
