//! Shared live-`codex` PTY harness for the idle-delivery runtime tests
//! (cas-5fff).
//!
//! Every helper here drives the **real** `codex` binary through a real PTY via
//! the shipped `Mux`/`Pane` code. Task cas-5fff exists because cas-893c's
//! negative result ("the Codex PTY path is NOT the cause") was established
//! against an interactive **bash** stand-in — bash accepts a bare
//! write-then-CR, a full-screen Codex TUI with its own composer state machine
//! does not. Nothing in this module may substitute another process for
//! `codex`; that substitution is precisely what let the bug survive.

#![allow(dead_code)]

use cas_mux::{Mux, Pane, PaneKind, Pty, PtyConfig, SupervisorCli};
use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub fn codex_available() -> bool {
    std::process::Command::new("codex")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn codex_sessions_root() -> PathBuf {
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

/// Newest rollout file mentioning `needle` (our scratch cwd) so a concurrently
/// running Codex session on this shared dev box can't be mistaken for ours.
fn find_rollout_containing(needle: &str, deadline: Instant) -> Option<PathBuf> {
    let root = codex_sessions_root();
    while Instant::now() < deadline {
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

/// A booted, trusted, first-turn-completed live Codex pane plus its rollout
/// file — the exact starting state every cas-5fff scenario needs.
pub struct CodexLive {
    pub mux: Mux,
    pub rt: tokio::runtime::Runtime,
    pub id: String,
    pub rollout: PathBuf,
    scratch: PathBuf,
}

/// Rollout record counts — the only trustworthy "did a turn happen" signal.
///
/// Searching for a reply token is NOT sufficient: an injected prompt that is
/// swallowed into the composer as an unsent draft still puts its own text
/// (including any token it asks for) into the pane, and a resumed rollout can
/// echo it. Counting `user_message` + `task_complete` records is what the
/// cas-1317 harness settled on and what the field evidence in cas-5fff is
/// expressed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnCounts {
    pub user_messages: usize,
    pub completions: usize,
}

impl CodexLive {
    /// Boot codex in a fresh scratch git repo, accept the trust prompt, run one
    /// short turn to completion, and let the pane go genuinely idle.
    pub fn boot(tag: &str) -> Option<Self> {
        if !codex_available() {
            eprintln!("SKIP: `codex` not on PATH");
            return None;
        }

        let scratch = std::env::temp_dir().join(format!("{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).expect("create scratch dir");
        let _ = std::process::Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(&scratch)
            .status();

        let id = format!("{tag}-pane");
        let config = PtyConfig {
            command: "codex".to_string(),
            args: vec!["--yolo".to_string(), "--no-alt-screen".to_string()],
            cwd: Some(scratch.clone()),
            env: vec![],
            rows: 24,
            cols: 80,
        };
        let pty = Pty::spawn(&id, config).expect("spawn codex pty");
        let pane = Pane::with_pty(&id, PaneKind::Worker, pty, 24, 80, SupervisorCli::Codex)
            .expect("wrap pty in pane");
        let mut mux = Mux::new(24, 80);
        mux.add_pane(pane);
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");

        // Drain startup (trust prompt, banner) then accept the trust prompt.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let _ = mux.poll_batch();
            std::thread::sleep(Duration::from_millis(200));
        }
        rt.block_on(mux.get(&id).unwrap().write(b"\r")).ok();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let _ = mux.poll_batch();
            std::thread::sleep(Duration::from_millis(200));
        }

        let rollout_deadline = Instant::now() + Duration::from_secs(20);

        rt.block_on(mux.inject(
            &id,
            "Reply with exactly the text FIRST-TURN-DONE and nothing else.",
        ))
        .expect("start first turn");

        let rollout = find_rollout_containing(
            scratch.to_str().expect("utf8 scratch path"),
            rollout_deadline,
        )
        .expect("rollout file for this scratch cwd must appear");

        let mut live = CodexLive {
            mux,
            rt,
            id,
            rollout,
            scratch,
        };

        // The first turn must end NORMALLY (task_complete), so the idle state
        // under test is a real post-completion idle, not a cancel.
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut done = false;
        while Instant::now() < deadline {
            live.pump(Duration::from_millis(300));
            let c = live.rollout_contents();
            if c.contains("FIRST-TURN-DONE") && c.contains("\"type\":\"task_complete\"") {
                done = true;
                break;
            }
        }
        assert!(
            done,
            "first turn must complete normally before the idle case can be tested — tail:\n{}",
            live.rollout_tail(20)
        );

        // Go genuinely idle, well past the quiescence poll's STABILITY_WINDOW
        // (300ms) and MAX_EXTRA_WAIT (4s).
        live.pump(Duration::from_secs(8));
        Some(live)
    }

    /// Drain the pane for `dur`, exactly as the daemon's main loop does.
    pub fn pump(&mut self, dur: Duration) {
        let until = Instant::now() + dur;
        while Instant::now() < until {
            let _ = self.mux.poll_batch();
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    pub fn rollout_contents(&self) -> String {
        let mut s = String::new();
        if let Ok(mut f) = std::fs::File::open(&self.rollout) {
            let _ = f.read_to_string(&mut s);
        }
        s
    }

    pub fn rollout_tail(&self, lines: usize) -> String {
        let c = self.rollout_contents();
        let tail: Vec<&str> = c.lines().rev().take(lines).collect();
        tail.into_iter()
            .rev()
            .map(|l| l.chars().take(240).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The pane's rendered viewport as plain text.
    ///
    /// The decisive diagnostic for this class of bug: it shows whether an
    /// injected payload is sitting in Codex's composer as an unsubmitted draft
    /// (write landed, submit lost) versus never having arrived at all.
    pub fn screen_text(&self) -> String {
        let Some(pane) = self.mux.get(&self.id) else {
            return String::new();
        };
        let Ok(lines) = pane.viewport_as_lines() else {
            return String::new();
        };
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn counts(&self) -> TurnCounts {
        let c = self.rollout_contents();
        TurnCounts {
            user_messages: c.matches("\"type\":\"user_message\"").count(),
            completions: c.matches("\"type\":\"task_complete\"").count(),
        }
    }

    /// Wait until the rollout shows BOTH a fresh `user_message` and a fresh
    /// `task_complete` past `before`. Returns false on timeout.
    pub fn wait_for_new_completed_turn(&mut self, before: TurnCounts, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            self.pump(Duration::from_millis(300));
            let now = self.counts();
            if now.user_messages > before.user_messages && now.completions > before.completions {
                return true;
            }
        }
        false
    }
}

impl Drop for CodexLive {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

/// A production-shaped supervisor→worker merge/re-close handoff.
///
/// Deliberately NOT a one-liner: every message that failed in the cas-5fff
/// field evidence (queue rows 6398/6399/6400) was 1.8-2.9 KB with 8-12
/// newlines, inline backticks and blank lines. Reproduced at that shape here
/// because the one thing the pre-existing cas-a5a7 idle assertion covered was
/// a single short line, which is not what the factory actually sends.
pub fn production_shaped_handoff(token: &str) -> String {
    format!(
        "Message from supervisor: Merged. factory/quiet-cobra-19 fast-forwarded into \
epic/epic-2026-07-31-factory-signal-fidelity-supervisor-cas-8c9c at \
e3959af64d2e4fd45979dda2db49478c5e2fcd98 — the exact tip you declared. \
Re-close now: `task action=close id=cas-ae2f`.\n\n\
Reviewed before merging, and it holds up. Specifically: the decision note names the \
chosen path instead of leaving it implicit; you did the trusted-spawn stamp AND the \
env mirror rather than only one, which is defensible since they revive the two \
independent resolution paths; the blast-radius audit covers all six gates with line \
ranges; and AC4 is answered with a real counterexample rather than an assertion.\n\n\
Two things to carry forward:\n\n\
1. When a ticket hands you a file list, treat it as a hypothesis and regenerate it. \
Two of the three lists this round were wrong.\n\
2. Put the proof command and its exit code in the close reason, not a summary of it.\n\n\
Nothing else is queued for you after this. Reply with exactly {token} and nothing \
else once you have read this."
    )
}

/// The largest shape observed in the field: queue row 6400 was 2907 bytes with
/// 12 newlines. Padded from the standard handoff so a size regression in the
/// framing contract is caught at the real upper bound, not just near it.
pub fn production_shaped_handoff_xl(token: &str) -> String {
    let base = production_shaped_handoff(token);
    let filler = "\n\nAdditional review detail you should carry into the next task: the \
sentinel you regenerated is the artifact that matters, not the ticket's copy of it; \
when the two disagree the ticket is the stale one and the close reason should say so \
explicitly with the regenerating command inlined.";
    let (head, tail) = base
        .split_once("Nothing else is queued")
        .expect("handoff template contains its closing paragraph");
    let mut out = String::with_capacity(3000);
    out.push_str(head);
    while out.len() + filler.len() + tail.len() + 24 < 2907 {
        out.push_str(filler);
    }
    out.push_str("\n\nNothing else is queued");
    out.push_str(tail);
    out
}
