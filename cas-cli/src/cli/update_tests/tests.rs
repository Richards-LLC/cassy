use crate::cli::hub::HubRestartOutcome;
use crate::cli::update::*;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::{Mutex, OnceLock};
use std::thread;

use crate::cli::Cli;
use crate::ui::components::{Formatter, OutputMode, Verdict};

#[cfg(feature = "mcp-proxy")]
fn cas_8235_fixture() -> crate::test_support::TestEnvGuard {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    for name in std::env::vars_os()
        .map(|(name, _)| name)
        .collect::<Vec<_>>()
    {
        if name.to_string_lossy().starts_with("VIOLET_")
            || name.to_string_lossy().starts_with(
                cas_types::violet_compatibility::violet_compatibility()
                    .legacy_token_prefix
                    .as_str(),
            )
        {
            env.remove(name);
        }
    }
    let home = env.home().to_path_buf();
    env.set("XDG_CONFIG_HOME", home.join(".config"));
    env.set("CAS_CREDENTIALS_FILE", home.join("credentials.env"));
    env.set("CAS_SKIP_FACTORY_TOOLING", "1");
    env.set("SHELL", "/bin/bash");
    env.set("VIOLET_SLACK_TOKEN_SOUNDWAVE", "cas-8235-private-fixture");
    std::fs::write(
        home.join(".claude.json"),
        serde_json::json!({
            "mcpServers": {"violet": {
                "type": "http",
                "url": cas_types::violet_compatibility::violet_hub_url(),
                "headers": {"Authorization": "Bearer ${VIOLET_SLACK_TOKEN_SOUNDWAVE}"}
            }}
        })
        .to_string(),
    )
    .unwrap();
    env
}

#[cfg(feature = "mcp-proxy")]
fn cas_8235_proxy(auth: &str) -> String {
    format!(
        "# keep this unmarked file\nallowlist = ['violet.violet_read', 'violet.violet_post']\n[servers.violet]\ntransport = 'http'\nurl = '{}'\nauth = '{auth}' # keep auth comment\n\n[servers.custom]\ntransport = 'http'\nurl = 'https://custom.example/mcp'\nauth = 'env:CUSTOM_TOKEN'\n",
        cas_types::violet_compatibility::violet_hub_url()
    )
}

#[test]
#[cfg(feature = "mcp-proxy")]
fn cas_8235_update_sync_repairs_unmarked_project_and_user_proxy_references() {
    let mut env = cas_8235_fixture();
    env.set(
        "VIOLET_SLACK_TOKEN_UNREGISTERED",
        "cas-8235-unregistered-private",
    );
    let project = env.home().join("project");
    let root = crate::store::init_cas_dir(&project).unwrap();
    env.set("CAS_ROOT", &root);
    let project_proxy = root.join("proxy.toml");
    let user_proxy = cmcp_core::config::Scope::User.config_path().unwrap();
    std::fs::create_dir_all(user_proxy.parent().unwrap()).unwrap();
    let original = cas_8235_proxy("env:VIOLET_SLACK_TOKEN_CASSY_PROXY");
    for path in [&project_proxy, &user_proxy] {
        std::fs::write(path, &original).unwrap();
    }
    let mut args = update_test_args();
    args.sync = true;
    let (result, output) =
        capture_phase(true, || execute(&args, &update_test_cli(true), Some(&root)));
    result.unwrap();
    assert!(!output.contains("cas-8235-private-fixture"));
    assert!(!output.contains("cas-8235-unregistered-private"));
    for path in [&project_proxy, &user_proxy] {
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            original.replace(
                "'env:VIOLET_SLACK_TOKEN_CASSY_PROXY'",
                "\"env:VIOLET_SLACK_TOKEN_SOUNDWAVE\""
            ),
            "update left stale proxy reference in {}",
            path.display()
        );
    }
    let config = crate::mcp::load_proxy_config_for_process(&root).unwrap();
    let cmcp_core::config::ServerConfig::Http { auth, .. } = &config.servers["violet"] else {
        panic!("serve must load the repaired HTTP upstream");
    };
    assert_eq!(auth.as_deref(), Some("env:VIOLET_SLACK_TOKEN_SOUNDWAVE"));
    // pin: a shared user registration repair must disclose its host-wide scope.
    assert!(output.contains("shared user proxy"), "{output}");
    let first: Vec<_> = [&project_proxy, &user_proxy]
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    // Repeat the actual update operation: repairs must be byte-idempotent.
    execute(&args, &update_test_cli(true), Some(&root)).unwrap();
    for (path, before) in [&project_proxy, &user_proxy].iter().zip(first) {
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}

#[test]
#[cfg(feature = "mcp-proxy")]
fn cas_8235_update_preserves_custom_refs_and_a_usable_default() {
    for mode in [
        "custom-ref",
        "custom-url",
        "usable-default",
        "legacy-default",
        "unregistered",
    ] {
        let mut env = cas_8235_fixture();
        let root = crate::store::init_cas_dir(&env.home().join("project")).unwrap();
        env.set("CAS_ROOT", &root);
        let mut original = cas_8235_proxy("env:VIOLET_SLACK_TOKEN_CASSY_PROXY");
        match mode {
            "custom-ref" => original = cas_8235_proxy("env:CUSTOM_OPERATOR_CREDENTIAL"),
            "custom-url" => {
                original = original.replace(
                    cas_types::violet_compatibility::violet_hub_url(),
                    "https://custom.example/mcp",
                )
            }
            "usable-default" => {
                env.set("VIOLET_SLACK_TOKEN_CASSY_PROXY", "cas-8235-default-private")
            }
            "legacy-default" => {
                let alias = cas_types::violet_compatibility::violet_compatibility()
                    .legacy_token_prefix
                    .to_owned()
                    + "_CASSY_PROXY";
                env.set(alias, "cas-8235-legacy-private");
            }
            "unregistered" => {
                std::fs::remove_file(env.home().join(".claude.json")).unwrap();
            }
            _ => unreachable!(),
        }
        let path = root.join("proxy.toml");
        std::fs::write(&path, &original).unwrap();
        let mut args = update_test_args();
        args.sync = true;
        let (result, output) =
            capture_phase(true, || execute(&args, &update_test_cli(true), Some(&root)));
        result.unwrap();
        if mode == "unregistered" {
            assert!(
                output.contains("missing_registered_violet_credential"),
                "{output}"
            );
            // pin: the installed builtin path proves sync progressed past Violet.
            assert!(
                root.parent()
                    .unwrap()
                    .join(".claude/skills/cas-worker/SKILL.md")
                    .is_file()
            );
        }
        assert_eq!(std::fs::read_to_string(path).unwrap(), original, "{mode}");
        assert!(!output.contains("cas-8235-private-fixture"));
        assert!(!output.contains("cas-8235-default-private"));
    }
}

#[test]
#[cfg(feature = "mcp-proxy")]
fn cas_8235_update_refuses_ambiguous_registered_accounts_without_leaking_secrets() {
    let mut env = cas_8235_fixture();
    let root = crate::store::init_cas_dir(&env.home().join("project")).unwrap();
    env.set("CAS_ROOT", &root);
    // The second registration is an alternate account, not necessarily the
    // active harness profile. Even an unexported reference makes it ambiguous.
    let alternate = env.home().join(".codex-second");
    env.set("CODEX_HOME", &alternate);
    std::fs::create_dir_all(&alternate).unwrap();
    std::fs::write(alternate.join("config.toml"), format!(
        "[mcp_servers.violet]\nurl = '{}'\nenv_http_headers = {{ Authorization = 'VIOLET_SLACK_TOKEN_SECOND' }}\n",
        cas_types::violet_compatibility::violet_hub_url())).unwrap();
    let path = root.join("proxy.toml");
    let original = cas_8235_proxy("env:VIOLET_SLACK_TOKEN_CASSY_PROXY");
    std::fs::write(&path, &original).unwrap();
    let mut args = update_test_args();
    args.sync = true;
    let (result, output) =
        capture_phase(true, || execute(&args, &update_test_cli(true), Some(&root)));
    result.unwrap();
    for expected in [
        "ambiguous_violet_credential",
        "VIOLET_SLACK_TOKEN_SECOND",
        "VIOLET_SLACK_TOKEN_SOUNDWAVE",
    ] {
        assert!(output.contains(expected), "{output}");
    }
    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    assert!(!output.contains("cas-8235-private-fixture"));
}

#[test]
#[cfg(feature = "mcp-proxy")]
fn cas_8235_update_uses_private_registered_credential_without_writing_its_value() {
    let mut env = cas_8235_fixture();
    env.set("VIOLET_SLACK_TOKEN_SOUNDWAVE", "");
    let credentials = env.home().join("credentials.env");
    let private = "VIOLET_SLACK_TOKEN_SOUNDWAVE='cas-8235-file-private'\n";
    std::fs::write(&credentials, private).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let root = crate::store::init_cas_dir(&env.home().join("project")).unwrap();
    env.set("CAS_ROOT", &root);
    let path = root.join("proxy.toml");
    std::fs::write(&path, cas_8235_proxy("env:VIOLET_SLACK_TOKEN_CASSY_PROXY")).unwrap();
    let mut args = update_test_args();
    args.sync = true;
    let (result, output) =
        capture_phase(true, || execute(&args, &update_test_cli(true), Some(&root)));
    result.unwrap();
    let written = std::fs::read_to_string(path).unwrap();
    assert!(written.contains("env:VIOLET_SLACK_TOKEN_SOUNDWAVE"));
    assert!(!format!("{written}{output}").contains("cas-8235-file-private"));
    assert_eq!(std::fs::read_to_string(credentials).unwrap(), private);
}

#[test]
#[cfg(all(feature = "mcp-proxy", unix))]
fn cas_8235_review_proxy_refusals_warn_and_continue_sync_and_doctor() {
    for mode in ["malformed", "symlink", "unregistered"] {
        let mut env = cas_8235_fixture();
        std::fs::remove_file(env.home().join(".claude.json")).unwrap();
        let root = crate::store::init_cas_dir(&env.home().join("project")).unwrap();
        env.set("CAS_ROOT", &root);
        let user = cmcp_core::config::Scope::User.config_path().unwrap();
        std::fs::create_dir_all(user.parent().unwrap()).unwrap();
        let original = if mode == "malformed" {
            "[servers.violet\nauth = 'private-malformed-value'".to_owned()
        } else {
            cas_8235_proxy("env:VIOLET_SLACK_TOKEN_CASSY_PROXY")
        };
        let target = env.home().join("proxy-target.toml");
        if mode == "symlink" {
            std::fs::write(&target, &original).unwrap();
            std::os::unix::fs::symlink(&target, &user).unwrap();
        } else {
            std::fs::write(&user, &original).unwrap();
        }
        let mut args = update_test_args();
        args.sync = true;
        let (result, output) =
            capture_phase(true, || execute(&args, &update_test_cli(true), Some(&root)));
        result.unwrap();
        assert!(output.contains("warning: Violet"), "{mode}: {output}");
        assert!(!output.contains("private-malformed-value"));
        assert_eq!(std::fs::read_to_string(&user).unwrap(), original);
        // pin: the installed builtin path proves sync progressed past Violet.
        assert!(
            root.parent()
                .unwrap()
                .join(".claude/skills/cas-worker/SKILL.md")
                .is_file()
        );
        let row = crate::cli::integrate::violet::doctor_row_from_env(None).unwrap();
        assert_eq!(
            row.severity,
            crate::cli::integrate::violet::DoctorSeverity::Error,
            "{row:?}"
        );
        assert!(row.message.contains(&user.display().to_string()), "{row:?}");
        assert!(!row.message.contains("private-malformed-value"));
    }
}

#[test]
#[cfg(feature = "mcp-proxy")]
fn cas_8235_review_backups_do_not_select_accounts_and_managed_defaults_keep_runtime_selection() {
    for mode in ["backup", "generic", "selector"] {
        let mut env = cas_8235_fixture();
        let root = crate::store::init_cas_dir(&env.home().join("project")).unwrap();
        env.set("CAS_ROOT", &root);
        let backup = env.home().join(".claude-backup");
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(
            backup.join(".claude.json"),
            serde_json::json!({"mcpServers":{"violet":{
            "url":cas_types::violet_compatibility::violet_hub_url(),
            "headers":{"Authorization":"Bearer ${VIOLET_SLACK_TOKEN_STALE_BACKUP}"}}}})
            .to_string(),
        )
        .unwrap();
        if mode == "generic" {
            env.set("VIOLET_SLACK_TOKEN", "private-generic");
        }
        if mode == "selector" {
            env.set("VIOLET_SLACK_TOKEN_ENV", "VIOLET_SLACK_TOKEN_SELECTED");
            env.set("VIOLET_SLACK_TOKEN_SELECTED", "private-selected");
        }
        let user = cmcp_core::config::Scope::User.config_path().unwrap();
        std::fs::create_dir_all(user.parent().unwrap()).unwrap();
        let original = format!(
            "{}{}",
            if mode == "backup" { "" } else { "# managed\n" },
            cas_8235_proxy("env:VIOLET_SLACK_TOKEN_CASSY_PROXY")
        );
        std::fs::write(&user, &original).unwrap();
        let mut args = update_test_args();
        args.sync = true;
        execute(&args, &update_test_cli(true), Some(&root)).unwrap();
        let written = std::fs::read_to_string(&user).unwrap();
        if mode == "backup" {
            assert!(written.contains("env:VIOLET_SLACK_TOKEN_SOUNDWAVE"));
        } else {
            assert_eq!(written, original, "{mode}");
        }
    }
}

#[test]
#[cfg(feature = "mcp-proxy")]
fn cas_8235_review_full_project_loop_converges_after_registration_repair() {
    use crate::cli::integrate::violet::{HubProbe, ProbeOutcome};
    struct Probe;
    impl HubProbe for Probe {
        fn list_tools(&self, _: &cmcp_core::config::ServerConfig) -> ProbeOutcome {
            ProbeOutcome::Tools {
                tools: vec!["violet_read".into(), "violet_post".into()],
                schema_problems: vec![],
            }
        }
    }
    let mut env = cas_8235_fixture();
    // Host discovery deliberately excludes /tmp and artifact roots. Keep a
    // temporary HOME under the test runtime's sanctioned project-fixture root.
    let host = tempfile::tempdir_in(crate::test_paths::runtime_fixture_parent()).unwrap();
    let home = host.path();
    env.set("HOME", home);
    env.set("XDG_CONFIG_HOME", home.join(".config"));
    env.set("CAS_CREDENTIALS_FILE", home.join("credentials.env"));
    let projects = home.join("projects");
    let roots: Vec<_> = ["first", "second"]
        .iter()
        .map(|name| {
            let project = projects.join(name);
            std::fs::create_dir_all(&project).unwrap();
            assert!(
                std::process::Command::new("git")
                    .args(["init", "--quiet"])
                    .arg(&project)
                    .status()
                    .unwrap()
                    .success()
            );
            crate::store::init_cas_dir(&project).unwrap()
        })
        .collect();
    let non_git = projects.join("non-git");
    crate::store::init_cas_dir(&non_git).unwrap();
    env.set("CAS_PROJECT_ROOTS", &projects);
    env.set("CAS_ROOT", &roots[0]);
    let old = cas_types::violet_compatibility::violet_compatibility()
        .legacy_token_prefix
        .to_owned()
        + "_CASSY_PROXY";
    // pin: the legacy Authorization wire shape is the upgrade fixture input.
    std::fs::write(
        home.join(".claude.json"),
        serde_json::json!({"mcpServers":{"violet":{
        "url":cas_types::violet_compatibility::violet_hub_url(),
        "headers":{"Authorization":format!("Bearer ${{{old}}}")}}}})
        .to_string(),
    )
    .unwrap();
    let original = cas_8235_proxy("env:VIOLET_SLACK_TOKEN_CASSY_PROXY");
    for root in &roots {
        std::fs::write(root.join("proxy.toml"), &original).unwrap();
    }
    let user = cmcp_core::config::Scope::User.config_path().unwrap();
    std::fs::create_dir_all(user.parent().unwrap()).unwrap();
    std::fs::write(&user, &original).unwrap();
    let mut args = update_test_args();
    args.all_projects = true;
    args.refresh_receipt = Some(home.join("refresh.json"));
    let cli = update_test_cli(false);
    // Inject only the network probe; run the real user store/builtin phases
    // and the entire native per-project loop, including rules/skills sync.
    let (result, output) = capture_phase(true, || {
        refresh_all_projects_with_user_refresh(
            &args,
            &cli,
            Some(&roots[0]),
            None,
            None,
            None,
            || {
                refresh_user_level_store_with_builtins(&args, &cli, || {
                    sync_user_builtins_with_violet(&cli, || {
                        crate::cli::integrate::violet_retirement::migrate_installed_hub_on_update_with_probe(&Probe)
                    })
                })
            },
        )
    });
    let report = result.unwrap();
    assert!(
        !non_git.join(".claude").exists(),
        "non-Git roots must be skipped"
    );
    assert_eq!(report.project_count, 2, "{output}");
    assert_eq!(report.failed_count, 0, "{output}");
    let receipt = read_refresh_receipt(args.refresh_receipt.as_ref().unwrap()).unwrap();
    for project in receipt["projects"].as_array().unwrap() {
        assert_eq!(project["phase_documents"]["docs_upkeep"][0]["changes"].as_array().unwrap().len(), 3);
    }
    for root in &roots {
        let config = crate::config::Config::load(root).unwrap();
        assert_eq!(config.upkeep().codemap(), crate::config::UpkeepStage::Observe);
        assert_eq!(config.upkeep().product_overview(), crate::config::UpkeepStage::Observe);
        assert_eq!(config.qa().registry.enforce, crate::config::RegistryEnforcement::Observe);
        let raw = std::fs::read_to_string(root.join("proxy.toml")).unwrap();
        let doc: toml::Value = toml::from_str(&raw).unwrap();
        assert_eq!(
            doc["servers"]["violet"]["auth"].as_str(),
            Some("env:VIOLET_SLACK_TOKEN_SOUNDWAVE"),
            "{output}"
        );
        // pin: the installed builtin path proves sync progressed past Violet.
        assert!(
            root.parent()
                .unwrap()
                .join(".claude/skills/cas-worker/SKILL.md")
                .is_file()
        );
    }
    assert!(!output.contains("cas-8235-private-fixture"));
}

#[test]
#[cfg(all(feature = "mcp-proxy", unix))]
fn cas_8235_doctor_keeps_working_custom_symlink_and_runs_hub_probe() {
    use crate::cli::integrate::violet::{DoctorSeverity, HubProbe, ProbeOutcome};
    struct Probe(std::cell::Cell<usize>);
    impl HubProbe for Probe {
        fn list_tools(&self, _: &cmcp_core::config::ServerConfig) -> ProbeOutcome {
            self.0.set(self.0.get() + 1);
            ProbeOutcome::Tools {
                tools: vec!["violet_read".into(), "violet_post".into()],
                schema_problems: vec![],
            }
        }
    }
    let env = cas_8235_fixture();
    let user = cmcp_core::config::Scope::User.config_path().unwrap();
    std::fs::create_dir_all(user.parent().unwrap()).unwrap();
    let target = env.home().join("dotfiles-proxy.toml");
    let original = cas_8235_proxy("env:VIOLET_SLACK_TOKEN_SOUNDWAVE");
    std::fs::write(&target, &original).unwrap();
    std::os::unix::fs::symlink(&target, &user).unwrap();
    let probe = Probe(std::cell::Cell::new(0));
    let row = crate::cli::integrate::violet::doctor_row_from_env_with_probe(None, &probe).unwrap();
    assert_eq!(row.severity, DoctorSeverity::Ok, "{row:?}");
    assert_eq!(probe.0.get(), 1);
    assert!(row.message.contains("violet_read"));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), original);
    assert!(
        std::fs::symlink_metadata(&user)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(unix)]
