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

    // Claude reports a structured response with `exitCode` and `stdout`.
    // Codex PostToolUse reports the real unified-exec response as a JSON
    // string instead. The latter has no exit-code field, so commit-time
    // validation is deferred to the active task lease below.
    let codex_response = tool_response.is_string();
    if !codex_response {
        let exit_code = tool_response
            .get("exitCode")
            .and_then(|v| v.as_i64())
            .unwrap_or(1);
        if exit_code != 0 {
            return; // Commit failed
        }
    }

    // Git's commit output identifies the object the invocation created even
    // when a later command moves HEAD. Resolve that object to a full SHA.
    // Quiet commits carry no hash, so HEAD remains the fallback proxy.
    let stdout = tool_response
        .get("stdout")
        .and_then(|v| v.as_str())
        .or_else(|| tool_response.as_str())
        .unwrap_or("");
    // HookInput.cwd describes where the shell tool started, not necessarily
    // where a compound command ran Git. Until the hook carries the executed
    // Git cwd explicitly, refuse to infer an anchor for commands that redirect
    // Git's repository context. Resolving either an abbreviated stdout hash or
    // HEAD in HookInput.cwd could otherwise persist an unrelated commit.
    let commit_hash = match (!commit_uses_redirected_git_context(command))
        .then(|| {
            extract_commit_hash(stdout)
                .and_then(|hash| resolve_git_revision(input, &hash))
                .or_else(|| resolve_commit_head(input))
        })
        .flatten()
    {
        Some(hash) => hash,
        None => return, // Couldn't find commit hash
    };

    // cas-3d37: snapshot the active factory task's latest committed tip at
    // commit time, not only after MERGE REQUIRED rejects a close. This makes
    // merge-before-first-close order-independent: once that SHA is an
    // ancestor of the epic branch, the zero-commit gate can prove real work
    // existed even though the live worker branch is now 0 commits ahead.
    let anchored =
        persist_active_task_factory_anchor(cas_root, input, &commit_hash, codex_response);
    if codex_response && !anchored {
        return;
    }

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

/// Resolve a revision to its full SHA in the tool invocation's working
/// directory. `HookInput.cwd` is authoritative for PostToolUse. Empty legacy
/// inputs fall back to the hook process cwd so existing harnesses keep working.
fn resolve_git_revision(input: &HookInput, revision: &str) -> Option<String> {
    let mut command = std::process::Command::new("git");
    command.args(["rev-parse", revision]);
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

fn resolve_commit_head(input: &HookInput) -> Option<String> {
    resolve_git_revision(input, "HEAD")
}

/// Return true when the command can run `git commit` against a repository
/// other than `HookInput.cwd`.
///
/// This intentionally recognizes only shell words before the commit
/// subcommand, so a commit message such as `-m "mention cd"` is not treated
/// as redirection.
fn commit_uses_redirected_git_context(command: &str) -> bool {
    let words = split_shell_words(command);
    for (git_index, word) in words.iter().enumerate() {
        if word != "git" {
            continue;
        }
        let Some(commit_offset) = words[git_index + 1..]
            .iter()
            .position(|word| word == "commit")
        else {
            continue;
        };
        let commit_index = git_index + 1 + commit_offset;
        if words[..git_index].iter().any(|word| word == "cd") {
            return true;
        }
        if words[git_index + 1..commit_index].iter().any(|word| {
            word == "-C"
                || (word.starts_with("-C") && word.len() > 2)
                || word == "--git-dir"
                || word.starts_with("--git-dir=")
        }) {
            return true;
        }
    }
    false
}

/// Split enough shell syntax to inspect words around a `git commit`
/// invocation. Quoted contents remain one word, so commit-message text cannot
/// masquerade as standalone `cd`, `git`, or Git option tokens.
fn split_shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;

    for ch in command.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            } else {
                word.push(ch);
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
            continue;
        }
        if ch.is_whitespace() || matches!(ch, '&' | ';' | '|' | '(' | ')') {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            continue;
        }
        word.push(ch);
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Persist commit-time merge evidence on the one active task held by a
/// factory worker. Best-effort by hook convention: missing agent/task stores,
/// no active lease, non-worker callers, and non-factory branches are no-ops.
fn persist_active_task_factory_anchor(
    cas_root: &std::path::Path,
    input: &HookInput,
    commit_hash: &str,
    require_commit_during_lease: bool,
) -> bool {
    let agent_store = match open_agent_store(cas_root) {
        Ok(store) => store,
        Err(_) => return false,
    };
    let agent_id = current_agent_id(input);
    let agent = match agent_store.get(&agent_id) {
        Ok(agent) if agent.role == AgentRole::Worker => agent,
        _ => return false,
    };
    let task_store = match open_task_store(cas_root) {
        Ok(store) => store,
        Err(_) => return false,
    };
    let mut active_tasks = agent_store
        .list_agent_leases(&agent.id)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|lease| {
            task_store
                .get(&lease.task_id)
                .ok()
                .map(|task| (lease, task))
        })
        .filter(|(_, task)| task.status == TaskStatus::InProgress);
    let Some((lease, mut task)) = active_tasks.next() else {
        return false;
    };
    // One-task-at-a-time is a factory invariant. If corrupt state exposes
    // multiple active tasks, do not guess which task owns this commit.
    if active_tasks.next().is_some() {
        return false;
    }
    if require_commit_during_lease
        && task.deliverables.factory_branch_anchor.as_deref() != Some(commit_hash)
        && !commit_is_from_active_lease(input, commit_hash, lease.acquired_at.timestamp())
    {
        return false;
    }

    let branch = git_branch_in(&input.cwd).or_else(get_current_branch);
    let Some(branch) = branch.filter(|name| name.starts_with("factory/")) else {
        return false;
    };
    task.deliverables.factory_branch_anchor = Some(commit_hash.to_string());
    task.deliverables.parked_branch = Some(branch);
    task.updated_at = chrono::Utc::now();
    task_store.update(&task).is_ok()
}

