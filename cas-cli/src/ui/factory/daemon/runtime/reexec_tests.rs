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
        // TestEnvGuard deliberately scrubs CAS_*; retain only this isolated
        // subprocess's explicit protocol inputs across that scrub.
        let protocol: Vec<_> = std::env::vars_os()
            .filter(|(key, _)| {
                let key = key.to_string_lossy();
                key.starts_with("CAS_REEXEC_TEST_")
                    || matches!(
                        key.as_ref(),
                        "CAS_DAEMON_HANDOFF_FD"
                            | "CAS_DAEMON_HANDOFF_FALLBACK"
                            | "CAS_DAEMON_ROLLBACK_BINARY"
                    )
            })
            .collect();
        let mut guard = crate::test_env_guard::TestEnvGuard::new();
        for (key, value) in protocol {
            guard.set(key, value);
        }
        if let Ok(fd) = std::env::var("CAS_DAEMON_HANDOFF_FD") {
            let fallback =
                std::env::var("CAS_DAEMON_HANDOFF_FALLBACK").ok().as_deref() == Some("1");
            let rollback =
                std::env::var_os("CAS_DAEMON_ROLLBACK_BINARY").map(std::path::PathBuf::from);
            guard.remove("CAS_DAEMON_HANDOFF_FD");
            guard.remove("CAS_DAEMON_HANDOFF_FALLBACK");
            guard.remove("CAS_DAEMON_ROLLBACK_BINARY");
            assert_eq!(
                super::super::reexec::resume_inherited_daemon(fd, fallback, rollback),
                std::process::ExitCode::SUCCESS
            );
            return;
        }
        assert_eq!(
            std::env::var("CAS_REEXEC_TEST_VERSION").as_deref(),
            Ok("old"),
            "an exec image without its handoff marker must fail, never spawn another fixture"
        );
        let root = std::path::PathBuf::from(root);
        crate::server_signals::ignore_sigpipe_for_server();
        let missing = root.join("missing-exec-image");
        let error = super::super::reexec::exec_image(&missing, -1, false, &missing);
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        let mut signal: libc::sigaction = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::sigaction(libc::SIGPIPE, std::ptr::null(), &mut signal) }, 0);
        assert_eq!(signal.sa_sigaction, libc::SIG_IGN, "failed exec must retain server SIGPIPE handling");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut daemon = daemon(&root);
            daemon.app.set_factory_session("provision-test".into());
            crate::ui::factory::SessionManager::new().ensure_dir().unwrap();
            let gui = crate::ui::factory::gui_socket_path("provision-test");
            if gui.exists() { std::fs::remove_file(&gui).unwrap(); }
            daemon.gui_listener = std::os::unix::net::UnixListener::bind(gui).unwrap();
            daemon.gui_listener.set_nonblocking(true).unwrap();
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
                daemon.app.adopt_worker_pane_for_test(pane);
            }
            let metadata = crate::ui::factory::create_metadata("provision-test", std::process::id(),
                daemon.app.supervisor_name(), daemon.app.worker_names(), None, Some(root.parent().unwrap().to_str().unwrap()), None);
            crate::ui::factory::SessionManager::new().save_daemon_metadata(metadata, &root).unwrap();
            std::fs::write(root.join("before.json"), serde_json::to_vec(&serde_json::json!({
                "pid": std::process::id(), "argv": std::env::args_os().collect::<Vec<_>>(), "panes": daemon.app.mux.panes().map(|pane| (pane.id(), pane.process_group_id())).collect::<Vec<_>>()
            })).unwrap()).unwrap();
            std::fs::write(root.join("ready"), "ready").unwrap();
            loop {
                if daemon.accept_gui_clients() {
                    use std::io::Write;
                    // Replay the real transport seam: a preceding output frame
                    // was only partly written when the reexec request arrived.
                    let frame = crate::ui::factory::protocol::encode_message(
                        &crate::ui::factory::protocol::DaemonMessage::Output {
                            pane_id: "kept-0".into(), data: b"queued before exec".to_vec(),
                        }).unwrap();
                    let cut = crate::ui::factory::protocol::FRAME_HEADER_SIZE + 2;
                    for client in daemon.gui_clients.values_mut() {
                        client.stream.write_all(&frame[..cut]).unwrap();
                        client.write_buf.extend(&frame[cut..]);
                    }
                }
                daemon.process_gui_client_input().await;
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    // Large debug images must never be staged in a RAM-backed temporary HOME.
    // The caller supplies the host's scratch TMPDIR; CI defaults to its checkout.
    let scratch = std::env::var_os("TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| crate::temp_hygiene::default_scratch_root("cas-reexec-tests"));
    std::fs::create_dir_all(&scratch).unwrap();
    let home = tempfile::Builder::new()
        .prefix("cas-reexec-")
        .tempdir_in(&scratch)
        .unwrap();
    // Unix socket names have a short fixed limit. Keep only a tiny symlink
    // and socket path under /tmp; all fixture files live in the disk HOME.
    let socket_home = tempfile::tempdir_in("/tmp").unwrap();
    let home_alias = socket_home.path().join("h");
    std::os::unix::fs::symlink(home.path(), &home_alias).unwrap();
    let mut env = crate::test_env_guard::TestEnvGuard::new();
    env.set("HOME", &home_alias);
    env.set("TMPDIR", &scratch);
    for (key, directory) in [
        ("XDG_CONFIG_HOME", ".config"),
        ("XDG_DATA_HOME", ".local/share"),
        ("XDG_CACHE_HOME", ".cache"),
    ] {
        env.set(key, home.path().join(directory));
    }
    for fail_adopt in [false, true] {
        let project = home_alias.join(if fail_adopt { "fallback" } else { "update" });
        std::fs::create_dir_all(&project).unwrap();
        crate::store::init_cas_dir(&project).unwrap();
        let root = project.join(".cas");
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
        let ready_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !root.join("ready").exists() {
            if std::time::Instant::now() >= ready_deadline || child.try_wait().unwrap().is_some() {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "reexec fixture did not start: {}",
                    std::fs::read_to_string(project.join("child.log")).unwrap()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let wire_receipt = crate::cli::factory::reexec::request("provision-test", &wrapper)
            .unwrap_or_else(|error| {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "{error:#}: {}",
                    std::fs::read_to_string(project.join("child.log")).unwrap()
                );
            });
        assert_eq!(wire_receipt.pid, child.id());
        assert_eq!(wire_receipt.fallback, fail_adopt);
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
    assert_eq!(
        before["argv"],
        serde_json::to_value(std::env::args_os().collect::<Vec<_>>())?
    );
    let state = daemon.build_session_state();
    let scrollback = daemon.build_scrollback(&state);
    let panes = before["panes"].as_array().unwrap();
    for original in panes {
        let name = original[0].as_str().unwrap();
        let replay: Vec<u8> = scrollback
            .get(name)
            .expect("reconnecting clients need the adopted pane replay")
            .iter()
            .flatten()
            .copied()
            .collect();
        assert!(
            String::from_utf8_lossy(&replay).contains(&format!("{name} history 0")),
            "reconnecting clients must retain historical lines"
        );
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
