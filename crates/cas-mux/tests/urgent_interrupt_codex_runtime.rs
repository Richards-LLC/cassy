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
