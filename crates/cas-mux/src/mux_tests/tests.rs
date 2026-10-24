use crate::harness::SupervisorCli;
use crate::mux::*;
use crate::pane::UserInputKind;
use crate::spec::{Effort, WorkerSpec};
use crate::test_env_guard::TestEnvGuard;
use std::path::PathBuf;

fn env_value<'a>(config: &'a crate::pty::PtyConfig, key: &str) -> Option<&'a str> {
    config
        .env
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn violet_registrations(home: &std::path::Path, token: &str, bypass: Option<&str>) {
    std::fs::write(home.join(".claude.json"), serde_json::json!({"mcpServers":{"violet":{"type":"http", "url":"https://violet-hub.vercel.app/mcp/slack", "headers":{
        "Authorization":format!("Bearer ${{{token}}}"), "x-vercel-protection-bypass":bypass.map(|v| format!("${{{v}}}")).unwrap_or_default()
    }}}}).to_string()).unwrap();
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(home.join(".codex/config.toml"), format!("[mcp_servers.violet]\nurl='https://violet-hub.vercel.app/mcp/slack'\nbearer_token_env_var='{token}'\n{}", bypass.map(|v| format!("env_http_headers={{x-vercel-protection-bypass='{v}'}}\n")).unwrap_or_default())).unwrap();
}
fn private_permissions(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn cas_678d_round3_factory_hydration_obeys_server_and_env_denials() {
    let mut env = TestEnvGuard::temp_home();
    let home = env.home().to_path_buf();
    env.set("XDG_CONFIG_HOME", home.join(".config"));
    let credentials = home.join("credentials.env");
    std::fs::write(
        &credentials,
        "VIOLET_SLACK_TOKEN_SELECTED='selected'\nVIOLET_SLACK_TOKEN_OTHER='other'\n",
    )
    .unwrap();
    private_permissions(&credentials);
    env.set("CAS_CREDENTIALS_FILE", &credentials);
    env.set("VIOLET_SLACK_TOKEN_SELECTED", "");
    env.set("VIOLET_SLACK_TOKEN_OTHER", "");
    violet_registrations(&home, "VIOLET_SLACK_TOKEN_SELECTED", None);
    let root = home.join("project/.cas");
    std::fs::create_dir_all(&root).unwrap();
    for (policy, allowed) in [
        ("", true),
        ("supervisor_only_mcp=['violet']", false),
        ("supervisor_only_env=['VIOLET_SLACK_TOKEN_SELECTED']", false),
    ] {
        std::fs::write(root.join("config.toml"), format!("[factory]\n{policy}\n")).unwrap();
        for inherited in [false, true] {
            env.set(
                "VIOLET_SLACK_TOKEN_SELECTED",
                if inherited { "inherited" } else { "" },
            );
            for cli in [SupervisorCli::Claude, SupervisorCli::Codex] {
                let configs = Mux::factory_pane_configs(&MuxConfig {
                    cwd: home.join("project"),
                    cas_root: Some(root.clone()),
                    workers: 1,
                    worker_cli: cli,
                    include_director: false,
                    ..Default::default()
                });
                let mut pane = configs
                    .into_iter()
                    .find(|(name, _)| name == "worker-1")
                    .unwrap()
                    .1;
                pane.hydrate_violet_environment();
                assert_eq!(
                    env_value(&pane, "VIOLET_SLACK_TOKEN_SELECTED"),
                    if allowed {
                        Some(if inherited { "inherited" } else { "selected" })
                    } else {
                        None
                    },
                    "{cli:?} policy={policy}"
                );
                assert!(env_value(&pane, "VIOLET_SLACK_TOKEN_OTHER").is_none());
                if !allowed {
                    assert!(
                        pane.env_remove
                            .contains(&"VIOLET_SLACK_TOKEN_SELECTED".into())
                    );
                }
            }
        }
    }
}

#[test]
fn cas_678d_round3_factory_authorization_survives_late_file_values() {
    let mut env = TestEnvGuard::temp_home();
    let home = env.home().to_path_buf();
    env.set("CAS_CREDENTIALS_FILE", home.join("credentials.env"));
    env.set("VIOLET_SLACK_TOKEN_LATE", "");
    violet_registrations(&home, "VIOLET_SLACK_TOKEN_LATE", None);
    for cli in [SupervisorCli::Claude, SupervisorCli::Codex] {
        let _ = std::fs::remove_file(home.join("credentials.env"));
        let configs = Mux::factory_pane_configs(&MuxConfig { cwd: home.clone(), workers: 1, worker_cli: cli, include_director: false, ..Default::default() });
        let mut pane = configs.into_iter().find(|(name,_)| name == "worker-1").unwrap().1;
        std::fs::write(home.join("credentials.env"), "VIOLET_SLACK_TOKEN_LATE=late-value\n").unwrap();
        private_permissions(&home.join("credentials.env"));
        pane.hydrate_violet_environment();
        assert_eq!(env_value(&pane,"VIOLET_SLACK_TOKEN_LATE"),Some("late-value"));
    }
}

#[test]
fn cas_678d_round4_proxy_and_explicit_grants_survive_native_scope() {
    let mut env = TestEnvGuard::temp_home();
    let home = env.home().to_path_buf();
    env.set("XDG_CONFIG_HOME", home.join(".config"));
    env.set("CAS_CREDENTIALS_FILE", home.join("credentials.env"));
    env.set("VIOLET_SLACK_TOKEN_PROXY_FIXTURE", "");
    env.set("VIOLET_SLACK_TOKEN_SELECTED", "");
    std::fs::write(home.join("credentials.env"), "VIOLET_SLACK_TOKEN_PROXY_FIXTURE=proxy-value\nVIOLET_SLACK_TOKEN_SELECTED=selected-value\n").unwrap();
    private_permissions(&home.join("credentials.env"));
    violet_registrations(&home, "VIOLET_SLACK_TOKEN_SELECTED", None);
    let root = home.join("project/.cas");
    std::fs::create_dir_all(&root).unwrap();
    let user = home.join(".config/code-mode-mcp/config.toml");
    std::fs::create_dir_all(user.parent().unwrap()).unwrap();
    let proxy = "[servers.violet]\ntransport='http'\nauth='env:VIOLET_SLACK_TOKEN_PROXY_FIXTURE'\n";
    for source in ["project", "user", "explicit"] {
        std::fs::write(
            root.join("proxy.toml"),
            if source == "project" { proxy } else { "" },
        )
        .unwrap();
        std::fs::write(&user, if source == "user" { proxy } else { "" }).unwrap();
        for (denial, allowed) in [
            ("", true),
            ("supervisor_only_mcp=['violet']", false),
            (
                "supervisor_only_env=['VIOLET_SLACK_TOKEN_PROXY_FIXTURE']",
                false,
            ),
        ] {
            std::fs::write(
                root.join("config.toml"),
                format!(
                    "[factory]\n{denial}\n{}",
                    if source == "explicit" {
                        "worker_credential_env=['VIOLET_SLACK_TOKEN_PROXY_FIXTURE']\n"
                    } else {
                        ""
                    }
                ),
            )
            .unwrap();
            for cli in [SupervisorCli::Claude, SupervisorCli::Codex] {
                std::fs::write(
                    home.join("credentials.env"),
                    "VIOLET_SLACK_TOKEN_SELECTED=selected-value\n",
                )
                .unwrap();
                let configs = Mux::factory_pane_configs(&MuxConfig {
                    cwd: home.join("project"),
                    cas_root: Some(root.clone()),
                    workers: 1,
                    worker_cli: cli,
                    include_director: false,
                    ..Default::default()
                });
                let mut pane = configs
                    .into_iter()
                    .find(|(name, _)| name == "worker-1")
                    .unwrap()
                    .1;
                // Authorized proxy values may arrive after config materialization.
                std::fs::write(home.join("credentials.env"), "VIOLET_SLACK_TOKEN_PROXY_FIXTURE=proxy-value\nVIOLET_SLACK_TOKEN_SELECTED=selected-value\n").unwrap();
                pane.hydrate_violet_environment();
                assert_eq!(
                    env_value(&pane, "VIOLET_SLACK_TOKEN_PROXY_FIXTURE"),
                    if allowed { Some("proxy-value") } else { None },
                    "{source} {denial} {cli:?}"
                );
                if !allowed {
                    assert!(
                        pane.env_remove
                            .contains(&"VIOLET_SLACK_TOKEN_PROXY_FIXTURE".into())
                    );
                }
            }
        }
    }
}

#[test]
fn cas_678d_factory_launch_uses_private_value_instead_of_shell_expression() {
    let mut guard = TestEnvGuard::temp_home();
    let home = guard.home().to_path_buf();
    guard.set("XDG_CONFIG_HOME", home.join(".config"));
    guard.set("SHELL", "/bin/bash");
    guard.set("CAS_CREDENTIALS_FILE", home.join("credentials.env"));
    guard.set("VIOLET_SLACK_TOKEN_FACTORY_FIXTURE", "");
    std::fs::write(
        home.join("credentials.env"),
        "export VIOLET_SLACK_TOKEN_FACTORY_FIXTURE='private-fixture'\n",
    )
    .unwrap();
    private_permissions(&home.join("credentials.env"));
    violet_registrations(&home, "VIOLET_SLACK_TOKEN_FACTORY_FIXTURE", None);
    std::fs::write(
        home.join(".profile"),
        "export VIOLET_SLACK_TOKEN_FACTORY_FIXTURE='$(pass show secret)'\n",
    )
    .unwrap();
    let proxy = home.join(".config/code-mode-mcp/config.toml");
    std::fs::create_dir_all(proxy.parent().unwrap()).unwrap();
    std::fs::write(proxy, "[servers.violet]\ntransport='http'\nurl='https://violet-hub.vercel.app/mcp/slack'\nauth='env:VIOLET_SLACK_TOKEN_FACTORY_FIXTURE'\n").unwrap();
    for cli in [SupervisorCli::Claude, SupervisorCli::Codex] {
        let configs = Mux::factory_pane_configs(&MuxConfig { cwd: home.clone(), workers: 1,
            worker_cli: cli, include_director: false, ..Default::default() });
        let (_, pane) = configs.iter().find(|(name, _)| name == "worker-1").unwrap();
        assert_eq!(env_value(pane, "VIOLET_SLACK_TOKEN_FACTORY_FIXTURE"), Some("private-fixture"));
    }
}

#[tokio::test]
async fn declared_worker_credentials_reach_claude_and_codex_from_env_or_file_cas_82bc() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_CLAUDE_SUPPORTED_FLAGS", "--mcp-config,--strict-mcp-config");
    let project = tempfile::tempdir().unwrap();
    let root = project.path().join(".cas");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.toml"),
        "[factory]\nworker_credential_env = ['GITHUB_TOKEN', 'VERCEL_TOKEN']\n").unwrap();
    let credentials = env.home().join("credentials.env");
    env.set("CAS_CREDENTIALS_FILE", &credentials);
    env.set("NEON_API_KEY", "not-granted-fixture");
    env.set("CAS_FACTORY_WORKER_CREDENTIAL_WARNING", "stale-fixture");
    env.remove("CAS_WORKER_GITHUB_READ_TOKEN");
    for from_file in [false, true] {
        if from_file {
            env.remove("GITHUB_TOKEN");
            env.remove("VERCEL_TOKEN");
            std::fs::write(&credentials,
                "export GITHUB_TOKEN='github-fixture'\nexport VERCEL_TOKEN='vercel-fixture'\n").unwrap();
        } else {
            env.set("GITHUB_TOKEN", "github-fixture");
            env.set("VERCEL_TOKEN", "vercel-fixture");
            std::fs::write(&credentials, "").unwrap();
        }
        for cli in [SupervisorCli::Claude, SupervisorCli::Codex] {
            let configs = Mux::factory_pane_configs(&MuxConfig {
                cwd: project.path().into(), cas_root: Some(root.clone()),
                workers: 1, worker_cli: cli, include_director: false,
                ..Default::default()
            });
            let worker = &configs.iter().find(|(name, _)| name == "worker-1").unwrap().1;
            assert_eq!(env_value(worker, "GITHUB_TOKEN"), Some("github-fixture"));
            assert_eq!(env_value(worker, "VERCEL_TOKEN"), Some("vercel-fixture"));
            assert!(env_value(worker, "NEON_API_KEY").is_none());
            assert!(worker.env_remove.iter().any(|name| name == "NEON_API_KEY"));
            assert!(!worker.env_remove.iter().any(|name| name == "GITHUB_TOKEN" || name == "VERCEL_TOKEN"));
            assert!(env_value(worker, "CAS_FACTORY_WORKER_CREDENTIAL_WARNING").is_none());
            assert!(env_value(worker, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
            assert_eq!(worker.args.iter().any(|arg| arg == "--strict-mcp-config"), cli == SupervisorCli::Claude);
            let mut probe = worker.clone();
            probe.command = "/bin/sh".into();
            probe.args = vec!["-c".into(),
                "test \"$GITHUB_TOKEN\" = github-fixture && test \"$VERCEL_TOKEN\" = vercel-fixture && test -z \"${NEON_API_KEY+x}${CAS_FACTORY_WORKER_CREDENTIAL_WARNING+x}\" && printf 'declared-credentials-received\\n'".into()];
            let mut pty = crate::pty::Pty::spawn("credential-probe", probe).unwrap();
            let output = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                let mut output = Vec::new();
                loop {
                    match pty.recv().await {
                        Some(crate::pty::PtyEvent::Output(bytes)) => output.extend(bytes),
                        Some(crate::pty::PtyEvent::Exited(code)) => { assert_eq!(code, Some(0)); break; }
                        Some(crate::pty::PtyEvent::Error(error)) => panic!("{error}"),
                        None => panic!("credential probe ended without process status"),
                    }
                }
                output
            }).await.expect("bounded credential handoff probe");
            assert!(String::from_utf8_lossy(&output).contains("declared-credentials-received"));
        }
    }
}

#[test]
fn missing_declared_worker_credential_warns_without_refusing_cas_82bc() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_CLAUDE_SUPPORTED_FLAGS", "--mcp-config,--strict-mcp-config");
    env.remove("GITHUB_TOKEN");
    env.remove("CAS_WORKER_GITHUB_READ_TOKEN");
    let credentials = env.home().join("credentials.env");
    std::fs::write(&credentials, "").unwrap();
    env.set("CAS_CREDENTIALS_FILE", &credentials);
    let project = tempfile::tempdir().unwrap();
    let root = project.path().join(".cas");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.toml"),
        "[factory]\nworker_credential_env = ['GITHUB_TOKEN']\n").unwrap();
    for cli in [SupervisorCli::Claude, SupervisorCli::Codex] {
        let configs = Mux::factory_pane_configs(&MuxConfig {
            cwd: project.path().into(), cas_root: Some(root.clone()),
            workers: 1, worker_cli: cli, include_director: false,
            ..Default::default()
        });
        let worker = &configs.iter().find(|(name, _)| name == "worker-1").unwrap().1;
        assert!(env_value(worker, "GITHUB_TOKEN").is_none());
        assert!(env_value(worker, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
        assert!(env_value(worker, "CAS_FACTORY_WORKER_CREDENTIAL_WARNING")
            .unwrap().contains("missing=GITHUB_TOKEN"));
    }
}

#[tokio::test]
async fn supervisor_only_credential_conflict_warns_and_denies_cas_82bc() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_CLAUDE_SUPPORTED_FLAGS", "--mcp-config,--strict-mcp-config");
    env.set("VERCEL_TOKEN", "operator-fixture");
    env.set("GH_TOKEN", "operator-fixture");
    env.set("CAS_WORKER_GITHUB_READ_TOKEN", "read-token-fixture");
    let project = tempfile::tempdir().unwrap();
    let root = project.path().join(".cas");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.toml"),
        "[factory]\nworker_credential_env = ['VERCEL_TOKEN', 'GH_TOKEN']\nsupervisor_only_env = ['VERCEL_TOKEN', 'GH_TOKEN']\n").unwrap();
    for cli in [SupervisorCli::Claude, SupervisorCli::Codex] {
        let configs = Mux::factory_pane_configs(&MuxConfig {
            cwd: project.path().into(), cas_root: Some(root.clone()),
            workers: 1, worker_cli: cli, include_director: false,
            ..Default::default()
        });
        let worker = &configs.iter().find(|(name, _)| name == "worker-1").unwrap().1;
        assert!(env_value(worker, "VERCEL_TOKEN").is_none());
        assert!(env_value(worker, "GH_TOKEN").is_none());
        assert!(worker.env_remove.iter().any(|name| name == "VERCEL_TOKEN"));
        assert!(worker.env_remove.iter().any(|name| name == "GH_TOKEN"));
        let warning = env_value(worker, "CAS_FACTORY_WORKER_CREDENTIAL_WARNING").unwrap();
        assert!(warning.contains("supervisor_only=GH_TOKEN,VERCEL_TOKEN"));
        assert!(!warning.contains("operator-fixture"));
        assert!(!warning.contains("read-token-fixture"));
        assert!(env_value(worker, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
        let mut probe = worker.clone();
        probe.command = "/bin/sh".into();
        probe.args = vec!["-c".into(),
            "test -z \"${VERCEL_TOKEN+x}${GH_TOKEN+x}${CAS_FACTORY_WORKER_CREDENTIAL_WARNING+x}\" && printf 'conflicting-grants-denied\\n'".into()];
        let mut pty = crate::pty::Pty::spawn("conflicting-credential-probe", probe).unwrap();
        let output = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut output = Vec::new();
            loop {
                match pty.recv().await {
                    Some(crate::pty::PtyEvent::Output(bytes)) => output.extend(bytes),
                    Some(crate::pty::PtyEvent::Exited(code)) => { assert_eq!(code, Some(0)); break; }
                    Some(crate::pty::PtyEvent::Error(error)) => panic!("{error}"),
                    None => panic!("conflicting credential probe ended without process status"),
                }
            }
            output
        }).await.expect("bounded denial precedence probe");
        assert!(String::from_utf8_lossy(&output).contains("conflicting-grants-denied"));
    }
}

#[test]
fn credential_allowlist_cannot_override_factory_identity_cas_82bc() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_AGENT_ROLE", "supervisor");
    let project = tempfile::tempdir().unwrap();
    let root = project.path().join(".cas");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.toml"),
        "[factory]\nworker_credential_env = ['CAS_AGENT_ROLE']\n").unwrap();
    let configs = Mux::factory_pane_configs(&MuxConfig {
        cwd: project.path().into(), cas_root: Some(root), workers: 1,
        worker_cli: SupervisorCli::Codex, include_director: false,
        ..Default::default()
    });
    let worker = &configs.iter().find(|(name, _)| name == "worker-1").unwrap().1;
    assert_eq!(env_value(worker, "CAS_AGENT_ROLE"), Some("worker"));
    assert!(env_value(worker, "CAS_FACTORY_WORKER_CREDENTIAL_WARNING")
        .unwrap().contains("reserved=CAS_AGENT_ROLE"));
}

