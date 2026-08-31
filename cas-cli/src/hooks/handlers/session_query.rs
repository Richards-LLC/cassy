//! What a SessionStart knows about the session it is starting (cas-3b80).
//!
//! `build_start.rs` assembles the `ContextQuery` that decides which memories a
//! session receives. Everything it can read from the hook input alone —
//! `user_prompt` and the in-progress task list — is either absent at
//! SessionStart or project-wide, which is why cas-b06c measured the shipped
//! Helpful-Memories ranking as *identical for all 56 labeled eval contexts* in
//! the fresh-session regime: `ContextQuery::has_content()` was false, so
//! `HybridContextScorer` early-returned onto the query-blind
//! `BasicContextScorer`.
//!
//! This module supplies the three session facts that fix that, all of them
//! already recorded by Cassy for other reasons:
//!
//! * **carried prompt** — every UserPromptSubmit is written to the prompt store
//!   for blame attribution. The most recent one is the project's last stated
//!   intent, and for a session that has not spoken yet it is the only one.
//! * **recent files** — `session_files.json` is wiped by the Stop hook (i.e.
//!   after *every* assistant turn), so a SessionStart essentially never saw it.
//!   The wipe now keeps a copy, which a new session reads as a fallback.
//! * **git branch** — a fresh session has no task and no prompt, but it is
//!   almost always on a branch, and in an epic/factory workflow the branch name
//!   states the topic.
//!
//! Each is a weak signal on its own. The point is not their strength; it is
//! that ranking on a weak signal is query-*dependent*, and the alternative
//! measured at 0.0071 precision@5 with exactly one distinct ranking.

use std::path::Path;

use cas_core::hooks::types::HookInput;

/// How stale the carried-forward prompt may be before a new session ignores it.
///
/// Long enough to cover an overnight or weekend gap in the same piece of work;
/// short enough that a project picked up a month later does not have its memory
/// ranking steered by whatever was being done back then.
pub const CARRIED_PROMPT_MAX_AGE_HOURS: i64 = 72;

/// Prompts shorter than this are acknowledgements ("yes", "go on", "ok do it")
/// and carry no topic. The same threshold `prompt_capture.rs` uses to decide a
/// prompt is not worth extracting context from.
const CARRIED_PROMPT_MIN_CHARS: usize = 20;

/// File the Stop hook copies `session_files.json` into before wiping it.
pub const PREVIOUS_SESSION_FILES: &str = "previous_session_files.json";

/// The session facts that seed the SessionStart `ContextQuery`.
#[derive(Debug, Clone, Default)]
pub struct SessionQueryContext {
    /// Files this session — or, failing that, the previous one — touched.
    pub recent_files: Vec<String>,
    /// The last prompt this project saw, if recent enough to still describe
    /// what is going on.
    pub carried_prompt: Option<String>,
    /// Branch checked out at the session's cwd.
    pub git_branch: Option<String>,
    /// The reading agent, used to scope task titles to its own work.
    pub agent_id: Option<String>,
}

/// Assemble the session facts for `cas_root` / `input`.
///
/// Every lookup is best-effort: a missing store, an unreadable file or a
/// non-repository cwd degrades that one signal to `None` and never fails the
/// hook.
pub fn session_query_context(cas_root: &Path, input: &HookInput) -> SessionQueryContext {
    SessionQueryContext {
        recent_files: session_context_files(cas_root),
        carried_prompt: carried_forward_prompt(cas_root),
        git_branch: session_git_branch(&input.cwd),
        agent_id: Some(super::current_agent_id(input)).filter(|id| !id.is_empty()),
    }
}

/// Files for the context query: this session's, else the previous session's.
///
/// The fallback is what makes a *fresh* session file-aware at all — see the
/// module docs on the Stop-hook wipe.
pub fn session_context_files(cas_root: &Path) -> Vec<String> {
    let current = super::get_session_files(cas_root);
    if !current.is_empty() {
        return current;
    }
    read_file_list(&cas_root.join(PREVIOUS_SESSION_FILES))
}

/// Preserve the current session's file list before it is wiped.
///
/// Called by `clear_session_files`; a failure leaves the previous snapshot in
/// place, which is a strictly better fallback than none.
pub fn archive_session_files(cas_root: &Path) {
    let current = super::get_session_files(cas_root);
    if current.is_empty() {
        return;
    }
    if let Ok(json) = serde_json::to_string(&current) {
        let _ = std::fs::write(cas_root.join(PREVIOUS_SESSION_FILES), json);
    }
}

