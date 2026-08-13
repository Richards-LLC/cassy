//! Real context reset for factory workers (cas-dffe, GH #145).
//!
//! # What was broken
//!
//! `coordination action=clear_context` used to enqueue the four characters
//! `/clear` as an ordinary `prompt_queue` message. For a Claude worker under
//! native Agent Teams that row is routed to the team **inbox** (a file), so the
//! worker read the string "/clear" as a teammate message, acknowledged it, and
//! carried on with its entire conversation still loaded. The tool nonetheless
//! reported `Queued /clear for <worker>`, so the failure was invisible to the
//! supervisor: the checkpoint-and-clear discipline silently degraded into
//! working context to exhaustion.
//!
//! # What replaces it
//!
//! A context reset is a **control command**, not a message. It is queued with
//! the [`CONTEXT_RESET_CONTROL`] sentinel as its payload, and the factory daemon
//! recognises that sentinel *before* any message routing runs: it never reaches
//! an inbox, and instead of typing the payload it types the recipient harness's
//! own reset command ([`context_reset_command`]) into the pane over the PTY —
//! the same interrupt-and-inject channel urgent messages use.
//!
//! # Why `/clear` over the PTY is the harness's real command channel
//!
//! Measured live against `claude` 2.1.224 in a real PTY (see the task record):
//! writing the bytes `/clear` followed by a CR into a booted Claude Code TUI
//! opens its slash-command menu with `/clear` as the first entry and the CR
//! submits it — the command executes for real. Claude Code then starts a NEW
//! session: it writes a new transcript file, under a new session id, whose head
//! carries a literal [`CLEAR_COMMAND_MARKER`] record. That file is the
//! verifiable post-condition this module exposes to the caller
//! ([`detect_context_reset`]), so a reset can be **confirmed** rather than
//! assumed — and a reset that cannot be confirmed is reported as a failure
//! instead of a cheerful "queued".
//!
//! A harness with no verified in-place reset command (Codex, Grok) returns
//! `None` from [`context_reset_command`]: the caller must say so rather than
//! claim a success it cannot deliver.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cas_mux::SupervisorCli;

/// Payload marking a `prompt_queue` row as a context-reset control command.
///
/// Deliberately not human prose: the daemon matches it exactly and hard-routes
/// the row to the PTY, so this text can never be delivered to a recipient as
/// readable content. The `\u{1}` sentinels keep it out of the space of things a
/// supervisor could type by hand into `action=message`.
pub const CONTEXT_RESET_CONTROL: &str = "\u{1}cas-control:context-reset\u{1}";

/// Summary shown for a queued context-reset row.
pub const CONTEXT_RESET_SUMMARY: &str = "context reset (control command)";

/// The literal record Claude Code writes into the transcript of the session a
/// `/clear` starts. Verified live (cas-dffe) and cross-checked against
/// pre-existing factory transcripts.
pub const CLEAR_COMMAND_MARKER: &str = "<command-name>/clear</command-name>";

/// How much of a candidate transcript's head is scanned for
/// [`CLEAR_COMMAND_MARKER`]. The marker lands within the first handful of
/// records (session mode, file-history snapshot, SessionStart hook attachments,
/// then the command itself), so a bounded read keeps this cheap even when the
/// projects directory holds multi-megabyte transcripts.
const TRANSCRIPT_HEAD_BYTES: usize = 64 * 1024;

/// Whether a queued prompt is the context-reset control command.
pub fn is_context_reset_control(prompt: &str) -> bool {
    prompt.trim() == CONTEXT_RESET_CONTROL
}

/// The harness's own in-place context-reset command, if one has been verified.
///
/// `Some` only for harnesses where the command was actually measured against
/// the real binary. Returning `None` is a load-bearing answer: it is what makes
/// `clear_context` refuse rather than pretend (GH #145 AC2).
pub fn context_reset_command(cli: SupervisorCli) -> Option<&'static str> {
    match cli {
        SupervisorCli::Claude => Some("/clear"),
        // No in-place reset command has been verified for these harnesses.
        // `codex` and `grok` each have their own new-conversation affordances,
        // but neither has been measured here, and an unverified guess typed
        // into a pane is exactly the silent-failure mode this task removes.
        SupervisorCli::Codex | SupervisorCli::Grok => None,
    }
}