/// GH #1047: configured supervisor-only resources win over project proxy
/// credential grants. Claude isolates local scopes; Codex disables named native
/// servers and remains spawnable with environment restrictions.
#[test]
fn factory_supervisor_only_mcp_and_env_isolate_claude_and_codex_gh_1047() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_CLAUDE_SUPPORTED_FLAGS", "--mcp-config,--strict-mcp-config");
    let project = tempfile::tempdir().unwrap();
    let cas_root = project.path().join(".cas");
    let worktree = project.path().join("worker");
    std::fs::create_dir_all(&cas_root).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(cas_root.join("config.toml"), r#"
[factory]
supervisor_only_mcp = ["vercel", "neon"]
supervisor_only_env = ["VERCEL_TOKEN", "NEON_API_KEY", "DEPLOY_FIXTURE_TOKEN"]
"#).unwrap();
    std::fs::write(cas_root.join("proxy.toml"), r#"
[servers.vercel]
transport = "http"
auth = "env:VERCEL_TOKEN"
[servers.neon]
transport = "http"
auth = "env:NEON_API_KEY"
[servers.context7]
transport = "http"
auth = "env:DEPLOY_FIXTURE_TOKEN"
"#).unwrap();
    let source = project.path().join(".mcp.json");
    std::fs::write(&source, serde_json::json!({"mcpServers": {
        "vercel": {"command": "vercel-fixture"},
        "neon": {"command": "neon-fixture"},
        "context7": {"command": "context7-fixture"}
    }}).to_string()).unwrap();
    let before = std::fs::read(&source).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&source, worktree.join(".mcp.json")).unwrap();
    #[cfg(not(unix))]
    std::fs::copy(&source, worktree.join(".mcp.json")).unwrap();
    for name in ["VERCEL_TOKEN", "NEON_API_KEY", "DEPLOY_FIXTURE_TOKEN"] {
        env.set(name, "operator-fixture");
    }
    for cli in [SupervisorCli::Claude, SupervisorCli::Codex] {
        let config = MuxConfig {
            cwd: project.path().into(),
            cas_root: Some(cas_root.clone()),
            workers: 1,
            worker_names: vec!["worker-1".into()],
            worker_cwds: [("worker-1".into(), worktree.clone())].into(),
            worker_cli: cli,
            include_director: false,
            ..Default::default()
        };
        let configs = Mux::factory_pane_configs(&config);
        let (_, worker) = configs.iter().find(|(name, _)| name == "worker-1").unwrap();
        let (_, supervisor) = configs.iter().find(|(name, _)| name == &config.supervisor_name).unwrap();
        for key in ["VERCEL_TOKEN", "NEON_API_KEY", "DEPLOY_FIXTURE_TOKEN"] {
            assert!(worker.env_remove.iter().any(|name| name == key), "{cli:?} must remove {key}");
            assert!(env_value(worker, key).is_none(), "{cli:?} must not grant {key}");
            assert!(!supervisor.env_remove.iter().any(|name| name == key));
            assert!(std::env::var_os(key).is_some(), "parent retains its credentials");
        }
        assert!(env_value(supervisor, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
        assert_eq!(std::fs::read(&source).unwrap(), before, "supervisor config is untouched");
        assert!(env_value(worker, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
        let materialized: serde_json::Value = serde_json::from_slice(
            &std::fs::read(cas_root.join("worker-mcp/worker-1.json")).unwrap()
        ).unwrap();
        assert!(!std::fs::symlink_metadata(cas_root.join("worker-mcp/worker-1.json")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(worktree.join(".mcp.json")).unwrap(), before, "existing worker configuration is untouched");
        assert!(materialized["mcpServers"].get("vercel").is_none());
        assert!(materialized["mcpServers"].get("neon").is_none());
        assert_eq!(materialized["mcpServers"].get("context7").is_some(), cli == SupervisorCli::Codex);
        assert!(materialized["mcpServers"].get("cas").is_some());
        match cli {
            SupervisorCli::Claude => {
                assert!(worker.args.iter().any(|arg| arg == "--strict-mcp-config"));
                let flag = worker.args.iter().position(|arg| arg == "--mcp-config").unwrap();
                assert_eq!(worker.args[flag + 1], cas_root.join("worker-mcp/worker-1.json").display().to_string());
            }
            SupervisorCli::Codex => {
                for name in ["vercel", "neon"] {
                    assert!(worker.args.iter().any(|arg|
                        arg == &format!("mcp_servers.{name}.enabled=false")));
                }
            }
            _ => unreachable!(),
        }
        assert!(!supervisor.args.iter().any(|arg| arg == "--strict-mcp-config"));
        assert_eq!(std::fs::read(&source).unwrap(), before, "supervisor config is untouched");
    }
}

/// Policy dispatch must follow the executable through supported nice forms,
/// including forms where the harness is not the third argument.
#[test]
fn worker_resource_policy_uses_effective_command_for_nice_wrappers_gh_1047() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_CLAUDE_SUPPORTED_FLAGS", "--mcp-config,--strict-mcp-config");
    let project = tempfile::tempdir().unwrap();
    let cas_root = project.path().join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    std::fs::write(
        cas_root.join("config.toml"),
        "[factory]\nsupervisor_only_mcp = ['vercel', 'neon']\n",
    )
    .unwrap();
    for cli in ["claude", "codex"] {
        for prefix in [
            vec![],
            vec!["--"],
            vec!["-n", "10"],
            vec!["--adjustment=10"],
        ] {
            let mut worker = crate::pty::PtyConfig {
                command: "nice".into(),
                args: prefix.into_iter().chain([cli, "--model", "claude"])
                    .map(str::to_owned).collect(),
                cwd: Some(project.path().to_path_buf()),
                ..Default::default()
            };
            crate::backend::finish_worker_config(
                &mut worker, SupervisorCli::Codex, None, None, Some(&cas_root),
            );
            assert!(env_value(&worker, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
            if cli == "claude" {
                let flag = worker.args.iter().position(|arg| arg == "--mcp-config").unwrap();
                assert!(worker.args.iter().any(|arg| arg == "--strict-mcp-config"));
                assert_eq!(worker.args[flag + 1],
                    cas_root.join("worker-mcp/worker.json").display().to_string());
            } else {
                assert!(!worker.args.iter().any(|arg| arg == "--strict-mcp-config"));
                for name in ["vercel", "neon"] {
                    assert!(worker.args.iter().any(|arg|
                        arg == &format!("mcp_servers.{name}.enabled=false")));
                }
            }
        }
    }
}

/// Grok's native project/user MCP discovery has no per-launch isolation flag.
/// Refuse a restricted worker while leaving the unrestricted adapter intact.
#[test]
fn restricted_grok_adapter_refuses_native_discovery_without_substitution_gh_1047() {
    let _env = TestEnvGuard::temp_home();
    let project = tempfile::tempdir().unwrap();
    let cas_root = project.path().join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    let original = r#"{"mcpServers":{"denied-fixture":{"command":"never-executed-fixture"}}}"#;
    std::fs::write(project.path().join(".mcp.json"), original).unwrap();
    for (policy, restricted) in [
        ("[factory]\n", false),
        ("[factory]\nsupervisor_only_mcp = ['denied-fixture']\n", true),
        ("[factory]\nsupervisor_only_env = ['DENIED_LITERAL_FIXTURE_TOKEN']\n", true),
        ("[factory]\nsupervisor_only_mcp = []\nsupervisor_only_env = []\n", false),
    ] {
        std::fs::write(cas_root.join("config.toml"), policy).unwrap();
        let config = MuxConfig {
            cwd: project.path().to_path_buf(),
            cas_root: Some(cas_root.clone()),
            workers: 1,
            worker_cli: SupervisorCli::Grok,
            supervisor_cli: SupervisorCli::Grok,
            include_director: false,
            ..Default::default()
        };
        let configs = Mux::factory_pane_configs(&config);
        let worker = &configs.iter().find(|(name, _)| name == "worker-1").unwrap().1;
        let supervisor = &configs.iter().find(|(name, _)| name == &config.supervisor_name).unwrap().1;
        assert_eq!(worker.effective_command(), "grok");
        assert_eq!(supervisor.effective_command(), "grok");
        assert_eq!(env_value(worker, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_some(), restricted);
        assert!(env_value(supervisor, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
        assert_eq!(std::fs::read_to_string(project.path().join(".mcp.json")).unwrap(), original);
    }
}

/// Read native TOML without discovery and disable only the affected servers.
/// Unknown plugin contributions must not prevent the worker from spawning.
#[test]
fn codex_env_policy_disables_literal_env_servers_preserving_safe_launch_gh_1047() {
    let mut env = TestEnvGuard::temp_home();
    let project = tempfile::tempdir().unwrap();
    let cas_root = project.path().join(".cas");
    let native_root = project.path().join(".codex");
    std::fs::create_dir_all(&cas_root).unwrap();
    std::fs::create_dir_all(&native_root).unwrap();
    let native_config = "[mcp_servers.context7]\ncommand = 'never-executed-fixture'\n[mcp_servers.context7.env]\nDENIED_LITERAL_FIXTURE_TOKEN = 'literal-fixture'\n[plugins.unknown_fixture]\nenabled = true\n";
    std::fs::write(native_root.join("config.toml"), native_config).unwrap();
    env.set("DENIED_LITERAL_FIXTURE_TOKEN", "parent-fixture");
    for (policy, env_restricted, server_restricted) in [
        ("[factory]\n", false, false),
        ("[factory]\nsupervisor_only_env = []\n", false, false),
        ("[factory]\nsupervisor_only_mcp = ['vercel']\n", false, true),
        ("[factory]\nsupervisor_only_env = ['DENIED_LITERAL_FIXTURE_TOKEN']\n", true, false),
        ("[factory]\nsupervisor_only_mcp = ['vercel']\nsupervisor_only_env = ['DENIED_LITERAL_FIXTURE_TOKEN']\n", true, true),
    ] {
        std::fs::write(cas_root.join("config.toml"), policy).unwrap();
        let config = MuxConfig {
            cwd: project.path().to_path_buf(),
            cas_root: Some(cas_root.clone()),
            workers: 1,
            worker_cli: SupervisorCli::Codex,
            supervisor_cli: SupervisorCli::Codex,
            include_director: false,
            ..Default::default()
        };
        let configs = Mux::factory_pane_configs(&config);
        let worker = &configs.iter().find(|(name, _)| name == "worker-1").unwrap().1;
        let supervisor = &configs.iter().find(|(name, _)| name == &config.supervisor_name).unwrap().1;
        assert_eq!(worker.effective_command(), "codex");
        assert_eq!(supervisor.effective_command(), "codex");
        assert!(env_value(supervisor, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
        assert!(!supervisor.env_remove.iter().any(|key| key == "DENIED_LITERAL_FIXTURE_TOKEN"));
        assert!(env_value(worker, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
        assert_eq!(worker.args.iter().any(|arg| arg == "mcp_servers.context7.enabled=false"), env_restricted);
        assert_eq!(worker.args.iter().any(|arg| arg == "mcp_servers.vercel.enabled=false"), server_restricted);
        if env_restricted {
            assert!(worker.env_remove.iter().any(|key| key == "DENIED_LITERAL_FIXTURE_TOKEN"));
            assert!(env_value(worker, "DENIED_LITERAL_FIXTURE_TOKEN").is_none());
        }
        assert_eq!(std::fs::read_to_string(native_root.join("config.toml")).unwrap(), native_config);
        assert!(std::env::var_os("DENIED_LITERAL_FIXTURE_TOKEN").is_some());
    }
}

/// The real PTY command builder must remove inherited AND explicitly granted
/// credentials for both supported harnesses without launching a live harness.
#[test]
fn codex_env_denials_read_selected_home_and_nested_project_layers_gh_1047() {
    let mut env = TestEnvGuard::temp_home();
    env.remove("CODEX_HOME");
    let project = tempfile::tempdir().unwrap();
    let cas_root = project.path().join(".cas");
    let nested = project.path().join("nested");
    let account = project.path().join("account");
    for path in [&cas_root, &project.path().join(".codex"), &nested.join(".codex"),
        &env.home().join(".codex"), &account] {
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::write(cas_root.join("config.toml"),
        "[factory]\nsupervisor_only_env = ['DENIED_LITERAL_FIXTURE_TOKEN']\n").unwrap();
    for (path, source) in [
        (env.home().join(".codex/config.toml"),
            "[mcp_servers.default_env.env]\nDENIED_LITERAL_FIXTURE_TOKEN = 'default-fixture'\n"),
        (account.join("config.toml"),
            "[mcp_servers.account_env.env]\nDENIED_LITERAL_FIXTURE_TOKEN = 'account-fixture'\n"),
        (project.path().join(".codex/config.toml"),
            "[mcp_servers.project_env.env]\nDENIED_LITERAL_FIXTURE_TOKEN = 'parent-fixture'\n"),
        (nested.join(".codex/config.toml"),
            "[mcp_servers.project_env.env]\nALLOWED_FIXTURE_TOKEN = 'child-fixture'\n\
             [mcp_servers.safe.env]\nALLOWED_FIXTURE_TOKEN = 'safe-fixture'\n\
             [plugins.unknown_fixture]\nenabled = true\n"),
    ] {
        std::fs::write(path, source).unwrap();
    }
    for alternate in [false, true] {
        let mut worker = crate::pty::PtyConfig {
            command: "codex".into(),
            cwd: Some(nested.clone()),
            ..Default::default()
        };
        if alternate {
            worker.env.push(("CODEX_HOME".into(), account.display().to_string()));
        }
        crate::backend::finish_worker_config(
            &mut worker, SupervisorCli::Codex, None, None, Some(&cas_root),
        );
        assert!(env_value(&worker, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
        for (server, disabled) in [
            ("project_env", true), ("safe", false),
            ("default_env", !alternate), ("account_env", alternate),
        ] {
            assert_eq!(worker.args.iter().any(|arg|
                arg == &format!("mcp_servers.{server}.enabled=false")), disabled);
        }
    }
    // Unreadable/invalid/unknown contributions warn rather than refusing the
    // whole worker. No parser error or credential value reaches launch errors.
    std::fs::write(account.join("config.toml"), "invalid = [ 'secret-fixture'").unwrap();
    let mut worker = crate::pty::PtyConfig {
        command: "codex".into(), cwd: Some(nested),
        env: vec![("CODEX_HOME".into(), account.display().to_string())],
        ..Default::default()
    };
    crate::backend::finish_worker_config(
        &mut worker, SupervisorCli::Codex, None, None, Some(&cas_root),
    );
    assert!(env_value(&worker, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
    assert!(worker.args.iter().any(|arg| arg == "mcp_servers.project_env.enabled=false"));
    assert!(worker.args.iter().all(|arg| !arg.contains("secret-fixture")));
}

/// The actual PTY boundary removes denied process env for both native adapters.
#[tokio::test]
async fn supervisor_only_env_is_absent_in_spawned_worker_process_gh_1047() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_CLAUDE_SUPPORTED_FLAGS", "--mcp-config,--strict-mcp-config");
    let project = tempfile::tempdir().unwrap();
    let cas_root = project.path().join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    std::fs::write(cas_root.join("config.toml"), "[factory]\nsupervisor_only_mcp = [\"vercel\", \"neon\"]\nsupervisor_only_env = [\"VERCEL_TOKEN\", \"NEON_API_KEY\", \"DEPLOY_FIXTURE_TOKEN\"]\n").unwrap();
    std::fs::write(cas_root.join("proxy.toml"), "[servers.context7]\ntransport = 'http'\nauth = 'env:DEPLOY_FIXTURE_TOKEN'\n").unwrap();
    let source = project.path().join(".mcp.json");
    let original = serde_json::json!({"mcpServers": {
        "vercel": {"command": "vercel-fixture"},
        "neon": {"command": "neon-fixture"}
    }}).to_string();
    std::fs::write(&source, &original).unwrap();
    for key in ["VERCEL_TOKEN", "NEON_API_KEY", "DEPLOY_FIXTURE_TOKEN"] {
        env.set(key, "inherited-fixture");
    }
    for cli in [SupervisorCli::Claude, SupervisorCli::Codex] {
        let configs = Mux::factory_pane_configs(&MuxConfig {
            cwd: project.path().into(), cas_root: Some(cas_root.clone()), workers: 1,
            worker_cli: cli, include_director: false, ..Default::default()
        });
        let mut worker = configs.into_iter().find(|(name, _)| name == "worker-1").unwrap().1;
        assert!(env_value(&worker, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
        let worker_mcp = cas_root.join("worker-mcp/worker-1.json");
        let mcp: serde_json::Value = serde_json::from_slice(&std::fs::read(worker_mcp).unwrap()).unwrap();
        assert!(mcp["mcpServers"].get("vercel").is_none() && mcp["mcpServers"].get("neon").is_none());
        // Replace only the harness executable with a local probe, retaining
        // the generated environment and the production Pty::spawn path.
        worker.command = "/bin/sh".into();
        worker.args = vec!["-c".into(), "test -z \"${VERCEL_TOKEN+x}${NEON_API_KEY+x}${DEPLOY_FIXTURE_TOKEN+x}\" && printf 'worker-env-isolated\\n'".into()];
        let mut pty = crate::pty::Pty::spawn("resource-probe", worker).unwrap();
        let events = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut output = Vec::new();
            loop {
                match pty.recv().await {
                    Some(crate::pty::PtyEvent::Output(bytes)) => output.extend(bytes),
                    Some(crate::pty::PtyEvent::Exited(code)) => { assert_eq!(code, Some(0)); break; }
                    Some(crate::pty::PtyEvent::Error(error)) => panic!("{error}"),
                    None => panic!("PTY ended without process status"),
                }
            }
            output
        }).await.expect("bounded local probe");
        assert!(String::from_utf8_lossy(&events).contains("worker-env-isolated"));
    }
    assert_eq!(std::fs::read_to_string(source).unwrap(), original, "shared-cwd workers keep supervisor configuration intact");
    for key in ["VERCEL_TOKEN", "NEON_API_KEY", "DEPLOY_FIXTURE_TOKEN"] {
        assert!(std::env::var_os(key).is_some());
    }
}

/// Environment restrictions must preserve supported Codex admission.
/// Execute only a local shell probe through the admitted config's PTY boundary;
/// actual native Codex discovery remains a separate supervisor demo.
#[tokio::test]
async fn codex_unrestricted_and_server_only_admission_preserves_spawn_gh_1047() {
    let _env = TestEnvGuard::temp_home();
    let project = tempfile::tempdir().unwrap();
    let cas_root = project.path().join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    for (policy, server_only) in [
        ("[factory]\n", false),
        ("[factory]\nsupervisor_only_mcp = ['vercel']\nsupervisor_only_env = []\n", true),
    ] {
        std::fs::write(cas_root.join("config.toml"), policy).unwrap();
        let configs = Mux::factory_pane_configs(&MuxConfig {
            cwd: project.path().to_path_buf(), cas_root: Some(cas_root.clone()),
            workers: 1, worker_cli: SupervisorCli::Codex,
            include_director: false, ..Default::default()
        });
        let mut worker = configs.into_iter().find(|(name, _)| name == "worker-1").unwrap().1;
        assert_eq!(worker.effective_command(), "codex");
        assert!(env_value(&worker, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
        assert_eq!(worker.args.iter().any(|arg| arg == "mcp_servers.vercel.enabled=false"), server_only);
        worker.env.push(("CAS_GH1047_SAFE_LAUNCH_FIXTURE".into(), "allowed-fixture".into()));
        worker.command = "/bin/sh".into();
        worker.args = vec!["-c".into(), "test \"$CAS_GH1047_SAFE_LAUNCH_FIXTURE\" = allowed-fixture && printf 'safe-codex-admission\\n'".into()];
        let mut pty = crate::pty::Pty::spawn("safe-codex-local-probe", worker).unwrap();
        let output = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut output = Vec::new();
            loop {
                match pty.recv().await {
                    Some(crate::pty::PtyEvent::Output(bytes)) => output.extend(bytes),
                    Some(crate::pty::PtyEvent::Exited(code)) => { assert_eq!(code, Some(0)); break; }
                    Some(crate::pty::PtyEvent::Error(error)) => panic!("{error}"),
                    None => panic!("PTY ended without process status"),
                }
            }
            output
        }).await.expect("bounded local safe-admission probe");
        assert!(String::from_utf8_lossy(&output).contains("safe-codex-admission"));
    }
}

#[test]
fn invalid_worker_resource_policy_refuses_launch_before_execution_gh_1047() {
    let _env = TestEnvGuard::temp_home();
    let project = tempfile::tempdir().unwrap();
    let cas_root = project.path().join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    for policy in [
        "[factory]\nsupervisor_only_mcp = 'not-a-list'\n",
        "[factory]\nsupervisor_only_mcp = ['vercel']\n",
    ] {
        std::fs::write(cas_root.join("config.toml"), policy).unwrap();
        std::fs::write(project.path().join(".mcp.json"), "invalid JSON fixture").unwrap();
        let configs = Mux::factory_pane_configs(&MuxConfig {
            cwd: project.path().into(), cas_root: Some(cas_root.clone()), workers: 1,
            include_director: false, ..Default::default()
        });
        let mut worker = configs.into_iter().find(|(name, _)| name == "worker-1").unwrap().1;
        worker.command = "/bin/sh".into();
        worker.args = vec!["-c".into(), "exit 0".into()];
        assert!(crate::pty::Pty::spawn("refused-worker", worker).is_err());
    }
    std::fs::write(cas_root.join("config.toml"), "[factory]\nsupervisor_only_mcp = ['server.with.dots']\n").unwrap();
    std::fs::write(project.path().join(".mcp.json"), "{}").unwrap();
    let configs = Mux::factory_pane_configs(&MuxConfig {
        cwd: project.path().into(), cas_root: Some(cas_root), workers: 1,
        worker_cli: SupervisorCli::Codex, include_director: false, ..Default::default()
    });
    let worker = configs.into_iter().find(|(name, _)| name == "worker-1").unwrap().1;
    assert!(env_value(&worker, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
    let override_arg = worker.args.iter()
        .find(|arg| arg.starts_with("mcp_servers={"))
        .expect("dotted name must use a parent table override");
    let overrides: toml::Value = toml::from_str(override_arg).unwrap();
    assert_eq!(overrides["mcp_servers"]["server.with.dots"]["enabled"].as_bool(), Some(false));
    assert!(overrides["mcp_servers"].get("server").is_none());
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[test]
fn factory_pane_configs_propagates_configured_proxy_credentials() {
    let mut env = TestEnvGuard::temp_home();
    let config_home = tempfile::tempdir().expect("temporary config home");
    let config_path = config_home.path().join("code-mode-mcp/config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(
        &config_path,
        r#"
[servers.violet]
transport = "http"
auth = "env:VIOLET_SLACK_TOKEN_TEST_WORKER"

[servers.violet.headers]
x-vercel-protection-bypass = "env:VIOLET_VERCEL_BYPASS_TEST_WORKER"

[servers.unrelated]
auth = "env:UNRELATED_CREDENTIAL_MUST_NOT_PROPAGATE"
"#,
    )
    .unwrap();
    env.set("XDG_CONFIG_HOME", config_home.path());
    env.set("VIOLET_SLACK_TOKEN_TEST_WORKER", "token-value");
    env.set("VIOLET_VERCEL_BYPASS_TEST_WORKER", "bypass-value");
    env.set("UNRELATED_CREDENTIAL_MUST_NOT_PROPAGATE", "unrelated");

    violet_registrations(
        env.home(),
        "VIOLET_SLACK_TOKEN_TEST_WORKER",
        Some("VIOLET_VERCEL_BYPASS_TEST_WORKER"),
    );
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        include_director: false,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);
    let (_, worker_config) = configs
        .iter()
        .find(|(name, _)| name == "worker-1")
        .expect("worker config must be present");

    assert_eq!(
        env_value(worker_config, "VIOLET_SLACK_TOKEN_TEST_WORKER"),
        Some("token-value")
    );
    assert_eq!(
        env_value(worker_config, "VIOLET_VERCEL_BYPASS_TEST_WORKER"),
        Some("bypass-value")
    );
    assert_eq!(
        env_value(worker_config, "UNRELATED_CREDENTIAL_MUST_NOT_PROPAGATE"),
        Some("unrelated"),
        "every configured upstream credential reference must reach the worker proxy"
    );
}

#[test]
fn factory_pane_configs_propagates_project_proxy_credentials() {
    let mut env = TestEnvGuard::temp_home();
    let config_home = tempfile::tempdir().expect("temporary config home");
    env.set("XDG_CONFIG_HOME", config_home.path());
    let project = tempfile::tempdir().expect("temporary project root");
    let cas_root = project.path().join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    std::fs::write(
        cas_root.join("proxy.toml"),
        r#"
[servers.neon]
transport = "http"
auth = "env:NEON_API_KEY_TEST_WORKER"
"#,
    )
    .unwrap();
    env.set("NEON_API_KEY_TEST_WORKER", "neon-value");

    let config = MuxConfig {
        cwd: project.path().to_path_buf(),
        cas_root: Some(cas_root),
        workers: 1,
        include_director: false,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);
    let (_, worker_config) = configs
        .iter()
        .find(|(name, _)| name == "worker-1")
        .expect("worker config must be present");

    assert_eq!(
        env_value(worker_config, "NEON_API_KEY_TEST_WORKER"),
        Some("neon-value")
    );
}

#[test]
fn factory_panes_forward_both_violet_credential_generations() {
    let mut env = TestEnvGuard::temp_home();
    let config_home = tempfile::tempdir().unwrap();
    let path = config_home.path().join("code-mode-mcp/config.toml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    env.set("XDG_CONFIG_HOME", config_home.path());
    env.set("HOME", config_home.path());
    violet_registrations(
        config_home.path(),
        "VIOLET_SLACK_TOKEN_COMPAT_TEST",
        Some("VIOLET_VERCEL_BYPASS"),
    );
    let contract = cas_types::violet_compatibility::violet_compatibility();
    let tokens =
        cas_types::violet_compatibility::violet_credential_names("VIOLET_SLACK_TOKEN_COMPAT_TEST");
    env.set(&tokens[1], "legacy-token");
    env.set(&contract.legacy_bypass_env, "legacy-bypass");
    for token in &tokens {
        let bypass = if token == &tokens[0] {
            "VIOLET_VERCEL_BYPASS"
        } else {
            &contract.legacy_bypass_env
        };
        env.set("VIOLET_SLACK_TOKEN_COMPAT_TEST", "new-token");
        env.set("VIOLET_VERCEL_BYPASS", "new-bypass");
        std::fs::write(&path, format!("[servers.violet]\ntransport = \"http\"\nauth = \"env:{token}\"\n[servers.violet.headers]\nx-vercel-protection-bypass = \"env:{bypass}\"\n")).unwrap();
        let config = MuxConfig {
            cwd: PathBuf::from("/tmp/test"),
            workers: 1,
            include_director: false,
            ..MuxConfig::default()
        };
        let configs = Mux::factory_pane_configs(&config);
        let (_, pane) = configs.iter().find(|(name, _)| name == "worker-1").unwrap();
        assert_eq!(env_value(pane, &tokens[1]), Some("legacy-token"));
        assert_eq!(
            env_value(pane, "VIOLET_SLACK_TOKEN_COMPAT_TEST"),
            Some("new-token")
        );
        assert_eq!(
            env_value(pane, &contract.legacy_bypass_env),
            Some("legacy-bypass")
        );
        assert_eq!(env_value(pane, "VIOLET_VERCEL_BYPASS"), Some("new-bypass"));
        env.remove("VIOLET_SLACK_TOKEN_COMPAT_TEST");
        env.remove("VIOLET_VERCEL_BYPASS");
        let configs = Mux::factory_pane_configs(&config);
        let (_, pane) = configs.iter().find(|(name, _)| name == "worker-1").unwrap();
        assert_eq!(env_value(pane, &tokens[1]), Some("legacy-token"));
        assert_eq!(
            env_value(pane, &contract.legacy_bypass_env),
            Some("legacy-bypass")
        );
        assert_eq!(env_value(pane, "VIOLET_SLACK_TOKEN_COMPAT_TEST"), None);
        assert_eq!(env_value(pane, "VIOLET_VERCEL_BYPASS"), None);
    }
}

/// cas-ff74 (GH #1005 item 2): a server defined only in the operator's user
/// config (VERCEL_TOKEN) reaches a worker when, and only when, the project
/// proxy.toml marks it `worker_access = "read-only"`. The worker's proxy then
/// forwards only its read routes.
#[test]
fn read_only_worker_access_grants_the_user_level_server_credential() {
    let mut env = TestEnvGuard::temp_home();
    let config_home = tempfile::tempdir().expect("temporary config home");
    let global_config = config_home.path().join("code-mode-mcp/config.toml");
    std::fs::create_dir_all(global_config.parent().unwrap()).unwrap();
    std::fs::write(
        &global_config,
        r#"
[servers.vercel]
transport = "http"
url = "https://mcp.vercel.com"
auth = "env:VERCEL_TOKEN"

[servers.neon]
transport = "http"
url = "https://mcp.neon.tech/mcp"
auth = "env:NEON_API_KEY"
"#,
    )
    .unwrap();
    let home = tempfile::tempdir().expect("temporary home");
    env.set("HOME", home.path());
    env.set("XDG_CONFIG_HOME", config_home.path());
    env.remove("CAS_CREDENTIALS_FILE");
    env.set("VERCEL_TOKEN", "vercel-fixture");
    env.set("NEON_API_KEY", "neon-fixture");

    let worker_config_for = |proxy_toml: &str| {
        let project = tempfile::tempdir().expect("temporary project root");
        let cas_root = project.path().join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();
        std::fs::write(cas_root.join("proxy.toml"), proxy_toml).unwrap();
        let config = MuxConfig {
            cwd: project.path().to_path_buf(),
            cas_root: Some(cas_root),
            workers: 1,
            worker_names: vec!["worker-1".to_string()],
            include_director: false,
            ..MuxConfig::default()
        };
        let configs = Mux::factory_pane_configs(&config);
        let (_, worker) = configs
            .into_iter()
            .find(|(name, _)| name == "worker-1")
            .expect("worker config must be present");
        worker
    };
    let stripped = |worker: &crate::pty::PtyConfig, key: &str| {
        env_value(worker, key).is_none()
            && worker.env_remove.iter().any(|candidate| candidate == key)
    };

    // Marked read-only: the user-level VERCEL_TOKEN is granted, NEON is not.
    let granted = worker_config_for(
        r#"
allowlist = ["vercel.get_runtime_errors"]

[worker_access]
vercel = "read-only"
"#,
    );
    assert_eq!(env_value(&granted, "VERCEL_TOKEN"), Some("vercel-fixture"));
    assert!(
        !granted
            .env_remove
            .iter()
            .any(|candidate| candidate == "VERCEL_TOKEN"),
        "a read-only grant must survive the worker credential policy"
    );
    assert!(
        stripped(&granted, "NEON_API_KEY"),
        "an unmarked user-level server stays stripped"
    );

    // Not marked: the same user-level credential stays stripped.
    let ungranted = worker_config_for(
        r#"
allowlist = ["vercel.get_runtime_errors"]
"#,
    );
    assert!(stripped(&ungranted, "VERCEL_TOKEN"));
    assert!(stripped(&ungranted, "NEON_API_KEY"));
}

/// Stock worker launches strip the operator's Neon API key while keeping
/// the task database file readable; no Neon service is contacted.
#[tokio::test]
async fn default_workers_keep_task_database_access_without_the_neon_api_key() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_CLAUDE_SUPPORTED_FLAGS", "--mcp-config,--strict-mcp-config");
    let project = tempfile::tempdir().unwrap();
    let cas_root = project.path().join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    env.set("XDG_CONFIG_HOME", project.path());
    env.remove("CAS_CREDENTIALS_FILE");
    env.remove("DATABASE_URL");
    env.set("NEON_API_KEY", "operator-neon-fixture");
    std::fs::write(project.path().join(".env.cas-db"),
        "DATABASE_URL=postgresql://task-db.invalid/task\n").unwrap();
    for cli in [SupervisorCli::Claude, SupervisorCli::Codex, SupervisorCli::Grok, SupervisorCli::OpenCode] {
        let config = MuxConfig {
            cwd: project.path().into(), cas_root: Some(cas_root.clone()), workers: 1,
            worker_cli: cli, include_director: false, ..Default::default()
        };
        let configs = Mux::factory_pane_configs(&config);
        let supervisor = &configs.iter().find(|(name, _)| name == &config.supervisor_name).unwrap().1;
        assert!(!supervisor.env_remove.iter().any(|name| name == "NEON_API_KEY"));
        let mut worker = configs.into_iter().find(|(name, _)| name == "worker-1").unwrap().1;
        assert!(env_value(&worker, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none(),
            "{cli:?}: {:?}", env_value(&worker, "CAS_FACTORY_WORKER_LAUNCH_ERROR"));
        assert!(env_value(&worker, "NEON_API_KEY").is_none());
        assert!(worker.env_remove.iter().any(|name| name == "NEON_API_KEY"));
        assert!(!worker.env_remove.iter().any(|name| name == "DATABASE_URL"));
        // Keep the actual environment/spawn path, replacing only the harness
        // executable with a bounded local probe that sources its own DB file.
        worker.command = "/bin/sh".into();
        worker.args = vec!["-c".into(),
            "test -z \"${NEON_API_KEY+x}\" && . ./.env.cas-db && test \"$DATABASE_URL\" = postgresql://task-db.invalid/task && printf 'task-db-only\\n'".into()];
        let mut pty = crate::pty::Pty::spawn("default-neon-policy-probe", worker).unwrap();
        let output = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut output = Vec::new();
            loop {
                match pty.recv().await {
                    Some(crate::pty::PtyEvent::Output(bytes)) => output.extend(bytes),
                    Some(crate::pty::PtyEvent::Exited(code)) => { assert_eq!(code, Some(0)); break; }
                    Some(crate::pty::PtyEvent::Error(error)) => panic!("{error}"),
                    None => panic!("PTY ended without process status"),
                }
            }
            output
        }).await.expect("bounded task database probe");
        assert!(String::from_utf8_lossy(&output).contains("task-db-only"));
    }
    assert_eq!(std::env::var("NEON_API_KEY").unwrap(), "operator-neon-fixture");
}

#[test]
fn factory_worker_configs_isolate_operator_credentials_for_every_harness() {
    let mut env = TestEnvGuard::temp_home();
    let home = tempfile::tempdir().expect("temporary home");
    let config_home = tempfile::tempdir().expect("temporary config home");
    let global_config = config_home.path().join("code-mode-mcp/config.toml");
    std::fs::create_dir_all(global_config.parent().unwrap()).unwrap();
    std::fs::write(
        &global_config,
        r#"
[servers.machine-global]
transport = "http"
auth = "env:GITHUB_TOKEN"
"#,
    )
    .unwrap();
    let project = tempfile::tempdir().expect("temporary project root");
    let cas_root = project.path().join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    std::fs::write(
        cas_root.join("proxy.toml"),
        r#"
[servers.scoped-provider]
transport = "http"
auth = "env:NEON_API_KEY"

[servers.scoped-context7]
transport = "http"
auth = "env:CONTEXT7_API_KEY"
"#,
    )
    .unwrap();
    env.set("HOME", home.path());
    env.set("XDG_CONFIG_HOME", config_home.path());
    env.remove("CAS_CREDENTIALS_FILE");
    env.remove("CAPAWESOME_TOKEN");
    env.remove("CAS_CLOUD_TOKEN");
    env.remove("CONTEXT7_API_KEY");
    env.remove("GH_TOKEN");
    env.set("GITHUB_TOKEN", "inherited-fixture");
    env.set("NEON_API_KEY", "scoped-fixture");
    env.set("CONTEXT7_API_KEY", "scoped-fixture");
    env.remove("VERCEL_TOKEN");
    env.remove("BROWSERLESS_API_KEY");

    for worker_cli in [
        SupervisorCli::Claude,
        SupervisorCli::Codex,
        SupervisorCli::Grok,
        SupervisorCli::OpenCode,
    ] {
        let config = MuxConfig {
            cwd: project.path().to_path_buf(),
            cas_root: Some(cas_root.clone()),
            workers: 1,
            worker_names: vec!["worker-1".to_string()],
            worker_cli,
            include_director: false,
            ..MuxConfig::default()
        };
        let configs = Mux::factory_pane_configs(&config);
        let (_, worker_config) = configs
            .iter()
            .find(|(name, _)| name == "worker-1")
            .expect("worker config must be present");

        assert_eq!(
            env_value(worker_config, "CAS_AGENT_ROLE"),
            Some("worker"),
            "every harness worker path must stamp its role before policy evaluation"
        );
        assert_eq!(env_value(worker_config, "CAS_FACTORY_MODE"), Some("1"));
        for key in ["NEON_API_KEY", "CONTEXT7_API_KEY"] {
            assert!(
                env_value(worker_config, key).is_some(),
                "explicit project credential grant must survive for each harness"
            );
        }
        for key in cas_pty::PROTECTED_OPERATOR_ENV {
            if ["NEON_API_KEY", "CONTEXT7_API_KEY"].contains(key) {
                assert!(
                    !worker_config
                        .env_remove
                        .iter()
                        .any(|candidate| candidate == key)
                );
            } else {
                assert!(
                    worker_config
                        .env_remove
                        .iter()
                        .any(|candidate| candidate == key)
                );
                assert!(
                    !worker_config
                        .env
                        .iter()
                        .any(|(candidate, _)| candidate == key)
                );
            }
        }

        let (_, supervisor_config) = configs
            .iter()
            .find(|(name, _)| name == &config.supervisor_name)
            .expect("supervisor config must be present");
        for key in cas_pty::PROTECTED_OPERATOR_ENV {
            assert!(
                !supervisor_config
                    .env_remove
                    .iter()
                    .any(|candidate| candidate == key)
            );
        }

        for role in [None, Some("operator")] {
            let mut unstamped_worker = worker_config.clone();
            unstamped_worker.env_remove.clear();
            unstamped_worker
                .env
                .retain(|(candidate, _)| candidate != "CAS_AGENT_ROLE");
            if let Some(role) = role {
                unstamped_worker
                    .env
                    .push(("CAS_AGENT_ROLE".to_string(), role.to_string()));
            }
            unstamped_worker.apply_worker_credential_policy();
            for key in cas_pty::PROTECTED_OPERATOR_ENV {
                if ["NEON_API_KEY", "CONTEXT7_API_KEY"].contains(key) {
                    assert!(
                        !unstamped_worker
                            .env_remove
                            .iter()
                            .any(|candidate| candidate == key)
                    );
                } else {
                    assert!(
                        unstamped_worker
                            .env_remove
                            .iter()
                            .any(|candidate| candidate == key)
                    );
                }
            }
        }
    }
}

/// cas-ea9c item 1 (GH #1005): an operator-provisioned read-only token
/// reaches every worker as its only GitHub credential. The operator's own
/// GH_TOKEN/GITHUB_TOKEN never do, and without the read token nothing
/// changes.
#[test]
fn worker_gets_the_read_only_github_token_and_never_the_operators() {
    let mut env = TestEnvGuard::temp_home();
    let home = tempfile::tempdir().expect("temporary home");
    let project = tempfile::tempdir().expect("temporary project root");
    let cas_root = project.path().join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    env.set("HOME", home.path());
    env.set("XDG_CONFIG_HOME", home.path());
    env.remove("CAS_CREDENTIALS_FILE");
    env.set("GH_TOKEN", "operator-write-token");
    env.set("GITHUB_TOKEN", "operator-write-token");
    let config = MuxConfig {
        cwd: project.path().to_path_buf(),
        cas_root: Some(cas_root),
        workers: 1,
        include_director: false,
        worker_cli: SupervisorCli::Claude,
        ..MuxConfig::default()
    };
    let worker = |config: &MuxConfig| {
        Mux::factory_pane_configs(config)
            .into_iter()
            .find(|(name, _)| name == "worker-1")
            .expect("worker config must be present")
            .1
    };
    let env_value = |pty: &cas_pty::PtyConfig, key: &str| {
        pty.env
            .iter()
            .rev()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.clone())
    };
    let removed = |pty: &cas_pty::PtyConfig, key: &str| pty.env_remove.iter().any(|k| k == key);

    // Without a read token, workers get no GitHub credential at all.
    let plain = worker(&config);
    assert_eq!(env_value(&plain, "GH_TOKEN"), None);
    assert!(removed(&plain, "GH_TOKEN") && removed(&plain, "GITHUB_TOKEN"));

    env.set(crate::WORKER_GITHUB_READ_TOKEN_ENV, "github_pat_readonly");
    let granted = worker(&config);
    assert_eq!(
        env_value(&granted, "GH_TOKEN").as_deref(),
        Some("github_pat_readonly"),
        "the worker's gh authenticates with the read-only token"
    );
    assert!(
        !removed(&granted, "GH_TOKEN"),
        "the explicit read token survives spawn"
    );
    assert!(
        removed(&granted, "GITHUB_TOKEN"),
        "the operator's GITHUB_TOKEN stays stripped"
    );
    assert!(
        removed(&granted, crate::WORKER_GITHUB_READ_TOKEN_ENV),
        "the provisioning variable itself is not passed on"
    );
    assert!(
        !granted
            .env
            .iter()
            .any(|(_, value)| value == "operator-write-token"),
        "the operator's token never reaches the worker"
    );
}

#[tokio::test]
async fn machine_global_protected_proxy_credentials_do_not_reach_worker_descendants() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_CLAUDE_SUPPORTED_FLAGS", "--mcp-config,--strict-mcp-config");
    let home = tempfile::tempdir().expect("temporary home");
    let config_home = tempfile::tempdir().expect("temporary config home");
    let global_config = config_home.path().join("code-mode-mcp/config.toml");
    std::fs::create_dir_all(global_config.parent().unwrap()).unwrap();
    std::fs::write(
        &global_config,
        r#"
[servers.machine-global-github]
transport = "http"
auth = "env:GITHUB_TOKEN"

[servers.machine-global-neon]
transport = "http"
auth = "env:NEON_API_KEY"

[servers.machine-global-forged-grant]
transport = "http"
auth = "env:CAS_FACTORY_WORKER_CREDENTIAL_GRANT"
"#,
    )
    .unwrap();
    let project = tempfile::tempdir().expect("temporary project root");
    let cas_root = project.path().join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    env.set("HOME", home.path());
    env.set("XDG_CONFIG_HOME", config_home.path());
    env.remove("CAS_CREDENTIALS_FILE");
    env.set("GITHUB_TOKEN", "machine-global-fixture");
    env.set("NEON_API_KEY", "machine-global-fixture");
    env.set(
        "CAS_FACTORY_WORKER_CREDENTIAL_GRANT",
        "NEON_API_KEY:operator-fixture",
    );

    let config = MuxConfig {
        cwd: project.path().to_path_buf(),
        cas_root: Some(cas_root),
        workers: 1,
        include_director: false,
        worker_cli: SupervisorCli::Claude,
        ..MuxConfig::default()
    };
    let (_, mut worker_config) = Mux::factory_pane_configs(&config)
        .into_iter()
        .find(|(name, _)| name == "worker-1")
        .expect("worker config must be present");
    for key in ["GITHUB_TOKEN", "NEON_API_KEY"] {
        assert!(
            !worker_config
                .env
                .iter()
                .any(|(candidate, _)| candidate == key)
        );
        assert!(
            worker_config
                .env_remove
                .iter()
                .any(|candidate| candidate == key)
        );
    }

    let marker_root = project
        .path()
        .join("target")
        .join(format!("cas-b3e1-global-{}", std::process::id()));
    std::fs::create_dir_all(marker_root.parent().expect("marker parent")).unwrap();
    let worker_marker = marker_root.with_extension("worker");
    let descendant_marker = marker_root.with_extension("descendant");
    let worker_marker_shell = shell_quote(&worker_marker.to_string_lossy());
    let descendant_marker_shell = shell_quote(&descendant_marker.to_string_lossy());
    let script = format!(
        "if [ -n \"${{GITHUB_TOKEN:-}}\" ] || [ -n \"${{NEON_API_KEY:-}}\" ]; then printf present > {worker_marker_shell}; else printf clear > {worker_marker_shell}; fi; sh -c 'if [ -n \"${{GITHUB_TOKEN:-}}\" ] || [ -n \"${{NEON_API_KEY:-}}\" ]; then printf present > \"$1\"; else printf clear > \"$1\"; fi' sh {descendant_marker_shell}; sleep 1",
    );
    worker_config.command = "sh".to_string();
    worker_config.args = vec!["-c".to_string(), script];
    let mut pty = crate::pty::Pty::spawn("global-credential-probe", worker_config)
        .expect("global credential probe must spawn");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match pty.recv().await {
                Some(crate::pty::PtyEvent::Exited(_)) => break,
                Some(crate::pty::PtyEvent::Error(error)) => {
                    panic!("global credential probe failed: {error}")
                }
                Some(crate::pty::PtyEvent::Output(_)) => {}
                None => panic!("global credential probe channel closed"),
            }
        }
    })
    .await
    .expect("global credential probe must exit before deadline");
    pty.kill();

    assert_eq!(
        std::fs::read_to_string(&worker_marker).expect("worker probe marker"),
        "clear"
    );
    assert_eq!(
        std::fs::read_to_string(&descendant_marker).expect("descendant probe marker"),
        "clear"
    );
    let _ = std::fs::remove_file(worker_marker);
    let _ = std::fs::remove_file(descendant_marker);
}

#[test]
fn factory_pane_configs_reads_proxy_credentials_from_cas_credentials_file() {
    let mut env = TestEnvGuard::temp_home();
    let config_home = tempfile::tempdir().expect("temporary config home");
    env.set("XDG_CONFIG_HOME", config_home.path());
    // `CAS_CREDENTIALS_FILE` controls the primary credentials source, but the
    // resolver also scans the login profile as a fallback. Keep that profile
    // under the fixture too, so a developer's real credentials cannot replace
    // the file value asserted below.
    let home = tempfile::tempdir().expect("temporary home");
    env.set("HOME", home.path());
    let config_path = config_home.path().join("code-mode-mcp/config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(
        &config_path,
        r#"
[servers.violet]
transport = "http"
auth = "env:VIOLET_SLACK_TOKEN_CREDENTIALS_FILE"

[servers.violet.headers]
x-vercel-protection-bypass = "env:VIOLET_VERCEL_BYPASS"
"#,
    )
    .unwrap();
    let credentials = config_home.path().join("cas/credentials.env");
    std::fs::create_dir_all(credentials.parent().unwrap()).unwrap();
    std::fs::write(
        &credentials,
        "export VIOLET_SLACK_TOKEN_CREDENTIALS_FILE='token-from-file'\nexport VIOLET_VERCEL_BYPASS='bypass-from-file'\n",
    )
    .unwrap();
    private_permissions(&credentials);
    violet_registrations(
        home.path(),
        "VIOLET_SLACK_TOKEN_CREDENTIALS_FILE",
        Some("VIOLET_VERCEL_BYPASS"),
    );
    env.set("CAS_CREDENTIALS_FILE", &credentials);
    env.remove("VIOLET_SLACK_TOKEN_CREDENTIALS_FILE");
    env.remove("VIOLET_VERCEL_BYPASS");

    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        include_director: false,
        worker_cli: SupervisorCli::Codex,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);
    let (_, worker_config) = configs
        .iter()
        .find(|(name, _)| name == "worker-1")
        .expect("worker config must be present");

    assert_eq!(
        env_value(worker_config, "VIOLET_SLACK_TOKEN_CREDENTIALS_FILE"),
        Some("token-from-file")
    );
    assert_eq!(
        env_value(worker_config, "VIOLET_VERCEL_BYPASS"),
        Some("bypass-from-file")
    );
    let credentials_arg = format!(
        "mcp_servers.cs.env.CAS_CREDENTIALS_FILE={}",
        serde_json::to_string(&credentials.to_string_lossy()).unwrap()
    );
    assert!(
        worker_config.args.contains(&credentials_arg),
        "Codex's restricted MCP environment must receive the credentials path; args={:?}",
        worker_config.args
    );
}

#[test]
fn factory_pane_configs_requires_private_file_for_canonical_violet() {
    let mut env = TestEnvGuard::temp_home();
    let config_home = tempfile::tempdir().expect("temporary config home");
    env.set("XDG_CONFIG_HOME", config_home.path());
    let home = tempfile::tempdir().expect("temporary home");
    env.set("HOME", home.path());
    env.set("SHELL", "/bin/bash");
    env.remove("CAS_CREDENTIALS_FILE");
    let config_path = config_home.path().join("code-mode-mcp/config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(
        &config_path,
        r#"
[servers.violet]
transport = "http"
auth = "env:VIOLET_SLACK_TOKEN_PROFILE_SOURCE"

[servers.violet.headers]
x-vercel-protection-bypass = "env:VIOLET_VERCEL_BYPASS_PROFILE_SOURCE"
"#,
    )
    .unwrap();
    let credentials = home.path().join("private/violet.env");
    std::fs::create_dir_all(credentials.parent().unwrap()).unwrap();
    std::fs::write(
        &credentials,
        "export VIOLET_SLACK_TOKEN_PROFILE_SOURCE='token-from-profile'\nexport VIOLET_VERCEL_BYPASS_PROFILE_SOURCE='bypass-from-profile'\n",
    )
    .unwrap();
    private_permissions(&credentials);
    violet_registrations(
        home.path(),
        "VIOLET_SLACK_TOKEN_PROFILE_SOURCE",
        Some("VIOLET_VERCEL_BYPASS_PROFILE_SOURCE"),
    );
    std::fs::write(
        home.path().join(".profile"),
        format!(
            "[ -f '{}' ] && . '{}'\n",
            credentials.display(),
            credentials.display()
        ),
    )
    .unwrap();
    env.remove("VIOLET_SLACK_TOKEN_PROFILE_SOURCE");
    env.remove("VIOLET_VERCEL_BYPASS_PROFILE_SOURCE");

    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        include_director: false,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);
    let (_, initial_worker) = configs.iter().find(|(name, _)| name == "worker-1").unwrap();
    assert_eq!(env_value(initial_worker, "VIOLET_SLACK_TOKEN_PROFILE_SOURCE"), None);
    // Canonical Violet launches do not depend on whether a login shell ran.
    env.set("CAS_CREDENTIALS_FILE", &credentials);
    let configs = Mux::factory_pane_configs(&config);
    let (_, worker_config) = configs
        .iter()
        .find(|(name, _)| name == "worker-1")
        .expect("worker config must be present");

    assert_eq!(
        env_value(worker_config, "VIOLET_SLACK_TOKEN_PROFILE_SOURCE"),
        Some("token-from-profile")
    );
    assert_eq!(
        env_value(worker_config, "VIOLET_VERCEL_BYPASS_PROFILE_SOURCE"),
        Some("bypass-from-profile")
    );
}

#[test]
fn factory_pane_configs_codex_worker_home_is_existing_host_home() {
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        include_director: false,
        supervisor_cli: SupervisorCli::Claude,
        worker_cli: SupervisorCli::Codex,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);
    let (_, worker_config) = configs
        .iter()
        .find(|(name, _)| name == "worker-1")
        .expect("worker config must be present");

    let home = spawned_env(worker_config, "HOME").expect("Codex worker HOME must be set");
    assert!(
        std::path::Path::new(home).is_dir(),
        "Codex worker HOME must exist on the host: {home}"
    );
    let profile = spawned_env(worker_config, "PLAYWRIGHT_MCP_USER_DATA_DIR")
        .expect("Codex worker Playwright profile must be set");
    assert_eq!(
        std::path::Path::new(profile).parent(),
        Some(std::path::Path::new(home)),
        "Playwright profile must be rooted under worker HOME: {profile}"
    );
}

fn codex_factory_session_arg(config: &crate::pty::PtyConfig) -> Option<&str> {
    config
        .args
        .iter()
        .find(|arg| arg.starts_with("mcp_servers.cs.env.CAS_FACTORY_SESSION="))
        .map(String::as_str)
}

// ── cas-d571: effort config flows through Mux::factory() to PTY args ─────────
// Tests the full MuxConfig → Mux::factory_pane_configs() → PtyConfig.args chain.
// Uses `factory_pane_configs` (config-only, no spawn) so tests run without a
// real `claude` or `codex` binary present.

#[test]
fn factory_pane_configs_supervisor_effort_reaches_pty_args() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_EFFORT_SUPPORTED", "1");
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        supervisor_effort: Some("low".to_string()),
        worker_effort: Some("high".to_string()),
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Claude,
        worker_cli: crate::harness::SupervisorCli::Claude,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    let (_, sup_config) = configs
        .iter()
        .find(|(name, _)| name == &config.supervisor_name)
        .expect("supervisor config must be present");
    let effort_idx = sup_config
        .args
        .iter()
        .position(|a| a == "--effort")
        .expect("supervisor PTY args must contain --effort");
    let effort_val = sup_config
        .args
        .get(effort_idx + 1)
        .expect("--effort must be followed by a value in supervisor PTY args");
    assert_eq!(
        effort_val, "low",
        "MuxConfig::supervisor_effort must reach supervisor PTY --effort arg"
    );
}

#[test]
fn factory_pane_configs_worker_effort_reaches_pty_args() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_EFFORT_SUPPORTED", "1");
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        supervisor_effort: Some("low".to_string()),
        worker_effort: Some("high".to_string()),
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Claude,
        worker_cli: crate::harness::SupervisorCli::Claude,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    let (_, worker_config) = configs
        .iter()
        .find(|(name, _)| name == "worker-1")
        .expect("worker-1 config must be present");
    let effort_idx = worker_config
        .args
        .iter()
        .position(|a| a == "--effort")
        .expect("worker PTY args must contain --effort");
    let effort_val = worker_config
        .args
        .get(effort_idx + 1)
        .expect("--effort must be followed by a value in worker PTY args");
    assert_eq!(
        effort_val, "high",
        "MuxConfig::worker_effort must reach worker PTY --effort arg"
    );
    // supervisor must be last in the returned vec (workers-first ordering)
    assert_eq!(
        configs.last().unwrap().0,
        config.supervisor_name,
        "supervisor must be the last entry in factory_pane_configs output"
    );
}

#[test]
fn factory_pane_configs_propagates_factory_session_to_process_and_codex_mcp_env() {
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        worker_names: vec!["codex-worker".to_string()],
        supervisor_name: "codex-supervisor".to_string(),
        factory_session: Some("factory-session-a".to_string()),
        include_director: false,
        supervisor_cli: SupervisorCli::Codex,
        worker_cli: SupervisorCli::Codex,
        ..MuxConfig::default()
    };

    let configs = Mux::factory_pane_configs(&config);

    for (name, pty_config) in configs {
        assert_eq!(
            env_value(&pty_config, "CAS_FACTORY_SESSION"),
            Some("factory-session-a"),
            "{name} PTY env must include CAS_FACTORY_SESSION"
        );
        assert!(
            pty_config.args.contains(
                &"mcp_servers.cs.env.CAS_FACTORY_SESSION=\"factory-session-a\"".to_string()
            ),
            "{name} Codex cs MCP env must include CAS_FACTORY_SESSION; args={:?}",
            pty_config.args
        );
    }
}

#[test]
fn factory_pane_configs_propagates_supervisor_name_to_codex_worker_mcp_env() {
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        worker_names: vec!["codex-worker".to_string()],
        supervisor_name: "codex-supervisor".to_string(),
        include_director: false,
        supervisor_cli: SupervisorCli::Codex,
        worker_cli: SupervisorCli::Codex,
        ..MuxConfig::default()
    };

    let configs = Mux::factory_pane_configs(&config);
    let worker = configs
        .iter()
        .find(|(name, _)| name == "codex-worker")
        .expect("worker pane config");

    assert_eq!(
        env_value(&worker.1, "CAS_SUPERVISOR_NAME"),
        Some("codex-supervisor"),
        "worker process env must know the owning supervisor"
    );
    assert!(
        worker
            .1
            .args
            .contains(&"mcp_servers.cs.env.CAS_SUPERVISOR_NAME=\"codex-supervisor\"".to_string()),
        "Codex starts cs with a restricted environment, so the owning supervisor name must be an explicit MCP env override; args={:?}",
        worker.1.args
    );
}

#[test]
fn build_add_worker_config_propagates_factory_session_for_dynamic_spawns() {
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 0,
        worker_names: vec![],
        factory_session: Some("factory-session-dynamic".to_string()),
        include_director: false,
        supervisor_cli: SupervisorCli::Claude,
        worker_cli: SupervisorCli::Codex,
        ..MuxConfig::default()
    };
    let mut mux = Mux::factory_state_for_test(&config);

    mux.set_default_worker_spec(WorkerSpec::codex_default("dynamic-worker"));
    let pty_config = mux.build_add_worker_config(
        "dynamic-worker",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        None,
    );

    assert_eq!(
        env_value(&pty_config, "CAS_FACTORY_SESSION"),
        Some("factory-session-dynamic")
    );
    assert!(
        pty_config.args.contains(
            &"mcp_servers.cs.env.CAS_FACTORY_SESSION=\"factory-session-dynamic\"".to_string()
        ),
        "dynamic Codex cs MCP env must include CAS_FACTORY_SESSION; args={:?}",
        pty_config.args
    );
}

#[test]
fn factory_session_codex_toml_arg_sanitizes_quote_and_newline() {
    let raw_session = "factory\"bad\nsession";
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        worker_names: vec!["codex-worker".to_string()],
        factory_session: Some(raw_session.to_string()),
        include_director: false,
        supervisor_cli: SupervisorCli::Codex,
        worker_cli: SupervisorCli::Codex,
        ..MuxConfig::default()
    };

    let configs = Mux::factory_pane_configs(&config);

    for (name, pty_config) in configs {
        assert_eq!(
            env_value(&pty_config, "CAS_FACTORY_SESSION"),
            Some(raw_session),
            "{name} plain PTY env should keep the raw session value"
        );
        let toml_arg =
            codex_factory_session_arg(&pty_config).expect("Codex cs MCP session env arg");
        assert_eq!(
            toml_arg,
            "mcp_servers.cs.env.CAS_FACTORY_SESSION=\"factory_bad_session\""
        );
        assert_eq!(
            toml_arg.matches('"').count(),
            2,
            "{name} TOML arg should contain only the wrapping quotes"
        );
        assert!(
            !toml_arg.contains('\n'),
            "{name} TOML arg must not contain raw newlines"
        );
    }
}

/// cas-34f7f: when MuxConfig effort fields are None, --effort must be OMITTED
/// from PtyConfig args entirely.  Role-based defaults (xhigh/high) now live in
/// the cascade resolver layer, not in pty.rs spawn functions.
#[test]
fn factory_pane_configs_none_effort_omits_effort_flag() {
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        supervisor_effort: None,
        worker_effort: None,
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Claude,
        worker_cli: crate::harness::SupervisorCli::Claude,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    let (_, sup_config) = configs
        .iter()
        .find(|(name, _)| name == &config.supervisor_name)
        .expect("supervisor config must be present");
    assert!(
        !sup_config.args.contains(&"--effort".to_string()),
        "supervisor with None effort must omit --effort (cas-34f7f); got: {:?}",
        sup_config.args
    );

    let (_, worker_config) = configs
        .iter()
        .find(|(name, _)| name == "worker-1")
        .expect("worker-1 config must be present");
    assert!(
        !worker_config.args.contains(&"--effort".to_string()),
        "worker with None effort must omit --effort (cas-34f7f); got: {:?}",
        worker_config.args
    );
}

#[test]
fn factory_pane_configs_codex_worker_inherits_supervisor_cli_in_cs_env_cas_1544() {
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Codex,
        worker_cli: crate::harness::SupervisorCli::Codex,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    let (_, worker_config) = configs
        .iter()
        .find(|(name, _)| name == "worker-1")
        .expect("worker-1 config must be present");
    let all_args = worker_config.args.join(" ");
    assert!(
        all_args.contains("mcp_servers.cs.env.CAS_FACTORY_SUPERVISOR_CLI=\"codex\""),
        "Codex worker cs MCP env must inherit the supervisor CLI alias for verification guidance: {all_args}"
    );
    assert!(
        worker_config
            .env
            .iter()
            .any(|(k, v)| k == "CAS_FACTORY_SUPERVISOR_CLI" && v == "codex"),
        "worker process env must also carry CAS_FACTORY_SUPERVISOR_CLI=codex: {:?}",
        worker_config.env
    );
}

// ── end cas-d571 ──────────────────────────────────────────────────────────────

// ── cas-3fed: per-worker spec storage + factory wiring ────────────────────────
// Tests the MuxConfig.resolved_worker_specs → factory_pane_configs per-worker
// CLI selection path, and the Mux::add_worker explicit spec override path.

#[test]
fn effective_command_parses_nice_layouts_cas_046c() {
    for (command, args, expected) in [
        ("codex", vec!["--model", "large", "--effort", "high"], "codex"),
        ("nice", vec!["-n", "10", "codex", "--model", "large", "--effort", "high"], "codex"),
        ("nice", vec!["--adjustment=10", "codex", "--effort", "high", "--model", "large"], "codex"),
        ("nice", vec!["--adjustment", "-5", "claude", "--effort", "high", "--model", "codex"], "claude"),
        ("nice", vec!["-n10", "claude", "--model", "codex"], "claude"),
        ("nice", vec!["--", "codex", "--model", "large"], "codex"),
        ("nice", vec!["claude", "--model", "codex"], "claude"),
        ("nice", vec![], "nice"),
        ("nice", vec!["--"], "nice"),
        ("nice", vec!["-n", "10"], "nice"),
        ("nice", vec!["-n"], "nice"),
        ("nice", vec!["-n", "not-a-number", "codex"], "nice"),
        ("nice", vec!["--adjustment=bad", "codex"], "nice"),
        ("nice", vec!["-nbad", "codex"], "nice"),
        ("nice", vec!["-n", "2147483648", "codex"], "nice"),
        ("nice", vec!["--unknown", "codex"], "nice"),
    ] {
        let config = crate::pty::PtyConfig {
            command: command.into(), args: args.into_iter().map(str::to_string).collect(),
            ..Default::default()
        };
        assert_eq!(config.effective_command(), expected, "command={command}, args={:?}", config.args);
    }
}

#[test]
fn factory_pane_configs_uses_per_worker_specs() {
    // worker-1 → Codex, worker-2 → Claude, but MuxConfig.worker_cli is Claude.
    // resolved_worker_specs must override the singular default per worker.
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 2,
        worker_names: vec!["worker-1".to_string(), "worker-2".to_string()],
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Claude,
        worker_cli: crate::harness::SupervisorCli::Claude,
        resolved_worker_specs: vec![
            WorkerSpec {
                name: Some("worker-1".to_string()),
                cli: crate::harness::SupervisorCli::Codex,
                model: None,
                effort: None,
                config_dir: Some("/accounts/codex-research".to_string()),
                requester_config_dir: None,
                requester_secure_storage_dir: None,
            },
            WorkerSpec {
                name: Some("worker-2".to_string()),
                cli: crate::harness::SupervisorCli::Claude,
                model: None,
                effort: None,
                config_dir: Some("/accounts/claude-review".to_string()),
                requester_config_dir: None,
                requester_secure_storage_dir: None,
            },
        ],
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    let (_, w1) = configs
        .iter()
        .find(|(n, _)| n == "worker-1")
        .expect("worker-1 must be present");
    let (_, w2) = configs
        .iter()
        .find(|(n, _)| n == "worker-2")
        .expect("worker-2 must be present");

    assert_eq!(
        w1.effective_command(),
        "codex",
        "worker-1 with Codex spec must use codex binary"
    );
    assert_eq!(
        w2.effective_command(),
        "claude",
        "worker-2 with Claude spec must use claude binary"
    );
    assert_eq!(
        spawned_env(w1, "CODEX_HOME"),
        Some("/accounts/codex-research"),
        "worker-1 must receive its own Codex account home"
    );
    assert_eq!(
        spawned_env(w1, "CAS_FACTORY_WORKER_ACCOUNT_DIR"),
        Some("/accounts/codex-research"),
        "worker-1 registration metadata must name its resolved account"
    );
    assert_eq!(
        spawned_env(w2, "CLAUDE_CONFIG_DIR"),
        Some("/accounts/claude-review"),
        "worker-2 must receive its own Claude account directory"
    );
    assert_eq!(
        spawned_env(w2, "CAS_FACTORY_WORKER_ACCOUNT_DIR"),
        Some("/accounts/claude-review"),
        "worker-2 registration metadata must name its resolved account"
    );
}

#[test]
fn factory_pane_configs_falls_back_to_singular_when_specs_empty() {
    // resolved_worker_specs is empty → all workers use worker_cli = Codex.
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 2,
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Claude,
        worker_cli: crate::harness::SupervisorCli::Codex,
        resolved_worker_specs: vec![],
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    for (name, pty_config) in &configs {
        if name == &config.supervisor_name {
            assert_eq!(
                pty_config.effective_command(),
                "claude",
                "supervisor must use claude binary"
            );
        } else {
            assert_eq!(
                pty_config.effective_command(),
                "codex",
                "worker {name} with empty resolved_worker_specs must fall back to worker_cli=Codex"
            );
            // PtyConfig::codex ignores the effort argument (_effort) intentionally;
            // verify --effort does NOT appear in the codex worker args (cas-206d coverage).
            assert!(
                !pty_config.args.iter().any(|a| a == "--effort"),
                "codex worker must NOT have --effort in args (codex ignores effort)"
            );
        }
    }
}

#[test]
fn add_worker_uses_explicit_spec() {
    // Mux default is Claude (builtin_default), but build_add_worker_config with
    // an explicit Codex spec must produce a codex PtyConfig.
    let mux = Mux::new(24, 80);

    let codex_spec = WorkerSpec {
        name: Some("dynamic-worker".to_string()),
        cli: crate::harness::SupervisorCli::Codex,
        model: None,
        effort: Some(Effort::High),
        config_dir: None,
        requester_config_dir: None,
        requester_secure_storage_dir: None,
    };

    let pty_config = mux.build_add_worker_config(
        "dynamic-worker",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        Some(codex_spec),
    );

    assert_eq!(
        pty_config.effective_command(),
        "codex",
        "explicit Codex spec must override Claude default in dynamic add_worker path"
    );

    // Without explicit spec, the default (Claude) must be used.
    let claude_config = mux.build_add_worker_config(
        "another-worker",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        None,
    );
    assert_eq!(
        claude_config.effective_command(),
        "claude",
        "no explicit spec must fall back to Mux default (Claude)"
    );
}

// ── end cas-3fed ──────────────────────────────────────────────────────────────

// ── cas-3fed autofix: priority-2 branch coverage ─────────────────────────────

#[test]
fn effective_worker_spec_uses_worker_specs_map() {
    // Priority 2: per-worker entry in Mux::worker_specs wins over the
    // default when no explicit spec is supplied (priority 1 absent).
    let mut mux = Mux::new(24, 80);
    // builtin_default → Claude; override just "worker-map" to Codex.
    let codex_spec = WorkerSpec {
        name: Some("worker-map".to_string()),
        cli: crate::harness::SupervisorCli::Codex,
        model: None,
        effort: None,
        config_dir: None,
        requester_config_dir: None,
        requester_secure_storage_dir: None,
    };
    mux.set_worker_spec("worker-map", codex_spec);

    // No explicit spec → should pick up the map entry.
    let effective = mux.effective_worker_spec("worker-map", None);
    assert_eq!(
        effective.cli,
        crate::harness::SupervisorCli::Codex,
        "worker_specs map entry must take priority over default when no explicit spec is passed"
    );

    // A name not in the map should still fall through to the default.
    let default_effective = mux.effective_worker_spec("unknown-worker", None);
    assert_eq!(
        default_effective.cli,
        crate::harness::SupervisorCli::Claude,
        "unknown worker must fall back to Mux default (Claude builtin_default)"
    );
}

// ── end priority-2 coverage ───────────────────────────────────────────────────

/// cas-2567 (AC2): the factory resolved slot 1 to Codex, Codex was not
/// logged in, and `apply_codex_fallback` rewrote the (unnamed) slot spec to
/// Claude. The pane launches from that slot spec, so the harness decisions
/// read through `effective_worker_spec(name)` must say Claude too; before the
/// fix they fell back to the singular `worker_cli` (still Codex), which routed
/// a Claude worker's mail over PTY and labelled it `[worker:codex]`.
#[test]
fn cas_2567_initial_worker_harness_matches_the_spec_it_launched_from() {
    let fallback_claude = WorkerSpec {
        name: None,
        ..WorkerSpec::builtin_default()
    };
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        worker_names: vec!["keen-gazelle-59".to_string()],
        include_director: false,
        supervisor_cli: SupervisorCli::Claude,
        worker_cli: SupervisorCli::Codex,
        resolved_worker_specs: vec![fallback_claude],
        ..MuxConfig::default()
    };
    let mux = Mux::factory_state_for_test(&config);
    assert_eq!(
        mux.effective_worker_spec("keen-gazelle-59", None).cli,
        SupervisorCli::Claude
    );
    let launched = Mux::factory_pane_configs(&config);
    let worker = launched
        .iter()
        .find(|(name, _)| name == "keen-gazelle-59")
        .expect("worker pane config");
    assert!(
        !worker.1.command.contains("codex")
            && !worker.1.args.iter().any(|arg| arg.contains("codex")),
        "the pane itself launches Claude: {:?} {:?}",
        worker.1.command,
        worker.1.args
    );
}

/// cas-2567: a spec that already carries a worker's name keeps precedence.
#[test]
fn cas_2567_named_spec_still_wins_for_its_worker() {
    let codex_named = WorkerSpec {
        name: Some("named-worker".to_string()),
        cli: SupervisorCli::Codex,
        ..WorkerSpec::builtin_default()
    };
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        worker_names: vec!["named-worker".to_string()],
        include_director: false,
        supervisor_cli: SupervisorCli::Claude,
        worker_cli: SupervisorCli::Claude,
        resolved_worker_specs: vec![codex_named],
        ..MuxConfig::default()
    };
    let mux = Mux::factory_state_for_test(&config);
    assert_eq!(
        mux.effective_worker_spec("named-worker", None).cli,
        SupervisorCli::Codex
    );
}

// ── cas-b68a: add_worker persists the resolved spec ──────────────────────────

/// Regression for cas-b68a: a dynamically-spawned Codex worker in a Claude-DEFAULT
/// factory must resolve as Codex via `effective_worker_spec(name, None)` AFTER the
/// spawn.
///
/// This is the load-bearing proof for AC2's live path. The daemon's harness-aware
/// message router (`FactoryApp::harness_for`) resolves a worker's harness through
/// `effective_worker_spec(name, None)`. Before the fix, `add_worker` *used* an
/// explicit per-spawn spec but never persisted it into `worker_specs`, so a Codex
/// worker added to a Claude-default factory (the exact bug scenario) resolved back
/// to the Claude default — and the entire routing fix would be inert, silently
/// inboxing messages the codex process can never read.
#[cfg(unix)]
#[test]
fn add_worker_persists_explicit_spec_so_effective_resolves_codex() {
    use std::os::unix::fs::PermissionsExt;

    let mut env = TestEnvGuard::temp_home();
    let bin_dir =
        std::env::temp_dir().join(format!("cas-mux-add-worker-test-{}", std::process::id()));
    std::fs::create_dir_all(&bin_dir).expect("fake-bin directory must be creatable");
    for binary in ["cas", "codex"] {
        let path = bin_dir.join(binary);
        std::fs::write(&path, b"#!/bin/sh\nexit 0\n").expect("fake worker binary must be writable");
        let mut permissions = std::fs::metadata(&path)
            .expect("fake worker binary metadata must be readable")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions)
            .expect("fake worker binary must be executable");
    }
    // Keep system utilities such as the optional `nice` wrapper available;
    // the fake directory remains first so the test never launches a real Codex.
    let mut path_entries = vec![bin_dir.clone()];
    if let Some(current_path) = std::env::var_os("PATH") {
        path_entries.extend(std::env::split_paths(&current_path));
    }
    let test_path = std::env::join_paths(path_entries)
        .expect("test PATH entries must be representable on this platform");
    env.set("PATH", test_path);

    let mut mux = Mux::new(24, 80);
    // Session default is Claude — the explicit per-spawn spec must persist and win.
    mux.set_default_worker_spec(WorkerSpec {
        name: None,
        cli: crate::harness::SupervisorCli::Claude,
        model: None,
        effort: None,
        config_dir: None,
        requester_config_dir: None,
        requester_secure_storage_dir: None,
    });

    // Pre-condition: with nothing registered, an unknown worker resolves to the
    // Claude default — i.e. without persistence the post-spawn lookup is wrong.
    assert_eq!(
        mux.effective_worker_spec("alice", None).cli,
        crate::harness::SupervisorCli::Claude,
        "precondition: unregistered worker resolves to the Claude session default"
    );

    // Dynamically spawn "alice" with an explicit Codex spec (the SpawnWorkers path).
    let dir = std::env::temp_dir();
    let spec = WorkerSpec::codex_default("alice");
    mux.add_worker("alice", dir, None, "supervisor", None, Some(spec))
        .expect("add_worker should spawn the worker pane");

    // Post-spawn: harness resolution (what FactoryApp::harness_for does) must now
    // report Codex, with NO explicit spec passed — proving the spec was persisted.
    assert_eq!(
        mux.effective_worker_spec("alice", None).cli,
        crate::harness::SupervisorCli::Codex,
        "after add_worker, a later harness lookup must resolve the persisted Codex \
         spec — not fall back to the Claude default (cas-b68a)"
    );

    drop(mux);
    std::fs::remove_dir_all(bin_dir).expect("fake-bin directory must be removable");
}

// ── cas-35fe: custom worker_names branch ─────────────────────────────────────

#[test]
fn factory_pane_configs_custom_worker_names() {
    // Use names that differ from auto-generated "worker-1"/"worker-2" so that
    // a regression swapping the custom-names branch back to auto-generation
    // would cause the assertions to fail.
    let config = MuxConfig {
        cwd: std::path::PathBuf::from("/tmp/test"),
        workers: 2,
        worker_names: vec!["alice".to_string(), "bob".to_string()],
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Claude,
        worker_cli: crate::harness::SupervisorCli::Claude,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    let names: Vec<&str> = configs.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        names.contains(&"alice"),
        "factory_pane_configs must honour custom worker name 'alice'"
    );
    assert!(
        names.contains(&"bob"),
        "factory_pane_configs must honour custom worker name 'bob'"
    );
    assert!(
        !names.contains(&"worker-1"),
        "factory_pane_configs must NOT auto-generate names when worker_names is non-empty"
    );
    assert!(
        !names.contains(&"worker-2"),
        "factory_pane_configs must NOT auto-generate names when worker_names is non-empty"
    );
}

// ── end cas-35fe ──────────────────────────────────────────────────────────────

// ── cas-5175: set_default_worker_spec → add_worker effort propagation ─────────

#[test]
fn add_worker_effort_propagates_to_pty_args() {
    // Verify that effort set on the Mux-wide default flows through
    // effective_worker_spec → build_add_worker_config → PtyConfig args.
    // Uses the config-only build_add_worker_config helper (no PTY spawn).
    use crate::spec::Effort;

    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_EFFORT_SUPPORTED", "1");
    let mut mux = Mux::new(24, 80);
    mux.set_default_worker_spec(crate::spec::WorkerSpec {
        name: None,
        cli: crate::harness::SupervisorCli::Claude,
        model: None,
        effort: Some(Effort::Low),
        config_dir: None,
        requester_config_dir: None,
        requester_secure_storage_dir: None,
    });

    let pty = mux.build_add_worker_config(
        "effort-worker",
        std::path::PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        None, // no explicit override → falls through to default
    );

    let effort_idx = pty
        .args
        .iter()
        .position(|a| a == "--effort")
        .expect("--effort must appear in PTY args when effort is set on the Mux default");
    let effort_val = pty
        .args
        .get(effort_idx + 1)
        .expect("--effort must be followed by a value");
    assert_eq!(
        effort_val, "low",
        "Effort::Low must reach PTY args as \"low\" via the default spec path"
    );
}

fn spawned_env<'a>(config: &'a crate::pty::PtyConfig, key: &str) -> Option<&'a str> {
    config
        .env
        .iter()
        .rev()
        .find_map(|(candidate, value)| (candidate == key).then_some(value.as_str()))
}

