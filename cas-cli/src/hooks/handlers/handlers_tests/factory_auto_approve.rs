//! Tests for factory-agent filesystem-tool auto-approve in PreToolUse.
//!
//! Root cause: Claude Code 2.1.116 team-mode escalates any "ask" permission
//! decision to the team leader via `Mq4()`, gated on a broken self-check
//! (`UG9()`) that compares the agent's `hP()` to the literal string
//! `"team-lead"`. Cassy agents have `agentId = "<name>@<team>"`, so the check
//! always fails, every Write/Edit/Bash escalates, and the supervisor ends
//! up asking itself for approval — self-deadlock.
//!
//! Fix: PreToolUse runs before the classifier. Returning
//! `{permissionDecision:"allow"}` short-circuits the whole decision flow so
//! `Mq4()` never fires. Scope is deliberately narrow — the same filesystem
//! tool list Cassy ships in the supervisor/worker `--settings` files.
//!
//! On main, the gate reads role from `CAS_AGENT_ROLE` env (the
//! `HookInput.agent_role` field landed later on the worktree-leak epic).
//! All tests that exercise the gate mutate process env and therefore
//! serialize on a local mutex, matching the pattern in
//! `agent_worktree_block`'s env-fallback test.

use crate::hooks::handlers::handle_pre_tool_use;
use cas_core::hooks::types::HookInput;

fn input_for(tool: &str, file_path: Option<&str>) -> HookInput {
    let tool_input = match file_path {
        Some(p) => serde_json::json!({"file_path": p, "content": "x"}),
        None => serde_json::json!({"command": "echo hi"}),
    };
    HookInput {
        session_id: "test-session".into(),
        cwd: "/test".into(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some(tool.into()),
        tool_input: Some(tool_input),
        ..HookInput::default()
    }
}

fn bash_input(command: &str) -> HookInput {
    HookInput {
        session_id: "test-session".into(),
        cwd: "/test".into(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some("Bash".into()),
        tool_input: Some(serde_json::json!({"command": command})),
        ..HookInput::default()
    }
}

fn allow_reason(out: &cas_core::hooks::types::HookOutput) -> Option<String> {
    let specific = out.hook_specific_output.as_ref()?;
    let value = serde_json::to_value(specific).ok()?;
    let decision = value.get("permissionDecision")?.as_str()?;
    if decision != "allow" {
        return None;
    }
    value
        .get("permissionDecisionReason")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn deny_reason(out: &cas_core::hooks::types::HookOutput) -> Option<String> {
    let specific = out.hook_specific_output.as_ref()?;
    let value = serde_json::to_value(specific).ok()?;
    if value.get("permissionDecision")?.as_str()? != "deny" {
        return None;
    }
    value
        .get("permissionDecisionReason")
        .and_then(|reason| reason.as_str())
        .map(str::to_string)
}

// ============================================================================
// Positive: factory agents get auto-approve for filesystem tool families.
// ============================================================================

#[test]
fn supervisor_write_is_auto_approved() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("supervisor"));
    let input = input_for("Write", Some("/test/foo.txt"));
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    let reason = allow_reason(&out).expect("expected allow");
    assert!(
        reason.contains("Factory agent auto-approve"),
        "allow reason should identify the factory bypass: {reason}"
    );
}

#[test]
fn worker_write_is_auto_approved() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("worker"));
    let input = input_for("Write", Some("/test/foo.txt"));
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    assert!(allow_reason(&out).is_some(), "worker Write must auto-approve");
}

#[test]
fn worker_edit_is_auto_approved() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("worker"));
    let input = input_for("Edit", Some("/test/foo.txt"));
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    assert!(allow_reason(&out).is_some(), "worker Edit must auto-approve");
}

#[test]
fn supervisor_bash_is_auto_approved() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("supervisor"));
    let input = input_for("Bash", None);
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    assert!(
        allow_reason(&out).is_some(),
        "supervisor Bash must auto-approve"
    );
}

#[test]
fn supervisor_read_is_auto_approved() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("supervisor"));
    let input = input_for("Read", Some("/tmp/foo.txt"));
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    assert!(allow_reason(&out).is_some());
}

