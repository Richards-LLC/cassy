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
use crate::test_support::TestEnvGuard;
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
    let mut env = TestEnvGuard::new();
    // Exercise the same path for factory and non-factory sessions.
    env.remove("CAS_AGENT_ROLE");

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
    let mut env = TestEnvGuard::new();
    env.remove("CAS_AGENT_ROLE");

    let dir = tempfile::tempdir().unwrap();
    let cas_root = cas_root_that_always_blocks_stop(dir.path());

    let out = handle_stop(&stop_payload(None), Some(&cas_root)).expect("handler ok");
    assert_eq!(
        decision_of(&out),
        None,
        "maintenance must not block when the key is absent"
    );
}

/// cas-7456 (GH #1302 F): a supervisor ended its turn with no reminder set and
/// slept for 11 h. A factory supervisor's Stop with no pending reminder warns
/// that it has no wake source (and never blocks); a pending reminder quiets it.
#[test]
fn a_supervisor_stop_with_no_pending_reminder_warns_of_no_wake_source_gh1302() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_AGENT_ROLE", "supervisor");
    env.set("CAS_AGENT_NAME", "brave-crow-50");
    env.set("CAS_FACTORY_SESSION", "gh1302-session");
    let dir = tempfile::tempdir().unwrap();
    let cas_root = crate::store::init_cas_dir(dir.path()).unwrap();
    let stop = || handle_stop(&stop_payload(None), Some(&cas_root)).expect("handler ok");
    let remind = |session: &str| {
        crate::store::open_reminder_store(&cas_root)
            .unwrap()
            .create_with_scope(
                "brave-crow-50",
                None,
                "watchdog: check the merge queue",
                cas_store::ReminderTriggerType::Time,
                Some(chrono::Utc::now() + chrono::Duration::minutes(10)),
                None,
                None,
                0,
                Some(session),
                None,
                true,
                None,
            )
            .unwrap();
    };
    // cas-b080: a reminder left pending by an earlier session cannot wake
    // this one, so it is not a wake source.
    remind("an-earlier-session");

    let warned = stop();
    assert_eq!(decision_of(&warned), None, "the warning never blocks the stop");
    let text = serde_json::to_string(&warned).unwrap();
    assert!(text.contains("No wake source"), "{text}");

    remind("gh1302-session");
    let quiet = serde_json::to_string(&stop()).unwrap();
    assert!(!quiet.contains("No wake source"), "a pending reminder is a wake source: {quiet}");
}