#[test]
fn explicit_config_dir_beats_requester_config_dir() {
    let mux = Mux::new(24, 80);
    let config = mux.build_add_worker_config(
        "claude-explicit",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        Some(WorkerSpec {
            name: None,
            cli: crate::harness::SupervisorCli::Claude,
            model: None,
            effort: None,
            config_dir: Some("~/.claude-explicit".to_string()),
            requester_config_dir: Some("~/.claude-supervisor".to_string()),
            requester_secure_storage_dir: None,
        }),
    );

    assert!(
        spawned_env(&config, "CLAUDE_CONFIG_DIR")
            .is_some_and(|dir| dir.ends_with("/.claude-explicit")),
        "explicit config dir must be tilde-expanded: {:?}",
        spawned_env(&config, "CLAUDE_CONFIG_DIR")
    );
    assert_eq!(
        spawned_env(&config, "CAS_FACTORY_CLAUDE_CONFIG_DIR_SOURCE"),
        Some("explicit")
    );
}

#[test]
fn cas_16c0_only_explicit_account_manifest_adds_connectors_under_strict_mcp() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_CLAUDE_SUPPORTED_FLAGS", "--mcp-config,--strict-mcp-config");
    let project = tempfile::tempdir().unwrap();
    let root = project.path().join(".cas");
    let selected = env.home().join("selected-account");
    let requester = env.home().join("supervisor-account");
    for directory in [&root, &selected, &requester] {
        std::fs::create_dir_all(directory).unwrap();
    }
    let gmail = serde_json::json!({
        "type": "claudeai-proxy", "url": "https://gmail.example.test/mcp", "id": "mcpsrv_selected"
    });
    std::fs::write(selected.join("cassy-worker-connectors.json"),
        serde_json::json!({"mcpServers": {"claude.ai Gmail": gmail}}).to_string()).unwrap();
    std::fs::write(requester.join("cassy-worker-connectors.json"),
        serde_json::json!({"mcpServers": {"claude.ai Other Account": {
            "type": "claudeai-proxy", "url": "https://other.example.test/mcp", "id": "mcpsrv_other"
        }}}).to_string()).unwrap();
    let project_mcp = serde_json::json!({"mcpServers": {
        "vercel": {"command": "never-run-vercel"}, "neon": {"command": "never-run-neon"}
    }}).to_string();
    std::fs::write(project.path().join(".mcp.json"), &project_mcp).unwrap();
    env.set("CLAUDE_CONFIG_DIR", requester.to_str().unwrap());
    // Reuse each worker name after the explicit grant to catch stale snapshots.
    for source in ["explicit", "supervisor", "host"] {
        let explicit = source == "explicit";
        let spec = WorkerSpec {
            name: Some("worker-1".into()),
            config_dir: explicit.then(|| selected.display().to_string()),
            requester_config_dir: (source != "host").then(|| requester.display().to_string()),
            ..WorkerSpec::builtin_default()
        };
        let configs = Mux::factory_pane_configs(&MuxConfig {
            cwd: project.path().into(), cas_root: Some(root.clone()), workers: 1,
            resolved_worker_specs: vec![spec.clone()], include_director: false,
            ..Default::default()
        });
        let initial = &configs.iter().find(|(name, _)| name == "worker-1").unwrap().1;
        let dynamic = Mux::new(24, 80).build_add_worker_config(
            "dynamic", project.path().into(), Some(&root), "supervisor", None, Some(spec));
        for pane in [initial, &dynamic] {
            assert!(env_value(pane, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
            assert!(pane.args.contains(&"--strict-mcp-config".into()));
            let index = pane.args.iter().position(|arg| arg == "--mcp-config").unwrap();
            let document: serde_json::Value = serde_json::from_slice(
                &std::fs::read(&pane.args[index + 1]).unwrap()).unwrap();
            let servers = document["mcpServers"].as_object().unwrap();
            assert!(servers.contains_key("cas"));
            assert_eq!(servers.len(), if explicit { 2 } else { 1 });
            if explicit {
                assert_eq!(servers["claude.ai Gmail"], gmail);
                assert_eq!(env_value(pane, "CLAUDE_CONFIG_DIR"), selected.to_str());
            }
        }
    }
    assert_eq!(std::fs::read_to_string(project.path().join(".mcp.json")).unwrap(), project_mcp);
}

#[test]
fn cas_16c0_missing_or_invalid_manifest_keeps_strict_cas_with_notice() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_CLAUDE_SUPPORTED_FLAGS", "--mcp-config,--strict-mcp-config");
    let project = tempfile::tempdir().unwrap();
    let root = project.path().join(".cas");
    let selected = env.home().join("selected-account");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&selected).unwrap();
    let manifest = selected.join("cassy-worker-connectors.json");
    for contents in [None, Some("not-json"),
        Some(r#"{"mcpServers":{"claude.ai Gmail":{"type":"http","url":"https://gmail.example.test/mcp","id":"mcpsrv_selected"}}}"#),
        Some(r#"{"mcpServers":{"claude.ai Gmail":{"type":"claudeai-proxy","url":"https://gmail.example.test/mcp","id":"mcpsrv_selected","env":{"PRIVATE_TOKEN":"never-copy-fixture"}}}}"#),
    ] {
        if let Some(contents) = contents {
            std::fs::write(&manifest, contents).unwrap();
        }
        let pane = Mux::new(24, 80).build_add_worker_config(
            "worker", project.path().into(), Some(&root), "supervisor", None,
            Some(WorkerSpec { config_dir: Some(selected.display().to_string()), ..WorkerSpec::builtin_default() }));
        assert!(env_value(&pane, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
        assert!(pane.args.contains(&"--strict-mcp-config".into()));
        let document: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join("worker-mcp/worker.json")).unwrap()).unwrap();
        assert_eq!(document["mcpServers"].as_object().unwrap().keys().collect::<Vec<_>>(), vec!["cas"]);
        let notice = env_value(&pane, "CAS_FACTORY_CLAUDE_CONNECTOR_NOTICE").expect("missing/invalid manifest must be visible");
        assert!(notice.contains("cassy-worker-connectors.json") && notice.contains("strict"), "{notice}");
        assert!(!notice.contains('\n'));
    }
}

#[test]
fn cas_16c0_manifest_cannot_regrant_denied_connectors_or_environment() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_CLAUDE_SUPPORTED_FLAGS", "--mcp-config,--strict-mcp-config");
    let project = tempfile::tempdir().unwrap();
    let root = project.path().join(".cas");
    let selected = env.home().join("selected-account");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&selected).unwrap();
    std::fs::write(root.join("config.toml"), "[factory]\nsupervisor_only_mcp=['calendar']\nsupervisor_only_env=['PRIVATE_TOKEN']\n").unwrap();
    env.set("PRIVATE_TOKEN", "never-forward-fixture");
    let mut servers = ["Gmail", "Vercel", "Neon", "Calendar"].into_iter().map(|name| (
        format!("claude.ai {name}"), serde_json::json!({"type":"claudeai-proxy",
            "url":format!("https://{}.example.test/mcp", name.to_lowercase()), "id":format!("mcpsrv_{name}")})
    )).collect::<serde_json::Map<_, _>>();
    let ceilings = serde_json::json!({"send_email": "blocked", "create_draft": "ask"});
    servers["claude.ai Gmail"]["toolPermissions"] = ceilings.clone();
    std::fs::write(selected.join("cassy-worker-connectors.json"),
        serde_json::json!({"mcpServers": servers}).to_string()).unwrap();
    let pane = Mux::new(24, 80).build_add_worker_config(
        "worker", project.path().into(), Some(&root), "supervisor", None,
        Some(WorkerSpec { config_dir: Some(selected.display().to_string()), ..WorkerSpec::builtin_default() }));
    let document: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("worker-mcp/worker.json")).unwrap()).unwrap();
    assert_eq!(document["mcpServers"].as_object().unwrap().keys().collect::<Vec<_>>(), vec!["cas", "claude.ai Gmail"]);
    assert_eq!(document["mcpServers"]["claude.ai Gmail"]["toolPermissions"], ceilings,
        "operator-exported native tool ceilings must reach Claude unchanged");
    assert!(pane.args.contains(&"--strict-mcp-config".into()));
    assert!(pane.env_remove.contains(&"PRIVATE_TOKEN".into()));
    assert!(env_value(&pane, "PRIVATE_TOKEN").is_none());
    assert!(!pane.args.contains(&"--managed-settings".into()), "native managed policy must stay authoritative");
}

