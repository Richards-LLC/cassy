//! Refusal must leave the running session's handles and terminal untouched.
use super::provisioning_tests::daemon;
use cas_mux::{Pane, PaneKind, Pty, PtyConfig, SupervisorCli};
use std::os::fd::AsRawFd;

#[tokio::test(flavor = "current_thread")]
async fn refused_reexec_preflight_leaves_live_session_untouched() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let dir = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon(&root);
    let pty = Pty::spawn(
        "kept",
        PtyConfig {
            command: "sh".into(),
            args: vec![
                "-c".into(),
                "read line; printf '%s\\n' \"$line\"; read end".into(),
            ],
            ..Default::default()
        },
    )
    .unwrap();
    let mut pane =
        Pane::with_pty("kept", PaneKind::Worker, pty, 24, 80, SupervisorCli::Codex).unwrap();
    pane.feed(b"scrollback before refusal\r\n").unwrap();
    let pid = pane.process_group_id().unwrap();
    let before = pane.dump_viewport().unwrap();
    daemon.app.mux.add_pane(pane);
    let fd = daemon.listener.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    let error = daemon
        .reexec(
            std::path::Path::new("/usr/bin/false"),
            "next",
            None,
            "refusal",
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("preflight"), "{error:#}");
    assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, flags);
    let pane = daemon.app.mux.get_mut("kept").unwrap();
    assert_eq!(pane.process_group_id(), Some(pid));
    assert_eq!(pane.dump_viewport().unwrap(), before);
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, 0);
    pane.deliver_user_input(b"still-live\r", cas_mux::UserInputKind::KeyStream)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            pane.drain_output();
            if pane.dump_viewport().unwrap().contains("still-live") {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    pane.kill_tree_force();
}

const EXEC_TEST: &str = "ui::factory::daemon::runtime::reexec_tests::reexec_adopts_two_panes_and_rolls_back_failed_candidate";

#[test]
fn reexec_adopts_two_panes_and_rolls_back_failed_candidate() {
    if let Some(root) = std::env::var_os("CAS_REEXEC_TEST_ROOT") {
        let _guard = crate::test_env_guard::TestEnvGuard::new();
        if std::env::var_os("CAS_DAEMON_HANDOFF_FD").is_some() {
            assert_eq!(
                super::super::reexec::resume_inherited_daemon(),
                Some(std::process::ExitCode::SUCCESS)
            );
            return;
        }
        let root = std::path::PathBuf::from(root);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut daemon = daemon(&root);
            daemon.app.set_factory_session("provision-test".into());
            for index in 0..2 {
                let name = format!("kept-{index}");
                let rows = 4 + index;
                let cols = 50 + index;
                let pty = Pty::spawn(&name, PtyConfig {
                    command: "sh".into(), args: vec!["-c".into(), "read line; printf 'received:%s\\n' \"$line\"; read end".into()],
                    rows, cols, ..Default::default()
                }).unwrap();
                let mut pane = Pane::with_pty(&name, PaneKind::Worker, pty, rows, cols, SupervisorCli::Codex).unwrap();
                for row in 0..10 { pane.feed(format!("{name} history {row}\r\n").as_bytes()).unwrap(); }
                daemon.app.mux.add_pane(pane);
            }
            let metadata = crate::ui::factory::create_metadata("provision-test", std::process::id(),
                daemon.app.supervisor_name(), &[], None, Some(root.parent().unwrap().to_str().unwrap()), None);
            crate::ui::factory::SessionManager::new().save_daemon_metadata(metadata, &root).unwrap();
            std::fs::write(root.join("before.json"), serde_json::to_vec(&serde_json::json!({
                "pid": std::process::id(), "panes": daemon.app.mux.panes().map(|pane| (pane.id(), pane.process_group_id())).collect::<Vec<_>>()
            })).unwrap()).unwrap();
            let binary = std::env::var_os("CAS_REEXEC_TEST_BINARY").unwrap();
            daemon.reexec(std::path::Path::new(&binary), "next", None, "same-pid").await.unwrap();
            panic!("successful exec must not return");
        });
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let env = crate::test_env_guard::TestEnvGuard::temp_home();
    for fail_adopt in [false, true] {
        let project = env
            .home()
            .join(if fail_adopt { "fallback" } else { "update" });
        std::fs::create_dir_all(&project).unwrap();
        let root = crate::store::init_cas_dir(&project).unwrap();
        let wrapper = project.join("next-cas");
        let executable = std::env::current_exe().unwrap();
        let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
        std::fs::write(&wrapper, format!("#!/bin/sh\nif [ \"$1\" = factory ] && [ \"$2\" = handoff-check ]; then printf '%s\\n' '{{\"supported_schemas\":[1]}}'; exit 0; fi\nexport CAS_REEXEC_TEST_VERSION=next\n{}\nexec {} \"$@\"\n",
            if fail_adopt { "export CAS_REEXEC_TEST_PANIC=1" } else { ":" }, quote(executable.to_str().unwrap()))).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let log = std::fs::File::create(project.join("child.log")).unwrap();
        let mut child = std::process::Command::new(&executable)
            .args(["--exact", EXEC_TEST, "--nocapture"])
            .env("CAS_REEXEC_TEST_ROOT", &root)
            .env("CAS_REEXEC_TEST_BINARY", &wrapper)
            .env("CAS_REEXEC_TEST_VERSION", "old")
            .env_remove("CAS_REEXEC_TEST_PANIC")
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!(
                    "reexec child timed out: {}",
                    std::fs::read_to_string(project.join("child.log")).unwrap()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert!(
            status.success(),
            "{}",
            std::fs::read_to_string(project.join("child.log")).unwrap()
        );
        let receipt: super::super::reexec::ReexecReceipt =
            serde_json::from_slice(&std::fs::read(root.join("after.json")).unwrap()).unwrap();
        assert_eq!(receipt.pid, child.id());
        assert_eq!(receipt.fallback, fail_adopt);
        assert_eq!(receipt.version, if fail_adopt { "old" } else { "next" });
        assert_eq!(receipt.panes_adopted, 2);
    }
}

pub(crate) async fn finish_exec_probe(
    daemon: &mut super::super::FactoryDaemon,
    receipt: &super::super::reexec::ReexecReceipt,
) -> anyhow::Result<()> {
    let root = daemon.app.cas_dir().to_path_buf();
    let before: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("before.json"))?)?;
    assert_eq!(before["pid"].as_u64(), Some(std::process::id() as u64));
    let panes = before["panes"].as_array().unwrap();
    for original in panes {
        let name = original[0].as_str().unwrap();
        let pane = daemon.app.mux.get_mut(name).unwrap();
        assert_eq!(
            pane.process_group_id(),
            original[1].as_u64().map(|pid| pid as u32)
        );
        assert!(pane.scrollback_info().total_scrollback > 4);
        let payload = format!("after-{name}");
        pane.deliver_user_input(
            format!("{payload}\r").as_bytes(),
            cas_mux::UserInputKind::KeyStream,
        )
        .await?;
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                pane.drain_output();
                if pane
                    .dump_viewport()
                    .unwrap()
                    .contains(&format!("received:{payload}"))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;
    }
    for pane in daemon.app.mux.panes_mut() {
        pane.kill_tree_force();
    }
    std::fs::write(root.join("after.json"), serde_json::to_vec(receipt)?)?;
    Ok(())
}