/// Why a context reset is impossible for `cli`, phrased for the supervisor.
pub fn unsupported_reason(cli: SupervisorCli) -> String {
    format!(
        "harness '{}' has no verified in-place context-reset command, so CAS cannot reset it \
         and will not report a reset it did not perform. Use shutdown_workers + spawn_workers \
         (same name/worktree) to recycle the worker instead.",
        cli.backend().name()
    )
}

/// The Claude config roots to search for a worker's transcripts, most specific
/// first.
///
/// A factory worker inherits `CLAUDE_CONFIG_DIR` from the supervisor that
/// spawned it (`spawn_workers` captures it at enqueue time), so the supervisor's
/// own value is the best guess; `~/.claude` is the default install location and
/// is always tried as well.
pub fn claude_config_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        let dir = dir.trim();
        if !dir.is_empty() {
            roots.push(PathBuf::from(dir));
        }
    }
    if let Some(home) = dirs::home_dir() {
        let default = home.join(".claude");
        if !roots.contains(&default) {
            roots.push(default);
        }
    }
    roots
}

/// Claude Code's project-directory name for a working directory: every `/` and
/// `.` collapsed to `-`. Mirrors `synthesized_transcript_path` in
/// `factory_ops`, kept here so the reset verifier does not depend on the MCP
/// tool module.
pub fn claude_project_slug(clone_path: &str) -> String {
    clone_path
        .chars()
        .map(|c| match c {
            '/' | '.' => '-',
            other => other,
        })
        .collect()
}

/// Every existing `<config-root>/projects/<slug>` directory for `clone_path`.
///
/// Returns all matches rather than the first: a worker's transcripts live under
/// exactly one root in practice, but which root that is depends on the
/// worker's inherited `CLAUDE_CONFIG_DIR`, which the agent record does not
/// carry. Watching both is cheap and cannot produce a false positive — the
/// evidence check requires a transcript that literally records a `/clear`.
pub fn transcript_dirs_for(clone_path: &str) -> Vec<PathBuf> {
    let slug = claude_project_slug(clone_path);
    claude_config_roots()
        .into_iter()
        .map(|root| root.join("projects").join(&slug))
        .filter(|dir| dir.is_dir())
        .collect()
}

/// Snapshot the transcript files currently present in `dirs`.
///
/// The reset check is "a transcript that did not exist before, that records a
/// `/clear`" — so the caller takes this snapshot *before* queueing the control
/// command.
pub fn snapshot_transcripts(dirs: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mut seen = BTreeSet::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                seen.insert(path);
            }
        }
    }
    seen
}

/// Whether a transcript's head records the `/clear` that started its session.
pub fn transcript_records_clear(path: &Path) -> bool {
    use std::io::Read;

    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = vec![0u8; TRANSCRIPT_HEAD_BYTES];
    let Ok(read) = file.read(&mut buf) else {
        return false;
    };
    buf.truncate(read);
    String::from_utf8_lossy(&buf).contains(CLEAR_COMMAND_MARKER)
}

/// Evidence that a context reset actually happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextResetEvidence {
    /// The session id of the conversation the reset started — the file stem of
    /// the new transcript, which is Claude Code's session UUID.
    pub session_id: String,
    /// Path to that transcript.
    pub transcript: PathBuf,
}

/// Look for a transcript that appeared after `before` was snapshotted and whose
/// head records a `/clear`.
///
/// Both conditions are required. "A new transcript exists" alone is not
/// evidence — a worker can start a fresh session for unrelated reasons — and
/// the marker alone is not evidence either, because a pre-existing transcript
/// may record a `/clear` from an earlier day.
pub fn detect_context_reset(
    dirs: &[PathBuf],
    before: &BTreeSet<PathBuf>,
) -> Option<ContextResetEvidence> {
    let mut candidates: Vec<PathBuf> = snapshot_transcripts(dirs)
        .into_iter()
        .filter(|path| !before.contains(path))
        .collect();
    // Newest first: if a worker somehow produced more than one new session,
    // the reset we asked for is the most recent one.
    candidates.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
    });
    candidates.reverse();

    candidates
        .into_iter()
        .find(|path| transcript_records_clear(path))
        .and_then(|transcript| {
            let session_id = transcript.file_stem()?.to_string_lossy().to_string();
            Some(ContextResetEvidence {
                session_id,
                transcript,
            })
        })
}