#[test]
fn requester_config_dir_applies_when_explicit_param_is_omitted() {
    let mux = Mux::new(24, 80);
    let config = mux.build_add_worker_config(
        "claude-supervisor",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        Some(WorkerSpec {
            name: None,
            cli: crate::harness::SupervisorCli::Claude,
            model: None,
            effort: None,
            config_dir: None,
            requester_config_dir: Some("~/.claude-supervisor".to_string()),
            requester_secure_storage_dir: None,
        }),
    );

    assert!(
        spawned_env(&config, "CLAUDE_CONFIG_DIR")
            .is_some_and(|dir| dir.ends_with("/.claude-supervisor")),
        "requester config dir must be tilde-expanded: {:?}",
        spawned_env(&config, "CLAUDE_CONFIG_DIR")
    );
    assert_eq!(
        spawned_env(&config, "CAS_FACTORY_CLAUDE_CONFIG_DIR_SOURCE"),
        Some("supervisor")
    );
}

#[test]
fn requester_secure_storage_selector_preserves_unset_empty_and_set() {
    for (secure_storage_dir, expected_env, expect_remove) in [
        (None, None, true),
        (Some(""), Some(""), false),
        (
            Some("~/.claude-keychain"),
            Some("~/.claude-keychain"),
            false,
        ),
    ] {
        let mux = Mux::new(24, 80);
        let config = mux.build_add_worker_config(
            "claude-secure-selector",
            PathBuf::from("/tmp/test"),
            None,
            "supervisor",
            None,
            Some(WorkerSpec {
                name: None,
                cli: crate::harness::SupervisorCli::Claude,
                model: None,
                effort: None,
                config_dir: None,
                requester_config_dir: Some("~/.claude-work".to_string()),
                requester_secure_storage_dir: secure_storage_dir.map(str::to_string),
            }),
        );

        let actual = spawned_env(&config, "CLAUDE_SECURESTORAGE_CONFIG_DIR");
        assert_eq!(
            actual.map(|value| value.rsplit('/').next().unwrap_or(value)),
            expected_env.map(|value| value.rsplit('/').next().unwrap_or(value))
        );
        assert_eq!(
            config
                .env_remove
                .iter()
                .any(|key| key == "CLAUDE_SECURESTORAGE_CONFIG_DIR"),
            expect_remove
        );
    }
}

