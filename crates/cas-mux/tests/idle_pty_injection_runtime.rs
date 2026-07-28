//! Runtime verification for cas-893c AC3: does an IDLE PTY-delivered worker
//! actually receive + auto-submit an injected message?
//!
//! `#[ignore]` — spawns a real child process attached to a real PTY (not
//! mocked), so it's excluded from the default `cargo test` gate per this
//! repo's live/e2e convention (see `cas-cli/tests/e2e/factory_e2e/lifecycle.rs`).
//! Run explicitly:
//!   cargo test -p cas-mux --test idle_pty_injection_runtime -- --ignored --nocapture
//!
//! `codex` itself isn't installed in this sandbox, so this uses a real,
//! plain interactive `bash` as a stand-in for a harness idling at its input
//! prompt. That's a deliberate, honest scope: the thing actually under test
//! is cas's OWN injection mechanism (`Pane::inject_prompt` — a raw PTY
//! write of text, then a `\r` after a harness-scaled settle delay), which
//! is harness-agnostic bytes-over-PTY. It validates, against a real
//! OS-level PTY rather than a code-trace assumption, the two claims
//! cas-893c's task description asked to verify before fixing anything:
//!
//!   1. `Pane::ready_for_injection()` state for a genuinely idle pane.
//!   2. Whether injected text auto-submits (the child receives a complete
//!      line) when the pane is idle.
//!
//! Result (see task cas-893c notes for the full writeup): both hold. The
//! readiness gate (`total_bytes_received > 0 && elapsed >= 5s`) has no
//! busy/idle input at all, so it can never be the reason an idle recipient
//! misses a message; and the write-then-delayed-CR mechanism reliably
//! produces a submitted line at the OS level. The Codex "stopped and not
//! resuming" symptom this task investigated is therefore NOT an idle-gate
//! problem — see the companion busy-injection note in the task record.

use cas_mux::{Pane, PaneKind, Pty, PtyConfig, SupervisorCli};
use std::io::Read;
use std::time::Duration;