/// Codex does not include a shell exit code in PostToolUse input. Require the
/// resolved HEAD commit to have been created no earlier than the current task
/// lease, with a one-second allowance for Git's whole-second timestamp.
fn commit_is_from_active_lease(input: &HookInput, commit_hash: &str, lease_epoch: i64) -> bool {
    let mut command = std::process::Command::new("git");
    command.args(["show", "-s", "--format=%ct", commit_hash]);
    if !input.cwd.trim().is_empty() {
        command.current_dir(&input.cwd);
    }
    command
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|timestamp| timestamp.trim().parse::<i64>().ok())
        .is_some_and(|commit_epoch| commit_epoch >= lease_epoch.saturating_sub(1))
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
    let mut bracket_hash = None;
    let mut full_hash = None;

    // Prefer the final `[branch hash]` commit-status line. Chained invocations
    // may create more than one object (for example commit then amend), and the
    // final object is the deliverable tip. Only use a bare full hash when no
    // commit-status line exists, so a later `git rev-parse` cannot override
    // the commit's own output.
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
                        bracket_hash = Some(potential_hash.to_string());
                    }
                }
            }
        }

        // Also support scripts that print only the full created commit hash.
        if line.len() == 40 && line.chars().all(|c| c.is_ascii_hexdigit()) {
            full_hash = Some(line.to_string());
        }
    }

    bracket_hash.or(full_hash)
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

    fn git_output(dir: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("git");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8(output.stdout).expect("git stdout")
    }

    fn worker_task_fixture() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        Agent,
        Task,
        std::sync::Arc<dyn crate::store::TaskStore>,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().to_path_buf();
        let cas_root = init_cas_dir(&repo).expect("init cas");
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
        git(&repo, &["add", "seed.txt"]);
        git(&repo, &["commit", "-q", "-m", "seed"]);
        git(&repo, &["checkout", "-q", "-b", "factory/test-worker"]);

        let agent_store = open_agent_store(&cas_root).expect("agent store");
        let mut agent = Agent::new("session-worker".to_string(), "test-worker".to_string());
        agent.role = AgentRole::Worker;
        agent_store.register(&agent).expect("register worker");
        let task_store = open_task_store(&cas_root).expect("task store");
        let mut task = Task::new("cas-034e-test".to_string(), "commit anchor".to_string());
        task.status = TaskStatus::InProgress;
        task.assignee = Some("test-worker".to_string());
        task_store.add(&task).expect("add task");
        agent_store
            .try_claim(&task.id, &agent.id, 600, None)
            .expect("claim task");

        (temp, cas_root, agent, task, task_store)
    }

    #[test]
    fn commit_then_reset_records_created_commit_not_post_command_head() {
        let (_temp, cas_root, agent, task, task_store) = worker_task_fixture();
        let repo = cas_root.parent().expect("repo");
        std::fs::write(repo.join("work.rs"), "fn work() {}\n").unwrap();
        git(repo, &["add", "work.rs"]);

        let output = Command::new("bash")
            .args([
                "-c",
                "git commit -m 'fix: task work' && git reset --hard HEAD~1",
            ])
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("commit then reset");
        assert!(output.status.success(), "commit then reset failed");
        let stdout = String::from_utf8(output.stdout).expect("command stdout");
        let abbreviated = extract_commit_hash(&stdout).expect("created commit hash");
        let expected = git_output(repo, &["rev-parse", &abbreviated])
            .trim()
            .to_string();
        let post_command_head = git_output(repo, &["rev-parse", "HEAD"]).trim().to_string();
        assert_ne!(expected, post_command_head, "fixture must move HEAD");

        detect_and_link_git_commit(
            &cas_root,
            &HookInput {
                session_id: agent.id,
                cwd: repo.display().to_string(),
                hook_event_name: "PostToolUse".to_string(),
                tool_name: Some("Bash".to_string()),
                tool_input: Some(serde_json::json!({
                    "command": "git commit -m 'fix: task work' && git reset --hard HEAD~1"
                })),
                tool_response: Some(serde_json::json!({
                    "exitCode": 0,
                    "stdout": stdout
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
    }

    #[test]
    fn commit_then_amend_records_final_created_commit() {
        let (_temp, cas_root, agent, task, task_store) = worker_task_fixture();
        let repo = cas_root.parent().expect("repo");
        std::fs::write(repo.join("work.rs"), "fn work() {}\n").unwrap();
        git(repo, &["add", "work.rs"]);

        let output = Command::new("bash")
            .args([
                "-c",
                "git commit -m 'work v1' && printf '\\n// amended\\n' >> work.rs \
                 && git add work.rs && git commit --amend -m 'work v2'",
            ])
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("commit then amend");
        assert!(output.status.success(), "commit then amend failed");
        let stdout = String::from_utf8(output.stdout).expect("command stdout");
        let expected = git_output(repo, &["rev-parse", "HEAD"]).trim().to_string();
        let original = git_output(repo, &["rev-parse", "HEAD@{1}"])
            .trim()
            .to_string();
        assert_ne!(
            expected, original,
            "fixture must replace the original commit"
        );

        detect_and_link_git_commit(
            &cas_root,
            &HookInput {
                session_id: agent.id,
                cwd: repo.display().to_string(),
                hook_event_name: "PostToolUse".to_string(),
                tool_name: Some("Bash".to_string()),
                tool_input: Some(serde_json::json!({
                    "command": "git commit -m 'work v1' && printf '\\n// amended\\n' >> work.rs \
                                && git add work.rs && git commit --amend -m 'work v2'"
                })),
                tool_response: Some(serde_json::json!({
                    "exitCode": 0,
                    "stdout": stdout
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
    }

    #[test]
    fn redirected_git_context_detection_is_scoped_before_commit() {
        assert!(commit_uses_redirected_git_context(
            "cd /tmp/other && git commit -q -m work"
        ));
        assert!(commit_uses_redirected_git_context(
            "git -C /tmp/other commit -m work"
        ));
        assert!(commit_uses_redirected_git_context(
            "git --git-dir=/tmp/other/.git commit -m work"
        ));
        assert!(commit_uses_redirected_git_context(
            "echo 'commit'; cd /tmp/other && git commit -m work"
        ));
        assert!(!commit_uses_redirected_git_context(
            "git commit -m 'mention cd and git -C without redirecting'"
        ));
    }

    #[test]
    fn quiet_commit_after_cd_does_not_anchor_hook_cwd_head() {
        let (_temp, cas_root, agent, task, task_store) = worker_task_fixture();
        let hook_repo = cas_root.parent().expect("hook repo");
        let hook_head = git_output(hook_repo, &["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let other = tempfile::tempdir().expect("other repo");
        git(other.path(), &["init", "-q", "-b", "factory/other"]);
        std::fs::write(other.path().join("work.rs"), "fn work() {}\n").unwrap();
        git(other.path(), &["add", "work.rs"]);
        let shell_command = format!(
            "cd '{}' && git commit -q -m 'fix: redirected work'",
            other.path().display()
        );
        let output = Command::new("bash")
            .args(["-c", &shell_command])
            .current_dir(hook_repo)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("quiet redirected commit");
        assert!(output.status.success(), "quiet redirected commit failed");
        assert!(output.stdout.is_empty(), "quiet commit must print no hash");
        let committed_elsewhere = git_output(other.path(), &["rev-parse", "HEAD"])
            .trim()
            .to_string();
        assert_ne!(
            committed_elsewhere, hook_head,
            "fixture must distinguish the committed repo from the hook cwd"
        );

        detect_and_link_git_commit(
            &cas_root,
            &HookInput {
                session_id: agent.id,
                cwd: hook_repo.display().to_string(),
                hook_event_name: "PostToolUse".to_string(),
                tool_name: Some("Bash".to_string()),
                tool_input: Some(serde_json::json!({
                    "command": shell_command
                })),
                tool_response: Some(serde_json::json!({
                    "exitCode": 0,
                    "stdout": ""
                })),
                agent_role: Some("worker".to_string()),
                ..Default::default()
            },
        );

        let anchored = task_store.get(&task.id).expect("task after hook");
        assert_eq!(
            anchored.deliverables.factory_branch_anchor, None,
            "redirected quiet commit must not anchor HookInput.cwd HEAD"
        );
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