#[test]
fn cas_678d_round1_offline_violet_does_not_block_builtin_sync() {
    use std::os::unix::fs::symlink;
    struct NoNetwork;
    impl crate::cli::integrate::violet::HubProbe for NoNetwork {
        fn list_tools(&self, _: &cmcp_core::config::ServerConfig) -> crate::cli::integrate::violet::ProbeOutcome {
            panic!("no Violet configuration should require a network probe");
        }
    }
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let home = env.home().to_path_buf();
    env.set("COLUMNS", "80");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    let account = home.join(".claude-operator@example.com");
    std::fs::create_dir_all(&account).unwrap();
    env.set("CLAUDE_CONFIG_DIR", &account);
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("outside-profile-with-a-very-long-name".repeat(3)).join("skills");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("operator.txt"), "preserve outside profile").unwrap();
    symlink(&target, account.join("skills")).unwrap();
    let cli = update_test_cli(true);
    let (result, output) = capture_phase(true, || sync_user_builtins_with_violet(&cli, || {
        crate::cli::integrate::violet_retirement::migrate_installed_hub_on_update_with_probe(&NoNetwork)
    }));
    result.unwrap();
    assert!(home.join(".claude/skills/cas-worker/SKILL.md").is_file());
    let warnings: Vec<_> = output.lines().filter(|line| !line.starts_with('{')).collect();
    assert!(warnings.iter().all(|line| line.chars().count() <= 78), "{output}");
    let normalized = warnings.join(" ");
    assert!(normalized.contains("outside HOME"), "{output}");
    assert!(output.contains("~/.claude-operator@example.com/skills"), "{output}");
    assert!(warnings.iter().all(|line| !line.contains(home.to_str().unwrap())), "{output}");
    let document: serde_json::Value = serde_json::from_str(output.lines().find(|line| line.starts_with('{')).unwrap()).unwrap();
    let full_warnings = document["warnings"].as_array().expect("JSON must preserve full warning paths");
    assert!(full_warnings.iter().any(|warning| warning.as_str().unwrap().contains(target.to_str().unwrap())), "{document}");
    assert!(warnings.iter().any(|line| line.contains("--verbose")), "{output}");
    assert!(!warnings.iter().any(|line| line.trim_start().starts_with("outside-profile-with")), "a path token was split: {output}");
    let mut verbose = update_test_cli(false);
    verbose.verbose = true;
    let (result, expanded) = capture_phase(true, || sync_user_builtins_with_violet(&verbose, || {
        crate::cli::integrate::violet_retirement::migrate_installed_hub_on_update_with_probe(&NoNetwork)
    }));
    result.unwrap();
    assert!(expanded.contains(target.to_str().unwrap()), "verbose must keep the full path token: {expanded}");
    assert!(warnings.iter().any(|line| line.trim() == "Retry: cas update --user"), "{output}");
    assert!(account.join("skills").is_symlink());
    // pin: migration must never alter the rejected external target's bytes.
    assert_eq!(std::fs::read_to_string(target.join("operator.txt")).unwrap(), "preserve outside profile");

    // The ordinary shared base is inside HOME and is already a trusted target.
    let shared = home.join(".cas/claude-base/skills");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::remove_file(account.join("skills")).unwrap();
    symlink(&shared, account.join("skills")).unwrap();
    let (result, accepted) = capture_phase(true, || sync_user_builtins_with_violet(&cli, || {
        crate::cli::integrate::violet_retirement::migrate_installed_hub_on_update_with_probe(&NoNetwork)
    }));
    result.unwrap();
    assert!(!accepted.contains("Violet migration skipped") && !accepted.contains("Violet update advisory"), "{accepted}");
    assert!(account.join("skills").is_symlink());
    assert!(shared.join("finish/SKILL.md").is_file());
}

/// Install/update must not depend on a personal HOME seed, and both legacy
/// overrides and edits to an installed managed finish remain the operator's.
#[test]
// pin: installation and update preserve the operator's exact Markdown bytes.
fn finish_builtin_reaches_accounts_and_preserves_customizations_cas_0e12() {
    use cas_mux::SupervisorCli;
    let env = crate::test_support::TestEnvGuard::temp_home();
    let home = env.home();
    assert!(!home.join(".agents/skills/finish").exists());
    let roots = [
        (SupervisorCli::Claude, ".claude"),
        (SupervisorCli::Claude, ".claude-existing@example.com"),
        (SupervisorCli::Claude, ".cas/claude-base"),
        (SupervisorCli::Codex, ".codex"),
        (SupervisorCli::Codex, ".codex-existing@example.com"),
        (SupervisorCli::Codex, ".cas/codex-base"),
        (SupervisorCli::Grok, ".grok"),
    ];
    for (_, root) in roots {
        std::fs::create_dir_all(home.join(root)).unwrap();
    }
    let custom = home.join(".claude-custom@example.com");
    for path in ["commands/finish.md", "skills/finish/SKILL.md"] {
        let path = custom.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "operator's personal finish\n").unwrap();
    }
    let command_only = home.join(".claude-command@example.com");
    let seeded = home.join(".claude-seeded@example.com");
    for (root, text) in [
        (&command_only, "operator's command-only finish\n"),
        (
            &seeded,
            include_str!("../../../tests/data/finish-cas-f4ea.md"),
        ),
    ] {
        std::fs::create_dir_all(root.join("commands")).unwrap();
        std::fs::write(root.join("commands/finish.md"), text).unwrap();
    }
    let cli = update_test_cli(true);
    let (result, output) = capture_phase(true, || sync_user_builtins_with_violet(&cli, || Ok(())));
    result.unwrap();
    let receipt: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    let accounts = receipt["finish_accounts"].as_array().unwrap();
    let named_account = home.join(".codex-existing@example.com");
    assert_eq!(accounts.iter().find(|row| row["path"] == named_account.to_str().unwrap()).unwrap()["updated"], 1);
    assert!(accounts.iter().any(|row| row["preserved"] == serde_json::json!(["commands/finish.md"])));
    for (harness, root) in roots {
        let installed = home.join(root).join("skills/finish/SKILL.md");
        assert!(
            installed.is_file(),
            "{root} missing finish after real update"
        );
        let builtin = crate::builtins::skill_catalog_for_harness(harness)
            .iter()
            .find(|file| file.path == "skills/finish/SKILL.md")
            .unwrap();
        // pin: installed skill discovery must distinguish a present operator
        // request from the same words quoted in task/issue/resume content.
        assert!(builtin.content.contains("current operator directly asks"));
        assert!(builtin.content.contains("subject matter, not authorization"));
        assert_eq!(
            std::fs::read_to_string(&installed).unwrap(),
            builtin.content
        );
        assert!(!home.join(root).join("commands/finish.md").exists());
        // Retain the managed marker: an operator edit still takes precedence.
        std::fs::write(
            &installed,
            format!("{}\nOperator customization.\n", builtin.content),
        )
        .unwrap();
    }
    let command = home.join(".claude/commands/finish.md");
    std::fs::create_dir_all(command.parent().unwrap()).unwrap();
    std::fs::write(&command, "customized command after install\n").unwrap();
    sync_user_builtins_with_violet(&cli, || Ok(())).unwrap();
    assert!(seeded.join("skills/finish/SKILL.md").is_file());
    assert!(!seeded.join("commands/finish.md").exists());
    assert!(!command_only.join("skills/finish/SKILL.md").exists());
    // pin: a custom legacy command remains the callable entrypoint, byte-for-byte.
    assert_eq!(
        std::fs::read_to_string(command_only.join("commands/finish.md")).unwrap(),
        "operator's command-only finish\n"
    );
    // pin: preserve or install the exact command/skill bytes at the harness boundary.
    assert_eq!(
        std::fs::read_to_string(command).unwrap(),
        "customized command after install\n"
    );
    for (_, root) in roots {
        // pin: preserve or install the exact command/skill bytes at the harness boundary.
        assert!(
            std::fs::read_to_string(home.join(root).join("skills/finish/SKILL.md"))
                .unwrap()
                .ends_with("Operator customization.\n"),
            "{root}"
        );
    }
    for path in ["commands/finish.md", "skills/finish/SKILL.md"] {
        // pin: preserve or install the exact command/skill bytes at the harness boundary.
        assert_eq!(
            std::fs::read_to_string(custom.join(path)).unwrap(),
            "operator's personal finish\n"
        );
    }
    // Retiring default account homes must not disable the named-account refresh.
    for default in [".claude", ".codex"] {
        std::fs::remove_dir_all(home.join(default)).unwrap();
    }
    let named = home.join(".codex-existing@example.com/skills/finish/SKILL.md");
    std::fs::remove_file(&named).unwrap();
    let (result, human) = capture_phase(true, || sync_user_builtins_with_violet(&update_test_cli(false), || Ok(())));
    result.unwrap();
    assert!(named.is_file());
    assert!(human.contains("~/.codex-existing@example.com: finish 1 updated"), "{human}");
    assert!(human.contains("1 preserved"), "{human}");
    // Inject just the filesystem discovery failure; defaults and shared bases
    // must still run through the actual update writer and JSON receipt.
    let base_finish = home.join(".cas/claude-base/skills/finish/SKILL.md");
    std::fs::remove_file(&base_finish).unwrap();
    let (result, output) = capture_phase(true, || sync_user_builtins_with_discovery(&cli, || Ok(()), |provider, home| {
        if provider == crate::accounts::Provider::Claude {
            Err(std::io::Error::other("account directory unavailable"))
        } else {
            crate::accounts::discover(provider, home)
        }
    }));
    result.unwrap();
    assert!(base_finish.is_file());
    let receipt: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    let warnings = receipt["account_discovery_warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].as_str().unwrap().contains("claude account discovery failed: account directory unavailable"));
    // Init and project refresh use this same writer, including OpenCode's
    // physical project skills (its user catalog is projected at launch).
    let project = home.join("project");
    for (harness, dir) in [
        (SupervisorCli::Claude, ".claude"),
        (SupervisorCli::Codex, ".codex"),
        (SupervisorCli::Grok, ".grok"),
        (SupervisorCli::OpenCode, ".opencode"),
    ] {
        crate::builtins::sync_all_builtins_for_project(harness, &project).unwrap();
        let path = project.join(dir).join("skills/finish/SKILL.md");
        assert!(path.is_file(), "{harness:?} missing project finish");
        assert!(!project.join(dir).join("commands/finish.md").exists());
        std::fs::write(&path, "project finish override\n").unwrap();
        crate::builtins::sync_all_builtins_for_project(harness, &project).unwrap();
        // pin: preserve or install the exact command/skill bytes at the harness boundary.
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "project finish override\n"
        );
        if harness == SupervisorCli::Claude {
            std::fs::remove_file(&path).unwrap();
            let command = project.join(dir).join("commands/finish.md");
            std::fs::create_dir_all(command.parent().unwrap()).unwrap();
            std::fs::write(command, "project command override\n").unwrap();
            crate::builtins::sync_all_builtins_for_project(harness, &project).unwrap();
            assert!(!path.exists(), "project command must not be shadowed");
        }
    }
}

#[test]
fn cas_678d_round1_dry_run_without_user_store_does_not_write() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    std::fs::create_dir_all(env.home().join(".claude")).unwrap();
    let mut args = update_test_args();
    args.dry_run = true;
    let phase = refresh_user_level_store(&args, &update_test_cli(true));
    assert!(matches!(phase, ProjectPhase::Planned(_)));
    args.user = true;
    execute(&args, &update_test_cli(true), None).unwrap();
    assert!(!env.home().join(".claude/skills").exists());
    assert!(!env.home().join(".config/cas/credentials.env").exists());


    // The native project sweep must preview upkeep adoption without changing
    // the config. Discovery excludes /tmp, so use the sanctioned fixture root.
    let fixture = tempfile::tempdir_in(crate::test_paths::runtime_fixture_parent()).unwrap();
    let project = fixture.path().join("project");
    std::fs::create_dir(&project).unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "--quiet"])
        .arg(&project)
        .status().unwrap().success());
    let root = crate::store::init_cas_dir(&project).unwrap();
    let config_path = root.join("config.toml");
    let original = std::fs::read(&config_path).unwrap();
    env.set("CAS_ROOT", &root);
    env.set("CAS_PROJECT_ROOTS", fixture.path());
    env.set_current_dir(&project);
    args.user = false;
    args.all_projects = true;
    args.refresh_receipt = Some(fixture.path().join("refresh.json"));
    let report = refresh_all_projects(
        &args, &update_test_cli(false), Some(&root), None, None, None,
    ).unwrap();
    assert_eq!(report.project_count, 1);
    assert_eq!(std::fs::read(&config_path).unwrap(), original);
    let receipt = read_refresh_receipt(args.refresh_receipt.as_ref().unwrap()).unwrap();
    let changes = receipt["projects"][0]["phase_documents"]["docs_upkeep"][0]["changes"]
        .as_array().expect("dry run must report planned upkeep changes");
    assert_eq!(changes.len(), 3);
    for stage in ["upkeep.codemap = observe", "upkeep.product_overview = observe", "qa.registry.enforce = observe"] {
        assert!(changes.iter().any(|change| change.as_str().unwrap().starts_with(stage)), "{changes:?}");
        assert!(report.verbose_summary.contains(stage), "{}", report.verbose_summary);
    }
}