#[test]
fn supervisor_notebook_edit_is_auto_approved() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("supervisor"));
    let input = input_for("NotebookEdit", Some("/test/n.ipynb"));
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    assert!(allow_reason(&out).is_some());
}

#[test]
fn factory_write_to_bare_tmp_is_denied_with_durable_remediation() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("worker"));
    let input = input_for("Write", Some("/tmp/cas-4060-proof.txt"));
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    let reason = deny_reason(&out).expect("bare tmp write must be denied");
    assert!(reason.contains("WORKSPACE CONTRACT"), "{reason}");
    assert!(reason.contains(".cas/artifacts"), "{reason}");
}

#[test]
fn factory_write_under_worktree_is_allowed() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("worker"));
    let input = input_for("Write", Some("/test/under-worktree.txt"));
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    assert!(
        allow_reason(&out).is_some(),
        "a write under the worktree must be allowed"
    );
}

#[test]
fn factory_write_under_configured_artifacts_root_is_allowed() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("worker"));
    let tmp = tempfile::tempdir().expect("tempdir");
    let artifacts_root = tmp.path().join("durable-artifacts");
    std::fs::create_dir_all(&artifacts_root).expect("artifacts root");
    std::fs::write(
        tmp.path().join("config.toml"),
        format!(
            "[factory]\nartifacts_root = {:?}\n",
            artifacts_root.display().to_string()
        ),
    )
    .expect("factory config");

    let artifact_path = artifacts_root.join("cas-4060").join("proof.txt");
    let input = input_for("Write", artifact_path.to_str());
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    assert!(
        allow_reason(&out).is_some(),
        "a write under the configured durable artifacts root must be allowed"
    );
}

#[test]
fn factory_write_under_declared_scratchpad_is_allowed() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("worker"));
    let tmp = tempfile::tempdir().expect("tempdir");
    let scratchpad = tmp.path().join("harness-scratchpad");
    let _scratchpad = set_env_var("CAS_SCRATCHPAD", scratchpad.as_os_str());

    let scratchpad_path = scratchpad.join("ephemeral-note.txt");
    let input = input_for("Write", scratchpad_path.to_str());
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    assert!(
        allow_reason(&out).is_some(),
        "a write under the declared harness scratchpad must be allowed"
    );
}

#[test]
fn supervisor_write_under_harness_file_memory_is_allowed_for_default_and_account_configs() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("supervisor"));
    let home = tempfile::tempdir().expect("home");
    let _home = set_env_var("HOME", home.path().as_os_str());
    let tmp = tempfile::tempdir().expect("tempdir");

    for config_dir in [".claude", ".claude-work"] {
        let path = home
            .path()
            .join(config_dir)
            .join("projects")
            .join("-home-pippenz-Petrastella-cas-src")
            .join("memory")
            .join("task-context.md");
        let input = input_for("Write", path.to_str());
        let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
        assert!(
            allow_reason(&out).is_some(),
            "supervisor harness-memory write must be allowed: {}",
            path.display()
        );
    }
}

#[test]
fn worker_write_under_harness_file_memory_remains_denied() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("worker"));
    let home = tempfile::tempdir().expect("home");
    let _home = set_env_var("HOME", home.path().as_os_str());
    let path = home
        .path()
        .join(".claude-work/projects/-home-pippenz-Petrastella-cas-src/memory/task-context.md");
    let input = input_for("Write", path.to_str());
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    assert!(
        deny_reason(&out).is_some(),
        "worker scope must not expand into supervisor harness memory"
    );
}

#[test]
fn factory_write_under_harness_session_scratchpad_is_allowed() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("worker"));
    let path = "/tmp/claude-1000/-home-pippenz-Petrastella-cas-src/f54c4e08-831c-4cb6-ae05-5bd663ec1fed/scratchpad/issue-ambient-recall.md";
    let mut input = input_for("Write", Some(path));
    input.session_id = "f54c4e08-831c-4cb6-ae05-5bd663ec1fed".into();
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    assert!(
        allow_reason(&out).is_some(),
        "the harness-advertised per-session scratchpad must be sanctioned"
    );
}

