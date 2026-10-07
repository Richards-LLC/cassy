use super::*;
use serde_json::json;

/// A raw `.claude.json` shaped like the live one: unrelated top-level state,
/// other projects with their own fields, and an untrusted entry for the cwd.
fn raw_config(cwd: &str) -> String {
    format!(
        r#"{{
  "numStartups": 412,
  "oauthAccount": {{"emailAddress": "someone@example.test", "organizationUuid": "org-1"}},
  "mcpServers": {{"cas": {{"command": "cas", "args": ["serve"]}}}},
  "tipsHistory": {{"a": 3, "b": 1.5}},
  "projects": {{
    "/home/a/other": {{"hasTrustDialogAccepted": false, "allowedTools": ["Bash"], "lastCost": 0.25}},
    "{cwd}": {{"allowedTools": [], "history": ["x"], "hasTrustDialogAccepted": false}}
  }}
}}"#
    )
}

fn without_trust_flag(mut value: Value, key: &str) -> Value {
    value["projects"][key]
        .as_object_mut()
        .unwrap()
        .remove(CLAUDE_TRUST_FIELD);
    value
}

#[test]
fn cas_0f5b_raw_merge_sets_only_the_trust_flag_and_keeps_every_other_value() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().join("worktree");
    std::fs::create_dir(&cwd).unwrap();
    let cwd = std::fs::canonicalize(&cwd).unwrap();
    let key = cwd.to_string_lossy().to_string();
    let config = dir.path().join(".claude.json");
    std::fs::write(&config, raw_config(&key)).unwrap();
    let before: Value = serde_json::from_str(&raw_config(&key)).unwrap();

    let outcome = ensure_claude_project_trusted_in(&config, &cwd).unwrap();
    assert_eq!(outcome, ClaudeTrustOutcome::Added(vec![key.clone()]));

    let after: Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(after["projects"][&key][CLAUDE_TRUST_FIELD], json!(true));
    // Every other key and value, including the cwd entry's own fields and the
    // other project's explicit false, is exactly as it was.
    assert_eq!(
        without_trust_flag(after.clone(), &key),
        without_trust_flag(before, &key)
    );
    assert_eq!(after["projects"]["/home/a/other"][CLAUDE_TRUST_FIELD], json!(false));

    // Idempotent: a second call writes nothing.
    let mtime = std::fs::metadata(&config).unwrap().modified().unwrap();
    assert_eq!(
        ensure_claude_project_trusted_in(&config, &cwd).unwrap(),
        ClaudeTrustOutcome::AlreadyPresent
    );
    assert_eq!(std::fs::metadata(&config).unwrap().modified().unwrap(), mtime);
}

#[test]
fn cas_0f5b_missing_config_and_missing_projects_are_created() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = std::fs::canonicalize(dir.path()).unwrap();
    let config = dir.path().join("cfg/.claude.json");
    ensure_claude_project_trusted_in(&config, &cwd).unwrap();
    let after: Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    assert!(is_trusted(&after, &cwd.to_string_lossy()));

    let flat = dir.path().join("flat.json");
    std::fs::write(&flat, r#"{"numStartups": 1}"#).unwrap();
    ensure_claude_project_trusted_in(&flat, &cwd).unwrap();
    let after: Value = serde_json::from_str(&std::fs::read_to_string(&flat).unwrap()).unwrap();
    assert_eq!(after["numStartups"], json!(1));
    assert!(is_trusted(&after, &cwd.to_string_lossy()));
}

#[test]
fn cas_0f5b_refuses_to_rewrite_a_config_it_cannot_parse() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = std::fs::canonicalize(dir.path()).unwrap();
    for body in ["{not json", "[1, 2]", r#"{"projects": []}"#, r#"{"projects": {"KEY": 3}}"#] {
        let config = dir.path().join(".claude.json");
        let body = body.replace("KEY", &cwd.to_string_lossy());
        std::fs::write(&config, &body).unwrap();
        assert!(
            ensure_claude_project_trusted_in(&config, &cwd).is_err(),
            "must refuse {body}"
        );
        assert_eq!(std::fs::read_to_string(&config).unwrap(), body, "left untouched");
    }
    assert_eq!(
        ensure_claude_project_trusted_in(&dir.path().join("x.json"), Path::new("relative/dir"))
            .unwrap(),
        ClaudeTrustOutcome::Skipped("agent cwd is not absolute; cannot key a Claude trust entry")
    );
}

/// A live session can leave `.claude.json` truncated mid-write. An existing
/// empty or whitespace-only file must refuse the launch and stay
/// byte-identical, never be replaced by `{"projects": …}`.
#[test]
fn cas_0f5b_refuses_an_existing_empty_config_and_leaves_it_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = std::fs::canonicalize(dir.path()).unwrap();
    let config = dir.path().join(".claude.json");
    for body in [&b""[..], b"   \n\t"] {
        std::fs::write(&config, body).unwrap();
        assert!(
            ensure_claude_project_trusted_in(&config, &cwd).is_err(),
            "must refuse an existing config of {body:?}"
        );
        assert_eq!(std::fs::read(&config).unwrap(), body, "left byte-identical");
    }
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.starts_with(".claude.json.cas-") && name != ".claude.json.cas-lock")
        .collect();
    assert!(leftovers.is_empty(), "no temp file left behind: {leftovers:?}");
}

#[cfg(unix)]
#[test]
fn cas_0f5b_keeps_the_file_mode_and_a_managed_symlink() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let cwd = std::fs::canonicalize(dir.path()).unwrap();
    let real = dir.path().join("real.json");
    std::fs::write(&real, r#"{"numStartups": 2}"#).unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = dir.path().join(".claude.json");
    symlink(&real, &link).unwrap();
    ensure_claude_project_trusted_in(&link, &cwd).unwrap();
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o600);
    let after: Value = serde_json::from_str(&std::fs::read_to_string(&real).unwrap()).unwrap();
    assert!(is_trusted(&after, &cwd.to_string_lossy()));
}

#[test]
fn cas_0f5b_config_path_prefers_the_agent_config_dir() {
    assert_eq!(
        claude_global_config_path(Some("/x/cfg")),
        Some(PathBuf::from("/x/cfg/.claude.json"))
    );
}