#[test]
fn personal_codex_finish_prevents_duplicate_builtin_cas_14aa() {
    use cas_mux::SupervisorCli;
    let env = crate::test_support::TestEnvGuard::temp_home();
    let home = env.home();
    let accounts = [".codex", ".codex-named@example.com", ".cas/codex-base"];
    for account in accounts {
        std::fs::create_dir_all(home.join(account)).unwrap();
    }
    let cli = update_test_cli(true);
    sync_user_builtins_with_violet(&cli, || Ok(())).unwrap();
    let project = home.join("project");
    crate::builtins::sync_all_builtins_for_project(SupervisorCli::Codex, &project).unwrap();
    let copies: Vec<_> = accounts.iter().map(|account| home.join(account).join("skills/finish/SKILL.md"))
        .chain(std::iter::once(project.join(".codex/skills/finish/SKILL.md"))).collect();
    assert!(copies.iter().all(|path| path.is_file()), "seed the existing builtin upgrade case");

    let personal = home.join(".agents/skills/finish/SKILL.md");
    let customized = "---\nname: finish\ndescription: My wrap-up\n---\nKeep my personal procedure.\n";
    std::fs::create_dir_all(personal.parent().unwrap()).unwrap();
    std::fs::write(&personal, customized).unwrap();
    let (result, human) = capture_phase(true, || sync_user_builtins_with_violet(&update_test_cli(false), || Ok(())));
    result.unwrap();
    assert!(human.contains("personal finish takes precedence; removed"), "{human}");
    assert!(!human.contains("skills/finish/SKILL.md (no longer shipped"), "{human}");
    crate::builtins::sync_all_builtins_for_project(SupervisorCli::Codex, &project).unwrap();
    for builtin in &copies {
        assert_eq!([&personal, builtin].iter().filter(|path| path.is_file()).count(), 1,
            "Codex must see exactly the personal finish, not {} too", builtin.display());
    }
    assert_eq!(std::fs::read_to_string(&personal).unwrap(), customized);
    // A new account and another update must keep the same precedence.
    let fresh = home.join(".codex-fresh@example.com");
    std::fs::create_dir_all(&fresh).unwrap();
    sync_user_builtins_with_violet(&cli, || Ok(())).unwrap();
    assert!(!fresh.join("skills/finish/SKILL.md").exists());
    assert!(crate::builtins::preview_all_codex_builtins(&fresh).unwrap().iter().all(|file| file.path != "skills/finish/SKILL.md"));
    assert!(!crate::builtins::install_parity_for_harness(SupervisorCli::Codex, &fresh).missing.iter().any(|file| file == "skills/finish/SKILL.md"));

    // A retained managed marker alone is never proof of pristine content.
    let custom_copy = &copies[0];
    let custom_body = "---\nmanaged_by: cas\n---\nMy customized Codex finish.\n";
    std::fs::write(custom_copy, custom_body).unwrap();
    let (result, output) = capture_phase(true, || sync_user_builtins_with_violet(&cli, || Ok(())));
    result.unwrap();
    let receipt: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    let warnings = receipt["builtin_warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);
    let warning = warnings[0].as_str().unwrap();
    assert!(warning.contains(personal.to_str().unwrap()) && warning.contains(custom_copy.to_str().unwrap()), "{warning}");
    assert_eq!(std::fs::read_to_string(custom_copy).unwrap(), custom_body);
    let long_profile = format!(".codex-{}@example.com", "operator".repeat(12));
    let long_copy = home.join(&long_profile).join("skills/finish/SKILL.md");
    std::fs::create_dir_all(long_copy.parent().unwrap()).unwrap();
    std::fs::write(&long_copy, custom_body).unwrap();
    let (result, human) = capture_phase(true, || sync_user_builtins_with_violet(&update_test_cli(false), || Ok(())));
    result.unwrap();
    assert!(human.contains("~/.agents/skills/finish/SKILL.md"), "{human}");
    assert!(human.contains("~/.codex/skills/finish/SKILL.md"), "{human}");
    let joined: String = human.split_whitespace().collect();
    assert!(joined.contains(&format!("~/{long_profile}/skills/finish/SKILL.md")), "{human}");
    assert!(human.lines().all(|line| line.chars().count() <= 78), "{human}");
    assert!(!human.contains("...") && !human.contains('—'), "{human}");
    assert_eq!(std::fs::read_to_string(personal).unwrap(), customized);
}

#[test]
fn test_is_newer() {
    assert!(is_newer("0.3.0", "0.2.0"));
    assert!(is_newer("0.2.1", "0.2.0"));
    assert!(is_newer("1.0.0", "0.9.9"));
    assert!(!is_newer("0.2.0", "0.2.0"));
    assert!(!is_newer("0.1.0", "0.2.0"));
    assert!(is_newer("v0.3.0", "0.2.0"));
    assert!(is_newer("0.3.0", "v0.2.0"));
}

#[test]
fn project_table_plain_uses_phase_glyphs_and_compact_project_names() {
    let receipts = vec![ProjectRefreshReceipt {
        project: PathBuf::from("/home/alice/projects/demo"),
        unregistered: false,
        retired_toml: None,
        migration: ProjectPhase::Ok("v248".to_owned()),
        search_index: ProjectPhase::Warning("busy".to_owned()),
        skills: ProjectPhase::Skipped("not installed".to_owned()),
        membership: ProjectPhase::Ok("2 memberships".to_owned()),
        cloud: ProjectPhase::Failed("timeout".to_owned()),
        details: String::new(),
        documents: Vec::new(),
        phase_details: Vec::new(),
    }];

    let output = render_project_table_plain(&receipts, false);

    assert!(output.contains("projects/demo"), "output was:\n{output}");
    assert!(output.contains("✓"), "output was:\n{output}");
    assert!(output.contains("⚠"), "output was:\n{output}");
    assert!(output.contains("✗"), "output was:\n{output}");
    assert!(output.contains("–"), "output was:\n{output}");
    assert!(!output.contains("/home/alice"), "output was:\n{output}");
    let row = output.lines().nth(1).expect("project table row");
    assert!(
        row.contains("timeout"),
        "most severe phase reason missing:\n{output}"
    );
}

#[test]
fn project_table_phase_headers_are_not_truncated_at_supported_widths() {
    let receipts = vec![ProjectRefreshReceipt {
        project: PathBuf::from(
            "/home/alice/projects/a-project-with-a-deliberately-long-name/another-project",
        ),
        unregistered: false,
        retired_toml: None,
        migration: ProjectPhase::Ok("v248".to_owned()),
        search_index: ProjectPhase::Ok("up to date".to_owned()),
        skills: ProjectPhase::Ok("up to date".to_owned()),
        membership: ProjectPhase::Ok("up to date".to_owned()),
        cloud: ProjectPhase::Ok("up to date".to_owned()),
        details: String::new(),
        documents: Vec::new(),
        phase_details: Vec::new(),
    }];

    for mode in [OutputMode::Plain, OutputMode::Styled] {
        for width in [80, 120] {
            let output = render_project_table_at_width(&receipts, false, mode, width);
            let header = output.lines().next().expect("project table header");
            for truncated in ["migra…", "inde…", "skill…", "membe…", "clou…"] {
                assert!(
                    !header.contains(truncated),
                    "phase header truncated at {width} columns in {mode:?}: {header:?}"
                );
            }
            for label in ["project", "migr", "index", "skills", "member", "cloud"] {
                assert!(
                    header.contains(label),
                    "missing {label:?} header at {width} columns in {mode:?}: {header:?}"
                );
            }
        }
    }
}

#[test]
fn non_ok_phase_details_include_phase_summary_when_capture_is_empty() {
    let receipt = ProjectRefreshReceipt {
        project: PathBuf::from("/home/alice/projects/demo"),
        unregistered: false,
        retired_toml: None,
        migration: ProjectPhase::Ok("v248".to_owned()),
        search_index: ProjectPhase::Ok("up to date".to_owned()),
        skills: ProjectPhase::Ok("up to date".to_owned()),
        membership: ProjectPhase::Ok("up to date".to_owned()),
        cloud: ProjectPhase::Warning("12 queued".to_owned()),
        details: String::new(),
        documents: Vec::new(),
        phase_details: vec![
            (true, String::new()),
            (true, String::new()),
            (true, String::new()),
            (true, String::new()),
            (false, String::new()),
        ],
    };
    let mut warnings = RepeatedWarningCollector::default();

    let detail = render_project_phase_details(&receipt, false, &mut warnings, "projects/demo");

    assert!(
        detail.contains("[WARN] cloud: 12 queued"),
        "cloud phase summary missing from detail output: {detail:?}"
    );
}

#[test]
fn cloud_warning_summary_stays_under_the_non_ok_project_row() {
    let receipt = ProjectRefreshReceipt {
        project: PathBuf::from("/home/alice/projects/demo"),
        unregistered: false,
        retired_toml: None,
        migration: ProjectPhase::Ok("v248".to_owned()),
        search_index: ProjectPhase::Ok("up to date".to_owned()),
        skills: ProjectPhase::Ok("up to date".to_owned()),
        membership: ProjectPhase::Ok("up to date".to_owned()),
        cloud: ProjectPhase::Warning("4 queued".to_owned()),
        details: String::new(),
        documents: Vec::new(),
        phase_details: vec![
            (true, String::new()),
            (true, String::new()),
            (true, String::new()),
            (true, String::new()),
            (false, "[WARN] Push incomplete · 4 pending\n".to_owned()),
        ],
    };
    let mut warnings = RepeatedWarningCollector::default();

    let detail = render_project_phase_details(&receipt, false, &mut warnings, "projects/demo");

    assert!(
        detail.contains("[WARN] Push incomplete · 4 pending"),
        "cloud warning summary missing from detail output: {detail:?}"
    );
}

#[test]
fn repeated_warnings_render_once_with_affected_project_count() {
    let mut warnings = RepeatedWarningCollector::default();
    warnings.record_builtin_paths("projects/one", [".claude/skills/cas-worker/SKILL.md"]);
    warnings.record_builtin_paths("projects/two", [".claude/skills/cas-worker/SKILL.md"]);
    warnings.record("Push incomplete; queued rows remain", "projects/one");
    warnings.record("Push incomplete; queued rows remain", "projects/two");

    let output = warnings.render(false);

    assert_eq!(
        output
            .matches("Cassy-managed builtin paths already tracked")
            .count(),
        1,
        "output was:\n{output}"
    );
    assert!(output.contains("2 projects"), "output was:\n{output}");
    assert_eq!(
        output
            .matches("Push incomplete; queued rows remain")
            .count(),
        1
    );
    assert_eq!(
        output.matches(".claude/skills/cas-worker/SKILL.md").count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn capture_phase_collects_both_stdout_and_stderr() {
    // OutputCapture redirects process-global descriptors. Keep this check
    // isolated from the other unit tests so their harness output cannot be
    // mistaken for phase output while the descriptors are redirected.
    static CAPTURE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = CAPTURE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("capture test lock should not be poisoned");

    let (value, output) = capture_phase(true, || {
        let stdout = b"captured stdout\n";
        let stderr = b"captured stderr\n";
        unsafe {
            let _ = libc::write(libc::STDOUT_FILENO, stdout.as_ptr().cast(), stdout.len());
            let _ = libc::write(libc::STDERR_FILENO, stderr.as_ptr().cast(), stderr.len());
        }
        42
    });

    assert_eq!(value, 42);
    assert!(output.contains("captured stdout"), "output was: {output:?}");
    assert!(output.contains("captured stderr"), "output was: {output:?}");
}

/// cas-be05 review: `--json` never runs a phase it cannot capture, because
/// that phase's documents would reach stdout beside the one receipt.
#[cfg(unix)]
#[test]
fn json_phase_is_not_run_when_its_output_cannot_be_captured_cas_be05() {
    let _output_lock = update_output_lock();
    let mut ran = false;
    let ((phase, output, documents), stdout) = capture_phase(true, || {
        capture_phase_documents_with(
            &update_test_cli(true),
            || Err(std::io::Error::other("descriptor limit")),
            || {
                ran = true;
                println!("{{\"leaked\":true}}");
                ProjectPhase::Ok("ran uncaptured".to_owned())
            },
        )
    });
    assert!(!ran, "an uncapturable --json phase must not run");
    assert!(
        matches!(&phase, ProjectPhase::Failed(detail) if detail.contains("not run") && detail.contains("descriptor limit")),
        "{phase:?}"
    );
    assert!(output.is_empty() && documents.is_empty());
    assert!(stdout.trim().is_empty(), "nothing reaches stdout: {stdout:?}");
}

/// Create a directory that looks exactly like an initialized Cassy project:
/// a `.cas/` directory holding a `cas.db` file.
fn make_project(root: &Path) {
    let cas_root = root.join(".cas");
    std::fs::create_dir_all(&cas_root).expect("create fixture .cas directory");
    std::fs::write(cas_root.join("cas.db"), b"").expect("create fixture store");
    std::fs::write(
        cas_root.join("config.toml"),
        format!("[project]\ncanonical_id = {:?}\n", root.file_name().unwrap().to_string_lossy()),
    )
    .expect("pin fixture identity");
}

/// Reproduce a config whose metadata is readable but whose open stalls.
/// The parent owns the FIFO and kills the unfixed discovery subprocess.
#[cfg(unix)]
#[test]
fn discovery_skips_a_stalled_known_config_and_keeps_healthy_roots_cas_1514() {
    use crate::bounded_process::{Deadline, run_command};
    use crate::store::KnownRepoStore;
    use std::process::Command;
    use std::time::{Duration, Instant};

    const CHILD: &str = "CAS_UPDATE_STALLED_CONFIG_FIXTURE";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = PathBuf::from(root);
        let healthy = root.join("healthy").canonicalize().unwrap();
        let blocked = root.join("blocked").canonicalize().unwrap();
        let copy = healthy.join(".cas/artifacts/cas-1514/copy");
        let started = Instant::now();
        let discovery = discover_local_projects(None);
        eprintln!("cas-1514 first discovery: {:?}", started.elapsed());
        assert!(started.elapsed() < Duration::from_secs(8));
        assert_eq!(discovery.projects, vec![healthy.clone()]);
        assert!(
            discovery
                .skipped_unregistered
                .iter()
                .any(|skip| skip.project == blocked && skip.reason.contains("timed out")),
            "blocked config must be named: {:?}",
            discovery.skipped_unregistered
        );
        assert!(
            discovery
                .skipped_unregistered
                .iter()
                .any(|skip| skip.project == copy && skip.reason.contains("artifacts"))
        );

        // Successful observations cannot cache ownership settings across passes.
        std::fs::write(
            healthy.join(".cas/config.toml"),
            format!(
                "[project]\ncanonical_id = \"healthy\"\n[factory]\nartifacts_root = {:?}\n",
                root.to_string_lossy()
            ),
        )
        .unwrap();
        let started = Instant::now();
        let second = discover_local_projects(None);
        eprintln!("cas-1514 repeat discovery: {:?}", started.elapsed());
        assert!(started.elapsed() < Duration::from_secs(8));
        assert!(
            second.projects.is_empty(),
            "fresh exclusion: {:?}",
            second.projects
        );
        // Complete the native refresh path too: after excluding these roots
        // it must reach the host phase and write its normal JSON receipt.
        let mut args = update_test_args();
        args.all_projects = true;
        args.dry_run = true;
        args.refresh_receipt = Some(root.join("refresh.json"));
        let mut host_phase_ran = false;
        let started = Instant::now();
        refresh_all_projects_with_user_refresh(
            &args,
            &update_test_cli(true),
            None,
            None,
            None,
            None,
            || {
                host_phase_ran = true;
                ProjectPhase::Skipped("fixture host phase".into())
            },
        )
        .unwrap();
        eprintln!("cas-1514 native refresh: {:?}", started.elapsed());
        assert!(host_phase_ran);
        let receipt = read_refresh_receipt(args.refresh_receipt.as_ref().unwrap()).unwrap();
        assert_eq!(receipt["refresh_status"], "complete");
        assert!(receipt["projects"].as_array().unwrap().is_empty());
        assert!(
            receipt["skipped_unregistered"]
                .as_array()
                .unwrap()
                .iter()
                .any(|skip| skip["project"] == blocked.to_string_lossy().as_ref()
                    && skip["reason"].as_str().unwrap().contains("timed out"))
        );
        return;
    }

    let mut env = crate::test_env_guard::TestEnvGuard::temp_home();
    crate::store::known_repos::ensure_host_schema().unwrap();
    // A durable-looking fixture avoids the disposable-root ownership rule.
    let fixture = tempfile::tempdir_in(crate::test_paths::runtime_fixture_parent()).unwrap();
    env.set_current_dir(env.home().to_path_buf());
    let healthy = fixture.path().join("healthy");
    let blocked = fixture.path().join("blocked");
    let copy = healthy.join(".cas/artifacts/cas-1514/copy");
    let store = crate::store::known_repos::open_host_known_repo_store().unwrap();
    for repo in [&healthy, &blocked, &copy] {
        make_project(repo);
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(repo)
                .status()
                .unwrap()
                .success()
        );
        store.upsert(repo).unwrap();
    }
    let fifo = blocked.join(".cas/config.toml");
    std::fs::remove_file(&fifo).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let output = run_command(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "cli::update::tests::discovery_skips_a_stalled_known_config_and_keeps_healthy_roots_cas_1514", "--nocapture"])
            .env(CHILD, fixture.path())
            .env("CAS_PROJECT_ROOTS", fixture.path()),
        Deadline::after(Duration::from_secs(20)), Duration::from_secs(20),
    ).expect("discovery must return despite a stalled known config");
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
fn discovery_finds_sibling_projects_below_a_parent_that_is_itself_a_project() {
    // The regression: `~/.cas` exists on every real host, and the scan used to
    // stop at the first ancestor carrying a `.cas` directory. That made the
    // filesystem walk a no-op — home was recorded, the walk returned, and home
    // was then dropped as "not a project", so nothing was ever discovered.
    let guard = crate::test_env_guard::TestEnvGuard::temp_home();
    let home = guard.home().to_path_buf();
    let workspace = home.join("Workspace");
    make_project(&workspace);
    make_project(&workspace.join("alpha"));
    make_project(&workspace.join("beta"));
    make_project(&workspace.join("nested").join("deep"));

    let mut scanned = BTreeSet::new();
    scan_for_projects(&home, MAX_SCAN_DEPTH, &mut scanned);

    for expected in [
        workspace.clone(),
        workspace.join("alpha"),
        workspace.join("beta"),
        workspace.join("nested").join("deep"),
    ] {
        assert!(
            scanned.contains(&expected),
            "{} missing from discovery: {:?}",
            expected.display(),
            scanned
        );
    }
    assert!(!scanned.contains(&home), "host state is not a project root");
}

#[test]
fn scanner_ignores_container_and_home_archive_without_creating_stores() {
    let guard = crate::test_env_guard::TestEnvGuard::temp_home();
    let home = guard.home().to_path_buf();
    let container = home.join("Projects");
    let first = container.join("one");
    let second = container.join("two");
    make_project(&first);
    make_project(&second);
    let archived = home.join("Archive/old");
    make_project(&archived);
    let mut scanned = BTreeSet::new();
    scan_for_projects(&home, MAX_SCAN_DEPTH, &mut scanned);
    assert!(!scanned.contains(&container));
    assert!(scanned.contains(&first));
    assert!(scanned.contains(&second));
    assert!(!scanned.contains(&archived));
    assert!(!container.join(".cas").exists());
}

#[test]
fn refresh_cloud_phases_leave_unlinked_project_unlinked() {
    let guard = crate::test_env_guard::TestEnvGuard::temp_home();
    let project = guard.home().join("time-tracking");
    make_project(&project);
    let cas_root = project.join(".cas");
    assert!(!cas_root.join("cloud.json").exists());
    assert!(matches!(
        refresh_project_membership(&cas_root, false),
        ProjectPhase::Skipped(_)
    ));
    assert!(matches!(
        sync_project_cloud(&cas_root, false, &update_test_cli(false)).0,
        ProjectPhase::Skipped(_)
    ));
    assert!(!cas_root.join("cloud.json").exists());
}

#[test]
fn discovery_never_returns_the_user_level_store_as_a_project() {
    let guard = crate::test_env_guard::TestEnvGuard::temp_home();
    let home = guard.home().to_path_buf();
    make_project(&home.join("Workspace"));

    let discovery = discover_local_projects(Some(&home.join(".cas")));

    assert_eq!(user_level_store_root(), Some(home.join(".cas")));
    assert!(
        !discovery.projects.contains(&home),
        "the user-level store must never be counted as a project: {:?}",
        discovery.projects
    );
}

#[test]
fn discovery_does_not_descend_into_cas_internal_directories() {
    let guard = crate::test_env_guard::TestEnvGuard::temp_home();
    let home = guard.home().to_path_buf();
    let project = home.join("Workspace").join("demo");
    make_project(&project);
    let worktree = project.join(".cas").join("worktrees").join("lane-1");
    make_project(&worktree);
    let backup = project.join(".cas").join("backup").join("20260904_000000");
    std::fs::create_dir_all(&backup).expect("create fixture backup");
    std::fs::write(backup.join("cas.db"), b"").expect("create fixture backup store");

    let mut scanned = BTreeSet::new();
    scan_for_projects(&home, MAX_SCAN_DEPTH, &mut scanned);

    assert!(scanned.contains(&project));
    assert!(
        !scanned.contains(&worktree),
        "factory worktrees under .cas/ are not separate projects: {:?}",
        scanned
    );
    assert!(
        !scanned.iter().any(|path| path.starts_with(&backup)),
        "migration backups under .cas/ are not projects: {:?}",
        scanned
    );
}

#[test]
fn discovery_separates_registered_projects_from_scan_only_and_storeless_ones() {
    let guard = crate::test_env_guard::TestEnvGuard::temp_home();
    let home = guard.home().to_path_buf();
    let workspace = home.join("Workspace");
    let with_store = workspace.join("with-store");
    make_project(&with_store);
    let storeless = workspace.join("storeless");
    std::fs::create_dir_all(storeless.join(".cas")).expect("create storeless fixture");
    std::fs::write(
        storeless.join(".cas/config.toml"),
        "[project]\ncanonical_id = \"storeless\"\n",
    )
    .expect("pin scan-only project identity");
    crate::store::known_repos::ensure_host_schema().expect("bootstrap known-repos schema");
    crate::store::known_repos::register_repo_strict(&with_store)
        .expect("register temp-root fixture");

    let discovery = discover_local_projects(None);

    assert!(
        !discovery.projects.contains(&with_store),
        "a temp-root project must not be refreshed by host discovery: {:?}",
        discovery.projects
    );
    assert!(
        discovery
            .skipped_unregistered
            .iter()
            .any(|skip| skip.project == with_store && skip.reason.contains("temp")),
        "a registered temp-root project must be reported with its exclusion reason: {:?}",
        discovery.skipped_unregistered
    );
    assert!(
        !discovery.unregistered.contains(&with_store),
        "a registered temp-root project must not be relabeled as scan-only: {:?}",
        discovery.unregistered
    );
    assert!(
        discovery
            .skipped_unregistered
            .iter()
            .any(|skip| skip.project == storeless),
        "a scan-only directory with no cas.db must be listed, not silently dropped: {:?}",
        discovery.skipped_unregistered
    );
    assert!(
        !discovery.projects.contains(&storeless),
        "a directory with no store has nothing to migrate: {:?}",
        discovery.projects
    );
}

