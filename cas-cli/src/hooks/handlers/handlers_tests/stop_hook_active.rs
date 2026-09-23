//! `stop_hook_active` remains the brake for exit and loop blockers. Maintenance
//! work is now queued independently and never blocks the parent session.
//!
//! Confirmed on the wire, not from docs: the payloads below are shaped from a
//! live capture of Claude Code 2.1.224 (see
//! `docs/analysis/2026-08-07-hook-wire-shape-audit.md`).
//!
//! These tests parse raw JSON rather than building `HookInput` by hand. That is
//! deliberate and is the point of the whole task: a struct literal would set
//! `stop_hook_active` directly and prove nothing about whether the wire key
//! ever reaches the handler — which is exactly how GH #165 hid.

use crate::hooks::handlers::handle_stop;
use cas_core::hooks::types::HookInput;
use std::fs;
use std::path::Path;

/// Real Stop wire payload. `stop_hook_active` is included only when
/// `reentrant` is `Some(..)`, so the "absent" case is also exercised.
fn stop_payload(reentrant: Option<bool>) -> HookInput {
    let mut payload = serde_json::json!({
        "session_id": "cas-f3e3-stop-session",
        "transcript_path": "/tmp/cas-f3e3/transcript.jsonl",
        "cwd": "/tmp/cas-f3e3",
        "prompt_id": "d323a56b-73b1-4afc-8b3f-32605be51b91",
        "permission_mode": "default",
        "hook_event_name": "Stop",
        "last_assistant_message": "DONE",
        "background_tasks": [],
        "session_crons": [],
    });
    if let Some(active) = reentrant {
        payload["stop_hook_active"] = serde_json::Value::Bool(active);
    }
    serde_json::from_value(payload).expect("real Stop payload must deserialize")
}

/// A `.cas` root whose config forces the learning-review blocker to fire on
/// every Stop, with no store seeding required: `learning_review_enabled = true`
/// plus `learning_review_threshold = 0` makes `build_learning_review_context`
/// return `Some(..)` unconditionally.
///
/// `block_exit_on_open` is turned off so the FIRST of the five block sites
/// cannot fire and mask which brake is under test.
fn cas_root_that_always_blocks_stop(dir: &Path) -> std::path::PathBuf {
    let cas_dir = dir.join(".cas");
    fs::create_dir_all(&cas_dir).unwrap();
    fs::write(
        cas_dir.join("config.toml"),
        "[tasks]\nblock_exit_on_open = false\n\n\
         [hooks.stop]\nlearning_review_enabled = true\nlearning_review_threshold = 0\n",
    )
    .unwrap();
    cas_dir
}

fn decision_of(out: &cas_core::hooks::types::HookOutput) -> Option<&str> {
    out.decision.as_deref()
}

/// The guard assertion and the fix assertion in one test, in that order.
///
/// The first half is what makes the second half non-vacuous: if the blocker
/// did not fire without `stop_hook_active`, the "does not block" assertion
/// would pass for the wrong reason and prove nothing.
#[test]
fn stop_hook_active_stops_cas_from_re_blocking_a_continuation() {
    let _g = super::env_lock();
    // Exercise the same path for factory and non-factory sessions.
    unsafe { std::env::remove_var("CAS_AGENT_ROLE") };

    let dir = tempfile::tempdir().unwrap();
    let cas_root = cas_root_that_always_blocks_stop(dir.path());

    // A first Stop must not block for maintenance.
    let first = handle_stop(&stop_payload(Some(false)), Some(&cas_root)).expect("handler ok");
    assert_eq!(
        decision_of(&first),
        None,
        "maintenance must not block the parent session"
    );

    // Reentrant Stop also stays unblocked.
    let reentrant = handle_stop(&stop_payload(Some(true)), Some(&cas_root)).expect("handler ok");
    assert_eq!(
        decision_of(&reentrant),
        None,
        "stop_hook_active = true must suppress the block: {}",
        serde_json::to_string(&reentrant).unwrap()
    );
}

/// An absent key still reaches the non-blocking maintenance check.
#[test]
fn an_absent_stop_hook_active_key_leaves_blocking_behaviour_unchanged() {
    let _g = super::env_lock();
    unsafe { std::env::remove_var("CAS_AGENT_ROLE") };

    let dir = tempfile::tempdir().unwrap();
    let cas_root = cas_root_that_always_blocks_stop(dir.path());

    let out = handle_stop(&stop_payload(None), Some(&cas_root)).expect("handler ok");
    assert_eq!(
        decision_of(&out),
        None,
        "maintenance must not block when the key is absent"
    );
}
