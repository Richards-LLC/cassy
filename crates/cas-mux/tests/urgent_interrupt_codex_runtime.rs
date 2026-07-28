//! Live-binary verification for cas-4208: does `Mux::interrupt_and_inject`
//! actually recover a mid-turn Codex worker with a *multi-line* urgent
//! redirect, and does the worker stay reachable afterward?
//!
//! `#[ignore]` — spawns a real `codex` CLI child attached to a real PTY,
//! per this repo's live/e2e convention (see `idle_pty_injection_runtime.rs`).
//! Unlike that file, this one needs the actual `codex` binary + a working
//! `~/.codex/auth.json` (not a bash stand-in) because the defect this
//! covers is specific to Codex's own TUI transition after Esc, which no
//! generic stand-in reproduces faithfully. Skip if `codex` isn't on PATH.
//! Run explicitly:
//!   cargo test -p cas-mux --test urgent_interrupt_codex_runtime -- --ignored --nocapture
//!
//! BACKGROUND: task cas-4208 reported that an urgent interrupt to a Codex
//! worker aborts its turn and the redirect never lands — the worker stays
//! alive but never emits another rollout record, including for later
//! *normal* messages. A manual live repro (task notes, done before this
//! fix existed) against this same `codex` binary via `tmux` isolated the
//! cause: it is NOT the multi-line payload itself (a raw multi-line write
//! recovers fine given enough settle) — it's a race between the trailing
//! submit and Codex's post-Esc "Conversation interrupted" transition. A
//! too-short settle lets the submit land mid-transition, where it's
//! silently swallowed, leaving the correction sitting in the composer as
//! an unsent draft that every later message just appends to.
//!
//! This test drives the same scenario through the actual shipped code path
//! (`Mux::interrupt_and_inject` / `wait_for_injection_readiness`) with a
//! deliberately tiny floor (the exact conditions that reproduced the bug
//! manually) and asserts the fix's quiescence poll compensates for it.
use cas_mux::{Mux, Pane, PaneKind, Pty, PtyConfig, SupervisorCli};
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