#[test]
fn schema_status_json_names_the_store_it_migrated() {
    let line = schema_status_json(
        Some(Path::new("/home/alice/projects/demo/.cas")),
        r#""schema_status":"updated","current_version":248"#,
    );

    let parsed: serde_json::Value = serde_json::from_str(&line).expect("schema status is JSON");
    assert_eq!(parsed["store"], "/home/alice/projects/demo/.cas");
    assert_eq!(parsed["schema_status"], "updated");
    assert_eq!(parsed["current_version"], 248);

    let unset = schema_status_json(None, r#""schema_status":"not_initialized""#);
    let parsed: serde_json::Value = serde_json::from_str(&unset).expect("schema status is JSON");
    assert!(parsed["store"].is_null(), "line was: {unset}");
}

#[test]
fn update_banner_reports_projects_failures_and_skipped_unregistered_stores() {
    let banner = update_banner_text(&RefreshReport {
        project_count: 29,
        failed_count: 1,
        failed_projects: 1,
        skipped_unregistered: 2,
        elapsed: std::time::Duration::from_secs(3),
        ..Default::default()
    });

    assert!(banner.contains("29 projects refreshed"), "banner: {banner}");
    assert!(banner.contains("1 failed"), "banner: {banner}");
    assert!(banner.contains("2 skipped"), "banner: {banner}");

    let clean = update_banner_text(&RefreshReport {
        project_count: 29,
        failed_count: 0,
        skipped_unregistered: 0,
        elapsed: std::time::Duration::from_secs(3),
        ..Default::default()
    });
    assert!(
        !clean.contains("skipped"),
        "a clean run stays quiet: {clean}"
    );
}

#[test]
fn refresh_receipt_names_each_skipped_project_and_why_it_was_not_refreshed() {
    let receipt = project_refresh_receipt_json(
        &[],
        &ProjectPhase::Ok("up to date".to_string()),
        &[SkippedProject {
            project: PathBuf::from("/tmp/container-copy"),
            reason: "its .cas has no [project] canonical_id pin and no git origin remote".to_string(),
        }],
        None,
        None,
    );

    assert_eq!(receipt["skipped_unregistered"][0]["project"], "/tmp/container-copy");
    assert!(receipt["skipped_unregistered"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("no git origin remote"));
}

#[test]
fn register_flag_parses_and_targets_a_project_path() {
    let parsed = crate::cli::try_parse_from_with_wordmark([
        "cas",
        "update",
        "--register",
        "/home/alice/projects/demo",
    ])
    .expect("--register must parse");

    let Some(crate::cli::Commands::Update(args)) = parsed.command else {
        panic!("expected update command");
    };
    assert_eq!(
        args.register.as_deref(),
        Some(Path::new("/home/alice/projects/demo"))
    );
}

#[test]
fn post_swap_command_targets_install_destination_path() {
    let install_destination = Path::new("/usr/local/bin/cas");
    let command = build_post_swap_command(install_destination, "3.7.7", true);

    assert_eq!(command.get_program(), "/usr/local/bin/cas");
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args, ["update", "--post-swap", "--from", "3.7.7", "--json"]);
}

#[test]
fn strip_deleted_suffix_from_linux_process_path() {
    assert_eq!(
        strip_deleted_suffix(PathBuf::from("/usr/local/bin/cas (deleted)")),
        PathBuf::from("/usr/local/bin/cas")
    );
    assert_eq!(
        strip_deleted_suffix(PathBuf::from("/usr/local/bin/cas")),
        PathBuf::from("/usr/local/bin/cas")
    );
}


#[test]
fn post_swap_flags_parse_as_a_hidden_terminal_mode() {
    let parsed = crate::cli::try_parse_from_with_wordmark([
        "cas",
        "update",
        "--post-swap",
        "--from",
        "3.7.7",
    ])
    .expect("post-swap flags must parse");

    let Some(crate::cli::Commands::Update(args)) = parsed.command else {
        panic!("expected update command");
    };
    assert!(args.post_swap);
    assert_eq!(args.from.as_deref(), Some("3.7.7"));
}

#[test]
fn post_swap_mode_is_a_terminal_update_path() {
    let _guard = crate::test_support::TestEnvGuard::temp_home();
    let args = UpdateArgs {
        check: false,
        version: None,
        yes: false,
        schema_only: false,
        sync: false,
        user: false,
        dry_run: false,
        keep_backup: false,
        all_projects: false,
        register: None,
        post_swap: true,
        from: Some("3.7.7".to_owned()),
        refresh_receipt: None,
        render_fixture: None,
    };
    let cli = Cli {
        json: true,
        full: false,
        verbose: false,
        command: None,
    };

    execute(&args, &cli, None).expect("post-swap mode must not re-enter binary update");
}

// =============================================================================
// cas-91ba: the post-install phases must run on the NEWLY installed binary.
// Installing 3.15.2 from 3.15.1 refreshed with the 3.15.1 image: 16 projects
// instead of 43, no user_level_store, and gabber-studio's ledger wedge still
// reported — a second `cas update` was needed to converge.
// =============================================================================

#[cfg(unix)]
fn write_stub_binary(path: &Path, body: &str) {
    crate::test_paths::warm_stub(path, body);
}

#[cfg(unix)]
#[test]
fn post_swap_refresh_runs_on_the_installed_binary_and_reports_its_version() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let temp_dir = tempfile::tempdir().expect("create post-swap test directory");
    let installed_binary = temp_dir.path().join("cas-new");
    let root = installed_binary.with_extension("root");
    std::fs::create_dir(&root).unwrap();
    env.set("CAS_ROOT", &root);
    env.set_current_dir(temp_dir.path());
    std::fs::write(root.join("config.toml"), "[factory]\nmerge_sweep = true\n").unwrap();
    std::fs::write(installed_binary.with_extension("receipt"), serde_json::json!({
        "refresh_binary_version": "9.9.9-stub", "projects": [{
            "store": root, "retired_toml": {
                "path": root.join("config.toml"), "status": "repaired", "removed": ["factory.merge_sweep"]
            }
        }], "user_level_store": {"status":"ok"}, "skipped_unregistered": []
    }).to_string()).unwrap();
    // The stub stands in for the freshly installed binary: it records its argv
    // and replaces its fixture config before returning a maintenance receipt.
    write_stub_binary(
        &installed_binary,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\n\
         printf '[factory]\\n' > \"$0.root/config.toml\"\ncat \"$0.receipt\"\n",
    );

    let (receipt, output) = capture_phase(true, || {
        let cli = crate::cli::try_parse_from_with_wordmark(["cas", "update", "--yes"]).unwrap();
        let _parent_notices = crate::cli::startup_config_notices(&cli);
        crate::config::Config::load(&root).unwrap();
        run_post_swap_refresh(&installed_binary, "3.15.1", "9.9.9-stub", true)
            .expect("the post-swap refresh must run the installed binary")
    });
    assert!(
        !output.contains("retired config entries remain ignored"),
        "parent replayed an entry repaired by its verified child: {output}"
    );

    assert_eq!(
        receipt["refresh_binary_version"], "9.9.9-stub",
        "the receipt must name the binary that actually refreshed: {receipt}"
    );
    let args = std::fs::read_to_string(installed_binary.with_extension("args"))
        .expect("stub binary should have recorded its arguments");
    assert!(args.contains("--post-swap"), "{args}");
    assert!(args.contains("3.15.1"), "{args}");
    assert!(args.contains("--json"), "{args}");

    // cas-be05: a child from before the one-document receipt prints each
    // phase's document first; the parent still takes the refresh receipt.
    let old_child = temp_dir.path().join("cas-old");
    write_stub_binary(
        &old_child,
        "#!/bin/sh\n\
         printf '%s\\n' '{\"store\":\"/x/.cas\",\"schema_status\":\"current\"}'\n\
         printf '%s\\n' '{\"refresh_binary_version\":\"9.9.8-old\",\"projects\":[],\"skipped_unregistered\":[]}'\n",
    );
    let receipt = run_post_swap_refresh(&old_child, "3.15.1", "9.9.8-old", true)
        .expect("a multi-document child still yields its refresh receipt");
    assert_eq!(receipt["refresh_binary_version"], "9.9.8-old", "{receipt}");
}

#[cfg(unix)]
#[test]
fn post_swap_refresh_failure_tells_the_operator_to_run_update_again() {
    let temp_dir = tempfile::tempdir().expect("create post-swap test directory");
    let missing = temp_dir.path().join("cas-not-installed");

    let error = run_post_swap_refresh(&missing, "3.15.1", "3.15.2", true)
        .expect_err("an unusable installed binary must not read as a successful refresh");
    let message = format!("{error:#}");
    assert!(
        message.contains("cas update"),
        "the operator must be told to run cas update again: {message}"
    );
}

#[cfg(unix)]
#[test]
fn post_swap_refresh_spawn_failure_has_a_distinct_receipt_status() {
    let temp_dir = tempfile::tempdir().expect("create post-swap test directory");
    let missing = temp_dir.path().join("cas-not-installed");

    let error = run_post_swap_refresh(&missing, "3.15.1", "3.15.2", true)
        .expect_err("an unusable installed binary must remain an error");
    let failure = error
        .downcast_ref::<PostSwapRefreshFailure>()
        .expect("spawn failure should carry a typed failure receipt");
    let receipt = failure
        .receipt
        .as_ref()
        .expect("spawn failure should carry a receipt");

    assert_eq!(receipt["refresh_status"], "spawn_failed");
    assert!(receipt["refresh_binary_version"].is_null(), "{receipt}");
    assert!(
        failure.to_string().contains("could not start")
            && failure.to_string().contains("cas update"),
        "spawn guidance must describe the unavailable child: {failure}"
    );
}

#[cfg(unix)]
#[test]
fn post_swap_refresh_failure_preserves_the_child_receipt_and_failed_project() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let temp_dir = tempfile::tempdir().expect("create post-swap test directory");
    let root = temp_dir.path().join(".cas");
    std::fs::create_dir(&root).unwrap();
    let original = "[factory]\nmerge_sweep = true\n";
    std::fs::write(root.join("config.toml"), original).unwrap();
    env.set("CAS_ROOT", &root);
    env.set_current_dir(temp_dir.path());
    let installed_binary = temp_dir.path().join("cas-failed-refresh");
    std::fs::write(installed_binary.with_extension("receipt"), serde_json::to_vec(&serde_json::json!({
        "refresh_binary_version": "9.9.9-stub",
        "projects": [{"project": "/tmp/failed-project", "cloud_sync": "FAILED: cloud sync: registration missing"}],
        "user_level_store": {"status": "FAILED: config is read-only; run cas update", "retired_toml": [{
            "path": root.join("config.toml"), "status": "failed", "error": "config is read-only"
        }]},
        "skipped_unregistered": []
    })).unwrap()).unwrap();
    write_stub_binary(
        &installed_binary,
        "#!/bin/sh\ncat \"$0.receipt\"\nprintf '%s' 'cloud refresh failed' >&2\nexit 1\n",
    );

    let (result, output) = capture_phase(true, || {
        let cli = crate::cli::try_parse_from_with_wordmark(["cas", "update"]).unwrap();
        let _scope = crate::cli::startup_config_notices(&cli);
        crate::config::Config::load(&root).unwrap();
        run_post_swap_refresh(&installed_binary, "3.15.1", "9.9.9-stub", true)
    });
    assert!(
        !output.contains("remain ignored"),
        "failed child receipt was reported twice: {output}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("config.toml")).unwrap(),
        original
    );
    let error = result.expect_err("a failed child refresh must remain an error");
    let failure = error
        .downcast_ref::<PostSwapRefreshFailure>()
        .expect("a child refresh failure should preserve structured failure data");
    let receipt = failure
        .receipt
        .as_ref()
        .expect("the child receipt should be retained");

    assert_eq!(receipt["binary_updated"], true);
    assert_eq!(receipt["version"], "9.9.9-stub");
    assert_eq!(receipt["refresh_binary_version"], "9.9.9-stub");
    assert_eq!(receipt["refresh_status"], "refresh_failed");
    assert_eq!(receipt["projects"][0]["project"], "/tmp/failed-project");
    assert_eq!(
        receipt["projects"][0]["cloud_sync"],
        "FAILED: cloud sync: registration missing"
    );
    assert_eq!(receipt["refresh_stderr"], "cloud refresh failed");
    assert!(
        failure.to_string().contains("refresh ran")
            && failure.to_string().contains("cloud refresh failed"),
        "failure guidance must describe an executed failed refresh: {failure}"
    );
}