fn read_file_list(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// The most recent captured prompt, if it is recent and substantive.
pub fn carried_forward_prompt(cas_root: &Path) -> Option<String> {
    let store = crate::store::open_prompt_store(cas_root).ok()?;
    let prompt = store.list_recent(1).ok()?.into_iter().next()?;
    carried_prompt_from(prompt.content, prompt.timestamp, chrono::Utc::now())
}

/// The freshness/substance rule, separated so it can be tested without a store.
pub fn carried_prompt_from(
    content: String,
    captured_at: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    if content.chars().count() < CARRIED_PROMPT_MIN_CHARS {
        return None;
    }
    if (now - captured_at).num_hours() > CARRIED_PROMPT_MAX_AGE_HOURS {
        return None;
    }
    Some(content)
}

/// Branch checked out at `cwd`, read straight from `.git` rather than through a
/// `git` subprocess: SessionStart already opens six SQLite stores and a Tantivy
/// index, and this signal is not worth a process spawn.
///
/// Handles the worktree layout (`.git` is a file pointing at the real gitdir),
/// which is the layout every factory worker runs in.
pub fn session_git_branch(cwd: &str) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }
    let git_dir = resolve_git_dir(Path::new(cwd))?;
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let branch = head.trim().strip_prefix("ref: refs/heads/")?;
    if branch.is_empty() {
        return None;
    }
    Some(branch.to_string())
}

fn resolve_git_dir(start: &Path) -> Option<std::path::PathBuf> {
    let mut dir = Some(start);
    while let Some(current) = dir {
        let candidate = current.join(".git");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if candidate.is_file() {
            // Worktree/submodule: ".git" holds "gitdir: <path>".
            let raw = std::fs::read_to_string(&candidate).ok()?;
            let target = raw.trim().strip_prefix("gitdir:")?.trim();
            let path = Path::new(target);
            return Some(if path.is_absolute() {
                path.to_path_buf()
            } else {
                current.join(path)
            });
        }
        dir = current.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    #[test]
    fn a_fresh_session_falls_back_to_the_previous_sessions_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();

        // Nothing recorded at all.
        assert!(session_context_files(root).is_empty());

        // A session touches files; the Stop hook wipes the live list but the
        // archive survives for the next SessionStart.
        std::fs::write(
            root.join("session_files.json"),
            r#"["cas-cli/src/hooks/scorer.rs"]"#,
        )
        .expect("write session files");
        archive_session_files(root);
        std::fs::remove_file(root.join("session_files.json")).expect("wipe");

        assert_eq!(
            session_context_files(root),
            vec!["cas-cli/src/hooks/scorer.rs".to_string()]
        );

        // A live list always wins over the archive.
        std::fs::write(root.join("session_files.json"), r#"["docs/RELEASE.md"]"#)
            .expect("write session files");
        assert_eq!(
            session_context_files(root),
            vec!["docs/RELEASE.md".to_string()]
        );
    }

    #[test]
    fn the_carried_prompt_must_be_recent_and_substantive() {
        let now = Utc::now();
        let real = "Fix the retrieval eval harness baseline".to_string();

        assert_eq!(
            carried_prompt_from(real.clone(), now - Duration::hours(5), now),
            Some(real.clone())
        );
        assert_eq!(
            carried_prompt_from(
                real.clone(),
                now - Duration::hours(CARRIED_PROMPT_MAX_AGE_HOURS + 1),
                now
            ),
            None,
            "a stale prompt must not steer a new session's memory ranking"
        );
        assert_eq!(carried_prompt_from("ok go".to_string(), now, now), None);
    }

    #[test]
    fn the_branch_is_read_from_a_worktree_git_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let real_git = temp.path().join("real-git");
        std::fs::create_dir_all(&real_git).expect("mkdir");
        std::fs::write(real_git.join("HEAD"), "ref: refs/heads/epic/memory-eval\n")
            .expect("write HEAD");

        let worktree = temp.path().join("worktree");
        std::fs::create_dir_all(worktree.join("nested/dir")).expect("mkdir");
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", real_git.display()),
        )
        .expect("write .git");

        assert_eq!(
            session_git_branch(worktree.join("nested/dir").to_str().expect("utf8")),
            Some("epic/memory-eval".to_string())
        );
        assert_eq!(session_git_branch(""), None);
        assert_eq!(
            session_git_branch(temp.path().to_str().expect("utf8")),
            None,
            "a cwd outside any repository has no branch"
        );
    }

    #[test]
    fn a_detached_head_contributes_no_branch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let git_dir = temp.path().join(".git");
        std::fs::create_dir_all(&git_dir).expect("mkdir");
        std::fs::write(git_dir.join("HEAD"), "c24f43c5deadbeef\n").expect("write HEAD");

        assert_eq!(
            session_git_branch(temp.path().to_str().expect("utf8")),
            None
        );
    }
}
