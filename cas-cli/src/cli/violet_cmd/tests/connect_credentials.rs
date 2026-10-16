use super::super::hub::connect_config;
use crate::test_support::TestEnvGuard;
use cmcp_core::config::{ServerConfig, VIOLET_SERVER, violet_hub_url};
use std::path::Path;

fn fixture() -> TestEnvGuard {
    let mut env = TestEnvGuard::temp_home();
    let legacy = cas_types::violet_compatibility::violet_compatibility();
    for (name, _) in std::env::vars_os() {
        let text = name.to_string_lossy();
        if text.starts_with("VIOLET_") || text.starts_with(&legacy.legacy_token_prefix) {
            env.remove(name);
        }
    }
    let home = env.home().to_path_buf();
    env.set("XDG_CONFIG_HOME", home.join(".config"));
    env.set("CAS_CREDENTIALS_FILE", home.join("credentials.env"));
    env.set("CODEX_HOME", home.join(".codex"));
    env.set("CLAUDE_CONFIG_DIR", &home);
    proxy(
        &home.join(".config/code-mode-mcp/config.toml"),
        true,
        "VIOLET_SLACK_TOKEN_CASSY_PROXY",
    );
    env
}

fn proxy(path: &Path, managed: bool, token: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!(
            "{}[servers.violet]\ntransport='http'\nurl={:?}\nauth='env:{token}'\n",
            if managed { "# managed\n" } else { "" },
            violet_hub_url()
        ),
    )
    .unwrap();
}