#[cfg(unix)]
#[test]
fn plain_post_swap_refresh_failure_preserves_the_child_receipt_and_failed_project() {
    let temp_dir = tempfile::tempdir().expect("create post-swap test directory");
    let installed_binary = temp_dir.path().join("cas-failed-plain-refresh");
    write_stub_binary(
        &installed_binary,
        "#!/bin/sh
receipt=''
while [ \"$#\" -gt 0 ]; do
  if [ \"$1\" = \"--refresh-receipt\" ]; then receipt=\"$2\"; shift 2; else shift; fi
done
printf '%s' '{\"refresh_binary_version\":\"9.9.9-stub\",\"projects\":[{\"project\":\"/tmp/failed-project\",\"cloud_sync\":\"FAILED: cloud sync: project registration missing\"}],\"user_level_store\":{\"status\":\"ok\"}}' > \"$receipt\"
exit 1
",
    );

    let error = run_post_swap_refresh(&installed_binary, "3.15.1", "9.9.9-stub", false)
        .expect_err("a failed plain child refresh must preserve its receipt");
    let failure = error
        .downcast_ref::<PostSwapRefreshFailure>()
        .expect("a child refresh failure should preserve structured failure data");
    let receipt = failure
        .receipt
        .as_ref()
        .expect("the child receipt should be retained");

    assert_eq!(receipt["binary_updated"], true);
    assert_eq!(receipt["refresh_binary_version"], "9.9.9-stub");
    assert_eq!(receipt["refresh_status"], "refresh_failed");
    assert_eq!(receipt["projects"][0]["project"], "/tmp/failed-project");
    assert!(
        failure.to_string().contains("/tmp/failed-project")
            && failure.to_string().contains("cas update --all-projects"),
        "plain guidance must name the failed project and remedy: {failure}"
    );
}

#[cfg(unix)]
#[test]
fn post_swap_refresh_failure_without_a_receipt_is_not_reported_as_skipped() {
    let temp_dir = tempfile::tempdir().expect("create post-swap test directory");
    let installed_binary = temp_dir.path().join("cas-no-receipt");
    write_stub_binary(
        &installed_binary,
        "#!/bin/sh\nprintf '%s' 'refresh child failed before producing a receipt' >&2\nexit 1\n",
    );

    let error = run_post_swap_refresh(&installed_binary, "3.15.1", "9.9.9-stub", true)
        .expect_err("a child failure without a receipt must remain an error");
    let failure = error
        .downcast_ref::<PostSwapRefreshFailure>()
        .expect("an executed child failure should carry a typed failure receipt");
    let receipt = failure
        .receipt
        .as_ref()
        .expect("an executed child failure should carry a receipt");

    assert_eq!(receipt["refresh_status"], "refresh_failed_no_receipt");
    assert!(
        failure.to_string().contains("post-swap child exited")
            && failure.to_string().contains("refresh child failed"),
        "guidance must distinguish an unreported child from a skipped refresh: {failure}"
    );
    assert_eq!(receipt["message"], failure.to_string());
}

#[test]
fn refresh_receipt_names_the_binary_that_ran_it() {
    let receipt = project_refresh_receipt_json(
        &[],
        &ProjectPhase::Ok(String::new()),
        &[],
        None,
        None,
    );

    assert_eq!(
        receipt["refresh_binary_version"],
        env!("CARGO_PKG_VERSION"),
        "every refresh receipt must name the binary version that produced it: {receipt}"
    );
}

#[test]
fn refresh_receipt_records_partial_failure_before_refresh_returns_error() {
    let temp_dir = tempfile::tempdir().expect("create refresh receipt directory");
    let path = temp_dir.path().join("refresh.json");
    let receipts = vec![ProjectRefreshReceipt {
        project: PathBuf::from("/tmp/failed-project"),
        unregistered: false,
        retired_toml: None,
        migration: ProjectPhase::Ok("migration".to_owned()),
        search_index: ProjectPhase::Ok("no stray root".to_owned()),
        skills: ProjectPhase::Ok("skills".to_owned()),
        membership: ProjectPhase::Ok("membership".to_owned()),
        cloud: ProjectPhase::Failed("cloud sync: project registration missing".to_owned()),
        details: String::new(),
        documents: Vec::new(),
        phase_details: Vec::new(),
    }];
    let receipt = project_refresh_receipt_json(
        &receipts,
        &ProjectPhase::Ok("up to date".to_owned()),
        &[],
        None,
        None,
    );

    assert_eq!(receipt["refresh_status"], "refresh_failed");
    write_refresh_receipt(&path, &receipt).expect("partial refresh receipt should be writable");
    let written = read_refresh_receipt(&path).expect("partial refresh receipt should be readable");
    assert_eq!(written["refresh_binary_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(written["projects"][0]["project"], "/tmp/failed-project");
    assert!(
        written["projects"][0]["cloud_sync"]
            .as_str()
            .expect("cloud phase summary")
            .starts_with("FAILED:")
    );
}

#[test]
fn combined_receipt_merges_the_installed_binary_refresh_into_one_document() {
    let refresh = serde_json::json!({
        "refresh_binary_version": "3.15.2",
        "projects": [],
        "user_level_store": {"status": "ok"},
    });

    let combined = combined_update_receipt("3.15.2", true, Some(&refresh), None, None);
    assert_eq!(combined["binary_updated"], true);
    assert_eq!(combined["version"], "3.15.2");
    assert_eq!(
        combined["refresh_binary_version"], "3.15.2",
        "the single receipt must state which image ran the refresh: {combined}"
    );
    assert!(combined["user_level_store"].is_object(), "{combined}");

    // No swap: no refresh receipt to merge, and no stale version claimed.
    let solo = combined_update_receipt("3.15.2", false, None, None, None);
    assert_eq!(solo["binary_updated"], false);
    assert!(solo.get("refresh_binary_version").is_none(), "{solo}");
}

#[test]
fn post_swap_first_launch_time_is_kept_in_refresh_receipt() {
    let refresh = serde_json::json!({
        "refresh_binary_version": "3.28.2",
        "refresh_status": "complete",
    });
    let child_receipt = with_first_launch_ms(refresh, Some(4187));
    let parent_receipt = combined_update_receipt("3.28.2", true, Some(&child_receipt), None, None);
    assert_eq!(parent_receipt["binary_first_launch_ms"], 4187);
    assert_eq!(parent_receipt["refresh_binary_version"], "3.28.2");
    assert!(
        with_first_launch_ms(serde_json::json!({}), None)
            .get("binary_first_launch_ms")
            .is_none()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn installed_binary_quarantine_is_removed_before_hub_relaunch() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let home = tempfile::tempdir().expect("isolated update fixture");
    let binary = home.path().join("cas");
    std::fs::write(&binary, "fixture").unwrap();
    let path = CString::new(binary.as_os_str().as_bytes()).unwrap();
    let attribute = c"com.apple.quarantine";
    let value = b"0081;00000000;Cassy;";
    // SAFETY: The NUL-terminated path/name and value buffer remain valid.
    let result = unsafe {
        libc::setxattr(
            path.as_ptr(),
            attribute.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    assert_eq!(result, 0, "{}", std::io::Error::last_os_error());

    remove_quarantine_if_present(&binary).unwrap();
    remove_quarantine_if_present(&binary).unwrap();
    // SAFETY: The path/name pointers are valid and a null output requests size.
    let remaining = unsafe {
        libc::getxattr(
            path.as_ptr(),
            attribute.as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    assert_eq!(remaining, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ENOATTR)
    );
}

#[cfg(target_os = "macos")]
#[test]
fn post_swap_age_includes_process_creation_before_cli_entry() {
    assert!(post_swap_first_launch_ms().unwrap() > 0);
}

#[test]
fn update_receipt_proves_the_hub_version_transition_and_manager() {
    let restart = HubRestartOutcome {
        previous_version: Some("3.26.0".to_owned()),
        current_version: Some("3.27.0".to_owned()),
        service_managed: true,
        prior_state: "unresponsive".to_owned(),
        action: "restarted".to_owned(),
        verified: true,
        loopback_verified: true,
        transport_verified: Some(true),
        public_url: Some("https://hub.tail.ts.net/".to_owned()),
        ..Default::default()
    };
    let receipt = combined_update_receipt(
        "3.27.0",
        true,
        None,
        None,
        Some(&restart),
    );

    assert_eq!(receipt["hub_restart"]["from_version"], "3.26.0");
    assert_eq!(receipt["hub_restart"]["to_version"], "3.27.0");
    assert_eq!(receipt["hub_restart"]["via"], "service");
    assert_eq!(receipt["hub_restart"]["prior_state"], "unresponsive");
    assert_eq!(receipt["hub_restart"]["action"], "restarted");
    assert_eq!(receipt["hub_restart"]["verified"], true);
    assert_eq!(receipt["hub_restart"]["transport_verified"], true);
    assert_eq!(receipt["hub_restart"]["public_url"], "https://hub.tail.ts.net/");
}

#[test]
fn failed_serve_publication_still_refreshes_projects_and_reports_update_error() {
    let transport_error = "cas update: Tailscale Serve publication failed: tailscale CLI is unavailable; hub remains loopback-only; run `cas hub restart --tailscale-serve`";
    let mut refreshed = false;
    let (refresh, reported_error) = refresh_after_hub_restart(
        HubRestartOutcome {
            transport_error: Some(transport_error.to_owned()),
            ..Default::default()
        },
        |error| {
            assert_eq!(error, Some(transport_error));
            refreshed = true;
            Ok::<_, anyhow::Error>(serde_json::json!({"projects": ["refreshed"]}))
        },
    )
    .expect("a transport warning must not prevent refresh");

    assert!(refreshed, "project refresh must continue after Serve failure");
    let receipt = combined_update_receipt(
        "3.15.2",
        false,
        Some(&refresh),
        reported_error.as_deref(),
        None,
    );
    assert_eq!(receipt["hub_transport"]["status"], "error");
    assert_eq!(receipt["hub_transport"]["message"], transport_error);
    let refresh_receipt = project_refresh_receipt_json(
        &[],
        &ProjectPhase::Ok("up to date".to_owned()),
        &[],
        reported_error.as_deref(),
        None,
    );
    assert_eq!(refresh_receipt["hub_transport"]["status"], "error");
    assert_eq!(refresh_receipt["hub_transport"]["message"], transport_error);

    let report = RefreshReport {
        project_count: 1,
        failed_count: 0,
        skipped_unregistered: 0,
        elapsed: std::time::Duration::from_millis(5),
        ..Default::default()
    };
    let sweep = crate::cli::update::findings::collect(
        &[],
        &ProjectPhase::Ok("up to date".to_owned()),
        &[],
        None,
        Some(transport_error),
    );
    let report = RefreshReport {
        sweep: Some(sweep),
        ..report
    };
    let banner = render_plain_at(160, |fmt| print_sweep(fmt, &report, false));
    assert!(banner.contains("[ERROR] hub"), "{banner}");
    assert!(banner.contains("cas hub restart --tailscale-serve"), "{banner}");
}

#[cfg(unix)]
#[test]
fn post_swap_refresh_rejects_a_child_reporting_a_different_version() {
    let temp_dir = tempfile::tempdir().expect("create post-swap test directory");
    let installed_binary = temp_dir.path().join("cas-stale");
    // A stale `cas` answering instead of the binary we just installed.
    write_stub_binary(
        &installed_binary,
        "#!/bin/sh\nprintf '%s' '{\"refresh_binary_version\":\"3.15.1\",\"projects\":[]}'\n",
    );

    let error = run_post_swap_refresh(&installed_binary, "3.15.0", "3.15.2", true)
        .expect_err("a refresh performed by the wrong version must not read as converged");
    let message = format!("{error:#}");
    assert!(message.contains("3.15.1") && message.contains("3.15.2"), "{message}");
    assert!(message.contains("cas update"), "{message}");
}

#[test]
fn version_verification_accepts_only_the_installed_version() {
    let binary = Path::new("/usr/local/bin/cas");
    assert!(verify_refresh_binary_version(Some("3.15.2"), "3.15.2", binary).is_ok());
    assert!(verify_refresh_binary_version(Some(" 3.15.2 "), "3.15.2", binary).is_ok());
    assert!(verify_refresh_binary_version(Some("3.15.1"), "3.15.2", binary).is_err());
    assert!(
        verify_refresh_binary_version(None, "3.15.2", binary).is_err(),
        "a child that reports no version proves nothing"
    );
}

#[test]
fn reported_version_is_the_semver_not_the_build_date() {
    // The real shapes `cas --version` prints.
    assert_eq!(
        parse_reported_version("cas 3.15.5 (a94b6ac 2026-09-04)\n").as_deref(),
        Some("3.15.5")
    );
    assert_eq!(
        parse_reported_version("cas 2.27.0 (9b52e17-dirty 2026-07-16)\n").as_deref(),
        Some("2.27.0")
    );
    assert_eq!(parse_reported_version("cas 3.15.5\n").as_deref(), Some("3.15.5"));
    assert_eq!(parse_reported_version("v3.15.5").as_deref(), Some("3.15.5"));
    assert_eq!(
        parse_reported_version("cas 9.99.0-rc.1 (a94b6ac 2026-09-04)").as_deref(),
        Some("9.99.0-rc.1")
    );

    // Nothing that is a version means we cannot vouch for the child.
    assert_eq!(parse_reported_version("cas (a94b6ac 2026-09-04)"), None);
    assert_eq!(parse_reported_version(""), None);
    assert_eq!(parse_reported_version("2026-09-04"), None);
}

#[cfg(unix)]
#[test]
fn post_swap_refresh_accepts_the_real_version_line_and_still_rejects_a_stale_binary() {
    let temp_dir = tempfile::tempdir().expect("create post-swap version test directory");

    // The interactive path asks the installed binary for its version.
    let installed_binary = temp_dir.path().join("cas-current");
    write_stub_binary(
        &installed_binary,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then \
         printf 'cas 3.15.5 (a94b6ac 2026-09-04)\\n'; fi\nexit 0\n",
    );
    run_post_swap_refresh(&installed_binary, "3.15.4", "3.15.5", false)
        .expect("a correct refresh under the installed binary must read as converged");

    let stale_binary = temp_dir.path().join("cas-stale");
    write_stub_binary(
        &stale_binary,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then \
         printf 'cas 3.15.4 (0000000 2026-09-03)\\n'; fi\nexit 0\n",
    );
    let error = run_post_swap_refresh(&stale_binary, "3.15.4", "3.15.5", false)
        .expect_err("a refresh performed by the wrong version must not read as converged");
    let message = format!("{error:#}");
    assert!(message.contains("3.15.4") && message.contains("3.15.5"), "{message}");
}

#[test]
fn skipped_refresh_receipt_says_the_update_did_not_converge() {
    let receipt = skipped_refresh_receipt(
        "3.15.2",
        "binary updated to 3.15.2; refresh did not run — run `cas update` again",
    );

    assert_eq!(receipt["binary_updated"], true);
    assert_eq!(receipt["version"], "3.15.2");
    assert!(
        receipt["refresh_binary_version"].is_null(),
        "nothing refreshed, so no version may be claimed: {receipt}"
    );
    assert_eq!(receipt["refresh_status"], "skipped");
    assert!(
        receipt["message"].as_str().unwrap().contains("cas update"),
        "{receipt}"
    );
}

fn update_test_args() -> UpdateArgs {
    UpdateArgs {
        check: false,
        version: Some("9.9.9".to_owned()),
        yes: false,
        schema_only: false,
        sync: false,
        user: false,
        dry_run: false,
        keep_backup: false,
        all_projects: false,
        register: None,
        post_swap: false,
        from: None,
        refresh_receipt: None,
        render_fixture: None,
    }
}

fn update_test_cli(json: bool) -> Cli {
    Cli {
        json,
        full: false,
        verbose: false,
        command: None,
    }
}

#[test]
fn non_tty_update_without_json_or_yes_fails_closed() {
    let args = update_test_args();
    let cli = update_test_cli(false);

    let error = configure_updater(&args, "3.17.1", &cli, false)
        .expect_err("a non-TTY update without --yes must fail before network access");
    assert!(
        error.to_string().contains("--yes") && error.to_string().contains("TTY"),
        "error should name the missing consent flag: {error:#}"
    );
}

#[test]
fn tty_update_without_yes_keeps_confirmation_and_human_output_enabled() {
    let args = update_test_args();
    let cli = update_test_cli(false);

    assert_eq!(
        update_interactivity(&args, &cli, true),
        // cas-70a8: CAS asks itself (confirm_install), so self_update never
        // prints its release chatter or its own prompt around the phase line.
        UpdateInteractivity {
            no_confirm: false,
            show_output: false,
            show_download_progress: false,
        }
    );
    // cas-70a8 QA: a failed install names the cause without self_update's
    // type prefix, and says the current binary is untouched.
    let error = install_error(
        &self_update::errors::Error::Network("Download request failed with status: 500".into()),
        "3.17.1",
        "9.9.9",
    );
    assert_eq!(
        error.to_string(),
        "could not install Cassy 9.9.9\n  Download request failed with status: 500\n  Cassy 3.17.1 is unchanged; rerun `cas update` to retry"
    );
}

#[test]
fn json_update_from_fake_release_never_prompts_or_leaks_self_update_output() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    env.set("GITHUB_TOKEN", "fixture-token-not-a-secret");
    let cli = update_test_cli(true);

    #[cfg(unix)]
    let _output_lock = update_output_lock();
    let mut failures = Vec::new();
    for download_fails in [true, false] {
        let mut args = update_test_args();
        if !download_fails {
            // cas-a99a: exercise the reported full `cas --json update --yes`,
            // including latest-release selection, rather than a pinned version.
            args.version = None;
            args.yes = true;
        }
        let (base_url, server) = fake_release_source(download_fails);
        env.set("CAS_UPDATE_GITHUB_API_URL", &base_url);
        let (result, output) = capture_phase(true, || run_full_update(
            &cli,
            false,
            || perform_update(&args, "3.17.1", &cli),
            |_| panic!("failed install must not invoke the installed binary"),
            |_| panic!("failed install must not restart the hub"),
            |_, _| panic!("failed install must not refresh projects"),
        ));
        server.join().expect("loopback release/download fixture");
        let error = result.expect_err("download/extraction refusal must remain nonzero");
        let cause = format!("{error:#}");
        assert!(cause.contains("could not install Cassy 9.9.9"), "{cause}");
        assert!(
            cause.contains(if download_fails { "500" } else { "archive" }),
            "the failure must reach the intended download/extraction boundary: {cause}"
        );
        let document = serde_json::from_str::<serde_json::Value>(&output);
        match document {
            Ok(receipt) => {
                assert_eq!(receipt["binary_updated"], false, "{receipt}");
                assert_eq!(receipt["version"], "3.17.1", "{receipt}");
                assert_eq!(receipt["binary_status"], "failed", "{receipt}");
                assert_eq!(receipt["refresh_status"], "not_run", "{receipt}");
                assert_eq!(receipt["error"]["phase"], "binary", "{receipt}");
                assert_eq!(receipt["error"]["message"], cause);
                assert!(receipt.get("projects").is_none() && receipt.get("hub_restart").is_none(), "{receipt}");
            }
            Err(_) => failures.push(format!("download_fails={download_fails}: expected one JSON object, got {output:?}; error={error:#}")),
        }
        assert!(!output.contains("Do you want to continue?") && !output.contains("Checking target-arch") && !output.contains("Looking for tag"), "{output}");
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

fn fake_release_source(download_fails: bool) -> (String, thread::JoinHandle<()>) {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("bind fake release source");
    let base_url = format!("http://{}", server.server_addr());
    let release = format!(
        r#"{{
            "tag_name": "v9.9.9",
            "name": "9.9.9",
            "created_at": "2026-09-05T00:00:00Z",
            "assets": [{{
                "name": "cas-{target}.tar.gz",
                "url": "{base_url}/asset",
                "browser_download_url": "{base_url}/asset"
            }}]
        }}"#,
        base_url = base_url,
        target = self_update::get_target(),
    );
    let handle = thread::spawn(move || {
        loop {
            let request = server.recv_timeout(Duration::from_secs(10)).unwrap()
                .expect("updater must reach only the loopback release/download fixture");
            let asset = request.url() == "/asset";
            let response = if asset {
                // Neither fixture can yield an executable or replace the test binary.
                tiny_http::Response::from_string("not a release archive")
                    .with_status_code(if download_fails { 500 } else { 200 })
            } else {
                // Unpinned --yes resolves a release list; pinned updates and
                // the initial latest-release lookup request a single object.
                let body = if request.url().ends_with("/releases") {
                    format!("[{release}]")
                } else {
                    release.clone()
                };
                tiny_http::Response::from_string(body)
            };
            request.respond(response).expect("respond with fake release");
            if asset { break; }
        }
    });
    (base_url, handle)
}

#[cfg(unix)]
fn update_output_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn render_plain_at(width: u16, render: impl FnOnce(&mut Formatter<'_>) -> std::io::Result<()>) -> String {
    let mut bytes = Vec::new();
    {
        let mut fmt = Formatter::new(
            &mut bytes,
            OutputMode::Plain,
            crate::ui::theme::ActiveTheme::default_dark(),
            width,
        );
        render(&mut fmt).expect("render");
    }
    String::from_utf8(bytes).expect("utf-8")
}

/// cas-4df0: `cas update --check` leads with the verdict, then two rows, then
/// at most one remedy; the plain render fits 80 columns.
#[test]
fn check_report_leads_with_the_verdict_and_names_one_remedy() {
    let clean = render_plain_at(80, |fmt| {
        render_check_report(
            fmt,
            &CheckView {
                current_version: "3.17.2",
                latest_version: "3.17.2",
                binary_update_available: false,
                schema: Some((254, 254, 0)),
            },
        )
    });
    assert!(
        clean.starts_with("[OK] up to date · Cassy 3.17.2 · schema v254\n"),
        "{clean}"
    );
    assert!(clean.contains("Binary    3.17.2 installed · 3.17.2 latest"), "{clean}");
    assert!(clean.contains("Schema    v254 applied · v254 latest"), "{clean}");
    assert!(!clean.contains("→"), "nothing to do, no remedy: {clean}");

    let pending = render_plain_at(80, |fmt| {
        render_check_report(
            fmt,
            &CheckView {
                current_version: "3.17.2",
                latest_version: "9.99.7",
                binary_update_available: true,
                schema: Some((254, 256, 2)),
            },
        )
    });
    assert!(
        pending.starts_with(
            "[WARN] update available · 3.17.2 → 9.99.7 · 2 migrations pending\n"
        ),
        "{pending}"
    );
    assert!(pending.contains("  → cas update\n"), "{pending}");
    assert!(pending.contains("cas update --dry-run previews the 2 pending"), "{pending}");

    let uninitialized = render_plain_at(80, |fmt| {
        render_check_report(
            fmt,
            &CheckView {
                current_version: "3.17.2",
                latest_version: "3.17.2",
                binary_update_available: false,
                schema: None,
            },
        )
    });
    assert!(uninitialized.contains("no Cassy store in this directory"), "{uninitialized}");
    for line in clean.lines().chain(pending.lines()).chain(uninitialized.lines()) {
        assert!(line.chars().count() <= 80, "overflow: {line:?}");
    }
}

/// cas-4df0 / cas-0aa5: the sweep's phase line names converged and failed
/// projects, and its receipt keeps the count grammar scripts grep for.
#[test]
fn refresh_phase_result_counts_projects_and_receipt_keeps_count_grammar() {
    let report = RefreshReport {
        project_count: 2,
        elapsed: std::time::Duration::from_millis(1200),
        ..Default::default()
    };
    let clean = refresh_phase_result(&Ok(report));
    assert_eq!((clean.verdict, clean.text.as_str()), (Verdict::Ok, "all 2 refreshed"));

    // cas-0aa5 QA F02: a failed user-level store is host state, never one
    // of the projects in "N of M".
    let failed = RefreshReport {
        project_count: 3,
        failed_count: 2,
        failed_projects: 1,
        user_level_failed: true,
        warned_count: 1,
        skipped_unregistered: 2,
        elapsed: std::time::Duration::from_secs(4),
        ..Default::default()
    };
    // cas-06b0: the human receipt counts projects too, and names the store.
    let receipt = update_banner_text(&failed);
    assert!(
        receipt.contains("3 projects refreshed · 1 failed · user-level store failed · 2 skipped"),
        "{receipt}"
    );
    let user_only = update_banner_text(&RefreshReport {
        project_count: 12,
        failed_count: 1,
        user_level_failed: true,
        ..Default::default()
    });
    assert!(user_only.contains("12 projects refreshed · 0 failed · user-level store failed"), "{user_only}");
    // cas-06b0: a mixed failure's exit reason breaks between clauses, so no
    // line passes 80 columns beside the error mark.
    let mixed = RefreshFailed(RefreshReport {
        project_count: 12,
        failed_count: 3,
        failed_projects: 2,
        user_level_failed: true,
        ..Default::default()
    });
    assert_eq!(
        binary_exit_reason(&mixed, "9.99.1", false),
        "2 of 12 projects not fully refreshed\n  the user-level store was not refreshed\n  Cassy 9.99.1 is current"
    );
    // cas-70a8: at 80 columns the elapsed time never sits alone on a line,
    // and the count grammar stays whole on the first.
    let wide = RefreshReport {
        project_count: 12,
        failed_count: 3,
        failed_projects: 2,
        user_level_failed: true,
        skipped_unregistered: 1,
        elapsed: std::time::Duration::from_millis(4000),
        ..Default::default()
    };
    assert_eq!(
        receipt_lines(&wide, 78),
        vec![
            format!("Cassy {} · 12 projects refreshed · 2 failed · 4.0s", env!("CARGO_PKG_VERSION")),
            "user-level store failed · 1 skipped".to_owned(),
        ]
    );
    let error: anyhow::Error = RefreshFailed(failed).into();
    let failed = refresh_phase_result(&Err(error));
    assert_eq!(failed.verdict, Verdict::Error);
    assert_eq!(
        failed.text,
        "1 of 3 failed · user-level store failed · 1 warning · 2 skipped"
    );
}

/// cas-0aa5: the operator's 3.62 `cas update -y` transcript, replayed from
/// its reconstructed fixture through the real phases and renderer. The
/// binary verdict and the refresh verdict are the first two results; each
/// failure is one finding with one remedy; raw phase output stays out.
#[cfg(unix)]
#[test]
fn partial_refresh_transcript_leads_with_binary_then_refresh_verdict_cas_0aa5() {
    let _output_lock = update_output_lock();
    let mut fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../docs/design/cli/fixtures/cas-update-partial-refresh.json"
    )))
    .unwrap();
    fixture["refresh_ms"] = 0.into();
    fixture["binary"]["elapsed_ms"] = 0.into();
    fixture["binary"]["current"] = "9.99.1".into();
    fixture["binary"]["latest"] = "9.99.1".into();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.json");
    std::fs::write(&path, fixture.to_string()).unwrap();
    let mut args = update_test_args();
    args.version = None;
    args.yes = true;

    let (result, output) = capture_phase(true, || {
        crate::cli::update::fixture::render(&path, &args, &update_test_cli(false))
    });

    let error = result.expect_err("a partial refresh must still exit nonzero");
    assert_eq!(
        error.to_string(),
        "2 of 12 projects not fully refreshed; Cassy 9.99.1 is current"
    );
    let results = output
        .lines()
        .filter(|line| line.starts_with('['))
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>();
    assert!(
        results[0].starts_with("[OK] [1/2] Cassy binary 9.99.1 is current"),
        "{output}"
    );
    assert!(
        results[1].starts_with("[ERROR] [2/2] Projects 2 of 12 failed · 4 warnings · 1 skipped"),
        "{output}"
    );
    assert_eq!(output.matches("recheck writers, remove only this lock").count(), 2, "{output}");
    // cas-0aa5 QA F01: the reason-specific next step (cas-7c6b) is the
    // remedy, never the blanket retry it overrides.
    assert!(
        output.contains("→ Follow https://github.com/Richards-LLC/petra-stella-cloud/issues/184")
            && output.contains("`cas cloud queue --retry --retry-reason invalid_json_nul`"),
        "{output}"
    );
    assert!(!output.contains("→ cas cloud queue --retry\n"), "{output}");
    for raw in ["Checking target-arch", "Managed files: 3 conflicts", "Retained intentionally"] {
        assert!(!output.contains(raw), "raw phase output {raw:?} leaked:\n{output}");
    }
    let lines = output.lines().collect::<Vec<_>>();
    let warning = lines
        .iter()
        .position(|line| line.contains("serve  Tailscale Serve inactive:"))
        .expect("hub Serve finding is printed");
    assert!(lines[warning].trim_end().ends_with("Tailscale Serve inactive:"), "{output}");
    assert_eq!(
        lines[warning + 1].trim(),
        "tailscale is stopped; hub remains loopback-only",
        "the transport cause belongs on its own indented line:\n{output}"
    );
    assert!(output.lines().all(|line| line.chars().count() <= 80), "{output}");
}

#[test]
fn cloud_findings_keep_c_locale_cause_and_remedy_cas_2c9c() {
    for (summary, cause, remedy) in [
        (
            "[WARN] Push incomplete - 1 rejected by cloud (invalid_json_nul x1) - invalid_json_nul: Follow https://github.com/Richards-LLC/petra-stella-cloud/issues/184, then `cas cloud queue --retry --retry-reason invalid_json_nul` - queued rows remain - run cas cloud queue --retry\n",
            "push incomplete: 1 rejected by cloud (invalid_json_nul x1)",
            "Follow https://github.com/Richards-LLC/petra-stella-cloud/issues/184, then `cas cloud queue --retry --retry-reason invalid_json_nul`",
        ),
        (
            "[WARN] Pull incomplete - 1 errors - run cas cloud pull\n",
            "pull incomplete: 1 errors",
            "cas cloud pull",
        ),
    ] {
        assert_eq!(
            findings::cause_and_remedy("cloud sync: counts only", summary),
            (cause.to_owned(), Some(remedy.to_owned())),
            "lost cloud finding from {summary:?}"
        );
    }
}

#[test]
fn store_lock_failure_names_holders_and_the_remedy_cas_91a3() {
    let cas_root = Path::new("/projects/soundwave-config/.cas");
    let phase = ProjectPhase::Failed("migration: database error: database is locked".to_string());
    assert!(is_store_lock_error(phase.detail()));
    let holders = vec![
        crate::hub::HubLockHolder {
            pid: 4242,
            age: None,
            phase: None,
            command: Some("cas serve".to_string()),
        },
        crate::hub::HubLockHolder {
            pid: 4343,
            age: None,
            phase: None,
            command: None,
        },
    ];
    assert_eq!(
        store_lock_remedy(cas_root, &holders),
        "store held open by pid 4242 (cas serve), pid 4343; stop the Cassy sessions in /projects/soundwave-config and rerun `cas update`"
    );
    assert!(store_lock_remedy(cas_root, &[]).starts_with("another process holds its write lock"));
    // Any other failure keeps its own words.
    let other = name_store_lock_holders(
        ProjectPhase::Failed("migration: disk full".to_string()),
        cas_root,
    );
    assert_eq!(other.detail(), "migration: disk full");
}

#[test]
fn store_schema_gate_names_a_failed_migration_cas_91a3() {
    let temp = tempfile::TempDir::new().unwrap();
    let failed = ProjectPhase::Failed("migration: database is locked".to_string());
    assert_eq!(
        store_schema_gate(temp.path(), &failed).as_deref(),
        Some("schema not current: the migration phase failed (see migr)")
    );
    // No store at all: nothing to read stale, nothing to gate.
    assert_eq!(
        store_schema_gate(temp.path(), &ProjectPhase::Ok("migration".into())),
        None
    );
}

/// cas-49c0: a factory worker's `cas update` refreshes only its own worktree.
#[test]
fn worker_update_is_scoped_to_its_own_worktree_cas_49c0() {
    use super::{UpdateMode, WorkerUpdatePlan, worker_update_plan};
    let worktree = std::path::Path::new("/repo/.cas/worktrees/quick-bear-33");
    assert_eq!(
        worker_update_plan(Some("worker"), Some(worktree), UpdateMode::Sync),
        WorkerUpdatePlan::SyncWorktree(worktree.to_path_buf()),
        "a worker's --sync writes its own worktree, not the main checkout"
    );
    for mode in [UpdateMode::AllProjects, UpdateMode::Register, UpdateMode::User, UpdateMode::Full, UpdateMode::SchemaOnly] {
        match worker_update_plan(Some("worker"), Some(worktree), mode) {
            WorkerUpdatePlan::Refuse(message) => assert!(
                message.contains("cas update --sync") && message.contains(&worktree.display().to_string()),
                "{mode:?}: {message}"
            ),
            other => panic!("{mode:?} must be refused for a worker: {other:?}"),
        }
    }
    // Read-only and post-swap runs stay available.
    for mode in [UpdateMode::Check, UpdateMode::PostSwap] {
        assert_eq!(worker_update_plan(Some("worker"), Some(worktree), mode), WorkerUpdatePlan::Unrestricted);
    }
    // A worker with no bound worktree cannot be scoped: refuse --sync too.
    assert!(matches!(
        worker_update_plan(Some("worker"), None, UpdateMode::Sync),
        WorkerUpdatePlan::Refuse(_)
    ));
    // The supervisor and an operator shell are unaffected.
    for role in [None, Some("supervisor"), Some("")] {
        for mode in [UpdateMode::Sync, UpdateMode::AllProjects, UpdateMode::Full] {
            assert_eq!(worker_update_plan(role, Some(worktree), mode), WorkerUpdatePlan::Unrestricted);
        }
    }
}

/// cas-8030: in the compact `cas update` view, a project whose only non-✓
/// phases are skipped ("not cloud-linked") prints its table row and no
/// details block; a project with a warned phase keeps its details. Every line
/// fits 80 columns. `verbose` still prints every project's details.
#[test]
fn skipped_phases_print_no_details_block_snapshot_cas_8030() {
    let unlinked = ProjectRefreshReceipt {
        project: PathBuf::from("/home/alice/projects/notes"),
        unregistered: false,
        retired_toml: None,
        migration: ProjectPhase::Ok("v248".to_owned()),
        search_index: ProjectPhase::Ok("up to date".to_owned()),
        skills: ProjectPhase::Ok("up to date".to_owned()),
        membership: ProjectPhase::Skipped("not cloud-linked".to_owned()),
        cloud: ProjectPhase::Skipped("not cloud-linked".to_owned()),
        details: String::new(),
        documents: Vec::new(),
        phase_details: vec![
            (true, "[OK] Migrations current".to_owned()),
            (false, "[SKIP] membership: not cloud-linked".to_owned()),
            (false, "[SKIP] cloud: not cloud-linked".to_owned()),
        ],
    };
    let warned = ProjectRefreshReceipt {
        project: PathBuf::from("/home/alice/projects/demo"),
        unregistered: false,
        retired_toml: None,
        migration: ProjectPhase::Ok("v248".to_owned()),
        search_index: ProjectPhase::Warning("index busy".to_owned()),
        skills: ProjectPhase::Ok("up to date".to_owned()),
        membership: ProjectPhase::Ok("2 memberships".to_owned()),
        cloud: ProjectPhase::Ok("synced".to_owned()),
        details: String::new(),
        documents: Vec::new(),
        phase_details: vec![
            (true, "[OK] Migrations current".to_owned()),
            (false, "[WARN] search index: index busy, retry later".to_owned()),
        ],
    };
    let receipts = vec![unlinked, warned];
    let user_level = ProjectPhase::Ok("current".to_owned());

    let compact = render_project_refresh_summary(&receipts, &user_level, "", &[], false);
    assert_eq!(
        compact,
        "  project         migr  index  skills  member  cloud  note            \n  projects/notes  ✓     ✓      ✓       –       –      not cloud-linked\n  projects/demo   ✓     ⚠      ✓       ✓       ✓      index busy      \n  projects/demo details:\n    [WARN] search index: index busy, retry later\n  [OK] user-level store: current\n",
        "compact output was:\n{compact}"
    );
    assert!(
        compact.lines().all(|line| line.chars().count() <= 80),
        "every line fits 80 columns:\n{compact}"
    );

    let verbose = render_project_refresh_summary(&receipts, &user_level, "", &[], true);
    assert!(
        verbose.contains("notes details:") && verbose.contains("demo details:"),
        "verbose prints every project's details:\n{verbose}"
    );
}

/// cas-937a: cas-8030 kept skipped phases table-only, and briefly took a dry
/// run's planned phases with them. The compact view then never said "DRY RUN"
/// (integration `update_all_projects_test.rs:88` failed on the epic). A planned
/// phase keeps its details block; a skipped one still gets none.
#[test]
fn dry_run_planned_phases_keep_their_details_block_cas_937a() {
    let planned = ProjectRefreshReceipt {
        project: PathBuf::from("/home/alice/projects/first"),
        unregistered: false,
        retired_toml: None,
        migration: ProjectPhase::Planned("would apply 2 migrations".to_owned()),
        search_index: ProjectPhase::Skipped("dry run".to_owned()),
        skills: ProjectPhase::Skipped("dry run".to_owned()),
        membership: ProjectPhase::Skipped("dry run".to_owned()),
        cloud: ProjectPhase::Skipped("dry run".to_owned()),
        details: String::new(),
        documents: Vec::new(),
        phase_details: vec![(
            false,
            "[DRY RUN] migration: would apply 2 migrations\n".to_owned(),
        )],
    };
    let compact = render_project_refresh_summary(
        &[planned],
        &ProjectPhase::Planned("migrate and sync builtins".to_owned()),
        "",
        &[],
        false,
    );
    assert!(
        compact.contains("  projects/first details:\n    [DRY RUN] migration: would apply 2 migrations"),
        "a dry run says so in the compact view:\n{compact}"
    );
}

/// cas-cef2 (GH #1242): the update says what happens to running factories.
#[test]
fn update_names_what_happens_to_running_factories_cas_cef2() {
    assert_eq!(running_factory_update_notice(&[], "9.99.1"), None);
    let notice = running_factory_update_notice(
        &[("pinned-session".into(), true), ("older-session".into(), false)],
        "9.99.1",
    )
    .expect("running sessions get a notice");
    assert!(notice.contains("2 running factory session(s) keep their current build"), "{notice}");
    assert!(notice.contains("pinned-session keep spawning workers"), "{notice}");
    assert!(notice.contains("older-session started before build pinning and cannot spawn"), "{notice}");
    assert!(notice.contains("9.99.1") && notice.contains("cas factory kill <session>"), "{notice}");
}

// One real Git repo carries the seven downstream drift shapes from cas-6718.
fn cas_6718_git(project: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn cas_6718_drifted_project(home: &Path) -> PathBuf {
    let project = home.join("drifted");
    crate::store::init_cas_dir(&project).unwrap();
    for (path, body) in [
        (".gitignore", ".cas/\nuser-output/\n"),
        (
            "CLAUDE.md",
            "Project instructions stay.\n\n<!-- CAS:BEGIN - This section is managed by CAS. Do not edit manually. -->\nold Cassy instructions\n<!-- CAS:END -->\n",
        ),
        (".claude/settings.json", "{\"theme\":\"project\"}\n"),
        (
            ".codex/config.toml",
            "# Project Codex preferences\nmodel = 'fixture-model'\n",
        ),
        (
            ".claude/skills/cas/SKILL.md",
            "---\nname: cas\nmanaged_by: cas\n---\nLegacy Cassy skill\n",
        ),
        (
            ".claude/agents/code-reviewer.md",
            "---\nmanaged_by: cas\n---\nRetired Cassy agent\n",
        ),
        (
            ".claude/workflows/cas-code-review.js",
            "// managed_by: cas\n// retired\n",
        ),
        (
            ".codex/hooks.json",
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"echo project-codex-hook"},{"type":"command","command":"cas hook PreToolUse"}]}]}}"#,
        ),
        ("README.md", "Project content\n"),
        (
            ".claude/skills/cas-personal/SKILL.md",
            "---\nname: personal-project-skill\n---\nProject-owned skill\n",
        ),
    ] {
        let path = project.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    let builtin = crate::builtins::BUILTIN_SKILLS
        .iter()
        .find(|f| f.path == "skills/cas-tdd/SKILL.md")
        .unwrap();
    let path = project.join(".claude").join(builtin.path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, builtin.content).unwrap();
    // A prior legitimate sync may have refreshed a still-tracked projection.
    let store = crate::store::open_rule_store(&project.join(".cas")).unwrap();
    let mut rule = crate::types::Rule::new("rule-fixture".into(), "Previous source content".into());
    rule.status = crate::types::RuleStatus::Proven;
    rule.helpful_count = 1;
    store.add(&rule).unwrap();
    let syncer = crate::sync::Syncer::with_defaults(&project);
    syncer.sync_rule(&rule).unwrap();
    cas_6718_git(&project, &["init", "-q"]);
    cas_6718_git(
        &project,
        &["config", "user.email", "fixture@example.invalid"],
    );
    cas_6718_git(&project, &["config", "user.name", "Fixture"]);
    cas_6718_git(&project, &["add", "."]);
    cas_6718_git(&project, &["commit", "-qm", "project baseline"]);
    cas_6718_git(&project, &["branch", "-M", "main"]);
    cas_6718_git(&project, &["checkout", "-qb", "repair/managed-files"]);
    rule.content = "Current authoritative source content".into();
    store.update(&rule).unwrap();
    syncer.sync_rule(&rule).unwrap();
    project
}

#[cfg(unix)]
#[test]
fn skills_refresh_reports_git_index_lock_without_removing_it_cas_7c9b() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, SystemTime};

    let child_case = std::env::var("CAS7C9B_LOCK_CASE").ok();
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    env.remove("CAS_AGENT_ROLE");
    env.remove("CAS_SKIP_FACTORY_TOOLING");
    let probes = env.home().join("probes");
    std::fs::create_dir(&probes).unwrap();
    // Control only the external process-inspection boundary. Git and the
    // complete skills refresh remain real, including the actual lock error.
    for (case, seconds, ps, lsof, expected) in [
        ("recent", 10, "echo sh", "exit 1", "busy"),
        ("abandoned", 172800, "echo sh", "exit 1", "stale (no writer observed)"),
        ("held", 172800, "echo sh", "echo p42", "busy"),
        ("git_running", 172800, "echo git", "exit 1", "busy"),
        ("uncertain", 172800, "echo sh", "echo denied >&2; exit 1", "unknown ownership"),
    ] {
        if child_case.as_deref() != Some(case) {
            if child_case.is_some() {
                continue;
            }
            for (name, body) in [("ps", ps), ("lsof", lsof)] {
                let path = probes.join(name);
                std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let test = "cli::update::tests::skills_refresh_reports_git_index_lock_without_removing_it_cas_7c9b";
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", test])
                .env("CAS7C9B_LOCK_CASE", case)
                .env("PATH", format!("{}:{}", probes.display(), std::env::var("PATH").unwrap()))
                .output()
                .unwrap();
            assert!(output.status.success(), "{case}: {}", String::from_utf8_lossy(&output.stderr));
            crate::test_child::assert_passed(&String::from_utf8_lossy(&output.stdout), test);
            continue;
        }
        let project = cas_6718_drifted_project(&env.home().join(case));
        let root = project.join(".cas");
        env.set("CAS_ROOT", &root);
        let index = std::fs::read(project.join(".git/index")).unwrap();
        let lock_path = project.join(".git/index.lock");
        let lock = std::fs::File::create(&lock_path).unwrap();
        lock.set_times(std::fs::FileTimes::new().set_modified(
            SystemTime::now() - Duration::from_secs(seconds),
        )).unwrap();
        drop(lock);
        let phase = refresh_project_files(&update_test_cli(true), &root, false);
        let ProjectPhase::Failed(finding) = phase else {
            panic!("{case}: locked skills refresh must fail: {phase:?}");
        };
        assert!(finding.contains(&format!("git index lock {expected}")), "{case}: {finding}");
        assert!(finding.contains(".git/index.lock") && finding.contains("age "), "{finding}");
        assert!(finding.contains("retry") && !finding.contains("Another git process"), "{finding}");
        assert_eq!(finding.contains("remove"), case == "abandoned", "{finding}");
        assert!(lock_path.exists(), "update must never remove the lock");
        assert_eq!(std::fs::read(project.join(".git/index")).unwrap(), index);
    }
}