#[test]
fn omitted_config_dirs_leave_claude_environment_untouched() {
    let mux = Mux::new(24, 80);
    let config = mux.build_add_worker_config(
        "claude-inherit",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        Some(WorkerSpec {
            name: None,
            cli: crate::harness::SupervisorCli::Claude,
            model: None,
            effort: None,
            config_dir: None,
            requester_config_dir: None,
            requester_secure_storage_dir: None,
        }),
    );

    assert_eq!(spawned_env(&config, "CLAUDE_CONFIG_DIR"), None);
    assert_eq!(
        spawned_env(&config, "CAS_FACTORY_CLAUDE_CONFIG_DIR_SOURCE"),
        None
    );
}

#[test]
fn codex_ignores_resolved_claude_config_dir() {
    let mux = Mux::new(24, 80);
    let config = mux.build_add_worker_config(
        "codex-no-claude-dir",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        Some(WorkerSpec {
            name: None,
            cli: crate::harness::SupervisorCli::Codex,
            model: None,
            effort: None,
            config_dir: Some("~/.claude-explicit".to_string()),
            requester_config_dir: Some("~/.claude-supervisor".to_string()),
            requester_secure_storage_dir: None,
        }),
    );

    assert_eq!(spawned_env(&config, "CLAUDE_CONFIG_DIR"), None);
}