fn private(home: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = home.join("credentials.env");
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn bearer(root: Option<&Path>) -> String {
    let config = connect_config(root).unwrap();
    match &config.servers[VIOLET_SERVER] {
        ServerConfig::Http { auth, .. } => auth.clone().unwrap(),
        _ => panic!("expected HTTP fixture"),
    }
}

#[test]
fn cas_0a4c_managed_proxy_uses_registered_private_token_and_environment_precedence() {
    let mut env = fixture();
    private(env.home(), "VIOLET_SLACK_TOKEN_ACCOUNT='private-fixture'\n");
    std::fs::write(env.home().join(".claude.json"), r#"{"mcpServers":{"violet":{"headers":{"Authorization":"Bearer ${VIOLET_SLACK_TOKEN_ACCOUNT}"}}}}"#).unwrap();
    assert_eq!(bearer(None), "private-fixture");
    env.set("VIOLET_SLACK_TOKEN_ACCOUNT", "export-fixture");
    assert_eq!(bearer(None), "export-fixture");
    env.set("VIOLET_SLACK_TOKEN_ACCOUNT", "  ");
    assert_eq!(bearer(None), "private-fixture");
}

#[test]
fn cas_0a4c_explicit_missing_selector_fails_even_with_usable_default() {
    let mut env = fixture();
    env.set("VIOLET_SLACK_TOKEN_CASSY_PROXY", "default-fixture");
    env.set("VIOLET_SLACK_TOKEN_ENV", "VIOLET_SLACK_TOKEN_SELECTED");
    let error = connect_config(None).unwrap_err();
    assert_eq!(error.code, "missing_credential");
    assert!(error.message.contains("VIOLET_SLACK_TOKEN_SELECTED"));
    assert!(!error.message.contains("default-fixture"));
    env.set("VIOLET_SLACK_TOKEN_SELECTED", "selected-fixture");
    assert_eq!(bearer(None), "selected-fixture");
}

#[test]
fn cas_0a4c_worker_denial_blocks_inherited_and_private_credentials() {
    let mut env = fixture();
    env.set("CAS_AGENT_ROLE", "worker");
    env.set("VIOLET_SLACK_TOKEN_CASSY_PROXY", "inherited-fixture");
    private(
        env.home(),
        "VIOLET_SLACK_TOKEN_CASSY_PROXY='private-fixture'\nVIOLET_SLACK_TOKEN_OTHER='other-fixture'\n",
    );
    let root = env.home().join(".cas");
    std::fs::write(
        root.join("config.toml"),
        "[factory]\nsupervisor_only_env=['VIOLET_SLACK_TOKEN_CASSY_PROXY']\n",
    )
    .unwrap();
    let error = connect_config(Some(&root)).unwrap_err();
    assert_eq!(error.code, "missing_credential");
    assert!(error.message.contains("VIOLET_SLACK_TOKEN_CASSY_PROXY"));
    assert!(!error.message.contains("inherited-fixture"));
}

#[test]
fn cas_0a4c_ambiguous_accounts_and_missing_native_registration_fail_closed() {
    let env = fixture();
    private(
        env.home(),
        "VIOLET_SLACK_TOKEN_A='a-fixture'\nVIOLET_SLACK_TOKEN_B='b-fixture'\n",
    );
    let error = connect_config(None).unwrap_err();
    assert!(error.message.contains("ambiguous"));
    for name in ["VIOLET_SLACK_TOKEN_A", "VIOLET_SLACK_TOKEN_B"] {
        assert!(error.message.contains(name));
    }
    let registration = env.home().join(".claude.json");
    std::fs::write(&registration, r#"{"mcpServers":{"violet":{"headers":{"Authorization":"Bearer ${VIOLET_SLACK_TOKEN_MISSING}"}}}}"#).unwrap();
    let error = connect_config(None).unwrap_err();
    assert!(error.message.contains("VIOLET_SLACK_TOKEN_MISSING"));
    assert!(error.message.contains(&registration.display().to_string()));
    assert!(
        error
            .message
            .contains(&env.home().join("credentials.env").display().to_string())
    );
    assert!(!error.message.contains("a-fixture"));
    assert!(!error.message.contains("b-fixture"));
}

#[test]
fn cas_0a4c_registered_account_beats_other_accounts_and_ignores_non_bearer_headers() {
    let env = fixture();
    private(
        env.home(),
        "VIOLET_SLACK_TOKEN_A='a-fixture'\nVIOLET_SLACK_TOKEN_B='b-fixture'\nVIOLET_VERCEL_BYPASS='bypass-fixture'\n",
    );
    std::fs::write(env.home().join(".claude.json"), r#"{"mcpServers":{"violet":{"headers":{"Authorization":"Bearer ${VIOLET_SLACK_TOKEN_B}","x-vercel-protection-bypass":"${VIOLET_VERCEL_BYPASS}","X-Unrelated":"${VIOLET_SLACK_TOKEN_A}"}}}}"#).unwrap();
    assert_eq!(bearer(None), "b-fixture");
    std::fs::remove_file(env.home().join(".claude.json")).unwrap();
    std::fs::create_dir_all(env.home().join(".codex")).unwrap();
    std::fs::write(
        env.home().join(".codex/config.toml"),
        "[mcp_servers.violet]\nbearer_token_env_var='VIOLET_SLACK_TOKEN_A'\n",
    )
    .unwrap();
    assert_eq!(bearer(None), "a-fixture");
    std::fs::write(env.home().join(".claude.json"), r#"{"mcpServers":{"violet":{"headers":{"Authorization":"Bearer ${VIOLET_SLACK_TOKEN_B}"}}}}"#).unwrap();
    assert!(
        connect_config(None)
            .unwrap_err()
            .message
            .contains("ambiguous")
    );
}

#[test]
fn cas_0a4c_unmanaged_project_custom_refs_and_custom_hubs_remain_authoritative() {
    let mut env = fixture();
    let user = env.home().join(".config/code-mode-mcp/config.toml");
    env.set("VIOLET_SLACK_TOKEN_ENV", "VIOLET_SLACK_TOKEN_SELECTED");
    env.set("VIOLET_SLACK_TOKEN_SELECTED", "selected-fixture");
    // A user-authored document with even the default reference stays strict.
    proxy(&user, false, "VIOLET_SLACK_TOKEN_CASSY_PROXY");
    assert!(connect_config(None).is_err());
    // A managed document with a custom reference keeps that exact account.
    proxy(&user, true, "CUSTOM_ACCOUNT_TOKEN");
    assert!(connect_config(None).is_err());
    private(
        env.home(),
        "CUSTOM_ACCOUNT_TOKEN='custom-private-fixture'\n",
    );
    assert_eq!(bearer(None), "custom-private-fixture");
    // Project precedence is authoritative even for identical managed defaults.
    proxy(&user, true, "VIOLET_SLACK_TOKEN_CASSY_PROXY");
    let root = env.home().join(".cas");
    proxy(
        &root.join("proxy.toml"),
        true,
        "VIOLET_SLACK_TOKEN_CASSY_PROXY",
    );
    assert!(connect_config(Some(&root)).is_err());
    env.set("VIOLET_SLACK_TOKEN_CASSY_PROXY", "project-fixture");
    assert_eq!(bearer(Some(&root)), "project-fixture");
    env.remove("VIOLET_SLACK_TOKEN_CASSY_PROXY");
    let raw = std::fs::read_to_string(&user)
        .unwrap()
        .replace(violet_hub_url(), "https://custom.example/mcp");
    std::fs::write(&user, raw).unwrap();
    assert!(connect_config(None).is_err());
}

#[test]
fn cas_0a4c_generic_unique_and_legacy_exact_credentials_use_shared_safe_file_resolution() {
    let mut env = fixture();
    private(env.home(), "VIOLET_SLACK_TOKEN_ACCOUNT='unique-fixture'\n");
    assert_eq!(bearer(None), "unique-fixture");
    env.set("VIOLET_SLACK_TOKEN", "generic-fixture");
    assert_eq!(bearer(None), "generic-fixture");
    env.set("VIOLET_SLACK_TOKEN_CASSY_PROXY", "default-fixture");
    assert_eq!(bearer(None), "default-fixture");
    env.remove("VIOLET_SLACK_TOKEN_CASSY_PROXY");
    env.remove("VIOLET_SLACK_TOKEN");
    let legacy = cas_types::violet_compatibility::violet_compatibility();
    private(
        env.home(),
        &format!(
            "{}_CASSY_PROXY='legacy-private-fixture'\n",
            legacy.legacy_token_prefix
        ),
    );
    assert_eq!(bearer(None), "legacy-private-fixture");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        env.home().join("credentials.env"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(connect_config(None).is_err());
}

#[test]
fn cas_0a4c_worker_policy_blocks_selectors_native_refs_headers_and_servers() {
    let mut env = fixture();
    env.set("CAS_AGENT_ROLE", "worker");
    env.set("VIOLET_SLACK_TOKEN_ENV", "VIOLET_SLACK_TOKEN_ACCOUNT");
    env.set("VIOLET_SLACK_TOKEN_ACCOUNT", "denied-inherited-fixture");
    private(
        env.home(),
        "VIOLET_SLACK_TOKEN_ACCOUNT='denied-private-fixture'\nVIOLET_SLACK_TOKEN_OTHER='other-fixture'\n",
    );
    let root = env.home().join(".cas");
    let policy = root.join("config.toml");
    std::fs::write(
        &policy,
        "[factory]\nsupervisor_only_env=['VIOLET_SLACK_TOKEN_ACCOUNT']\n",
    )
    .unwrap();
    assert!(connect_config(Some(&root)).is_err());
    env.remove("VIOLET_SLACK_TOKEN_ENV");
    std::fs::write(env.home().join(".claude.json"), r#"{"mcpServers":{"violet":{"headers":{"Authorization":"Bearer ${VIOLET_SLACK_TOKEN_ACCOUNT}"}}}}"#).unwrap();
    assert!(connect_config(Some(&root)).is_err());
    env.set("VIOLET_SLACK_TOKEN_CASSY_PROXY", "allowed-fixture");
    let user = env.home().join(".config/code-mode-mcp/config.toml");
    let mut raw = std::fs::read_to_string(&user).unwrap();
    raw.push_str(
        "[servers.violet.headers]\nx-vercel-protection-bypass='env:VIOLET_VERCEL_BYPASS'\n",
    );
    std::fs::write(user, raw).unwrap();
    env.set("VIOLET_VERCEL_BYPASS", "denied-bypass-fixture");
    std::fs::write(
        &policy,
        "[factory]\nsupervisor_only_env=['VIOLET_VERCEL_BYPASS']\n",
    )
    .unwrap();
    assert!(connect_config(Some(&root)).is_err());
    std::fs::write(&policy, "[factory]\nsupervisor_only_mcp=['violet']\n").unwrap();
    assert_eq!(
        connect_config(Some(&root)).unwrap_err().code,
        "not_configured"
    );
}

#[test]
fn cas_0a4c_review_denied_bearer_cannot_use_legacy_spelling() {
    let mut env = fixture();
    env.set("CAS_AGENT_ROLE", "worker");
    let legacy = cas_types::violet_compatibility::violet_compatibility();
    let old = format!("{}_SUPERVISOR", legacy.legacy_token_prefix);
    let canonical = "VIOLET_SLACK_TOKEN_SUPERVISOR";
    let root = env.home().join(".cas");
    for denied in [canonical, old.as_str()] {
        std::fs::write(
            root.join("config.toml"),
            format!("[factory]\nsupervisor_only_env=[{denied:?}]\n"),
        )
        .unwrap();
        for selected in [old.as_str(), canonical] {
            env.set("VIOLET_SLACK_TOKEN_ENV", selected);
            for inherited in [false, true] {
                private(env.home(), &format!("{old}='denied-fixture'\n"));
                if inherited {
                    env.set(&old, "denied-fixture");
                } else {
                    env.remove(&old);
                }
                let error = connect_config(Some(&root)).unwrap_err();
                assert_eq!(error.code, "missing_credential");
                assert!(!error.message.contains("denied-fixture"));
            }
        }
    }
}

#[test]
fn cas_0a4c_review_selector_rejects_unrelated_secrets_and_bypass() {
    let mut env = fixture();
    let legacy = cas_types::violet_compatibility::violet_compatibility();
    for selected in [
        "UNRELATED_SECRET",
        "VIOLET_VERCEL_BYPASS",
        legacy.legacy_bypass_env.as_str(),
        "VIOLET_SLACK_TOKEN_ENV",
    ] {
        private(env.home(), &format!("{selected}='unrelated-fixture'\n"));
        env.set("VIOLET_SLACK_TOKEN_ENV", selected);
        let error = connect_config(None).unwrap_err();
        assert_eq!(error.code, "missing_credential");
        assert!(!error.message.contains("unrelated-fixture"));
    }
}

#[test]
fn cas_0a4c_review_legacy_selector_has_parity_and_modern_precedence() {
    let mut env = fixture();
    let legacy = cas_types::violet_compatibility::violet_compatibility();
    private(
        env.home(),
        "VIOLET_SLACK_TOKEN_A='a-fixture'\nVIOLET_SLACK_TOKEN_B='b-fixture'\nVIOLET_SLACK_TOKEN_CASSY_PROXY='default-fixture'\n",
    );
    env.set(
        &legacy.legacy_token_selector_env,
        format!("{}_A", legacy.legacy_token_prefix),
    );
    assert_eq!(bearer(None), "a-fixture");
    env.set("VIOLET_SLACK_TOKEN_ENV", "VIOLET_SLACK_TOKEN_B");
    assert_eq!(bearer(None), "b-fixture");
    env.set("VIOLET_SLACK_TOKEN_ENV", "VIOLET_SLACK_TOKEN_MISSING");
    assert!(
        connect_config(None)
            .unwrap_err()
            .message
            .contains("VIOLET_SLACK_TOKEN_MISSING")
    );
}