#[test]
fn managed_commit_names_local_only_pr_next_step_cas_3b812() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    env.remove("CAS_AGENT_ROLE");
    env.remove("CAS_SKIP_FACTORY_TOOLING");
    let project = cas_6718_drifted_project(env.home());
    let root = project.join(".cas");
    env.set("CAS_ROOT", &root);
    let before = cas_6718_git(&project, &["rev-parse", "HEAD"]);
    let (result, output) = capture_phase(true, || {
        sync_claude_files_into(&update_test_cli(false), Some(&root), Some(&project))
    });
    let report = result.unwrap();
    assert_eq!(report.outcome, "committed");
    assert_ne!(cas_6718_git(&project, &["rev-parse", "HEAD"]), before);
    let next: Vec<_> = output.lines().filter(|line| line.contains("local-only")).collect();
    assert_eq!(next.len(), 1, "committed repair needs one local-only next step: {output}");
    assert!(next[0].contains("origin") && next[0].contains("PR"), "{}", next[0]);
    assert!(next[0].is_ascii() && next[0].len() <= 80, "{}", next[0]);
    let (result, current) = capture_phase(true, || {
        sync_claude_files_into(&update_test_cli(false), Some(&root), Some(&project))
    });
    assert_eq!(result.unwrap().outcome, "current");
    assert!(!current.contains("local-only"), "{current}");
    // The multi-project sweep must not hide a pending PR in successful details.
    let sweep_project = cas_6718_drifted_project(&env.home().join("sweep"));
    let sweep_root = sweep_project.join(".cas");
    env.set("CAS_ROOT", &sweep_root);
    let (phase, _) = capture_phase(true, || refresh_project_files(&update_test_cli(false), &sweep_root, false));
    assert!(matches!(phase, ProjectPhase::Warning(_)), "{phase:?}");
    assert!(phase.detail().contains("local-only") && phase.detail().contains("PR"), "{phase:?}");
}

