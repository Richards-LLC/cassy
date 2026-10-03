//! cas-49c0: the worker write guard through the real `cas hook PreToolUse`
//! binary. A Codex worker's `apply_patch` into the supervisor's main checkout
//! is refused, naming the path and the worker's own worktree; a patch inside
//! the worktree is not refused.

use assert_cmd::Command;
use tempfile::TempDir;

fn cas_cmd(dir: &TempDir) -> Command {
    let mut cmd = Command::new(cas::test_paths::binary(
        "cas",
        option_env!("CARGO_BIN_EXE_cas").map(Into::into),
    ));
    let home = dir.path().join(".test-home");
    let xdg = dir.path().join(".test-xdg-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    if let Some(host_home) = std::env::var_os("HOME") {
        cmd.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    cmd.env("HOME", home).env("XDG_CONFIG_HOME", xdg);
    cmd.current_dir(dir.path());
    for key in [
        "CAS_ROOT",
        "CAS_AGENT_ROLE",
        "CAS_AGENT_NAME",
        "CAS_CLONE_PATH",
        "CAS_FACTORY_MODE",
        "CAS_FACTORY_SESSION",
        "CAS_FACTORY_WORKER_CLI",
        "CAS_FACTORY_SUPERVISOR_CLI",
        "CAS_SCRATCHPAD",
        "CAS_SCRATCHPAD_PATH",
        "CLAUDE_SCRATCHPAD",
    ] {
        cmd.env_remove(key);
    }
    cmd.env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

/// Run the Codex worker's PreToolUse hook for one `apply_patch` call and
/// return its combined stdout and stderr.
fn codex_worker_apply_patch(main: &TempDir, worktree: &std::path::Path, patch: &str) -> String {
    let input = serde_json::json!({
        "session_id": "cas-49c0-worker-session",
        "cwd": worktree.to_string_lossy(),
        "hook_event_name": "PreToolUse",
        "tool_use_id": format!("cas-49c0-{}", std::process::id()),
        "tool_name": "apply_patch",
        "tool_input": { "command": patch },
    });
    let out = cas_cmd(main)
        .current_dir(worktree)
        .args(["hook", "PreToolUse"])
        .env("CAS_HOOK_HARNESS", "codex")
        .env("CAS_AGENT_ROLE", "worker")
        .env("CAS_AGENT_NAME", "strong-puma-16")
        .env("CAS_FACTORY_MODE", "1")
        .env("CAS_CLONE_PATH", worktree)
        .write_stdin(input.to_string())
        .output()
        .expect("hook command must not panic");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn codex_worker_apply_patch_into_the_main_checkout_is_refused_cas_49c0() {
    let main = TempDir::new().unwrap();
    cas_cmd(&main).args(["init", "--yes"]).assert().success();
    let worktree = main.path().join(".cas/worktrees/strong-puma-16");
    std::fs::create_dir_all(worktree.join("src")).unwrap();
    let main_file = main.path().join("cas-cli/src/mcp/tools/service/core.rs");

    let refused = codex_worker_apply_patch(
        &main,
        &worktree,
        &format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n-old\n+include_foreign\n*** End Patch\n",
            main_file.display()
        ),
    );
    let main_name = main_file.canonicalize().unwrap_or_else(|_| {
        main.path().canonicalize().unwrap().join("cas-cli/src/mcp/tools/service/core.rs")
    });
    assert!(refused.contains("deny"), "{refused}");
    assert!(refused.contains("FACTORY WORKSPACE CONTRACT"), "{refused}");
    assert!(
        refused.contains(&main_name.display().to_string())
            || refused.contains(&main_file.display().to_string()),
        "the refusal names the main-checkout path: {refused}"
    );
    assert!(
        refused.contains(&worktree.display().to_string())
            || refused.contains(&worktree.canonicalize().unwrap().display().to_string()),
        "the refusal names the worker's own worktree: {refused}"
    );

    let allowed = codex_worker_apply_patch(
        &main,
        &worktree,
        "*** Begin Patch\n*** Add File: src/new.rs\n+pub fn new() {}\n*** End Patch\n",
    );
    assert!(
        !allowed.contains("FACTORY WORKSPACE CONTRACT"),
        "a patch inside the worktree is not refused: {allowed}"
    );
}