/// How long `clear_context` waits for the post-condition before declaring the
/// reset unconfirmed. Overridable via `CAS_CONTEXT_RESET_TIMEOUT_SECS` (tests
/// and impatient operators).
pub fn confirmation_timeout() -> std::time::Duration {
    const DEFAULT_SECS: u64 = 25;
    let secs = std::env::var("CAS_CONTEXT_RESET_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_SECS);
    std::time::Duration::from_secs(secs)
}

/// Poll interval while waiting for the post-condition.
pub const CONFIRMATION_POLL: std::time::Duration = std::time::Duration::from_millis(400);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_sentinel_is_not_plain_text() {
        // The regression this task exists for: a row whose payload is the
        // literal "/clear" is an ordinary message and will be delivered as
        // readable text. The control command must never be that shape.
        assert!(!is_context_reset_control("/clear"));
        assert!(!is_context_reset_control("please run /clear"));
        assert!(is_context_reset_control(CONTEXT_RESET_CONTROL));
        // Trailing whitespace from a round-trip through the queue is tolerated.
        assert!(is_context_reset_control(&format!(
            "{CONTEXT_RESET_CONTROL}\n"
        )));
    }

    #[test]
    fn only_claude_has_a_verified_reset_command() {
        assert_eq!(context_reset_command(SupervisorCli::Claude), Some("/clear"));
        assert_eq!(context_reset_command(SupervisorCli::Codex), None);
        assert_eq!(context_reset_command(SupervisorCli::Grok), None);
        // The refusal must name the harness and the alternative.
        let reason = unsupported_reason(SupervisorCli::Codex);
        assert!(reason.contains("codex"), "{reason}");
        assert!(reason.contains("shutdown_workers"), "{reason}");
    }

    #[test]
    fn project_slug_matches_claude_code_layout() {
        assert_eq!(
            claude_project_slug("/home/u/Petra/cas-src/.cas/worktrees/rapid-dragon-50"),
            "-home-u-Petra-cas-src--cas-worktrees-rapid-dragon-50"
        );
    }

    fn write_transcript(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn reset_is_confirmed_only_by_a_new_transcript_that_records_the_clear() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();
        let dirs = vec![dir.clone()];

        // Pre-existing transcript, including one that recorded a /clear in a
        // previous life — neither may count as evidence for THIS reset.
        write_transcript(&dir, "old-session.jsonl", "{\"type\":\"user\"}\n");
        write_transcript(
            &dir,
            "older-clear.jsonl",
            &format!("{{\"content\":\"{CLEAR_COMMAND_MARKER}\"}}\n"),
        );
        let before = snapshot_transcripts(&dirs);
        assert_eq!(before.len(), 2);

        // Nothing new yet → unconfirmed.
        assert_eq!(detect_context_reset(&dirs, &before), None);

        // A new session that is NOT a clear (e.g. a resume) is not evidence.
        write_transcript(&dir, "unrelated-new.jsonl", "{\"type\":\"user\"}\n");
        assert_eq!(detect_context_reset(&dirs, &before), None);

        // The real thing.
        let expected = write_transcript(
            &dir,
            "9f0d2b7e-post-clear.jsonl",
            &format!("{{\"type\":\"mode\"}}\n{{\"content\":\"{CLEAR_COMMAND_MARKER}\"}}\n"),
        );
        let evidence = detect_context_reset(&dirs, &before).expect("reset must be confirmed");
        assert_eq!(evidence.transcript, expected);
        assert_eq!(evidence.session_id, "9f0d2b7e-post-clear");
    }

    #[test]
    fn confirmation_timeout_is_overridable() {
        let _lock = crate::hooks::test_env_lock();
        let previous = std::env::var("CAS_CONTEXT_RESET_TIMEOUT_SECS").ok();
        unsafe {
            std::env::set_var("CAS_CONTEXT_RESET_TIMEOUT_SECS", "2");
        }
        assert_eq!(confirmation_timeout(), std::time::Duration::from_secs(2));
        unsafe {
            match previous {
                Some(value) => std::env::set_var("CAS_CONTEXT_RESET_TIMEOUT_SECS", value),
                None => std::env::remove_var("CAS_CONTEXT_RESET_TIMEOUT_SECS"),
            }
        }
    }
}