#[test]
fn managed_reconcile_never_commits_to_integration_branches_cas_32e3() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    env.remove("CAS_AGENT_ROLE");
    env.remove("CAS_SKIP_FACTORY_TOOLING");
    let home = env.home().to_path_buf();
    let mut failures = Vec::new();
    for (branch, can_commit) in [
        ("main", false),
        ("staging", false),
        ("production", false),
        ("release/base", false),
        ("epic/shared", false),
        ("repair/managed", true),
        ("detached", false),
        ("repair/config-unreadable", false),
        ("release/shared-state", false),
        ("repair/unknown-default", false),
    ] {
        let project = cas_6718_drifted_project(&home.join(branch));
        let root = if branch == "release/shared-state" {
            // A worktree may use the main checkout's store/config without a
            // project-local .cas. Exercise that actual sync call-site pattern.
            // Remove generated rule WIP that needs the local store's authority;
            // this row isolates commit policy after the store moves away.
            cas_6718_git(&project, &["checkout", "--", ".claude/rules/cas/rule-fixture.md"]);
            let shared = home.join("shared-cas-state");
            std::fs::rename(project.join(".cas"), &shared).unwrap();
            shared
        } else {
            project.join(".cas")
        };
        env.set("CAS_ROOT", &root);
        cas_6718_git(&project, &["update-ref", "refs/remotes/origin/production", "HEAD"]);
        cas_6718_git(&project, &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/production"]);
        if branch == "detached" {
            cas_6718_git(&project, &["checkout", "--detach", "-q", "HEAD"]);
        } else {
            cas_6718_git(&project, &["checkout", "-qB", branch]);
        }
        if branch == "main" {
            // A same-named tag must not change the checked-out branch identity.
            cas_6718_git(&project, &["tag", "main", "HEAD"]);
        }
        let path = root.join("config.toml");
        let mut config: toml_edit::DocumentMut = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        let configured_base = if branch == "release/shared-state" { branch } else { "release/base" };
        config["factory"]["epic_base_branch"] = toml_edit::value(configured_base);
        std::fs::write(&path, config.to_string()).unwrap();
        if branch == "repair/config-unreadable" {
            // A directory fails config reading on every host, including root.
            std::fs::remove_file(&path).unwrap();
            std::fs::create_dir(&path).unwrap();
        }
        if branch == "repair/unknown-default" {
            cas_6718_git(&project, &["branch", "-D", "main"]);
            cas_6718_git(&project, &["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"]);
            cas_6718_git(&project, &["update-ref", "-d", "refs/remotes/origin/production"]);
        }
        let before = cas_6718_git(&project, &["rev-parse", "HEAD"]);
        let (result, output) = capture_phase(true, || sync_claude_files_into(
            &update_test_cli(branch != "repair/config-unreadable"), Some(&root), Some(&project),
        ));
        let report = match result {
            Ok(report) => report,
            Err(error) => {
                failures.push(format!("{branch}: update aborted instead of keeping repair staged: {error}"));
                continue;
            }
        };
        let after = cas_6718_git(&project, &["rev-parse", "HEAD"]);
        if (after != before) != can_commit || report.commit.is_some() != can_commit {
            failures.push(format!("{branch}: expected can_commit={can_commit}, outcome={}, commit={:?}", report.outcome, report.commit));
        }
        assert!(report.commit_error.is_none(), "{branch}: {:?}", report.commit_error);
        if !can_commit && report.commit.is_none() {
            assert_eq!(report.outcome, "staged", "{branch}");
            assert!(report.staged > 0);
            assert!(!cas_6718_git(&project, &["diff", "--cached", "--name-only"]).is_empty());
        }
        if branch == "repair/config-unreadable" {
            let json = serde_json::to_value(&report).unwrap();
            let warning = json["commit_warning"].as_str().expect("guard error needs a structured warning");
            assert!(warning.contains("config") && warning.contains("staged"), "{warning}");
            let lines: Vec<_> = output.lines().filter(|line| line.contains(warning)).collect();
            assert_eq!(lines.len(), 1, "{output}");
            assert!(lines[0].is_ascii() && lines[0].len() <= 78, "{}", lines[0]);
            assert!(path.is_dir(), "update must not replace the unreadable config");
        }
        if branch != "repair/unknown-default" {
            assert_eq!(cas_6718_git(&project, &["rev-parse", "refs/remotes/origin/production"]), before);
        }
        assert!(check_managed_commit(report).is_ok(), "staged guard refusals must exit successfully");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn cas_6718_update_repairs_managed_git_drift_and_is_idempotent() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    env.remove("CAS_AGENT_ROLE");
    env.remove("CAS_SKIP_FACTORY_TOOLING");
    let project = cas_6718_drifted_project(env.home());
    let root = project.join(".cas");
    env.set("CAS_ROOT", &root);
    let harnesses = crate::cli::project_files::harnesses(&project);
    assert!(crate::cli::project_files::drift(&project, &harnesses).unwrap() > 0);
    let mut args = update_test_args();
    args.sync = true;
    let original = cas_6718_git(&project, &["status", "--porcelain"]);
    args.dry_run = true;
    execute(&args, &update_test_cli(true), Some(&root)).unwrap();
    assert_eq!(cas_6718_git(&project, &["status", "--porcelain"]), original);
    args.dry_run = false;
    execute(&args, &update_test_cli(true), Some(&root)).unwrap();
    let status = cas_6718_git(&project, &["status", "--porcelain"]);
    assert!(
        status.is_empty(),
        "update left managed drift: {status}\n{}",
        cas_6718_git(&project, &["diff"])
    );
    assert!(cas_6718_git(&project, &["log", "-1", "--format=%s"]).starts_with("chore(cassy):"));
    for path in [
        ".claude/skills/cas/SKILL.md",
        ".claude/skills/cas-tdd/SKILL.md",
        ".claude/rules/cas/rule-fixture.md",
        "AGENTS.md",
    ] {
        assert!(project.join(path).is_file());
        assert!(cas_6718_git(&project, &["ls-files", "--", path]).is_empty());
        cas_6718_git(&project, &["check-ignore", path]);
    }
    for path in [
        ".claude/agents/code-reviewer.md",
        ".claude/workflows/cas-code-review.js",
    ] {
        assert!(!project.join(path).exists());
        assert!(cas_6718_git(&project, &["ls-files", "--", path]).is_empty());
    }
    // pin: user bytes outside the CAS block must survive exactly, including blank lines.
    assert!(
        std::fs::read_to_string(project.join("CLAUDE.md"))
            .unwrap()
            .starts_with("Project instructions stay.\n\n")
    );
    assert_eq!(
        crate::cli::project_files::drift(&project, &harnesses).unwrap(),
        0
    );
    // pin: project TOML comments outside the managed MCP table must survive.
    assert!(
        std::fs::read_to_string(project.join(".codex/config.toml"))
            .unwrap()
            .starts_with("# Project Codex preferences\n")
    );
    let hooks: serde_json::Value =
        serde_json::from_slice(&std::fs::read(project.join(".codex/hooks.json")).unwrap()).unwrap();
    assert!(
        hooks["hooks"]["PreToolUse"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["hooks"][0]["command"] == "echo project-codex-hook")
    );
    let first = cas_6718_git(&project, &["rev-parse", "HEAD"]);
    // A repeated refresh covers the same writers without rewriting identical
    // content (cas-df15). Seed an old timestamp instead of sleeping.
    let unchanged = [".claude/settings.json", ".claude/rules/cas/rule-fixture.md"];
    let stamp = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000);
    for path in unchanged {
        std::fs::OpenOptions::new().write(true).open(project.join(path)).unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(stamp)).unwrap();
    }
    execute(&args, &update_test_cli(true), Some(&root)).unwrap();
    assert_eq!(cas_6718_git(&project, &["rev-parse", "HEAD"]), first);
    assert!(cas_6718_git(&project, &["status", "--porcelain"]).is_empty());
    for path in unchanged {
        assert_eq!(std::fs::metadata(project.join(path)).unwrap().modified().unwrap(), stamp,
            "unchanged refresh rewrote {path}");
    }

    // Permission membership is unchanged when project tooling reorders the allow list.
    let settings = project.join(".claude/settings.json");
    let original_settings = std::fs::read(&settings).unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(&original_settings).unwrap();
    value["permissions"]["allow"]
        .as_array_mut()
        .unwrap()
        .rotate_left(1);
    std::fs::write(settings, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    assert_eq!(
        crate::cli::project_files::drift(&project, &harnesses).unwrap(),
        0,
        "doctor must compare permission membership, not array order"
    );

    // The no-write fast path must still repair changed authoritative inputs.
    std::fs::write(project.join(".claude/settings.json"), original_settings).unwrap();
    let config_path = root.join("config.toml");
    let mut config: toml_edit::DocumentMut = std::fs::read_to_string(&config_path).unwrap().parse().unwrap();
    config["hooks"]["pre_tool_use"]["timeout"] = toml_edit::value(17);
    std::fs::write(config_path, config.to_string()).unwrap();
    let store = crate::store::open_rule_store(&root).unwrap();
    let mut rule = store.get("rule-fixture").unwrap();
    rule.content = "Changed authoritative input must still be repaired".into();
    store.update(&rule).unwrap();
    execute(&args, &update_test_cli(true), Some(&root)).unwrap();
    assert!(std::fs::read_to_string(project.join(unchanged[1])).unwrap().contains(&rule.content));
    let settings: serde_json::Value = serde_json::from_slice(
        &std::fs::read(project.join(unchanged[0])).unwrap()).unwrap();
    assert!(settings["hooks"]["PreToolUse"].as_array().unwrap().iter()
        .any(|group| group["hooks"][0]["timeout"] == 17));
}

#[test]
fn cas_6718_update_preserves_project_edits_hooks_and_existing_staging() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    // Model the supervisor actor in this isolated handler fixture.
    env.set("CAS_AGENT_ROLE", "supervisor");
    env.remove("CAS_SKIP_FACTORY_TOOLING");
    let project = cas_6718_drifted_project(env.home());
    let root = project.join(".cas");
    env.set("CAS_ROOT", &root);
    let head = cas_6718_git(&project, &["rev-parse", "HEAD"]);
    std::fs::write(project.join("README.md"), "Staged project work\n").unwrap();
    cas_6718_git(&project, &["add", "README.md"]);
    let staged_readme = cas_6718_git(&project, &["show", ":README.md"]);
    let outside = "Unstaged project instruction.\n\n";
    let claude = project.join("CLAUDE.md");
    std::fs::write(
        &claude,
        format!("{outside}{}", std::fs::read_to_string(&claude).unwrap()),
    )
    .unwrap();
    let command = "echo project-owned-hook";
    let settings = serde_json::json!({"theme":"project", "hooks":{"PreToolUse":[{"hooks":[{"type":"command", "command":command}]}]}});
    std::fs::write(project.join(".claude/settings.json"), settings.to_string()).unwrap();
    let mut args = update_test_args();
    args.sync = true;
    let (result, output) =
        capture_phase(true, || execute(&args, &update_test_cli(true), Some(&root)));
    result.unwrap();
    let receipt: serde_json::Value = output.lines().last().unwrap().parse().unwrap();
    assert_eq!(receipt["managed_files"]["outcome"], "staged");
    assert_eq!(cas_6718_git(&project, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        cas_6718_git(&project, &["show", ":README.md"]),
        staged_readme
    );
    assert!(
        cas_6718_git(&project, &["diff", "--cached", "--name-only"])
            .lines()
            .any(|p| p == "README.md")
    );
    assert!(
        std::fs::read_to_string(&claude)
            .unwrap()
            .starts_with(outside)
    );
    assert!(!cas_6718_git(&project, &["show", ":CLAUDE.md"]).contains(outside));
    let actual: serde_json::Value =
        serde_json::from_slice(&std::fs::read(project.join(".claude/settings.json")).unwrap())
            .unwrap();
    assert!(
        actual["hooks"]["PreToolUse"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["hooks"][0]["command"] == command)
    );
    assert!(!cas_6718_git(&project, &["show", ":.claude/settings.json"]).contains(command));
    assert!(
        project
            .join(".claude/skills/cas-personal/SKILL.md")
            .is_file()
    );
    // Ignore exceptions and staged edits to owned text are both preflight conflicts.
    for (path, suffix) in [
        (".gitignore", "\n!/AGENTS.md\n"),
        ("CLAUDE.md", "manual managed edit"),
    ] {
        let file = project.join(path);
        let original = std::fs::read_to_string(&file).unwrap();
        let conflicted = if path == "CLAUDE.md" {
            original.replace("@AGENTS.md", suffix)
        } else {
            format!("{original}{suffix}")
        };
        std::fs::write(&file, &conflicted).unwrap();
        if path == "CLAUDE.md" {
            cas_6718_git(&project, &["add", path]);
        }
        let index = cas_6718_git(&project, &["ls-files", "--stage"]);
        let (result, output) =
            capture_phase(true, || execute(&args, &update_test_cli(true), Some(&root)));
        result.unwrap();
        let receipt: serde_json::Value = output.lines().last().unwrap().parse().unwrap();
        assert!(
            receipt["managed_files"]["conflicts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p == path),
            "{receipt}"
        );
        if path == ".gitignore" {
            let (result, human) = capture_phase(true, || {
                execute(&args, &update_test_cli(false), Some(&root))
            });
            result.unwrap();
            assert!(
                human.starts_with("Managed files: 1 conflicts preserved;"),
                "{human}"
            );
            assert!(!human.contains("[OK] Syncing"), "{human}");
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), conflicted);
        assert_eq!(cas_6718_git(&project, &["ls-files", "--stage"]), index);
        std::fs::write(file, original).unwrap();
    }

    // A refused automatic commit differs from the intentional staging above.
    // Keep Git/hooks real and preserve their refusal instead of disabling them.
    // Native discovery deliberately rejects /tmp and artifact roots.
    let host = tempfile::tempdir_in(crate::test_paths::runtime_fixture_parent()).unwrap();
    env.set("HOME", host.path());
    env.set("XDG_CONFIG_HOME", host.path().join(".config"));
    env.set("CAS_CREDENTIALS_FILE", host.path().join("credentials.env"));
    let mut diagnostic_failures = Vec::new();
    #[cfg(unix)]
    for mode in ["all-projects", "sync", "worker-sync", "commit-msg-sync", "human-sync"] {
        use std::os::unix::fs::PermissionsExt;
        let fixture_home = host.path().join(mode);
        let project = cas_6718_drifted_project(&fixture_home);
        let root = project.join(".cas");
        env.set("CAS_ROOT", &root);
        env.set("CAS_CLONE_PATH", &project);
        env.set("CAS_AGENT_ROLE", if mode == "worker-sync" { "worker" } else { "supervisor" });
        env.set("CAS_PROJECT_ROOTS", &fixture_home);
        env.set_current_dir(&project);
        // Hook refusal is tested on a branch where automatic commits are allowed.
        cas_6718_git(&project, &["branch", "-M", "repair/hook-refusal"]);
        let head = cas_6718_git(&project, &["rev-parse", "HEAD"]);
        let readme = project.join("README.md");
        std::fs::write(&readme, "Unstaged user work survives refusal\n").unwrap();
        let user_before = std::fs::read(&readme).unwrap();
        let hook = project.join(".git/hooks/pre-commit");
        let commit_msg = mode == "commit-msg-sync";
        std::fs::write(
            &hook,
            if commit_msg {
                "#!/bin/sh\nexit 0\n"
            } else {
                "#!/bin/sh\necho 'pre-commit: fixture refuses the managed commit' >&2\nexit 1\n"
            },
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let message_hook = project.join(".git/hooks/commit-msg");
        if commit_msg {
            std::fs::write(&message_hook,
                "#!/bin/sh\necho 'commit-msg: fixture refuses the managed commit' >&2\nexit 1\n").unwrap();
            std::fs::set_permissions(&message_hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let message_hook_before = std::fs::read(&message_hook).ok();
        let hook_before = std::fs::read(&hook).unwrap();
        let config_before = std::fs::read(project.join(".git/config")).unwrap();
        let mut args = update_test_args();
        args.all_projects = mode == "all-projects";
        args.sync = mode != "all-projects";
        let human = mode == "human-sync";
        env.set("COLUMNS", "80");
        let (result, output) =
            capture_phase(true, || execute(&args, &update_test_cli(!human), Some(&root)));
        assert!(
            result.is_err(),
            "{mode}: refused commit must make update incomplete, got success:\n{output}"
        );
        if human {
            let error = format!("[ERROR] {}", result.unwrap_err());
            let visible = format!("{output}{error}");
            if visible.matches("pre-commit: fixture refuses").count() != 1
                || visible.matches("--show-origin").count() != 1
                || error.lines().any(|line| ratatui::text::Line::from(line).width() > 80)
            {
                diagnostic_failures.push(format!(
                    "human sync repeats or overflows: causes={}, remedies={}; {error}",
                    visible.matches("pre-commit: fixture refuses").count(),
                    visible.matches("--show-origin").count(),
                ));
            }
        } else {
            let rows = output
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .collect::<Vec<_>>();
            assert_eq!(rows.len(), 1, "{mode}: expected one JSON receipt:\n{output}");
            let receipt = &rows[0];
            // cas-fd1e: the all-projects receipt owns each phase's documents;
            // sync still reports managed files directly. Both preserve the refusal.
            let managed = if mode == "all-projects" {
                receipt.pointer("/projects/0/phase_documents/skills/0/managed_files")
            } else {
                receipt.get("managed_files")
            }
            .unwrap_or_else(|| panic!("{mode}: missing managed-file receipt:\n{output}"));
            assert_eq!(managed["outcome"], "commit_failed", "{output}");
            assert!(
                managed["conflicts"].as_array().unwrap().is_empty(),
                "{output}"
            );
            let reason = managed["commit_error"]
                .as_str()
                .expect("distinct commit error");
            assert!(
                reason.contains(if commit_msg { "commit-msg: fixture refuses" } else { "pre-commit: fixture refuses" })
                    && reason.contains("--show-origin"),
                "{reason}"
            );
            if commit_msg {
                if reason.contains("active pre-commit hook") || !reason.contains("hooks directory") {
                    diagnostic_failures.push(format!("commit-msg refusal misattributed: {reason}"));
                }
            }
            if mode == "all-projects" {
                assert_eq!(receipt["refresh_status"], "refresh_failed", "{output}");
                assert!(
                    receipt["projects"][0]["skills"]
                        .as_str()
                        .unwrap()
                        .starts_with("FAILED:"),
                    "{output}"
                );
            }
        }
        assert_eq!(cas_6718_git(&project, &["rev-parse", "HEAD"]), head);
        assert!(!cas_6718_git(&project, &["diff", "--cached", "--name-only"]).is_empty());
        assert_eq!(std::fs::read(&readme).unwrap(), user_before);
        assert_eq!(std::fs::read(&hook).unwrap(), hook_before);
        assert_eq!(std::fs::read(&message_hook).ok(), message_hook_before);
        assert_eq!(
            std::fs::read(project.join(".git/config")).unwrap(),
            config_before
        );
    }
    assert!(diagnostic_failures.is_empty(), "{diagnostic_failures:#?}");
}

#[test]
fn cas_9815_update_distinguishes_old_shipped_skill_from_user_delta() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    env.remove("CAS_AGENT_ROLE");
    env.remove("CAS_SKIP_FACTORY_TOOLING");
    // Exact flat-source blob dce50663ef7eb3cb5d8c0dd2dc237f741d8c99e8,
    // also observed read-only in Penguinz after a previous Cassy sync.
    let old = include_str!("../../../tests/fixtures/managed-files/cas-worker-pre-3.61.md");
    let path = ".claude/skills/cas-worker/SKILL.md";
    let current = crate::builtins::BUILTIN_SKILLS
        .iter()
        .find(|f| f.path == "skills/cas-worker/SKILL.md")
        .unwrap()
        .content;
    for user_delta in [false, true] {
        let fixture_home = env.home().join(if user_delta { "edited" } else { "old" });
        let project = cas_6718_drifted_project(&fixture_home);
        let root = project.join(".cas");
        env.set("CAS_ROOT", &root);
        let file = project.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, current).unwrap();
        cas_6718_git(&project, &["add", "-f", path]);
        cas_6718_git(&project, &["commit", "-qm", "tracked managed baseline"]);
        let working = format!(
            "{old}{}",
            if user_delta {
                "\nProject-specific instruction.\n"
            } else {
                ""
            }
        );
        std::fs::write(&file, &working).unwrap();
        let index = cas_6718_git(&project, &["ls-files", "--stage"]);
        let mut args = update_test_args();
        args.sync = true;
        let (result, output) =
            capture_phase(true, || execute(&args, &update_test_cli(true), Some(&root)));
        result.unwrap();
        let receipt: serde_json::Value =
            serde_json::from_str(output.lines().last().unwrap()).unwrap();
        let conflicts = receipt["managed_files"]["conflicts"].as_array().unwrap();
        if user_delta {
            assert!(conflicts.iter().any(|p| p == path), "{receipt}");
            assert_eq!(std::fs::read_to_string(&file).unwrap(), working);
            assert_eq!(cas_6718_git(&project, &["ls-files", "--stage"]), index);
        } else {
            assert!(
                conflicts.is_empty(),
                "old Cassy output blocked update: {receipt}"
            );
            assert_eq!(std::fs::read_to_string(&file).unwrap(), current);
            assert!(cas_6718_git(&project, &["status", "--porcelain"]).is_empty());
            assert!(cas_6718_git(&project, &["ls-files", "--", path]).is_empty());
        }
    }
}

/// cas-4585: exercise the native update sweep with physical cwd, HOME and
/// registry confined to the fixture. Git, SQLite and managed-file refresh
/// remain real; only the unrelated user builtin/network phase is inert.
#[test]
fn cas_4585_native_update_retires_config_preserving_values_comments_and_noop() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    // Host-wide discovery excludes /tmp, so use its existing sanctioned
    // fixture parent rather than weakening project eligibility.
    let fixture_home = tempfile::tempdir_in(crate::test_paths::runtime_fixture_parent()).unwrap();
    let home = fixture_home.path().canonicalize().unwrap();
    env.set("HOME", &home);
    env.set_current_dir(&home);
    env.set("XDG_CONFIG_HOME", home.join(".config"));
    env.remove("CAS_AGENT_ROLE");
    env.remove("CAS_SKIP_FACTORY_TOOLING");
    let projects = home.join("projects");
    let project = cas_6718_drifted_project(&projects);
    let root = project.join(".cas");
    let host = crate::store::init_cas_dir(&home).unwrap();
    let global = crate::config::global_cas_dir().unwrap();
    std::fs::create_dir_all(&global).unwrap();
    env.set("CAS_ROOT", &root);
    env.set_current_dir(&project);
    env.set("CAS_PROJECT_ROOTS", &projects);
    let legacy = concat!(
        "# operator introduction\n",
        "[factory] # factory header\n",
        "# preserve retirement rationale\n",
        "merge_sweep = true # preserve inline note\n",
        "merge_sweep_timeout_secs = 17\n",
        "strict_cli = false # active explicit override\n",
        "future_policy = 'keep me'\n",
        "[factory.merge_sweep_env] # former environment\n",
        "TOKEN = 'fixture-only-token'\n",
        "[code_review]\nowner = 'worker'\nfuture_owner = 'keep this too'\n",
        "[jev] # old integration\nendpoint = 'fixture-only-endpoint'\n",
        "[project]\ncanonical_id = 'retirement-fixture'\n",
        // Explicit stages opt out of the upkeep adoption window (covered by
        // the native sweep test), so only retirement changes this config.
        "[upkeep]\ncodemap = 'enforce'\nproduct_overview = 'enforce'\n",
        "[qa.registry]\nenforce = 'enforce'\n",
    );
    for dir in [&root, &host, &global] {
        std::fs::write(dir.join("config.toml"), legacy).unwrap();
    }
    let before: crate::config::Config = toml::from_str(legacy).unwrap();
    let mut args = update_test_args();
    args.all_projects = true;
    args.refresh_receipt = Some(home.join("refresh.json"));
    let cli = update_test_cli(false);
    let run = || {
        let (result, output) = capture_phase(true, || {
            let startup =
                crate::cli::try_parse_from_with_wordmark(["cas", "update", "--all-projects"])
                    .unwrap();
            let _notices = crate::cli::startup_config_notices(&startup);
            // The real binary first reads config to decide crash-reporting
            // policy, then startup logging, then project discovery.
            let _enabled = crate::sentry::is_sentry_enabled();
            crate::config::Config::load(&root).unwrap();
            refresh_all_projects_with_user_refresh(
                &args,
                &cli,
                Some(&root),
                None,
                None,
                None,
                || refresh_user_level_store_with_builtins(&args, &cli, || Ok(())),
            )
        });
        #[cfg(unix)]
        assert!(
            !output.contains("ignored and dropped on the next save")
                && !output.contains("[jev] config section is retired"),
            "raw startup notice escaped structured maintenance: {output}"
        );
        result
    };
    let report = run().unwrap();
    let sweep = report.sweep.as_ref().unwrap();
    assert_eq!(sweep.config.repaired, 3);
    assert_eq!(sweep.config.entries_removed, 15);
    let rendered = render_plain_at(80, |fmt| findings::print(fmt, sweep).map(|_| ()));
    assert!(rendered.contains("15 retired entries removed in 3 files"), "{rendered}");
    let receipt = read_refresh_receipt(args.refresh_receipt.as_ref().unwrap()).unwrap();
    assert_eq!(receipt["projects"].as_array().unwrap().len(), 1);
    for dir in [&root, &host, &global] {
        let raw = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        let parsed: toml::Value = toml::from_str(&raw).unwrap();
        assert!(parsed["factory"].get("merge_sweep").is_none(), "update must retire the ignored sweep key");
        assert!(parsed["factory"].get("merge_sweep_env").is_none());
        assert!(parsed.get("jev").is_none());
        assert!(parsed["code_review"].get("owner").is_none());
        assert_eq!(parsed["code_review"]["future_owner"].as_str(), Some("keep this too"));
        for comment in ["operator introduction", "factory header", "retirement rationale", "inline note", "active explicit override", "former environment", "old integration"] {
            assert!(raw.contains(comment), "lost operator comment {comment}");
        }
        let after: crate::config::Config = toml::from_str(&raw).unwrap();
        assert_eq!(serde_json::to_value(&before).unwrap(), serde_json::to_value(after).unwrap());
    }
    let backup = receipt["projects"][0]["retired_toml"]["backup"].as_str().unwrap();
    assert_eq!(std::fs::read_to_string(backup).unwrap(), legacy);
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(backup).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let original = std::fs::metadata(root.join("config.toml"))
        .unwrap()
        .modified()
        .unwrap();
    // Current read-only configs need no write eligibility, including during
    // a real native repeat across project, host and global stores.
    for dir in [&root, &host, &global] {
        let path = dir.join("config.toml");
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
    }
    let repeat = run().unwrap();
    assert_eq!(repeat.sweep.as_ref().unwrap().config.repaired, 0);
    assert_eq!(
        std::fs::metadata(root.join("config.toml"))
            .unwrap()
            .modified()
            .unwrap(),
        original
    );
    let receipt = read_refresh_receipt(args.refresh_receipt.as_ref().unwrap()).unwrap();
    assert_eq!(
        receipt["projects"][0]["retired_toml"]["status"],
        "unchanged"
    );
    assert!(!project.join(".cas/.cas").exists());

    // Planning a retired read-only config must also leave it untouched and
    // create neither a config lock nor a before-copy directory.
    let preview = home.join("readonly-preview");
    std::fs::create_dir(&preview).unwrap();
    let path = preview.join("config.toml");
    std::fs::write(&path, legacy).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(crate::config::retire_config(&preview, true).status, "planned");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), legacy);
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), modified);
    assert_eq!(std::fs::read_dir(&preview).unwrap().count(), 1);
}