// ── end cas-5175 ──────────────────────────────────────────────────────────────

#[test]
fn test_mux_new() {
    let mux = Mux::new(24, 80);
    assert_eq!(mux.size(), (24, 80));
    assert!(mux.focused().is_none());
}

#[test]
fn test_mux_add_pane() {
    let mut mux = Mux::new(24, 80);
    let pane = Pane::director("test", 24, 80).unwrap();
    mux.add_pane(pane);

    assert!(mux.get("test").is_some());
    assert_eq!(mux.focused_id(), Some("test"));
}

#[test]
fn test_mux_focus_navigation() {
    let mut mux = Mux::new(24, 80);
    mux.add_pane(Pane::director("pane1", 24, 40).unwrap());
    mux.add_pane(Pane::director("pane2", 24, 40).unwrap());

    assert_eq!(mux.focused_id(), Some("pane1"));

    mux.focus_next();
    assert_eq!(mux.focused_id(), Some("pane2"));

    mux.focus_next();
    assert_eq!(mux.focused_id(), Some("pane1")); // Wraps around

    mux.focus_prev();
    assert_eq!(mux.focused_id(), Some("pane2"));
}

#[test]
fn test_pane_count() {
    let mut mux = Mux::new(24, 80);
    assert_eq!(mux.pane_count(), 0);

    mux.add_pane(Pane::director("pane1", 24, 40).unwrap());
    assert_eq!(mux.pane_count(), 1);

    mux.add_pane(Pane::director("pane2", 24, 40).unwrap());
    assert_eq!(mux.pane_count(), 2);

    mux.remove_pane("pane1");
    assert_eq!(mux.pane_count(), 1);
}

#[test]
fn test_remove_pane_focus_transfer() {
    let mut mux = Mux::new(24, 80);
    mux.add_pane(Pane::director("pane1", 24, 40).unwrap());
    mux.add_pane(Pane::director("pane2", 24, 40).unwrap());

    // Focus is on pane1 (first added)
    assert_eq!(mux.focused_id(), Some("pane1"));

    // Remove focused pane, focus should transfer to next
    mux.remove_pane("pane1");
    assert_eq!(mux.focused_id(), Some("pane2"));
    assert_eq!(mux.pane_count(), 1);
}

// ── cas-c931: urgent interrupt-and-redirect by-name primitives ──────────────

#[test]
fn pane_bytes_received_present_for_known_pane_none_for_missing() {
    let mut mux = Mux::new(24, 80);
    mux.add_pane(Pane::director("w1", 24, 80).unwrap());

    // Known pane: returns Some (0 until output is drained).
    assert_eq!(mux.pane_bytes_received("w1"), Some(0));
    // Unknown pane: None.
    assert_eq!(mux.pane_bytes_received("ghost"), None);
}

/// Helper: a real PTY-backed pane (running `cat`) so write paths exercise an
/// actual backend. `Pane::director` uses `PaneBackend::None`, which rejects
/// writes — useless for the urgent-redirect write tests. Returns None if a PTY
/// can't be spawned in this environment so the test skips rather than flakes.
fn cat_pane(name: &str) -> Option<Pane> {
    Pane::shell(name, PathBuf::from("/tmp"), Some("cat"), 24, 80).ok()
}

/// Real PTY stand-in carrying the production Codex framing/submit delay.
fn codex_cat_pane(name: &str) -> Option<Pane> {
    let pty = crate::pty::Pty::spawn(
        name,
        crate::pty::PtyConfig {
            command: "cat".to_string(),
            args: vec![],
            cwd: Some(PathBuf::from("/tmp")),
            env: vec![],
            env_remove: vec![],
            rows: 24,
            cols: 80,
        },
    )
    .ok()?;
    Pane::with_pty(
        name,
        PaneKind::Supervisor,
        pty,
        24,
        80,
        SupervisorCli::Codex,
    )
    .ok()
}

struct SubmitFailWriter {
    writes: usize,
}

impl std::io::Write for SubmitFailWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.writes += 1;
        if self.writes == 1 {
            Ok(buf.len())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "deterministic delayed submit failure",
            ))
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn settled_pane_snapshot(
    mux: &mut Mux,
    pane_id: &str,
) -> cas_factory_protocol::TerminalSnapshot {
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    mux.poll_batch();
    mux.get_pane_snapshot(pane_id).expect("pane snapshot").0
}

#[cfg(target_os = "linux")]
fn proc_state_and_group(pid: u32) -> Option<(char, u32)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1.trim_start();
    let mut fields = after_comm.split_whitespace();
    let state = fields.next()?.chars().next()?;
    fields.next()?;
    let pgid = fields.next()?.parse().ok()?;
    Some((state, pgid))
}