#[test]
#[ignore = "spawns a real PTY child process — run explicitly, see module docs"]
fn idle_pane_injection_auto_submits_real_pty() {
    let tmp = std::env::temp_dir().join(format!(
        "cas-893c-idle-inject-{}-{}.log",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_file(&tmp);

    // A plain interactive `bash` sitting at its input prompt — this
    // genuinely models "idle" the same way a harness waiting for user input
    // does: a live process blocked reading a line from its controlling PTY.
    // `-i` + an explicit `PS1` guarantee it actually writes a prompt byte to
    // the PTY (some minimal `/bin/sh` builds print nothing without one,
    // which — as an earlier run of this test caught for real — leaves
    // `total_bytes_received` at 0 forever and `ready_for_injection()` false;
    // real harnesses always emit startup/banner output, so that's a
    // property of the trivial stand-in, not of the readiness gate).
    let config = PtyConfig {
        command: "bash".to_string(),
        args: vec![
            "--norc".to_string(),
            "--noprofile".to_string(),
            "-i".to_string(),
        ],
        cwd: Some(std::env::temp_dir()),
        env: vec![("PS1".to_string(), "cas893c$ ".to_string())],
        rows: 24,
        cols: 80,
    };
    let pty = Pty::spawn("cas-893c-idle-test", config).expect("spawn real pty");
    let mut pane = Pane::with_pty(
        "cas-893c-idle-test",
        PaneKind::Shell,
        pty,
        24,
        80,
        SupervisorCli::Claude,
    )
    .expect("wrap pty in pane");

    // Drain startup output so `total_bytes_received` advances the same way
    // the daemon's poll loop does — this is exactly what
    // `ready_for_injection()` gates on.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let _ = pane.drain_output();
        std::thread::sleep(Duration::from_millis(50));
    }

    eprintln!(
        "DEBUG bytes_received={} ready={} exited={:?}",
        pane.bytes_received(),
        pane.ready_for_injection(),
        pane.has_exited()
    );
    // Claim 1: an idle pane (alive, no work in flight, past the 5s startup
    // grace) does NOT fail the readiness gate.
    assert!(
        pane.ready_for_injection(),
        "an idle pane must be ready_for_injection() — the gate has no busy/idle \
         input, only total_bytes_received>0 && elapsed>=5s"
    );

    // Claim 2: does injected text auto-submit while idle? Inject a real
    // shell command line — the same "type text, wait, send CR" mechanism
    // `deliver_to_worker`/`interrupt_and_inject` use in production — and
    // check the idle `sh` actually executed it as a submitted line.
    let command = format!("echo hello-cas-893c-idle-nudge >> {}", tmp.display());
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(pane.inject_prompt(&command))
        .expect("inject_prompt");

    // inject_prompt sends the CR on a background task after a settle delay
    // (150ms for non-codex harnesses, 500ms for codex). Give it real time
    // to land, draining periodically the way the daemon's poll loop does.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut got = String::new();
    while std::time::Instant::now() < deadline {
        let _ = pane.drain_output();
        if let Ok(mut f) = std::fs::File::open(&tmp) {
            got.clear();
            let _ = f.read_to_string(&mut got);
            if got.contains("hello-cas-893c-idle-nudge") {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let _ = std::fs::remove_file(&tmp);
    assert!(
        got.contains("hello-cas-893c-idle-nudge"),
        "an idle pane must auto-submit injected text (write + delayed CR) — \
         the child shell never executed the injected command line. Got: {got:?}"
    );
}

/// A normal (non-urgent) PTY inject must queue behind an in-flight turn,
/// never cancel it. The shell models a harness that is busy until its
/// foreground command completes: injected input may be buffered, but must
/// not execute until after the original turn writes its completion marker.
#[test]
#[ignore = "spawns a real PTY child process — run explicitly, see module docs"]
fn non_urgent_injection_does_not_break_in_flight_turn() {
    let tmp = std::env::temp_dir().join(format!(
        "cas-a5a7-nonurgent-{}-{}.log",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_file(&tmp);

    let config = PtyConfig {
        command: "bash".to_string(),
        args: vec![
            "--norc".to_string(),
            "--noprofile".to_string(),
            "-i".to_string(),
        ],
        cwd: Some(std::env::temp_dir()),
        env: vec![("PS1".to_string(), "casa5a7$ ".to_string())],
        rows: 24,
        cols: 80,
    };
    let pty = Pty::spawn("cas-a5a7-busy-test", config).expect("spawn real pty");
    let mut pane = Pane::with_pty(
        "cas-a5a7-busy-test",
        PaneKind::Shell,
        pty,
        24,
        80,
        SupervisorCli::Claude,
    )
    .expect("wrap pty in pane");
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");

    let startup_deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < startup_deadline {
        let _ = pane.drain_output();
        std::thread::sleep(Duration::from_millis(50));
    }

    let first = format!("sleep 2; echo FIRST-TURN-DONE >> {}", tmp.display());
    rt.block_on(pane.inject_prompt(&first))
        .expect("start in-flight turn");
    std::thread::sleep(Duration::from_millis(750));

    let followup = format!("echo NONURGENT-AFTER >> {}", tmp.display());
    rt.block_on(pane.inject_prompt(&followup))
        .expect("queue non-urgent input");

    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    let mut got = String::new();
    while std::time::Instant::now() < deadline {
        let _ = pane.drain_output();
        got = std::fs::read_to_string(&tmp).unwrap_or_default();
        if got.contains("NONURGENT-AFTER") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = std::fs::remove_file(&tmp);

    let lines: Vec<&str> = got.lines().collect();
    assert_eq!(
        lines,
        vec!["FIRST-TURN-DONE", "NONURGENT-AFTER"],
        "normal injection must wait behind the active turn rather than cancel it"
    );
}