#[test]
#[cfg(unix)]
fn cas_4585_retired_notices_survive_nested_failure_and_other_modes() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let home = env.home().to_path_buf();
    env.set_current_dir(&home);
    let root = home.join(".cas");
    std::fs::create_dir_all(&root).unwrap();
    env.set("CAS_ROOT", &root);
    let path = root.join("config.toml");
    let original = "[factory]\nmerge_sweep = true\n[jev]\nendpoint = 'fixture-only'\n";
    std::fs::write(&path, original).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions).unwrap();
    let startup =
        crate::cli::try_parse_from_with_wordmark(["cas", "update", "--all-projects"]).unwrap();
    let projection = tempfile::tempdir().unwrap();
    std::fs::create_dir(projection.path().join(".cas")).unwrap();
    std::fs::write(projection.path().join(".cas/config.toml"), original).unwrap();
    let (_, output) = capture_phase(true, || {
        let _outer = crate::cli::startup_config_notices(&startup);
        let _enabled = crate::sentry::is_sentry_enabled();
        let (_, nested) = capture_phase(true, || {
            let _inner = crate::cli::startup_config_notices(&startup);
            crate::config::Config::load(&root).unwrap();
        });
        assert!(
            nested.is_empty(),
            "nested scope drained its caller's notices: {nested}"
        );
        // The real index-side hook projection must attribute its copied
        // config to the owning project, never a disposable temp directory.
        crate::cli::hook::configure_claude_hooks_for_index(projection.path(), &home).unwrap();
        assert_eq!(crate::config::retire_config(&root, false).status, "failed");
        // A refused or failed maintenance does not resolve this file. Scope
        // exit replays it even if the command never reaches a final receipt.
    });
    assert!(
        output
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("retired config settings remain ignored"),
        "{output}"
    );
    assert!(
        !output.contains(&projection.path().display().to_string()),
        "temporary projection became an operator finding: {output}"
    );
    assert!(!output.contains("ignored and dropped on the next save"));
    assert!(
        output.lines().all(|line| line.chars().count() <= 80),
        "{output}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    // --check is not native maintenance: its ordinary config diagnostic is
    // neither hidden nor consumed by the earlier updater scope.
    let check = crate::cli::try_parse_from_with_wordmark(["cas", "update", "--check"]).unwrap();
    let (_, ordinary) = capture_phase(true, || {
        let _scope = crate::cli::startup_config_notices(&check);
        crate::config::Config::load(&root).unwrap();
    });
    assert!(
        ordinary.contains("ignored and dropped on the next save"),
        "{ordinary}"
    );
}

#[test]
fn cas_4585_native_update_keeps_bad_configs_and_other_phase_remedies() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let fixture_home = tempfile::tempdir_in(crate::test_paths::runtime_fixture_parent()).unwrap();
    let home = fixture_home.path().canonicalize().unwrap();
    env.set("HOME", &home);
    env.set_current_dir(&home);
    env.set("XDG_CONFIG_HOME", home.join(".config"));
    env.remove("CAS_AGENT_ROLE");
    env.remove("CAS_SKIP_FACTORY_TOOLING");
    let projects = home.join("projects");
    let healthy = cas_6718_drifted_project(&projects.join("healthy"));
    let broken = cas_6718_drifted_project(&projects.join("broken"));
    let readonly = cas_6718_drifted_project(&projects.join("readonly"));
    let host = crate::store::init_cas_dir(&home).unwrap();
    let global = crate::config::global_cas_dir().unwrap();
    std::fs::create_dir_all(&global).unwrap();
    let retired = "[factory]\nmerge_sweep = true\n[jev]\nendpoint = 'fixture-only'\n[code_review]\nowner = 'worker'\n";
    std::fs::write(global.join("config.toml"), retired).unwrap();
    let readonly_config = readonly.join(".cas/config.toml");
    std::fs::write(&readonly_config, retired).unwrap();
    let mut permissions = std::fs::metadata(&readonly_config).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&readonly_config, permissions).unwrap();
    let root = healthy.join(".cas");
    env.set("CAS_ROOT", &root);
    env.set_current_dir(&healthy);
    env.set("CAS_PROJECT_ROOTS", &projects);
    let invalid = "[factory\nmerge_sweep = true\n";
    for directory in [&host, &broken.join(".cas")] {
        std::fs::write(directory.join("config.toml"), invalid).unwrap();
    }
    #[cfg(unix)]
    {
        // Ambiguous ownership is an inaccessible cleanup target even though
        // an ordinary config reader can follow it. Never replace its link.
        let linked = broken.join(".cas/config.toml");
        std::fs::remove_file(&linked).unwrap();
        std::os::unix::fs::symlink(host.join("config.toml"), linked).unwrap();
    }
    std::fs::write(
        root.join("config.toml"),
        "[factory]\nmerge_sweep = true\nstrict_cli = false\n",
    )
    .unwrap();
    let mut args = update_test_args();
    args.all_projects = true;
    args.refresh_receipt = Some(home.join("refresh.json"));
    let (result, output) = capture_phase(true, || {
        refresh_all_projects_with_user_refresh(
            &args,
            &update_test_cli(false),
            Some(&root),
            None,
            None,
            None,
            || ProjectPhase::Failed("builtin repair failed; run `cas update --user`".into()),
        )
    });
    assert!(result.is_err());
    let report = refresh_report(&result).unwrap();
    assert_eq!(report.failed_projects, 2);
    assert!(report.user_level_failed);
    let sweep = report.sweep.as_ref().unwrap();
    assert_eq!(sweep.config.repaired, 2);
    assert_eq!(sweep.config.failed, 3);
    let rendered = render_plain_at(80, |fmt| findings::print(fmt, sweep).map(|_| ()));
    assert!(rendered.contains("builtin repair failed"));
    assert!(
        !output.contains("retired config entries remain ignored")
            && !output.contains("retired config settings remain ignored"),
        "receipted config failures must not replay duplicate or projection findings: {output}"
    );
    // Longer phase remedies can wrap this cause between words at 80 columns.
    let prose = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(
        prose.matches("config is read-only").count(),
        1,
        "{rendered}"
    );
    assert!(rendered.lines().all(|line| line.chars().count() <= 80), "{rendered}");
    assert!(
        rendered.contains("cas update --user"),
        "existing phase remedy lost: {rendered}"
    );
    assert!(
        sweep
            .findings
            .iter()
            .any(|finding| finding.cause.contains("repair config before updating"))
    );
    let receipt = read_refresh_receipt(args.refresh_receipt.as_ref().unwrap()).unwrap();
    assert_eq!(receipt["config_maintenance"]["failed"], 3);
    assert_eq!(receipt["config_maintenance"]["repaired"], 2);
    assert!(
        !receipt["user_builtins"]
            .as_str()
            .unwrap()
            .contains("retired TOML entries removed"),
        "another file's successful cleanup leaked into failure prose: {receipt}"
    );
    assert_eq!(std::fs::read_to_string(&readonly_config).unwrap(), retired);
    for directory in [&host, &broken.join(".cas")] {
        assert_eq!(
            std::fs::read_to_string(directory.join("config.toml")).unwrap(),
            invalid
        );
        assert!(!directory.join("config-retirement-backups").exists());
    }
    #[cfg(unix)]
    assert!(
        std::fs::symlink_metadata(broken.join(".cas/config.toml"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        !std::fs::read_to_string(root.join("config.toml"))
            .unwrap()
            .contains("merge_sweep")
    );
}

/// Different valid TOML spellings must retire the same ignored values without
/// treating a '#' inside a secret as a comment to preserve.
#[test]
fn cas_4585_retirement_preserves_dotted_inline_and_multiline_config() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    assert_eq!(
        crate::config::retire_config(directory.path(), false).status,
        "missing"
    );
    std::fs::write(&path, "[factory]\nstrict_cli = false\n").unwrap();
    assert_eq!(
        crate::config::retire_config(directory.path(), false).status,
        "unchanged"
    );
    assert_eq!(
        std::fs::read_dir(directory.path()).unwrap().count(),
        1,
        "a current config must not create locks or backups"
    );
    for raw in [
        "factory.merge_sweep = [\n 'fixture#secret', # keep array comment\n]\nfactory.merge_sweep_timeout_secs = 17\nfactory.strict_cli = false # keep active comment\ncode_review.owner = 'worker'\ncode_review.future_owner = '#keep'\njev.deep.endpoint = 'fixture#secret'\n",
        "factory = { strict_cli = false, merge_sweep = true, future_policy = 'keep', merge_sweep_env = { TOKEN = 'fixture#secret' }, merge_sweep_timeout_secs = 17 } # keep active comment\ncode_review = { owner = 'worker', future_owner = '#keep' }\njev = { endpoint = 'fixture#secret' } # keep integration comment\n",
    ] {
        std::fs::write(&path, raw).unwrap();
        let before: crate::config::Config = toml::from_str(raw).unwrap();
        let plan = crate::config::retire_config(directory.path(), true);
        assert_eq!(plan.status, "planned", "{plan:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        let receipt = crate::config::retire_config(directory.path(), false);
        assert_eq!(receipt.status, "repaired", "{receipt:?}");
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(
            !after.contains("fixture#secret"),
            "retired string leaked into preserved comments"
        );
        for comment in raw
            .lines()
            .filter_map(|line| line.find("# keep").map(|at| &line[at..]))
        {
            assert!(after.contains(comment), "lost {comment}");
        }
        let current: crate::config::Config = toml::from_str(&after).unwrap();
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(current).unwrap()
        );
        let doc: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(doc["code_review"]["future_owner"].as_str(), Some("#keep"));
        assert_eq!(
            crate::config::retire_config(directory.path(), false).status,
            "unchanged"
        );
    }
}

#[test]
fn modified_reference_warning_wraps_losslessly_and_uses_user_scope_cas_5e59() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    env.set("NO_COLOR", "1");
    let home = env.home().to_path_buf();
    let root = crate::store::init_cas_dir(&home.join("project")).unwrap();
    env.set("CAS_ROOT", &root);
    let destination = home.join(".claude");
    std::fs::create_dir_all(&destination).unwrap();
    sync_all_builtins_for_harness(cas_mux::SupervisorCli::Claude, &destination).unwrap();
    let path = "skills/cas-worker/references/discipline.md";
    let edited = destination.join(path);
    let local = "# Local instructions\n\nPreserve every word of our local customization.\n";
    std::fs::write(&edited, local).unwrap();

    for columns in ["80", "120"] {
        env.set("COLUMNS", columns);
        let (result, output) = capture_phase(true, || {
            UpdateStepTracker::new(1, true).run("Distributing built-ins to user-level", || {
                sync_user_builtins_with_violet(&update_test_cli(false), || Ok(()))
            })
        });
        result.unwrap();
        let warning = output.split_once("[WARN]").expect("modified reference warning").1;
        assert!(output.lines().all(|line| ratatui::text::Line::from(line).width() <= 80), "{output}");
        let prose = warning.split_whitespace().collect::<Vec<_>>().join(" ");
        // pin: preserve the existing explanation in full; wrapping must not elide it.
        assert!(prose.contains("1 locally modified builtin reference file(s) in ~/.claude were preserved (content matches no version Cassy has shipped):"), "{output}");
        assert!(prose.contains(path), "{output}");
        // pin: lossless advice must keep every word and the actionable user command.
        assert!(prose.contains("Review each file; to accept the Cassy version, delete it and rerun `cas update --user`."), "{output}");
        assert!(output.lines().any(|line| line.contains("`cas update --user`.")), "{output}");
        assert!(!prose.contains("--sync") && !prose.contains('…'), "{output}");
        assert_eq!(std::fs::read_to_string(&edited).unwrap(), local);
    }

    // The same real reporting path must retain the project-scoped remedy.
    let result = SyncResult {
        modified_reference_files: vec![path.to_owned()],
        ..SyncResult::default()
    };
    let (reported, output) = capture_phase(true, || {
        report_builtin_sync(&result, ".claude", &ActiveTheme::default())
    });
    reported.unwrap();
    assert!(output.lines().all(|line| ratatui::text::Line::from(line).width() <= 80), "{output}");
    let prose = output.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(prose.contains("cas update --sync") && !prose.contains("--user"), "{output}");
}

#[test]
fn update_reaps_dead_metadata_without_touching_live_sessions_cas_9299() {
    use crate::ui::factory::{SessionManager, create_metadata, gui_socket_path, metadata_path};
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let roots = env.home().join("empty-projects");
    std::fs::create_dir_all(&roots).unwrap();
    env.set("CAS_PROJECT_ROOTS", &roots);
    env.remove("CAS_ROOT");
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let dead = child.id();
    assert!(child.wait().unwrap().success());
    let manager = SessionManager::new();
    for (name, pid) in [
        ("dead9299", dead),
        ("held9299", dead),
        ("live9299", std::process::id()),
    ] {
        manager
            .save_metadata(&create_metadata(
                name,
                pid,
                "supervisor",
                &[],
                None,
                None,
                None,
            ))
            .unwrap();
    }
    for name in ["dead9299", "live9299"] {
        drop(std::os::unix::net::UnixListener::bind(gui_socket_path(name)).unwrap());
    }
    let held = std::os::unix::net::UnixListener::bind(gui_socket_path("held9299")).unwrap();
    let args = update_test_args();
    let run = || {
        refresh_all_projects_with_user_refresh(
            &args,
            &update_test_cli(true),
            None,
            None,
            None,
            None,
            || ProjectPhase::Skipped("isolated user phase".into()),
        )
        .unwrap()
    };
    run();
    assert!(
        !metadata_path("dead9299").exists(),
        "update must reap a proven-dead record"
    );
    assert!(
        metadata_path("held9299").exists(),
        "held metadata retains project attribution"
    );
    assert!(metadata_path("live9299").exists());
    assert!(gui_socket_path("live9299").exists());
    assert!(gui_socket_path("held9299").exists());
    assert_eq!(
        gui_socket_path("dead9299").exists(),
        !cfg!(any(target_os = "linux", target_os = "macos"))
    );
    std::fs::write(metadata_path("bad9299"), "incomplete receipt").unwrap();
    run();
    assert!(metadata_path("bad9299").exists());
    assert!(
        metadata_path("live9299").exists(),
        "repeat cleanup retains live ownership"
    );
    drop(held);
}
