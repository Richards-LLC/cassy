//! cas-5fff: does a NON-URGENT message to an ALREADY-IDLE **real** Codex
//! worker actually start a turn?
//!
//! Field evidence (supervisor, 2026-07-31, cas 2.38.1): three non-urgent
//! supervisor→worker messages to three idle Codex workers were written to the
//! PTY, reported `stage: delivered`, and started **zero** turns. Each rollout's
//! last record stayed `task_complete` from minutes earlier. Every merge handoff
//! in that epic had to be force-closed.
//!
//! `#[ignore]` — spawns a real `codex` CLI child on a real PTY. Run explicitly:
//!   cargo test -p cas-mux --test nonurgent_idle_codex_runtime -- --ignored --nocapture
//!
//! WHY A REAL BINARY IS MANDATORY HERE: cas-893c closed with an explicit
//! negative result — "the Codex PTY path is NOT the cause" — that was measured
//! against an interactive **bash** stand-in because codex wasn't installed in
//! that sandbox. bash accepts a bare write-then-CR; Codex's full-screen TUI,
//! with its own composer + paste-burst state machine, does not. That single
//! substitution is why this bug survived three months. Do not reintroduce it.

#[path = "support/real_pty_serial.rs"]
mod real_pty_serial;
#[path = "support/codex_live.rs"]
mod codex_live;

use std::time::Duration;

/// AC1/AC2: a production-shaped (multi-line, ~1.8 KB, backticks, blank lines)
/// non-urgent coordination message delivered to an already-idle Codex pane must
/// create a fresh `user_message` and complete a fresh turn.
///
/// This is the exact shape the factory sends and the exact shape that failed
/// 3-for-3 in the field. The pre-existing cas-a5a7 assertion in
/// `urgent_interrupt_codex_runtime.rs` covered only a single short line, which
/// is why it stayed green while production was dead.
#[test]
#[ignore = "spawns a real `codex` CLI child process — run explicitly, see module docs"]
fn idle_nonurgent_production_shaped_message_starts_turn() {
    let _serial = real_pty_serial::lock();
    let Some(mut live) = codex_live::CodexLive::boot("cas-5fff-nonurgent") else {
        return;
    };

    let before = live.counts();
    eprintln!("[cas-5fff] counts before non-urgent inject: {before:?}");

    let payload = codex_live::production_shaped_handoff("NONURGENT-IDLE-OK");
    eprintln!(
        "[cas-5fff] payload: {} bytes, {} newlines",
        payload.len(),
        payload.matches('\n').count()
    );

    live.rt
        .block_on(live.mux.inject(&live.id.clone(), &payload))
        .expect("non-urgent inject must not error at the PTY layer");

    let started = live.wait_for_new_completed_turn(before, Duration::from_secs(90));
    let screen = live.screen_text();
    assert!(
        started,
        "cas-5fff: a NON-URGENT production-shaped message to an already-idle Codex \
         worker must create a new user_message and complete a new turn. \
         counts before={before:?} after={:?}\n\n--- PANE SCREEN ---\n{screen}\n\n\
         --- ROLLOUT TAIL ---\n{}",
        live.counts(),
        live.rollout_tail(25)
    );
}

/// AC2 upper bound: the largest shape seen in the field (queue row 6400 —
/// 2907 bytes, 12 newlines). Guards against a framing fix that happens to work
/// at 1 KB but not at the real ceiling.
#[test]
#[ignore = "spawns a real `codex` CLI child process — run explicitly, see module docs"]
fn idle_nonurgent_largest_field_payload_starts_turn() {
    let _serial = real_pty_serial::lock();
    let Some(mut live) = codex_live::CodexLive::boot("cas-5fff-xl") else {
        return;
    };

    let before = live.counts();
    let payload = codex_live::production_shaped_handoff_xl("XL-IDLE-OK");
    eprintln!(
        "[cas-5fff] XL payload: {} bytes, {} newlines",
        payload.len(),
        payload.matches('\n').count()
    );

    live.rt
        .block_on(live.mux.inject(&live.id.clone(), &payload))
        .expect("non-urgent inject must not error at the PTY layer");

    let started = live.wait_for_new_completed_turn(before, Duration::from_secs(90));
    let screen = live.screen_text();
    assert!(
        started,
        "cas-5fff: the largest field-observed payload must still submit. \
         counts before={before:?} after={:?}\n\n--- PANE SCREEN ---\n{screen}\n\n\
         --- ROLLOUT TAIL ---\n{}",
        live.counts(),
        live.rollout_tail(25)
    );
}

/// cas-5fff widened the ticket's framing: `Mux::interrupt_and_inject` funnels
/// through the very same `Pane::inject_prompt`, so the URGENT path samples the
/// identical swallow — which is exactly why the supervisor measured urgent as
/// "usually, retry if not" (queue rows 6401 @2448 B woke, 6402 @2392 B did
/// not, 6403 @758 B woke) rather than reliably.
///
/// This pins the urgent path at a production payload size against an
/// already-idle pane, the combination the field evidence shows failing.
#[test]
#[ignore = "spawns a real `codex` CLI child process — run explicitly, see module docs"]
fn idle_urgent_production_shaped_message_starts_turn() {
    let _serial = real_pty_serial::lock();
    let Some(mut live) = codex_live::CodexLive::boot("cas-5fff-urgent") else {
        return;
    };

    let before = live.counts();
    let payload = codex_live::production_shaped_handoff("URGENT-IDLE-OK");
    let id = live.id.clone();

    // The real production floor (`queue_and_events.rs` urgent_settle_duration).
    let production_floor = Duration::from_millis(1200);
    live.rt
        .block_on(
            live.mux
                .interrupt_and_inject(&id, &payload, production_floor),
        )
        .expect("interrupt_and_inject must succeed");

    let started = live.wait_for_new_completed_turn(before, Duration::from_secs(90));
    let screen = live.screen_text();
    assert!(
        started,
        "cas-5fff: an URGENT production-shaped message to an already-idle Codex worker \
         must create a new user_message and complete a new turn. \
         counts before={before:?} after={:?}\n\n--- PANE SCREEN ---\n{screen}\n\n\
         --- ROLLOUT TAIL ---\n{}",
        live.counts(),
        live.rollout_tail(25)
    );
}

/// Control: the single-short-line shape cas-a5a7 already asserted. Kept
/// separate so a failure here vs. above discriminates "all non-urgent idle
/// delivery is broken" from "only the real production payload shape is".
#[test]
#[ignore = "spawns a real `codex` CLI child process — run explicitly, see module docs"]
fn idle_nonurgent_single_line_message_starts_turn() {
    let _serial = real_pty_serial::lock();
    let Some(mut live) = codex_live::CodexLive::boot("cas-5fff-oneline") else {
        return;
    };

    let before = live.counts();
    eprintln!("[cas-5fff] counts before single-line inject: {before:?}");

    live.rt
        .block_on(live.mux.inject(
            &live.id.clone(),
            "Message from supervisor: Reply with exactly ONELINE-IDLE-OK and nothing else.",
        ))
        .expect("non-urgent inject must not error at the PTY layer");

    let started = live.wait_for_new_completed_turn(before, Duration::from_secs(90));
    assert!(
        started,
        "control: a single-line non-urgent message to an idle Codex worker must \
         start and complete a turn. counts before={before:?} after={:?}\nRollout tail:\n{}",
        live.counts(),
        live.rollout_tail(25)
    );
}