#[cfg(target_os = "linux")]
#[test]
fn kill_all_terminates_a_synthetic_long_lived_child_group() {
    let config = crate::pty::PtyConfig {
        command: "sh".to_string(),
        args: vec!["-c".to_string(), "sleep 300 & wait".to_string()],
        cwd: Some(PathBuf::from("/tmp")),
        env: vec![],
        env_remove: vec![],
        rows: 24,
        cols: 80,
    };
    let Ok(pty) = crate::pty::Pty::spawn("synthetic-worker", config) else {
        return;
    };
    let pane = Pane::with_pty(
        "synthetic-worker",
        PaneKind::Worker,
        pty,
        24,
        80,
        SupervisorCli::Claude,
    )
    .unwrap();
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);
    let pgid = mux
        .pane_process_group_id("synthetic-worker")
        .expect("PTY worker must expose its process group");

    let mut long_lived_child = None;
    for _ in 0..40 {
        long_lived_child = std::fs::read_dir("/proc").ok().and_then(|entries| {
            entries.flatten().find_map(|entry| {
                let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
                (pid != pgid
                    && proc_state_and_group(pid)
                        .is_some_and(|(state, group)| state != 'Z' && group == pgid))
                .then_some(pid)
            })
        });
        if long_lived_child.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let child_pid = long_lived_child.expect("synthetic shell must spawn its long-lived child");

    mux.kill_all();

    for _ in 0..40 {
        if proc_state_and_group(child_pid).is_none_or(|(state, _)| state == 'Z') {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("factory-exit kill_all left synthetic child {child_pid} alive");
}

#[test]
fn mux_pane_exit_event_carries_signal_status_from_the_real_pty_child() {
    let config = crate::pty::PtyConfig {
        command: "sh".to_string(),
        args: vec!["-c".to_string(), "kill -TERM $$".to_string()],
        cwd: Some(PathBuf::from("/tmp")),
        env: vec![],
        env_remove: vec![],
        rows: 24,
        cols: 80,
    };
    let pty = crate::pty::Pty::spawn("exit-signal-worker", config).expect("shell must spawn");
    let pane = Pane::with_pty(
        "exit-signal-worker",
        PaneKind::Worker,
        pty,
        24,
        80,
        SupervisorCli::Claude,
    )
    .expect("worker pane must build");
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(event) = mux.poll() {
            if let MuxEvent::PaneExited {
                pane_id,
                exit_code,
                exit_signal,
            } = event
            {
                assert_eq!(pane_id, "exit-signal-worker");
                assert!(
                    exit_code.is_some(),
                    "portable status code must remain available"
                );
                assert!(
                    exit_signal.is_some_and(|signal| !signal.is_empty()),
                    "Mux must preserve the PTY child's signal evidence"
                );
                return;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "worker exit event did not arrive before deadline"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn mux_retries_wait_status_after_pty_eof_without_blocking_poll() {
    let temp = std::env::temp_dir().join(format!(
        "cas-mux-delayed-wait-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock must be after epoch")
            .as_nanos()
    ));
    std::fs::create_dir(&temp).expect("temporary Mux evidence directory");
    let ready = temp.join("eof-ready");
    let done = temp.join("wait-status-ready");
    let ready_shell = shell_quote(&ready.to_string_lossy());
    let done_shell = shell_quote(&done.to_string_lossy());
    let config = crate::pty::PtyConfig {
        command: "sh".to_string(),
        args: vec![
            "-c".to_string(),
            format!(
                "printf ready > {ready_shell}; exec 0<&- 1>&- 2>&-; sleep 2; printf done > {done_shell}; exit 37"
            ),
        ],
        cwd: Some(PathBuf::from("/tmp")),
        env: vec![],
        env_remove: vec![],
        rows: 24,
        cols: 80,
    };
    let pty = crate::pty::Pty::spawn("delayed-wait-status-worker", config)
        .expect("delayed wait-status worker must spawn");
    let pane = Pane::with_pty(
        "delayed-wait-status-worker",
        PaneKind::Worker,
        pty,
        24,
        80,
        SupervisorCli::Claude,
    )
    .expect("worker pane must build");
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !ready.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "worker must close its PTY after writing the readiness marker"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    loop {
        if let Some(MuxEvent::PaneExited {
            pane_id,
            exit_code,
            exit_signal,
        }) = mux.poll()
        {
            assert_eq!(pane_id, "delayed-wait-status-worker");
            assert_eq!(exit_code, Some(37));
            assert!(
                exit_signal.is_none(),
                "normal delayed exit must not invent signal evidence"
            );
            assert!(
                done.exists(),
                "Mux must deliver wait status after PTY EOF, not finalize early"
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Mux did not deliver delayed wait status before deadline"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    std::fs::remove_dir_all(&temp).expect("remove temporary Mux evidence directory");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn graceful_kill_worker_waits_before_escalating_a_term_ignoring_group() {
    let config = crate::pty::PtyConfig {
        command: "sh".to_string(),
        args: vec![
            "-c".to_string(),
            "trap '' TERM HUP; sleep 300 & wait".to_string(),
        ],
        cwd: Some(PathBuf::from("/tmp")),
        env: vec![],
        env_remove: vec![],
        rows: 24,
        cols: 80,
    };
    let Ok(pty) = crate::pty::Pty::spawn("synthetic-grace-worker", config) else {
        return;
    };
    let pane = Pane::with_pty(
        "synthetic-grace-worker",
        PaneKind::Worker,
        pty,
        24,
        80,
        SupervisorCli::Claude,
    )
    .unwrap();
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);
    let pgid = mux
        .pane_process_group_id("synthetic-grace-worker")
        .expect("PTY worker must expose its process group");

    // Give the shell time to install its TERM/HUP ignores before teardown.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let started = std::time::Instant::now();
    mux.kill_worker("synthetic-grace-worker", false)
        .await
        .unwrap();

    assert!(
        started.elapsed() >= std::time::Duration::from_millis(2_750),
        "TERM-ignoring group must receive a real grace period before escalation"
    );
    for _ in 0..40 {
        if proc_state_and_group(pgid).is_none_or(|(state, _)| state == 'Z') {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("graceful escalation left synthetic process group {pgid} alive");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn graceful_kill_worker_returns_when_the_group_honors_term() {
    let config = crate::pty::PtyConfig {
        command: "sh".to_string(),
        args: vec!["-c".to_string(), "sleep 300 & wait".to_string()],
        cwd: Some(PathBuf::from("/tmp")),
        env: vec![],
        env_remove: vec![],
        rows: 24,
        cols: 80,
    };
    let Ok(pty) = crate::pty::Pty::spawn("synthetic-term-worker", config) else {
        return;
    };
    let pane = Pane::with_pty(
        "synthetic-term-worker",
        PaneKind::Worker,
        pty,
        24,
        80,
        SupervisorCli::Claude,
    )
    .unwrap();
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let started = std::time::Instant::now();
    mux.kill_worker("synthetic-term-worker", false)
        .await
        .unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "TERM-compliant groups should not consume the escalation grace window"
    );
}

/// GH #1363: a worker teardown that outlasts its deadline hands control back
/// to the daemon loop with the stage it reached and its process group, and
/// the off-loop thread still finishes the kill.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_worker_teardown_returns_at_its_deadline_naming_stage_and_group() {
    let config = crate::pty::PtyConfig {
        command: "sh".to_string(),
        args: vec!["-c".to_string(), "trap '' TERM HUP; sleep 300 & wait".to_string()],
        cwd: Some(PathBuf::from("/tmp")),
        env: vec![],
        env_remove: vec![],
        rows: 24,
        cols: 80,
    };
    let Ok(pty) = crate::pty::Pty::spawn("stalled-teardown-worker", config) else {
        return;
    };
    let pane = Pane::with_pty(
        "stalled-teardown-worker",
        PaneKind::Worker,
        pty,
        24,
        80,
        SupervisorCli::Claude,
    )
    .unwrap();
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);
    let pgid = mux.pane_process_group_id("stalled-teardown-worker").unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // The TERM-ignoring group holds kill_tree in its 3 s grace window.
    let started = std::time::Instant::now();
    let outcome = mux
        .kill_worker_within(
            "stalled-teardown-worker",
            false,
            std::time::Duration::from_millis(300),
        )
        .await
        .unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(
        outcome,
        crate::mux::WorkerTeardown::Stalled {
            stage: "signal and reap (Pty::kill_tree)",
            process_group: Some(pgid),
        }
    );
    assert!(mux.get("stalled-teardown-worker").is_none());

    for _ in 0..200 {
        let probe = std::process::Command::new("sh")
            .args(["-c", &format!("kill -0 -- -{pgid} 2>/dev/null")])
            .status()
            .unwrap();
        if !probe.success() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("the off-loop teardown never finished killing group {pgid}");
}

#[tokio::test]
async fn break_turn_targets_pane_by_name_independent_of_focus() {
    let mut mux = Mux::new(24, 80);
    let (Some(w1), Some(w2)) = (cat_pane("w1"), cat_pane("w2")) else {
        return; // no PTY available — skip
    };
    mux.add_pane(w1);
    mux.add_pane(w2);

    // Focus is on w1, but we break w2 by name — must succeed regardless of focus.
    assert_eq!(mux.focused_id(), Some("w1"));
    assert!(mux.break_turn("w2").await.is_ok());

    // Unknown target is a pane-not-found error, not a panic.
    assert!(mux.break_turn("ghost").await.is_err());
}

#[tokio::test]
async fn interrupt_and_inject_errors_on_missing_pane() {
    let mut mux = Mux::new(24, 80);
    let res = mux
        .interrupt_and_inject("nope", "hi", std::time::Duration::from_millis(0))
        .await;
    assert!(
        res.is_err(),
        "missing pane must surface an error, not panic"
    );
}

#[tokio::test]
async fn interrupt_and_inject_breaks_then_injects_by_name() {
    let mut mux = Mux::new(24, 80);
    let Some(w1) = cat_pane("w1") else {
        return; // no PTY available — skip
    };
    mux.add_pane(w1);

    mux.deliver_user_input_to("w1", b"operator draft", UserInputKind::KeyStream)
        .await
        .expect("mark dirty through attached-client input");
    assert!(
        mux.panes
            .get("w1")
            .expect("pane exists")
            .is_composer_dirty()
    );

    // Zero settle keeps the test fast; the ordering (Esc then inject) is
    // enforced by interrupt_and_inject internally. The urgent path must bypass
    // non-urgent composer deferral and its Esc must clear the stale draft.
    let res = mux
        .interrupt_and_inject(
            "w1",
            "STOP and switch tasks",
            std::time::Duration::from_millis(0),
        )
        .await;
    assert!(res.is_ok(), "urgent redirect to a live pane must succeed");
    assert!(
        !mux.panes
            .get("w1")
            .expect("pane exists")
            .is_composer_dirty(),
        "urgent break-turn Esc clears the composer before redirect"
    );
}

#[tokio::test]
async fn urgent_queue_inject_preserves_draft_then_interrupts_after_submit_cas_eacc() {
    let mut mux = Mux::new(24, 80);
    let Some(w1) = codex_cat_pane("urgent-safe") else {
        return;
    };
    mux.add_pane(w1);

    mux.deliver_user_input_to(
        "urgent-safe",
        b"unfinished human sentence",
        UserInputKind::KeyStream,
    )
    .await
    .expect("type operator draft");
    let draft_before = settled_pane_snapshot(&mut mux, "urgent-safe").await;

    let deferred = mux
        .interrupt_and_inject_preserving_composer(
            "urgent-safe",
            "urgent lifecycle redirect",
            std::time::Duration::ZERO,
        )
        .await
        .expect("dirty composer is a durable deferral, not an error");
    assert_eq!(deferred, InjectOutcome::DeferredComposerDirty);
    assert!(
        mux.get("urgent-safe").unwrap().is_composer_dirty(),
        "urgent delivery must not send Esc and clear the human draft"
    );
    assert!(
        !mux.get("urgent-safe").unwrap().is_turn_in_flight(),
        "urgent delivery must not submit the human draft"
    );
    let draft_after = settled_pane_snapshot(&mut mux, "urgent-safe").await;
    assert_eq!(
        draft_after, draft_before,
        "urgent deferral must preserve the Codex supervisor's visible draft byte-for-byte"
    );

    mux.deliver_user_input_to("urgent-safe", b"\r", UserInputKind::KeyStream)
        .await
        .expect("human submits at the safe boundary");
    let delivered = mux
        .interrupt_and_inject_preserving_composer(
            "urgent-safe",
            "urgent lifecycle redirect",
            std::time::Duration::ZERO,
        )
        .await
        .expect("urgent row delivers after the safe boundary");
    assert_eq!(delivered, InjectOutcome::Delivered);
    assert!(
        mux.get("urgent-safe").unwrap().is_turn_in_flight(),
        "urgent semantics remain interrupt-and-inject once the composer is clean"
    );
}

// ── cas-1a4d: non-urgent injects defer around operator drafts ───────────────

#[tokio::test]
async fn nonurgent_inject_defers_until_composer_clears_cas_1a4d() {
    let mut mux = Mux::new(24, 80);
    let Some(pane) = cat_pane("operator-pane") else {
        return;
    };
    mux.add_pane(pane);

    mux.deliver_user_input_to(
        "operator-pane",
        b"half-written thought",
        UserInputKind::KeyStream,
    )
    .await
    .expect("type draft");
    let outcome = mux
        .inject("operator-pane", "non-urgent report")
        .await
        .expect("dirty inject should be deferred");
    assert_eq!(outcome, InjectOutcome::DeferredComposerDirty);
    assert!(
        !mux.panes
            .get("operator-pane")
            .expect("pane exists")
            .is_turn_in_flight(),
        "deferred report must not submit into the dirty composer"
    );

    mux.send_input_to("operator-pane", b"\x1b")
        .await
        .expect("standalone Esc clears draft");
    let outcome = mux.inject("operator-pane", "non-urgent report").await;

    assert_eq!(
        outcome.expect("durable queue retry after clear"),
        InjectOutcome::Delivered
    );
    assert!(
        mux.panes
            .get("operator-pane")
            .expect("pane exists")
            .is_turn_in_flight(),
        "retained report delivers as soon as the composer becomes clean"
    );
}

#[tokio::test]
async fn nonurgent_inject_never_expires_a_dirty_composer_cas_eacc() {
    let mut mux = Mux::new(24, 80);
    let Some(pane) = codex_cat_pane("timeout-pane") else {
        return;
    };
    mux.add_pane(pane);

    mux.deliver_user_input_to("timeout-pane", b"draft", UserInputKind::KeyStream)
        .await
        .expect("type draft");
    let draft_before = settled_pane_snapshot(&mut mux, "timeout-pane").await;
    let first = mux
        .inject("timeout-pane", "director lifecycle event one")
        .await
        .expect("dirty inject defers");
    let repeated = mux
        .inject("timeout-pane", "director lifecycle event two")
        .await
        .expect("repeated dirty inject also defers");

    assert_eq!(first, InjectOutcome::DeferredComposerDirty);
    assert_eq!(repeated, InjectOutcome::DeferredComposerDirty);
    assert!(
        mux.get("timeout-pane").unwrap().is_composer_dirty(),
        "elapsed time and repeated events must never make the draft writable"
    );
    assert!(
        !mux.get("timeout-pane").unwrap().is_turn_in_flight(),
        "neither event may append to or submit the unfinished human draft"
    );
    let draft_after = settled_pane_snapshot(&mut mux, "timeout-pane").await;
    assert_eq!(
        draft_after, draft_before,
        "repeated director events must leave the Codex supervisor draft byte-for-byte intact"
    );

    mux.deliver_user_input_to("timeout-pane", b"\r", UserInputKind::KeyStream)
        .await
        .expect("human submits at the safe boundary");
    assert_eq!(
        mux.inject("timeout-pane", "director lifecycle event one")
            .await
            .unwrap(),
        InjectOutcome::Delivered
    );
    assert_eq!(
        mux.inject("timeout-pane", "director lifecycle event two")
            .await
            .unwrap(),
        InjectOutcome::DeferredComposerDirty,
        "a second event stays separate while the first awaits its Codex submit CR"
    );
    tokio::time::sleep(std::time::Duration::from_millis(550)).await;
    assert_eq!(
        mux.inject("timeout-pane", "director lifecycle event two")
            .await
            .unwrap(),
        InjectOutcome::Delivered,
        "the next FIFO event releases exactly once after the prior submit boundary"
    );
}

#[tokio::test]
async fn failed_submit_keeps_later_injections_deferred_cas_eacc() {
    let mut mux = Mux::new(24, 80);
    let Some(pane) = codex_cat_pane("submit-failure") else {
        return;
    };
    pane.replace_pty_writer_for_test(Box::new(SubmitFailWriter { writes: 0 }))
        .await
        .expect("install deterministic writer");
    mux.add_pane(pane);

    let InjectStart::Started(ticket) = mux.begin_inject("submit-failure", "director lifecycle event one").unwrap() else {
        panic!("clean pane begins a tracked normal write");
    };
    assert!(mux.advance_injects().await.is_empty(), "payload enqueue is not Delivered");
    assert!(
        mux.get("submit-failure").unwrap().inject_submit_pending(),
        "the lane closes as soon as the payload is written"
    );

    tokio::time::sleep(std::time::Duration::from_millis(550)).await;
    let done = mux.advance_injects().await;
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].ticket, ticket);
    assert!(done[0].result.as_ref().unwrap_err().to_string().contains("submit"), "failed CR never yields Delivered");

    assert!(
        mux.get("submit-failure").unwrap().inject_submit_pending(),
        "a failed submit CR must keep the pane unsafe instead of reopening the lane"
    );
    assert_eq!(
        mux.inject("submit-failure", "director lifecycle event two")
            .await
            .expect("unsafe pane causes durable deferral"),
        InjectOutcome::DeferredComposerDirty,
        "later non-urgent lifecycle payload must not coalesce with the unsubmitted first payload"
    );
    assert_eq!(
        mux.interrupt_and_inject_preserving_composer(
            "submit-failure",
            "urgent lifecycle event",
            std::time::Duration::ZERO,
        )
        .await
        .expect("urgent row also stays durable"),
        InjectOutcome::DeferredComposerDirty,
        "urgent semantics must not override the hard unsafe state after submit failure"
    );
}

#[tokio::test]
async fn clean_inject_surfaces_write_failure_cas_0b64() {
    let mut mux = Mux::new(24, 80);
    mux.add_pane(Pane::director("no-backend", 24, 80).expect("director pane"));

    // Pane::director has no writable PTY. State tracking happens before the
    // expected write error so this gives us a deterministic dirty target and
    // a deterministic deferred-delivery failure.
    assert!(
        mux.deliver_user_input_to("no-backend", b"draft", UserInputKind::KeyStream)
            .await
            .is_err()
    );
    assert_eq!(
        mux.inject("no-backend", "must stay durable")
            .await
            .expect("dirty target returns a deferred outcome"),
        InjectOutcome::DeferredComposerDirty
    );
    mux.get("no-backend")
        .unwrap()
        .observe_raw_client_input(b"\x1b");
    assert!(!mux.get("no-backend").unwrap().is_composer_dirty());
    assert!(
        mux.inject("no-backend", "must stay durable").await.is_err(),
        "a clean target must surface the failed real write so the durable queue retries"
    );
}

#[tokio::test]
async fn pane_without_client_input_injects_immediately_cas_1a4d() {
    let mut mux = Mux::new(24, 80);
    let Some(pane) = cat_pane("worker-pane") else {
        return;
    };
    mux.add_pane(pane);

    let outcome = mux
        .inject("worker-pane", "ordinary worker message")
        .await
        .expect("clean worker inject");

    assert_eq!(outcome, InjectOutcome::Delivered);
    assert!(
        mux.panes
            .get("worker-pane")
            .expect("pane exists")
            .is_turn_in_flight(),
        "a pane with no attached-client input keeps immediate delivery"
    );
}

#[tokio::test]
async fn deferred_payload_is_not_retained_across_teardown_or_respawn_cas_0b64() {
    let mut mux = Mux::new(24, 80);
    let Some(old_pane) = cat_pane("reused-name") else {
        return;
    };
    mux.add_pane(old_pane);
    mux.deliver_user_input_to("reused-name", b"operator draft", UserInputKind::KeyStream)
        .await
        .expect("mark old pane dirty");

    assert_eq!(
        mux.inject("reused-name", "old process payload")
            .await
            .expect("defer to durable queue"),
        InjectOutcome::DeferredComposerDirty
    );
    mux.remove_pane("reused-name");

    let Some(new_pane) = cat_pane("reused-name") else {
        return;
    };
    mux.add_pane(new_pane);
    mux.flush_deferred_injections().await;

    assert!(
        !mux.panes
            .get("reused-name")
            .expect("respawned pane exists")
            .is_turn_in_flight(),
        "old deferred payload must never be flushed into a same-name respawn"
    );
}

// ── cas-4208: post-break settle must observe real quiescence, not a blind
// sleep, for harnesses without a real textbox submit (Codex) ────────────────

/// A real PTY-backed pane running a script that emits a short burst of
/// output over ~600ms and then goes quiet (staying alive afterward so the
/// pane doesn't exit mid-test) — a stand-in for Codex's post-Esc
/// "Conversation interrupted" transition, which keeps redrawing for a while
/// before the composer is actually ready for fresh input again. A live
/// repro against the real `codex` binary (task cas-4208 notes) showed a
/// flat sleep can race exactly that transition and silently swallow the
/// follow-up submit; these tests pin the fix against a cheap, deterministic
/// stand-in rather than requiring the real binary. Returns `None` if `sh`
/// isn't available so the test skips rather than flakes.
fn ticking_pane(name: &str, harness: SupervisorCli) -> Option<Pane> {
    let config = crate::pty::PtyConfig {
        command: "sh".to_string(),
        args: vec![
            "-c".to_string(),
            "i=0; while [ $i -lt 6 ]; do i=$((i+1)); echo tick$i; sleep 0.1; done; sleep 5"
                .to_string(),
        ],
        cwd: Some(PathBuf::from("/tmp")),
        env: vec![],
        env_remove: vec![],
        rows: 24,
        cols: 80,
    };
    let pty = crate::pty::Pty::spawn(name, config).ok()?;
    Pane::with_pty(name, PaneKind::Worker, pty, 24, 80, harness).ok()
}

/// The multi-line payload shape from the actual cas-4208 incident (a blank
/// line plus indented follow-up lines) — pinned specifically so a future
/// single-line probe can't re-certify this path as working (the original
/// cas-8d76 live verification used exactly that too-simple single-line
/// probe, which is why this regression shipped unnoticed).
const MULTILINE_REDIRECT: &str =
    "STOP. Abandon that and instead run:\n\n  echo redirected\n\nThen reply INTERRUPTED-OK.";

#[tokio::test]
async fn interrupt_and_inject_waits_for_real_output_quiescence_on_codex() {
    let Some(pane) = ticking_pane("cas4208-codex-tick", SupervisorCli::Codex) else {
        return; // no PTY available — skip
    };
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);

    // Deliberately far shorter than the ~600ms output burst the pane emits:
    // proves the wait is driven by observed quiescence, not just this floor.
    let floor = std::time::Duration::from_millis(50);
    let start = std::time::Instant::now();
    let res = mux
        .interrupt_and_inject("cas4208-codex-tick", MULTILINE_REDIRECT, floor)
        .await;
    let elapsed = start.elapsed();

    assert!(res.is_ok(), "urgent redirect to a live pane must succeed");
    assert!(
        elapsed >= std::time::Duration::from_millis(500),
        "Codex (supports_textbox_submit=false) must wait for the pane's own \
         output burst to actually go quiet (~600ms here) rather than injecting \
         right after the {floor:?} floor — got elapsed={elapsed:?}. This is the \
         cas-4208 regression: a flat sleep races a still-transitioning child \
         and can leave the correction sitting unsent in the composer."
    );
}

#[tokio::test]
async fn interrupt_and_inject_keeps_flat_floor_for_textbox_submit_harnesses() {
    // Same ticking child, tagged Claude this time — must NOT wait for the
    // burst. Regression guard for cas-4208 AC4: Claude/Grok behavior (a flat
    // sleep already recovers reliably per that task's live control-group
    // evidence) must stay exactly as before.
    let Some(pane) = ticking_pane("cas4208-claude-tick", SupervisorCli::Claude) else {
        return; // no PTY available — skip
    };
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);

    let floor = std::time::Duration::from_millis(50);
    let start = std::time::Instant::now();
    let res = mux
        .interrupt_and_inject("cas4208-claude-tick", MULTILINE_REDIRECT, floor)
        .await;
    let elapsed = start.elapsed();

    assert!(res.is_ok(), "urgent redirect to a live pane must succeed");
    assert!(
        elapsed < std::time::Duration::from_millis(400),
        "Claude/Grok (supports_textbox_submit=true) must keep the old flat-floor \
         behavior unchanged — got elapsed={elapsed:?}, which suggests the new \
         quiescence poll leaked into a harness that must not use it"
    );
}

// ── cas-06a2: Pane::resize must reach the kernel PTY winsize ────────────────

#[test]
fn pane_resize_sets_kernel_pty_winsize() {
    let Some(mut pane) = cat_pane("winsize") else {
        eprintln!("skipping: PTY spawn unavailable in this environment");
        return;
    };
    assert_eq!(pane.pty_winsize(), Some((24, 80)), "spawn size");
    for (rows, cols) in [(56, 177), (56, 74), (10, 40), (56, 74)] {
        pane.resize(rows, cols).expect("resize");
        assert_eq!(pane.size(), (rows, cols));
        assert_eq!(
            pane.pty_winsize(),
            Some((rows, cols)),
            "the child's `stty size` must follow every resize"
        );
    }
}

#[test]
fn pty_less_pane_reports_no_winsize() {
    let pane = Pane::director("d", 24, 80).unwrap();
    assert_eq!(pane.pty_winsize(), None);
}

fn spawn_test_pty(name: &str, script: &str) -> crate::pty::Pty {
    crate::pty::Pty::spawn(
        name,
        crate::pty::PtyConfig {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            cwd: Some(PathBuf::from("/tmp")),
            env: vec![],
            env_remove: vec![],
            rows: 24,
            cols: 80,
        },
    )
    .expect("spawn test pty")
}

/// Factory loop v2 §7 (cas-786a6): a pane whose child stopped reading fills
/// its PTY buffer and then its input buffer, and stops taking input. No
/// write waits on it, and another pane's echo is unaffected.
#[test]
fn flv2_pty_write_never_blocks_lane() {
    // Raw mode without echo: the line discipline neither echoes nor discards,
    // so unread input fills the kernel buffer and a blocking write waits.
    let mut stalled = spawn_test_pty("stalled", "stty raw -echo; printf ready; exec sleep 60");
    let mut echo = spawn_test_pty("echo", "printf ready; exec cat");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for pty in [&mut stalled, &mut echo] {
        runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while let Some(event) = pty.recv().await {
                    if matches!(&event, crate::pty::PtyEvent::Output(bytes) if String::from_utf8_lossy(bytes).contains("ready")) {
                        break;
                    }
                }
            })
            .await
            .expect("pane printed ready");
        });
    }

    // The writes run on their own thread so a write that blocks fails the
    // test instead of hanging it.
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let chunk = vec![b'x'; 4096];
        let mut slowest = std::time::Duration::ZERO;
        let mut refused = None;
        for _ in 0..512 {
            let started = std::time::Instant::now();
            let result = runtime.block_on(stalled.write(&chunk));
            slowest = slowest.max(started.elapsed());
            if let Err(error) = result {
                refused = Some(error.to_string());
                break;
            }
        }
        let _ = done_tx.send((slowest, refused));
        stalled
    });
    let (slowest, refused) = done_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("2 MiB of writes to a pane that does not read returned within 5 s");
    let _stalled = writer.join().unwrap();
    assert!(
        slowest < std::time::Duration::from_millis(50),
        "a PTY write waited {slowest:?} on a pane that does not read"
    );
    let refused = refused.expect("the full pane stops taking input");
    assert!(refused.contains("buffer full"), "refusal names the full buffer: {refused}");

    // The other pane still echoes promptly.
    let started = std::time::Instant::now();
    runtime.block_on(async {
        echo.write(b"ping\r").await.expect("the echo pane takes input");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let mut seen = String::new();
            while let Some(event) = echo.recv().await {
                if let crate::pty::PtyEvent::Output(bytes) = event {
                    seen.push_str(&String::from_utf8_lossy(&bytes));
                    if seen.contains("ping") {
                        break;
                    }
                }
            }
        })
        .await
        .expect("the echo pane echoed while the other pane was full");
    });
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}

