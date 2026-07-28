use crate::hooks::handlers::*;

pub fn generate_file_change_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let random: u32 = rand::random();
    format!("fc-{:x}-{:04x}", timestamp, random & 0xFFFF)
}

/// Compute content hash using SHA-256
pub fn compute_content_hash(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Get the repository name from the current directory
pub fn get_repository_name() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Normalize an absolute file path to a relative path
///
/// Enables cross-clone file tracking in factory mode by stripping the
/// clone-specific directory prefix. Handles both main repo and worktree/clone paths.
///
/// Examples:
/// - `/Users/user/project/src/foo.rs` -> `src/foo.rs` (main repo)
/// - `/Users/user/worktrees/swift-fox/src/foo.rs` -> `src/foo.rs` (clone)
pub fn normalize_to_relative_path(cas_root: &std::path::Path, file_path: &str) -> String {
    use std::path::Path;

    let path = Path::new(file_path);

    // If already relative, return as-is
    if path.is_relative() {
        return file_path.to_string();
    }

    // Try to strip the project root (parent of .cas directory)
    if let Some(project_root) = cas_root.parent() {
        if let Ok(relative) = path.strip_prefix(project_root) {
            return relative.to_string_lossy().to_string();
        }
    }

    // Try to strip current working directory (handles clone directories)
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(relative) = path.strip_prefix(&cwd) {
            return relative.to_string_lossy().to_string();
        }
    }

    // Fallback: return original path
    file_path.to_string()
}

