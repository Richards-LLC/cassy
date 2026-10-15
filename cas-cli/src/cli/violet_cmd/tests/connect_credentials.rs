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
