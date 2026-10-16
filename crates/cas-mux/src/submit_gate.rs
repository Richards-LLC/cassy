//! Hold an injected prompt's submit CR until a precondition is met
//! (cas-786a6 review round 2, L1).
//!
//! The daemon registers a machine prompt's provenance envelope off its
//! loop. The recipient's hook reads that envelope when the prompt is
//! submitted, so the submit CR must not go out before the envelope is
//! written. A caller that starts such a write registers a gate for the
//! pane with [`hold_next_submit`]; the next inject on that pane takes it
//! and its detached submit task waits for the gate, at most
//! [`SUBMIT_GATE_MAX_WAIT`], before writing the CR. Nothing on the loop
//! waits.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The longest a submit CR waits for its gate.
pub const SUBMIT_GATE_MAX_WAIT: Duration = Duration::from_secs(2);

/// A one-shot precondition for a submit CR.
#[derive(Debug, Clone, Default)]
pub struct SubmitGate(Arc<(Mutex<bool>, Condvar)>);

impl SubmitGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Let the submit go.
    pub fn open(&self) {
        let (open, cv) = &*self.0;
        *open.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        cv.notify_all();
    }

    pub fn is_open(&self) -> bool {
        *self.0 .0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Block until open or `timeout` (tests and threads; never a lane).
    pub fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let (open, cv) = &*self.0;
        let mut guard = open.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while !*guard {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            guard = cv
                .wait_timeout(guard, left)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
        true
    }

    /// Wait in a task, polling, until open or `timeout`.
    pub(crate) async fn opened(&self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        while !self.is_open() {
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        true
    }
}

fn gates() -> &'static Mutex<HashMap<String, SubmitGate>> {
    static GATES: OnceLock<Mutex<HashMap<String, SubmitGate>>> = OnceLock::new();
    GATES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Hold the submit CR of the next inject on `pane` until `gate` opens.
pub fn hold_next_submit(pane: &str, gate: SubmitGate) {
    gates()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(pane.to_string(), gate);
}

/// The gate held for `pane`'s next submit, if any (tests).
pub fn pending_submit_gate(pane: &str) -> Option<SubmitGate> {
    gates()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(pane)
        .cloned()
}

pub(crate) fn take(pane: &str) -> Option<SubmitGate> {
    gates()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(pane)
}