/// cas-786a6: the urgent inject is a per-pane state machine. The turn break
/// goes out at once, the payload follows on a later advance once the floor
/// has passed, and nothing else is typed into the pane meanwhile.
#[tokio::test]
async fn urgent_inject_is_a_per_pane_state_machine() {
    let mut mux = Mux::new(24, 80);
    let pty = spawn_test_pty("urgent-pane", "exec cat");
    let pane = Pane::with_pty("urgent-pane", PaneKind::Worker, pty, 24, 80, SupervisorCli::Claude)
        .expect("pane");
    mux.add_pane(pane);
    let floor = std::time::Duration::from_millis(300);

    let started = std::time::Instant::now();
    let InjectStart::Started(ticket) = mux
        .begin_interrupt_and_inject("urgent-pane", "redirect", floor, true)
        .await
        .unwrap()
    else {
        panic!("a clean pane starts the inject");
    };
    assert!(started.elapsed() < std::time::Duration::from_millis(100), "begin never waits for the settle");
    assert!(mux.inject_pending("urgent-pane"));
    assert!(mux.advance_injects().await.is_empty(), "the floor has not passed");
    assert_eq!(
        mux.inject("urgent-pane", "normal").await.unwrap(),
        InjectOutcome::DeferredComposerDirty,
        "nothing is typed between the turn break and the payload"
    );
    assert_eq!(
        mux.begin_interrupt_and_inject("urgent-pane", "second", floor, true)
            .await
            .unwrap(),
        InjectStart::Deferred,
        "one urgent inject per pane"
    );

    tokio::time::sleep(floor).await;
    // The payload is queued on this advance; its write receipt reports on
    // this or a following one.
    let mut done = mux.advance_injects().await;
    for _ in 0..50 {
        if !done.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        done = mux.advance_injects().await;
    }
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].ticket, ticket);
    assert_eq!(done[0].result.as_ref().unwrap(), &InjectOutcome::Delivered);
    assert!(!mux.inject_pending("urgent-pane"));
    assert!(!mux.has_pending_injects());
}

/// Records every byte the pane's writer thread writes; with a gate, blocks
/// until opened, like a child that stopped reading.
struct RecordingWriter {
    out: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    gate: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

impl std::io::Write for RecordingWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let (open, cv) = &*self.gate;
        let mut open = open.lock().unwrap();
        while !*open {
            open = cv.wait(open).unwrap();
        }
        self.out.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

type Recorded = (
    std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
);

async fn recording_pane(name: &str, cli: SupervisorCli, open: bool) -> (Pane, Recorded) {
    let pty = spawn_test_pty(name, "exec sleep 60");
    let pane = Pane::with_pty(name, PaneKind::Worker, pty, 24, 80, cli).expect("pane");
    let out = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let gate = std::sync::Arc::new((std::sync::Mutex::new(open), std::sync::Condvar::new()));
    pane.replace_pty_writer_for_test(Box::new(RecordingWriter {
        out: out.clone(),
        gate: gate.clone(),
    }))
    .await
    .expect("install recording writer");
    (pane, (out, gate))
}

fn open_gate(recorded: &Recorded) {
    *recorded.1.0.lock().unwrap() = true;
    recorded.1.1.notify_all();
}

/// cas-786a6 review H1: a submit CR queued while the pane's input buffer is
/// full is still taken, so the payload is submitted once the child reads and
/// the pane is not left deaf.
#[tokio::test]
async fn a_full_input_buffer_never_strands_the_submit_cr() {
    let (pane, recorded) = recording_pane("cr-full", SupervisorCli::Claude, false).await;
    pane.inject_prompt("redirect").await.expect("payload queued");
    // Operator input fills the buffer while the child is not reading.
    let chunk = vec![b'k'; 64 * 1024];
    let mut refused = false;
    for _ in 0..16 {
        if pane.write(&chunk).await.is_err() {
            refused = true;
            break;
        }
    }
    assert!(refused, "the buffer filled");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(pane.inject_submit_pending());
    open_gate(&recorded);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while pane.inject_submit_pending() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        !pane.inject_submit_pending(),
        "the CR was written once the child read; the pane takes injects again"
    );
    assert_eq!(recorded.0.lock().unwrap().last(), Some(&b'\r'));
}

/// cas-786a6 review M3: operator keys typed during a destructive urgent
/// inject's settle are held and delivered after the payload and its submit
/// CR, never inside them.
#[tokio::test]
async fn operator_input_during_the_settle_is_held_until_the_payload_is_submitted() {
    let mut mux = Mux::new(24, 80);
    let (pane, recorded) = recording_pane("held", SupervisorCli::Claude, true).await;
    mux.add_pane(pane);
    let floor = std::time::Duration::from_millis(100);
    assert!(matches!(
        mux.begin_interrupt_and_inject("held", "/clear", floor, false)
            .await
            .unwrap(),
        InjectStart::Started(_)
    ));
    mux.send_input_to("held", b"fix the").await.unwrap();
    mux.deliver_user_input_to("held", b"\r", crate::pane::UserInputKind::KeyStream)
        .await
        .unwrap();
    assert!(mux.input_held("held"));
    tokio::time::sleep(floor).await;
    let mut done = mux.advance_injects().await;
    for _ in 0..50 {
        if !done.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        done = mux.advance_injects().await;
    }
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].result.as_ref().unwrap(), &InjectOutcome::Delivered);
    // The submit CR follows 150 ms later; held input waits for it.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    mux.advance_injects().await;
    assert!(!mux.input_held("held"));
    mux.get("held").unwrap().input_drained_for_test();
    let written = String::from_utf8_lossy(&recorded.0.lock().unwrap()).to_string();
    let payload = written.find("/clear").expect("payload written");
    let cr = written[payload..].find('\r').map(|at| payload + at).expect("submit CR");
    let typed = written.find("fix the").expect("held input delivered");
    assert!(payload < cr && cr < typed, "Esc, payload, CR, then the operator's keys: {written:?}");
}

/// cas-786a6 review L3/L4: an inject whose pane closes, or is respawned
/// under the same name, mid-settle fails; the new pane is never typed into.
#[tokio::test]
async fn a_pane_closed_or_respawned_mid_settle_fails_its_inject() {
    let mut mux = Mux::new(24, 80);
    let (pane, _) = recording_pane("gone", SupervisorCli::Claude, true).await;
    mux.add_pane(pane);
    let floor = std::time::Duration::from_millis(50);
    mux.begin_interrupt_and_inject("gone", "redirect", floor, true).await.unwrap();
    mux.remove_pane("gone");
    tokio::time::sleep(floor).await;
    let done = mux.advance_injects().await;
    assert_eq!(done.len(), 1);
    assert!(done[0].result.is_err());

    let (pane, _) = recording_pane("same", SupervisorCli::Claude, true).await;
    mux.add_pane(pane);
    mux.begin_interrupt_and_inject("same", "old payload", floor, true).await.unwrap();
    mux.remove_pane("same");
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let (respawned, recorded) = recording_pane("same", SupervisorCli::Claude, true).await;
    mux.add_pane(respawned);
    tokio::time::sleep(floor).await;
    let done = mux.advance_injects().await;
    assert_eq!(done.len(), 1);
    assert!(done[0].result.is_err(), "a respawned pane is a different pane");
    mux.get("same").unwrap().input_drained_for_test();
    assert!(!String::from_utf8_lossy(&recorded.0.lock().unwrap()).contains("old payload"));
}

/// cas-786a6 review round 2, M: a second urgent inject on a pane waits until
/// the first payload's submit CR is written, so the pane sees Esc, payload,
/// CR, then Esc, payload, CR, never a turn break between a payload and its CR.
#[tokio::test]
async fn back_to_back_urgent_injects_never_break_between_payload_and_cr() {
    let mut mux = Mux::new(24, 80);
    let (pane, recorded) = recording_pane("b2b", SupervisorCli::Claude, true).await;
    mux.add_pane(pane);
    let floor = std::time::Duration::from_millis(20);
    let mut started = 0;
    let mut finished = 0;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while finished < 2 && std::time::Instant::now() < deadline {
        if started < 2 {
            let payload = if started == 0 { "FIRST" } else { "SECOND" };
            if let InjectStart::Started(_) = mux
                .begin_interrupt_and_inject("b2b", payload, floor, false)
                .await
                .unwrap()
            {
                started += 1;
            }
        }
        finished += mux.advance_injects().await.len();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(finished, 2);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while mux.pane_submit_pending("b2b") && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    mux.get("b2b").unwrap().input_drained_for_test();
    let written = recorded.0.lock().unwrap().clone();
    assert_eq!(
        written,
        b"\x1b\x1b[200~FIRST\x1b[201~\r\x1b\x1b[200~SECOND\x1b[201~\r".to_vec(),
        "{:?}",
        String::from_utf8_lossy(&written)
    );
}

/// cas-786a6 review round 2, L1: a submit gate held for a pane holds the
/// next inject's CR until it opens.
#[tokio::test]
async fn a_held_submit_gate_holds_the_cr_until_it_opens() {
    let (pane, recorded) = recording_pane("gated", SupervisorCli::Claude, true).await;
    let gate = crate::submit_gate::SubmitGate::new();
    crate::submit_gate::hold_next_submit("gated", gate.clone());
    pane.inject_prompt("payload").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    pane.input_drained_for_test();
    assert_eq!(recorded.0.lock().unwrap().as_slice(), b"\x1b[200~payload\x1b[201~", "no CR while the gate is shut");
    assert!(pane.inject_submit_pending());
    gate.open();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while pane.inject_submit_pending() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    pane.input_drained_for_test();
    assert_eq!(recorded.0.lock().unwrap().as_slice(), b"\x1b[200~payload\x1b[201~\r");
}

#[test]
fn cas_569f_claude_role_options_and_cas_only_scope() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_CLAUDE_SUPPORTED_FLAGS", "--name,--remote-control,--fallback-model,--mcp-config,--strict-mcp-config");
    env.set("CAS_FACTORY_NICE_WORKER", "0");
    let project = tempfile::tempdir().unwrap();
    let root = project.path().join(".cas");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.toml"), "[factory.claude]\nremote_control = true\nfallback_model = 'sonnet'\n").unwrap();
    let source = project.path().join(".mcp.json");
    let original = serde_json::json!({"mcpServers": {
        "cas": {"command": "fixture-cas", "env": {"KEEP": "yes"}},
        "external": {"command": "must-not-discover"}
    }}).to_string();
    std::fs::write(&source, &original).unwrap();
    let configs = Mux::factory_pane_configs(&MuxConfig {
        cwd: project.path().into(), cas_root: Some(root.clone()),
        workers: 1, worker_cli: SupervisorCli::Claude,
        supervisor_cli: SupervisorCli::Claude, include_director: false,
        ..Default::default()
    });
    for (name, pane) in &configs {
        assert!(env_value(pane, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
        let value = |flag| pane.args.windows(2).find(|pair| pair[0] == flag).map(|pair| pair[1].as_str());
        assert_eq!(value("--name"), Some(name.as_str()));
        assert_eq!(value("--fallback-model"), Some("sonnet"));
        if env_value(pane, "CAS_AGENT_ROLE") == Some("worker") {
            assert!(!pane.args.iter().any(|arg| arg == "--remote-control"));
            assert!(pane.args.iter().any(|arg| arg == "--strict-mcp-config"));
            let path = value("--mcp-config").unwrap();
            let document: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let servers = document["mcpServers"].as_object().unwrap();
            assert_eq!(servers.keys().collect::<Vec<_>>(), vec!["cas"]);
            assert_eq!(servers["cas"]["command"], "fixture-cas");
            assert_eq!(servers["cas"]["env"]["KEEP"], "yes");
            #[cfg(unix)] {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
            }
        } else {
            assert_eq!(value("--remote-control"), Some(name.as_str()));
            assert!(!pane.args.iter().any(|arg| arg == "--strict-mcp-config"));
            assert!(env_value(pane, "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC").is_none());
        }
    }
    assert_eq!(std::fs::read_to_string(source).unwrap(), original);
}

#[test]
fn cas_569f_claude_unsupported_and_denied_launches_refuse_before_spawn() {
    let mut env = TestEnvGuard::temp_home();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().join(".cas");
    std::fs::create_dir_all(&root).unwrap();
    for (flags, options, worker_error, supervisor_error) in [
        ("", "", true, false),
        ("--mcp-config", "", true, false),
        ("--strict-mcp-config", "", true, false),
        ("--mcp-config,--strict-mcp-config", "[factory.claude]\nremote_control=true", false, true),
        ("--mcp-config,--strict-mcp-config", "[factory.claude]\nfallback_model='sonnet'", true, true),
        ("--mcp-config,--strict-mcp-config", "[factory]\nsupervisor_only_mcp=['cas']", true, false),
        ("--mcp-config,--strict-mcp-config", "[factory.claude]\nremote_control='invalid'", true, true),
    ] {
        env.set("CAS_FACTORY_CLAUDE_SUPPORTED_FLAGS", flags);
        std::fs::write(root.join("config.toml"), options).unwrap();
        let configs = Mux::factory_pane_configs(&MuxConfig {
            cwd: project.path().into(), cas_root: Some(root.clone()), workers: 1,
            worker_cli: SupervisorCli::Claude, supervisor_cli: SupervisorCli::Claude,
            include_director: false, ..Default::default()
        });
        for (_, pane) in configs {
            let is_worker = env_value(&pane, "CAS_AGENT_ROLE") == Some("worker");
            let should_refuse = if is_worker {worker_error} else {supervisor_error};
            assert_eq!(env_value(&pane, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_some(), should_refuse,
                "flags={flags}, options={options}, worker={is_worker}");
            if should_refuse {
                assert!(crate::pty::Pty::spawn("refused-before-discovery", pane).is_err());
            }
        }
    }
}

#[test]
fn cas_569f_defaults_and_native_cas_preserve_environment_denials() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_CLAUDE_SUPPORTED_FLAGS", "--mcp-config,--strict-mcp-config");
    let project = tempfile::tempdir().unwrap();
    let root = project.path().join(".cas");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.toml"), "[factory]\nsupervisor_only_env=['CAS_ROOT']").unwrap();
    let configs = Mux::factory_pane_configs(&MuxConfig {
        cwd: project.path().into(), cas_root: Some(root.clone()), workers: 1,
        worker_cli: SupervisorCli::Claude, supervisor_cli: SupervisorCli::Claude,
        include_director: false, ..Default::default()
    });
    for (_, pane) in configs {
        assert!(env_value(&pane, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
        for absent in ["--name", "--remote-control", "--fallback-model"] {
            assert!(!pane.args.contains(&absent.to_string()));
        }
        if env_value(&pane, "CAS_AGENT_ROLE") == Some("worker") {
            let document: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("worker-mcp/worker-1.json")).unwrap()).unwrap();
            let server = &document["mcpServers"]["cas"];
            assert_eq!(server["command"], "cas");
            assert_eq!(server["args"], serde_json::json!(["serve"]));
            assert!(server["env"].get("CAS_ROOT").is_none());
            assert!(pane.env_remove.contains(&"CAS_ROOT".into()));
            assert_eq!(env_value(&pane, "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"), Some("1"));
        }
    }
}

#[test]
fn cas_569f_dynamic_claude_launch_keeps_account_team_and_factory_identity() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_CLAUDE_SUPPORTED_FLAGS", "--name,--fallback-model,--mcp-config,--strict-mcp-config");
    let project = tempfile::tempdir().unwrap();
    let root = project.path().join(".cas");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.toml"), "[factory.claude]\nfallback_model='haiku'\n").unwrap();
    let account = env.home().join("selected-account");
    std::fs::create_dir_all(&account).unwrap();
    let teams = crate::pty::TeamsSpawnConfig {
        team_name: "fixture-team".into(), agent_id: "dynamic@fixture-team".into(),
        agent_name: "dynamic".into(), agent_color: "blue".into(),
        agent_type: "general-purpose".into(), parent_session_id: Some("lead-session".into()),
        lead_session_id: None, settings_path: Some("fixture-settings.json".into()),
    };
    let config = MuxConfig { factory_session: Some("factory-569f".into()), ..Default::default() };
    let mux = Mux::factory_state_for_test(&config);
    let spec = WorkerSpec { config_dir: Some(account.display().to_string()), ..WorkerSpec::builtin_default() };
    let pane = mux.build_add_worker_config("dynamic", project.path().into(), Some(&root), "supervisor", Some(&teams), Some(spec));
    assert!(env_value(&pane, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none());
    for (flag, value) in [("--name", "dynamic"), ("--fallback-model", "haiku"),
        ("--team-name", "fixture-team"), ("--settings", "fixture-settings.json"),
        ("--disallowedTools", "AskUserQuestion"), ("--permission-mode", "bypassPermissions")] {
        assert!(pane.args.windows(2).any(|pair| pair == [flag, value]), "missing {flag} {value}");
    }
    assert!(pane.args.contains(&"--strict-mcp-config".into()));
    assert_eq!(env_value(&pane, "CLAUDE_CONFIG_DIR"), account.to_str());
    assert_eq!(env_value(&pane, "CAS_FACTORY_SESSION"), Some("factory-569f"));
    assert_eq!(env_value(&pane, "CAS_SUPERVISOR_NAME"), Some("supervisor"));
}

#[test]
fn cas_940c_worker_mcp_modes_preserve_supervisor_and_codex_launches() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_CLAUDE_SUPPORTED_FLAGS", "--mcp-config,--strict-mcp-config");
    for mode in [None, Some("cas_only"), Some("inherit")] {
        for cli in [SupervisorCli::Claude, SupervisorCli::Codex] {
            let project = tempfile::tempdir().unwrap();
            let root = project.path().join(".cas");
            std::fs::create_dir_all(&root).unwrap();
            if let Some(mode) = mode {
                std::fs::write(root.join("config.toml"), format!("[factory.claude]\nworker_mcp='{mode}'\n")).unwrap();
            }
            let original = serde_json::json!({"mcpServers": {
                "cas": {"command": "cas-fixture"},
                "additional": {"command": "additional-fixture"}
            }}).to_string();
            std::fs::write(project.path().join(".mcp.json"), &original).unwrap();
            let configs = Mux::factory_pane_configs(&MuxConfig {
                cwd: project.path().into(), cas_root: Some(root.clone()), workers: 1,
                worker_cli: cli, supervisor_cli: SupervisorCli::Claude,
                include_director: false, ..Default::default()
            });
            for (_, pane) in configs {
                assert!(env_value(&pane, "CAS_FACTORY_WORKER_LAUNCH_ERROR").is_none(), "{mode:?} {cli:?}");
                let strict = cli == SupervisorCli::Claude && mode != Some("inherit") && env_value(&pane, "CAS_AGENT_ROLE") == Some("worker");
                for flag in ["--strict-mcp-config", "--mcp-config"] {
                    assert_eq!(pane.args.contains(&flag.to_string()), strict, "mode={mode:?} cli={cli:?} flag={flag}");
                }
                if env_value(&pane, "CAS_AGENT_ROLE") == Some("worker") {
                    let path = root.join("worker-mcp/worker-1.json");
                    assert_eq!(path.exists(), strict, "mode={mode:?} cli={cli:?}");
                    if strict {
                        let document: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                        assert_eq!(document["mcpServers"].as_object().unwrap().keys().collect::<Vec<_>>(), vec!["cas"]);
                    }
                }
            }
            assert_eq!(std::fs::read_to_string(project.path().join(".mcp.json")).unwrap(), original);
        }
    }
}

#[test]
fn cas_940c_inherit_refuses_denials_and_needs_no_strict_capability() {
    let mut env = TestEnvGuard::temp_home();
    // Inherit must also work on a CLI without strict/config options.
    env.set("CAS_FACTORY_CLAUDE_SUPPORTED_FLAGS", "");
    for (policy, refused) in [
        ("", false),
        ("worker_credential_env=['CAS_940C_ALLOWED']", false),
        ("supervisor_only_mcp=['external']", true),
        ("supervisor_only_env=['PRIVATE_FIXTURE']", true),
    ] {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("config.toml"), format!("[factory]\n{policy}\n[factory.claude]\nworker_mcp='inherit'\n")).unwrap();
        let mut pane = Mux::new(24, 80).build_add_worker_config("worker", project.path().into(), Some(&root), "supervisor", None, None);
        let error = env_value(&pane, "CAS_FACTORY_WORKER_LAUNCH_ERROR");
        assert_eq!(error.is_some(), refused, "policy={policy}: {error:?}");
        assert!(!root.join("worker-mcp/worker.json").exists());
        assert!(!pane.args.contains(&"--strict-mcp-config".into()));
        if refused {
            let message = error.unwrap();
            assert!(message.contains("cas_only") && message.contains("denial policy"));
            pane.command = "/bin/sh".into();
            pane.args = vec!["-c".into(), "exit 0".into()];
            assert!(crate::pty::Pty::spawn("inherit-refused-before-discovery", pane).is_err());
        }
    }
}