#[test]
fn factory_write_under_another_session_scratchpad_is_denied() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("worker"));
    let path = "/tmp/claude-1000/-home-pippenz-Petrastella-cas-src/f54c4e08-831c-4cb6-ae05-5bd663ec1fed/scratchpad/issue-ambient-recall.md";
    let input = input_for("Write", Some(path));
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    let reason = deny_reason(&out).expect("another session's scratchpad is bare /tmp");
    assert!(reason.contains(path), "{reason}");
}

#[test]
fn factory_bash_commit_message_with_slash_leading_token_is_allowed() {
    let _g = super::env_lock();
    // The live false positive was a supervisor merge. Workers have a separate
    // branch-protection gate which deliberately refuses merge commands before
    // the workspace contract is reached.
    let _role = set_role_env(Some("supervisor"));
    let input = bash_input(
        "git merge --no-ff factory/auth -m 'Merge auth redirects (> /api/me), without file creation'",
    );
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    assert!(
        allow_reason(&out).is_some(),
        "path-shaped prose in a commit message is not a filesystem write target"
    );
}

#[test]
fn factory_bash_redirect_to_bare_tmp_is_denied() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("worker"));
    let input = bash_input("printf '%s\\n' proof > /tmp/cas-b76f-proof.txt");
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    let reason = deny_reason(&out).expect("bare /tmp redirect must remain denied");
    assert!(reason.contains("/tmp/cas-b76f-proof.txt"), "{reason}");
}

#[test]
fn factory_bash_stream_device_redirects_are_allowed() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("worker"));
    let tmp = tempfile::tempdir().expect("tempdir");

    for command in [
        "printf proof 2>/dev/null",
        "printf proof 1>/dev/null",
        "printf proof &>/dev/null",
        "printf proof >/dev/stdout",
        "printf proof >/dev/stderr",
        "printf proof >/dev/tty",
    ] {
        let input = bash_input(command);
        let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
        assert!(
            allow_reason(&out).is_some(),
            "stream-device redirect must not be treated as file creation: {command}"
        );
    }
}

#[test]
fn factory_bash_touch_under_home_is_denied() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("worker"));
    let input = bash_input("touch $HOME/cas-b76f-stray.txt");
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    let reason = deny_reason(&out).expect("stray $HOME write must remain denied");
    assert!(reason.contains("cas-b76f-stray.txt"), "{reason}");
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn set_env_var(key: &'static str, value: &std::ffi::OsStr) -> EnvVarGuard {
    let previous = std::env::var_os(key);
    unsafe { std::env::set_var(key, value) };
    EnvVarGuard { key, previous }
}

// ============================================================================
// cas_root=None path — the case the deadlock reporter was hitting.
//
// When Cassy is not initialized in the supervisor's cwd at hook-dispatch time,
// `handle_pre_tool_use(&input, None)` is invoked. Prior to cas-7f33 this
// returned `HookOutput::empty()` immediately, causing Claude Code's classifier
// to fall through to team-mode leader-escalation (UG9 bug) and self-deadlock.
// The factory auto-approve must fire even without a Cassy root.
// ============================================================================

#[test]
fn supervisor_write_is_auto_approved_without_cas_root() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("supervisor"));
    let input = input_for("Write", Some("/test/foo.txt"));
    let out = handle_pre_tool_use(&input, None).expect("handler ok");
    let reason = allow_reason(&out).expect(
        "supervisor Write must auto-approve even when cas_root is None (deadlock case)",
    );
    assert!(
        reason.contains("Factory agent auto-approve"),
        "allow reason should identify the factory bypass: {reason}"
    );
}

#[test]
fn worker_edit_is_auto_approved_without_cas_root() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("worker"));
    let input = input_for("Edit", Some("/test/foo.txt"));
    let out = handle_pre_tool_use(&input, None).expect("handler ok");
    assert!(
        allow_reason(&out).is_some(),
        "worker Edit must auto-approve even when cas_root is None (deadlock case)"
    );
}

#[test]
fn solo_user_write_without_cas_root_is_not_auto_approved() {
    // When CAS_AGENT_ROLE is unset AND cas_root is None, we must still
    // fall through to Claude Code's normal flow — the bypass is strictly
    // scoped to factory agents.
    let _g = super::env_lock();
    let _role = set_role_env(None);
    let input = input_for("Write", Some("/test/foo.txt"));
    let out = handle_pre_tool_use(&input, None).expect("handler ok");
    assert!(
        allow_reason(&out).is_none(),
        "standalone sessions with no cas_root must not get the factory auto-approve"
    );
}