/// Capture a file change for attribution tracking
///
/// Called from PostToolUse for Write and Edit tools.
/// Records which file was changed and links to the current prompt/session.
pub fn capture_file_change_for_attribution(
    cas_root: &std::path::Path,
    input: &HookInput,
    tool_name: &str,
) {
    // Get tool input
    let tool_input = match &input.tool_input {
        Some(ti) => ti,
        None => return,
    };

    // Extract file path and normalize to relative path
    let file_path_raw = match tool_input.get("file_path").and_then(|v| v.as_str()) {
        Some(fp) => fp,
        None => return,
    };

    // Normalize absolute paths to relative paths for cross-clone compatibility
    let file_path = normalize_to_relative_path(cas_root, file_path_raw);

    // Open the file change store
    let store = match open_file_change_store(cas_root) {
        Ok(s) => s,
        Err(_) => return,
    };

    // Use session_id-based agent ID for attribution
    let agent_id = current_agent_id(input);

    // Get the most recent prompt for this session (for attribution linking)
    let prompt_id = get_current_prompt_id(cas_root, &input.session_id);

    // Determine change type and compute content hash
    let (change_type, old_content_hash, new_content_hash) = match tool_name {
        "Edit" => {
            let old_string = tool_input
                .get("old_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let new_string = tool_input
                .get("new_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let old_hash = if old_string.is_empty() {
                None
            } else {
                Some(compute_content_hash(old_string))
            };
            let new_hash = compute_content_hash(new_string);

            (ChangeType::Modified, old_hash, new_hash)
        }
        "Write" => {
            let content = tool_input
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let new_hash = compute_content_hash(content);

            (ChangeType::Created, None, new_hash)
        }
        _ => return,
    };

    let file_change = FileChange::with_prompt(
        generate_file_change_id(),
        input.session_id.clone(),
        agent_id,
        prompt_id,
        get_repository_name(),
        file_path.to_string(),
        change_type,
        tool_name.to_string(),
        old_content_hash,
        new_content_hash,
    );

    // Store silently - attribution is best-effort
    let _ = store.add(&file_change);
}

/// Get the most recent prompt ID for a session
pub fn get_current_prompt_id(cas_root: &std::path::Path, session_id: &str) -> Option<String> {
    let store = open_prompt_store(cas_root).ok()?;
    let prompts = store.list_by_session(session_id, 1).ok()?;
    prompts.into_iter().next().map(|p| p.id)
}

// =============================================================================
// GIT COMMIT DETECTION (Code Attribution)
// =============================================================================

/// Detect git commit command and link uncommitted file changes
///
/// Called from PostToolUse for Bash commands that contain "git commit".
/// Links all uncommitted file_changes for this session to the commit.
pub fn detect_and_link_git_commit(cas_root: &std::path::Path, input: &HookInput) {
    // Get tool input
    let tool_input = match &input.tool_input {
        Some(ti) => ti,
        None => return,
    };

    // Check if this is a git commit command
    let command = match tool_input.get("command").and_then(|v| v.as_str()) {
        Some(cmd) => cmd,
        None => return,
    };

    if !is_git_commit_command(command) {
        return;
    }

    // Get tool response to extract commit hash
    let tool_response = match &input.tool_response {
        Some(tr) => tr,
        None => return,
    };

    // Check for successful exit
    let exit_code = tool_response
        .get("exitCode")
        .and_then(|v| v.as_i64())
        .unwrap_or(1);
    if exit_code != 0 {
        return; // Commit failed
    }

    // Resolve the committed tip from git itself. Git's normal stdout only
    // carries an abbreviated hash and `git commit -q` carries none at all;
    // HEAD is the durable, full-SHA signal both attribution and task close
    // need after a supervisor merges the branch before the first close.
    let stdout = tool_response
        .get("stdout")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let commit_hash = match resolve_commit_head(input).or_else(|| extract_commit_hash(stdout)) {
        Some(hash) => hash,
        None => return, // Couldn't find commit hash
    };

    // cas-3d37: snapshot the active factory task's latest committed tip at
    // commit time, not only after MERGE REQUIRED rejects a close. This makes
    // merge-before-first-close order-independent: once that SHA is an
    // ancestor of the epic branch, the zero-commit gate can prove real work
    // existed even though the live worker branch is now 0 commits ahead.
    persist_active_task_factory_anchor(cas_root, input, &commit_hash);

    // Open stores
    let file_change_store = match open_file_change_store(cas_root) {
        Ok(s) => s,
        Err(_) => return,
    };

    let commit_link_store = match open_commit_link_store(cas_root) {
        Ok(s) => s,
        Err(_) => return,
    };

    // Get uncommitted file changes for this session
    let uncommitted = match file_change_store.list_uncommitted(&input.session_id) {
        Ok(changes) => changes,
        Err(_) => return,
    };

    if uncommitted.is_empty() {
        return; // Nothing to link
    }

    // Extract metadata
    let agent_id = current_agent_id(input);
    let branch = get_current_branch().unwrap_or_else(|| "unknown".to_string());
    let message = extract_commit_message(command).unwrap_or_else(|| "No message".to_string());
    let author = get_git_author().unwrap_or_else(|| "Unknown".to_string());

    // Collect files and prompt IDs from uncommitted changes
    let files_changed: Vec<String> = uncommitted.iter().map(|c| c.file_path.clone()).collect();
    let prompt_ids: Vec<String> = uncommitted
        .iter()
        .filter_map(|c| c.prompt_id.clone())
        .collect();

    // Create commit link
    let commit_link = CommitLink::new(
        commit_hash.clone(),
        input.session_id.clone(),
        agent_id,
        branch,
        message,
        files_changed,
        prompt_ids,
        author,
    );

    // Store the commit link
    let _ = commit_link_store.add(&commit_link);

    // Link file changes to the commit
    let change_ids: Vec<String> = uncommitted.iter().map(|c| c.id.clone()).collect();
    let _ = file_change_store.link_to_commit(&change_ids, &commit_hash);
}

/// Resolve the full SHA at HEAD in the tool invocation's working directory.
///
/// `HookInput.cwd` is authoritative for PostToolUse. Empty legacy inputs fall
/// back to the hook process cwd so existing harnesses keep working.
fn resolve_commit_head(input: &HookInput) -> Option<String> {
    let mut command = std::process::Command::new("git");
    command.args(["rev-parse", "HEAD"]);
    if !input.cwd.trim().is_empty() {
        command.current_dir(&input.cwd);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(sha)
    } else {
        None
    }
}

/// Persist commit-time merge evidence on the one active task held by a
/// factory worker. Best-effort by hook convention: missing agent/task stores,
/// no active lease, non-worker callers, and non-factory branches are no-ops.
fn persist_active_task_factory_anchor(
    cas_root: &std::path::Path,
    input: &HookInput,
    commit_hash: &str,
) {
    let agent_store = match open_agent_store(cas_root) {
        Ok(store) => store,
        Err(_) => return,
    };
    let agent_id = current_agent_id(input);
    let agent = match agent_store.get(&agent_id) {
        Ok(agent) if agent.role == AgentRole::Worker => agent,
        _ => return,
    };
    let task_store = match open_task_store(cas_root) {
        Ok(store) => store,
        Err(_) => return,
    };
    let mut active_tasks = agent_store
        .list_agent_leases(&agent.id)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|lease| task_store.get(&lease.task_id).ok())
        .filter(|task| task.status == TaskStatus::InProgress);
    let Some(mut task) = active_tasks.next() else {
        return;
    };
    // One-task-at-a-time is a factory invariant. If corrupt state exposes
    // multiple active tasks, do not guess which task owns this commit.
    if active_tasks.next().is_some() {
        return;
    }

    let branch = git_branch_in(&input.cwd).or_else(get_current_branch);
    let Some(branch) = branch.filter(|name| name.starts_with("factory/")) else {
        return;
    };
    task.deliverables.factory_branch_anchor = Some(commit_hash.to_string());
    task.deliverables.parked_branch = Some(branch);
    task.updated_at = chrono::Utc::now();
    let _ = task_store.update(&task);
}

fn git_branch_in(cwd: &str) -> Option<String> {
    if cwd.trim().is_empty() {
        return None;
    }
    std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|branch| branch.trim().to_string())
}

/// Check if a command is a git commit command
pub fn is_git_commit_command(command: &str) -> bool {
    let cmd_lower = command.to_lowercase();
    // Match "git commit" but not "git commit --amend" etc. that just show status
    cmd_lower.contains("git commit") && !cmd_lower.contains("--dry-run")
}