fn codex_available() -> bool {
    std::process::Command::new("codex")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Poll `~/.codex/sessions/**/*.jsonl` for the newest rollout file whose
/// contents mention `needle` (our scratch cwd), so we don't accidentally
/// grab some *other* concurrently-running Codex session's rollout on a
/// shared dev machine (bit us once during manual investigation).
fn find_rollout_containing(needle: &str, deadline: std::time::Instant) -> Option<PathBuf> {
    let root = dirs_codex_sessions_root();
    while std::time::Instant::now() < deadline {
        if let Ok(mut entries) = glob_jsonl(&root) {
            entries.sort_by_key(|p| {
                std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            });
            for p in entries.into_iter().rev().take(20) {
                if let Ok(contents) = std::fs::read_to_string(&p) {
                    if contents.contains(needle) {
                        return Some(p);
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

fn dirs_codex_sessions_root() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home).join(".codex/sessions")
}

fn glob_jsonl(root: &PathBuf) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    fn walk(dir: &PathBuf, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out)?;
            } else if path.extension().is_some_and(|e| e == "jsonl") {
                out.push(path);
            }
        }
        Ok(())
    }
    walk(root, &mut out)?;
    Ok(out)
}

#[test]
#[ignore = "spawns a real `codex` CLI child process — run explicitly, see module docs"]
fn multiline_urgent_interrupt_recovers_codex_and_stays_reachable() {
    if !codex_available() {
        eprintln!("SKIP: `codex` not on PATH");
        return;
    }

    let scratch = std::env::temp_dir().join(format!(
        "cas-4208-codex-runtime-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("create scratch dir");
    // `codex` refuses to run in a dir it doesn't trust unless it's a git
    // repo it recognizes; a bare `git init` is enough for the trust prompt
    // to resolve non-interactively across runs in this same scratch tree.
    let _ = std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(&scratch)
        .status();

    let config = PtyConfig {
        command: "codex".to_string(),
        args: vec!["--yolo".to_string(), "--no-alt-screen".to_string()],
        cwd: Some(scratch.clone()),
        env: vec![],
        rows: 24,
        cols: 80,
    };
    let pty = Pty::spawn("cas4208-codex-rt", config).expect("spawn codex pty");
    let pane = Pane::with_pty("cas4208-codex-rt", PaneKind::Worker, pty, 24, 80, SupervisorCli::Codex)
        .expect("wrap pty in pane");
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");

    // Drain startup (trust prompt, banner) and accept the trust prompt if
    // it appears (Enter selects the default "Yes, continue").
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let _ = mux.poll_batch();
        std::thread::sleep(Duration::from_millis(200));
    }
    rt.block_on(mux.get("cas4208-codex-rt").unwrap().write(b"\r"))
        .ok();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let _ = mux.poll_batch();
        std::thread::sleep(Duration::from_millis(200));
    }

    // Kick off a turn that will still be running when we interrupt it.
    rt.block_on(mux.inject("cas4208-codex-rt", "Think step by step and write a long, detailed 400-word story about a lighthouse keeper. Take your time."))
        .expect("start turn");

    let rollout_deadline = std::time::Instant::now() + Duration::from_secs(15);
    let rollout = find_rollout_containing(
        scratch.to_str().expect("utf8 scratch path"),
        rollout_deadline,
    )
    .expect("rollout file for this scratch cwd must appear");

    // Give the turn a moment to actually be in flight before interrupting.
    std::thread::sleep(Duration::from_millis(1500));

    // The exact conditions that reproduced the bug manually: a deliberately
    // tiny floor (the daemon's real 1200ms floor is not the point under
    // test — a slow/loaded host can blow through it the same way a tiny
    // floor does here) plus a genuinely multi-line payload.
    let multiline = "STOP. Abandon that and instead run:\n\n  echo cas-4208-fixed\n\nThen reply CAS-4208-FIXED-OK.";
    let tiny_floor = Duration::from_millis(10);
    rt.block_on(mux.interrupt_and_inject("cas4208-codex-rt", multiline, tiny_floor))
        .expect("interrupt_and_inject must succeed");

    // Drain until the rollout shows the full redirect landed as ONE
    // user_message and the worker actually produced a fresh reply.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut contents = String::new();
    let mut saw_full_redirect = false;
    let mut saw_reply = false;
    while std::time::Instant::now() < deadline {
        let _ = mux.poll_batch();
        contents.clear();
        if let Ok(mut f) = std::fs::File::open(&rollout) {
            let _ = f.read_to_string(&mut contents);
        }
        saw_full_redirect = contents.contains("cas-4208-fixed");
        saw_reply = contents.contains("CAS-4208-FIXED-OK");
        if saw_full_redirect && saw_reply {
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    assert!(
        saw_full_redirect,
        "the full multi-line redirect must land as a real user_message after \
         the fix — rollout tail:\n{}",
        contents.lines().rev().take(20).collect::<Vec<_>>().join("\n")
    );
    assert!(
        saw_reply,
        "Codex must actually act on the redirect and reply, not go silent — \
         rollout tail:\n{}",
        contents.lines().rev().take(20).collect::<Vec<_>>().join("\n")
    );

    // AC3: the worker must remain reachable afterward — a subsequent
    // NORMAL (non-urgent) message must also land as its own turn.
    rt.block_on(mux.inject(
        "cas4208-codex-rt",
        "Confirm you are still reachable by replying STILL-REACHABLE-OK.",
    ))
    .expect("normal follow-up inject must succeed");

    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    let mut saw_followup = false;
    while std::time::Instant::now() < deadline {
        let _ = mux.poll_batch();
        contents.clear();
        if let Ok(mut f) = std::fs::File::open(&rollout) {
            let _ = f.read_to_string(&mut contents);
        }
        if contents.contains("STILL-REACHABLE-OK") {
            saw_followup = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    assert!(
        saw_followup,
        "worker must still be reachable after the urgent interrupt — a \
         normal follow-up message never landed (this is the 'deaf to all \
         later messages' symptom from the cas-4208 report). Rollout tail:\n{}",
        contents.lines().rev().take(30).collect::<Vec<_>>().join("\n")
    );

    let _ = std::fs::remove_dir_all(&scratch);
}

/// Live-binary verification for cas-1317: does `Mux::interrupt_and_inject`
/// recover an ALREADY-IDLE Codex worker — one whose first turn ended
/// normally (`task_complete` in the rollout, not a cancel) before the urgent
/// ever arrives?
///
/// This is a different shape from
/// `multiline_urgent_interrupt_recovers_codex_and_stays_reachable` above,
/// which deliberately catches the pane MID-TURN. The original incident
/// (task cas-1317 notes) recorded a worker that finished its turn normally,
/// sat idle for ~3 minutes, then received an urgent that started nothing —
/// no new rollout record ever appeared. v2.31.0's fix
/// (`wait_for_injection_readiness`'s quiescence poll) was verified only
/// against the mid-turn case; for an idle pane the poll is satisfied almost
/// immediately, so there's a real chance it reduces to the pre-fix
/// behavior and does NOT cover this variant. This test exercises that path
/// end to end through the real shipped code
/// (`Mux::interrupt_and_inject` / `Pane::break_turn`) against a real `codex`
/// child.
///
/// Run explicitly:
///   cargo test -p cas-mux --test urgent_interrupt_codex_runtime -- --ignored --nocapture already_idle
#[test]
#[ignore = "spawns a real `codex` CLI child process — run explicitly, see module docs"]
fn already_idle_urgent_interrupt_starts_new_turn() {
    if !codex_available() {
        eprintln!("SKIP: `codex` not on PATH");
        return;
    }

    let scratch = std::env::temp_dir().join(format!(
        "cas-1317-codex-runtime-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("create scratch dir");
    let _ = std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(&scratch)
        .status();

    let config = PtyConfig {
        command: "codex".to_string(),
        args: vec!["--yolo".to_string(), "--no-alt-screen".to_string()],
        cwd: Some(scratch.clone()),
        env: vec![],
        rows: 24,
        cols: 80,
    };
    let pty = Pty::spawn("cas1317-codex-rt", config).expect("spawn codex pty");
    let pane = Pane::with_pty("cas1317-codex-rt", PaneKind::Worker, pty, 24, 80, SupervisorCli::Codex)
        .expect("wrap pty in pane");
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");

    // Drain startup (trust prompt, banner) and accept the trust prompt.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let _ = mux.poll_batch();
        std::thread::sleep(Duration::from_millis(200));
    }
    rt.block_on(mux.get("cas1317-codex-rt").unwrap().write(b"\r"))
        .ok();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let _ = mux.poll_batch();
        std::thread::sleep(Duration::from_millis(200));
    }

    let rollout_deadline = std::time::Instant::now() + Duration::from_secs(15);

    // First turn: short and deterministic, so it completes quickly and
    // NORMALLY (no cancel involved at all).
    rt.block_on(mux.inject(
        "cas1317-codex-rt",
        "Reply with exactly the text FIRST-TURN-DONE and nothing else.",
    ))
    .expect("start first turn");

    let rollout = find_rollout_containing(
        scratch.to_str().expect("utf8 scratch path"),
        rollout_deadline,
    )
    .expect("rollout file for this scratch cwd must appear");

    // Wait for the first turn to genuinely end on its own — both the reply
    // text AND an explicit `task_complete` record, so we know this is a
    // normal completion and not us racing a still-in-flight turn.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut contents = String::new();
    let mut first_turn_done = false;
    while std::time::Instant::now() < deadline {
        let _ = mux.poll_batch();
        contents.clear();
        if let Ok(mut f) = std::fs::File::open(&rollout) {
            let _ = f.read_to_string(&mut contents);
        }
        if contents.contains("FIRST-TURN-DONE") && contents.contains("\"type\":\"task_complete\"")
        {
            first_turn_done = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    assert!(
        first_turn_done,
        "first turn must complete normally (task_complete) before we can test the \
         idle case — rollout tail:\n{}",
        contents.lines().rev().take(20).collect::<Vec<_>>().join("\n")
    );

    // Let the pane go genuinely idle: no more PTY output, well past the
    // v2.31.0 quiescence poll's own STABILITY_WINDOW (300ms) and MAX_EXTRA_WAIT
    // (4s), so this urgent cannot benefit from any leftover in-flight state.
    let idle_until = std::time::Instant::now() + Duration::from_secs(8);
    while std::time::Instant::now() < idle_until {
        let _ = mux.poll_batch();
        std::thread::sleep(Duration::from_millis(200));
    }

    let first_user_messages = contents.matches("\"type\":\"user_message\"").count();
    let first_completions = contents.matches("\"type\":\"task_complete\"").count();

    // cas-a5a7 AC4: exercise the exact normal coordination-message PTY shape
    // first. The literal framing is what `frame_pty_payload` supplies for a
    // Codex recipient. Count rollout records instead of searching for the
    // requested reply token: that token also occurs in the injected prompt,
    // so a swallowed composer draft could otherwise produce a false pass.
    rt.block_on(mux.inject(
        "cas1317-codex-rt",
        "Message from supervisor: Reply with exactly IDLE-NONURGENT-OK and nothing else.",
    ))
    .expect("normal coordination inject must succeed");

    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    let mut nonurgent_observed = false;
    while std::time::Instant::now() < deadline {
        let _ = mux.poll_batch();
        contents.clear();
        if let Ok(mut f) = std::fs::File::open(&rollout) {
            let _ = f.read_to_string(&mut contents);
        }
        let user_messages = contents.matches("\"type\":\"user_message\"").count();
        let completions = contents.matches("\"type\":\"task_complete\"").count();
        if user_messages > first_user_messages && completions > first_completions {
            nonurgent_observed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    assert!(
        nonurgent_observed,
        "cas-a5a7: a normal coordination message to an ALREADY-IDLE Codex worker \
         must create a new user_message and complete a new turn. Rollout tail:\n{}",
        contents.lines().rev().take(30).collect::<Vec<_>>().join("\n")
    );

    // Return to a genuinely idle prompt before testing urgent delivery.
    let idle_until = std::time::Instant::now() + Duration::from_secs(8);
    while std::time::Instant::now() < idle_until {
        let _ = mux.poll_batch();
        std::thread::sleep(Duration::from_millis(200));
    }
    let before_urgent_user_messages = contents.matches("\"type\":\"user_message\"").count();
    let before_urgent_completions = contents.matches("\"type\":\"task_complete\"").count();

    eprintln!(
        "[cas-1317] pane bytes_received before urgent: {:?}",
        mux.pane_bytes_received("cas1317-codex-rt")
    );

    // The real production floor (queue_and_events.rs urgent_settle_duration
    // default), not the deliberately-tiny one the mid-turn test uses — this
    // test is about the idle-pane shape, not the settle-race shape.
    let production_floor = Duration::from_millis(1200);
    rt.block_on(mux.interrupt_and_inject(
        "cas1317-codex-rt",
        "Message from supervisor: URGENT: reply with exactly IDLE-URGENT-OK and nothing else.",
        production_floor,
    ))
    .expect("interrupt_and_inject must succeed (no PTY/IO error)");

    // A successful write is not sufficient: the rollout must show both a
    // fresh user_message and a completed fresh turn.
    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    let mut urgent_observed = false;
    while std::time::Instant::now() < deadline {
        let _ = mux.poll_batch();
        contents.clear();
        if let Ok(mut f) = std::fs::File::open(&rollout) {
            let _ = f.read_to_string(&mut contents);
        }
        let user_messages = contents.matches("\"type\":\"user_message\"").count();
        let completions = contents.matches("\"type\":\"task_complete\"").count();
        if user_messages > before_urgent_user_messages && completions > before_urgent_completions {
            urgent_observed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    assert!(
        urgent_observed,
        "cas-a5a7: an urgent coordination message to an ALREADY-IDLE Codex \
         worker must create a new user_message and complete a new turn. Rollout tail:\n{}",
        contents.lines().rev().take(30).collect::<Vec<_>>().join("\n")
    );

    let _ = std::fs::remove_dir_all(&scratch);
}