// ============================================================================
// Negatives: must NOT auto-approve outside scope.
// ============================================================================

#[test]
fn solo_user_write_is_not_auto_approved() {
    // CAS_AGENT_ROLE unset — the handler must leave the permission decision
    // to Claude Code's normal flow so user-facing approvals keep working.
    let _g = super::env_lock();
    let _role = set_role_env(None);
    let input = input_for("Write", Some("/test/foo.txt"));
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    assert!(
        allow_reason(&out).is_none(),
        "standalone sessions must not get the factory auto-approve"
    );
}

#[test]
fn factory_agent_unknown_tool_is_not_auto_approved() {
    // Tools outside the filesystem allowlist (e.g., Agent, Task, MCP) must
    // fall through so their specialized handling still runs.
    let _g = super::env_lock();
    let _role = set_role_env(Some("worker"));
    let input = input_for("WebFetch", None);
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    assert!(
        allow_reason(&out).is_none(),
        "WebFetch is not in the factory auto-approve list"
    );
}

#[test]
fn factory_agent_unknown_tool_without_cas_root_is_not_auto_approved() {
    // Mirror of `factory_agent_unknown_tool_is_not_auto_approved` for the
    // hoisted `cas_root=None` path. Locks in the allowlist guard on the
    // rescue branch so a future refactor that drops the `contains()`
    // check fails the suite instead of silently broadening the bypass.
    let _g = super::env_lock();
    let _role = set_role_env(Some("worker"));
    let input = input_for("WebFetch", None);
    let out = handle_pre_tool_use(&input, None).expect("handler ok");
    assert!(
        allow_reason(&out).is_none(),
        "WebFetch is not in the factory auto-approve list (cas_root=None path)"
    );
}

// ============================================================================
// Structural invariants documented — not exercised directly in unit tests.
// ============================================================================
//
// The factory auto-approve gate appears TWICE in `handle_pre_tool_use`
// after cas-7f33:
//
//   1. A HOISTED copy above the `cas_root` early return. Fires only when
//      `cas_root is None` (i.e. Cassy not initialized in cwd). This rescues
//      the deadlock case the bug reporter hit, where the supervisor
//      session had no Cassy root resolved at hook-dispatch time and the
//      hook was returning empty, letting Claude Code's team-mode
//      classifier escalate to the non-existent leader.
//
//   2. The original copy AFTER the protection block. Fires only on the
//      `cas_root is Some` path. This ordering is load-bearing for the
//      `.env` deny invariant: writing a `.env` file (or any file
//      matched by `hooks.pre_tool_use.protection.files` / `.patterns`)
//      returns a deny BEFORE the factory auto-approve can fire, so the
//      bypass can never silently allow a secret-file write.
//
// The invariant is preserved because protection gates live INSIDE the
// cas_root=Some section — they read config via `stores.config()` and
// cannot run when cas_root is None. Hoisting the auto-approve above the
// cas_root check therefore does not widen the surface on any path where
// the .env guard previously applied.
//
// The gate also runs AFTER the verification jail, the factory
// `SendMessage` block, the codemap freshness gate, and the
// worktrees_enabled block — all of which are inside the cas_root=Some
// section. All of those short-circuit with deny, so the auto-approve
// never overrides them either.

// ----------------------------------------------------------------------------
// Env helpers — use the shared process-wide mutex from mod.rs so that
// concurrent tests across sibling modules don't race on CAS_AGENT_ROLE.
// ----------------------------------------------------------------------------

struct RoleGuard(Option<String>);

impl Drop for RoleGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.0 {
                Some(v) => std::env::set_var("CAS_AGENT_ROLE", v),
                None => std::env::remove_var("CAS_AGENT_ROLE"),
            }
        }
    }
}

fn set_role_env(role: Option<&str>) -> RoleGuard {
    let prev = std::env::var("CAS_AGENT_ROLE").ok();
    unsafe {
        match role {
            Some(v) => std::env::set_var("CAS_AGENT_ROLE", v),
            None => std::env::remove_var("CAS_AGENT_ROLE"),
        }
    }
    RoleGuard(prev)
}