/// Extract commit hash from git commit output
///
/// Git commit output format: "[branch hash] message"
/// Example: "[main abc1234] Add new feature"
pub fn extract_commit_hash(stdout: &str) -> Option<String> {
    // Look for pattern: [branch hash] or just a commit hash line
    for line in stdout.lines() {
        let line = line.trim();

        // Format: [branch abc1234] message
        if line.starts_with('[') {
            if let Some(bracket_end) = line.find(']') {
                let inside = &line[1..bracket_end];
                // Split by space and get the hash (second word)
                let parts: Vec<&str> = inside.split_whitespace().collect();
                if parts.len() >= 2 {
                    let potential_hash = parts[1];
                    // Git short hash is typically 7+ chars, full is 40
                    if potential_hash.len() >= 7
                        && potential_hash.chars().all(|c| c.is_ascii_hexdigit())
                    {
                        return Some(potential_hash.to_string());
                    }
                }
            }
        }

        // Also check for full 40-char hash
        if line.len() == 40 && line.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(line.to_string());
        }
    }

    None
}

/// Extract commit message from git commit command
pub fn extract_commit_message(command: &str) -> Option<String> {
    // Look for -m "message" or -m 'message' pattern
    let patterns = ["-m \"", "-m '", "-m \"$(", "--message=\"", "--message='"];

    for pattern in patterns {
        if let Some(start) = command.find(pattern) {
            let msg_start = start + pattern.len();
            let quote_char = if pattern.contains('\'') { '\'' } else { '"' };

            // Find the closing quote
            let remaining = &command[msg_start..];
            if let Some(end) = remaining.find(quote_char) {
                return Some(remaining[..end].to_string());
            }
        }
    }

    // Try heredoc pattern: -m "$(cat <<'EOF'\nmessage\nEOF\n)"
    if command.contains("<<") {
        // Extract what's between heredoc markers
        if let Some(start) = command.find("<<") {
            let after_marker = &command[start + 2..];
            if let Some(marker_end) = after_marker.find('\n') {
                let marker = after_marker[..marker_end]
                    .trim()
                    .trim_matches('\'')
                    .trim_matches('"');
                let after_first_marker = &after_marker[marker_end + 1..];
                if let Some(msg_end) = after_first_marker.find(marker) {
                    let message = after_first_marker[..msg_end].trim();
                    if !message.is_empty() {
                        return Some(message.to_string());
                    }
                }
            }
        }
    }

    None
}

/// Get current git branch name
pub fn get_current_branch() -> Option<String> {
    std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
}

/// Get git author from config
pub fn get_git_author() -> Option<String> {
    let name = std::process::Command::new("git")
        .args(["config", "user.name"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())?;

    let email = std::process::Command::new("git")
        .args(["config", "user.email"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())?;

    Some(format!("{name} <{email}>"))
}

#[cfg(test)]
mod commit_anchor_tests {
    use super::*;
    use crate::store::init_cas_dir;
    use std::process::Command;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn successful_worker_commit_records_active_task_anchor_before_close() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        let cas_root = init_cas_dir(repo).expect("init cas");
        git(repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
        git(repo, &["add", "seed.txt"]);
        git(repo, &["commit", "-q", "-m", "seed"]);
        git(repo, &["checkout", "-q", "-b", "factory/test-worker"]);

        let agent_store = open_agent_store(&cas_root).expect("agent store");
        let mut agent = Agent::new("session-worker".to_string(), "test-worker".to_string());
        agent.role = AgentRole::Worker;
        agent_store.register(&agent).expect("register worker");
        let task_store = open_task_store(&cas_root).expect("task store");
        let mut task = Task::new("cas-3d37-test".to_string(), "commit anchor".to_string());
        task.status = TaskStatus::InProgress;
        task.assignee = Some("test-worker".to_string());
        task_store.add(&task).expect("add task");
        agent_store
            .try_claim(&task.id, &agent.id, 600, None)
            .expect("claim task");

        std::fs::write(repo.join("work.rs"), "fn work() {}\n").unwrap();
        git(repo, &["add", "work.rs"]);
        // Quiet commit intentionally has no stdout hash. The hook must resolve
        // full HEAD directly instead of depending on human-oriented output.
        git(repo, &["commit", "-q", "-m", "fix: task work"]);
        let expected = resolve_commit_head(&HookInput {
            cwd: repo.display().to_string(),
            ..Default::default()
        })
        .expect("head sha");

        detect_and_link_git_commit(
            &cas_root,
            &HookInput {
                session_id: agent.id,
                cwd: repo.display().to_string(),
                hook_event_name: "PostToolUse".to_string(),
                tool_name: Some("Bash".to_string()),
                tool_input: Some(serde_json::json!({
                    "command": "git commit -q -m 'fix: task work'"
                })),
                tool_response: Some(serde_json::json!({
                    "exitCode": 0,
                    "stdout": ""
                })),
                agent_role: Some("worker".to_string()),
                ..Default::default()
            },
        );

        let anchored = task_store.get(&task.id).expect("anchored task");
        assert_eq!(
            anchored.deliverables.factory_branch_anchor.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(
            anchored.deliverables.parked_branch.as_deref(),
            Some("factory/test-worker")
        );
    }
}
