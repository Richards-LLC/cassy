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
    let pty = Pty::spawn("kept", PtyConfig {
        command: "sh".into(), args: vec!["-c".into(), "read line; printf '%s\\n' \"$line\"; read end".into()],
        ..Default::default()
    }).unwrap();
    let mut pane = Pane::with_pty("kept", PaneKind::Worker, pty, 24, 80, SupervisorCli::Codex).unwrap();
    pane.feed(b"scrollback before refusal\r\n").unwrap();
    let pid = pane.process_group_id().unwrap();
    let before = pane.dump_viewport().unwrap();
    daemon.app.mux.add_pane(pane);
    let fd = daemon.listener.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    let error = daemon.reexec(std::path::Path::new("/usr/bin/false"), "next", None, "refusal").await.unwrap_err();
    assert!(error.to_string().contains("preflight"), "{error:#}");
    assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, flags);
    let pane = daemon.app.mux.get_mut("kept").unwrap();
    assert_eq!(pane.process_group_id(), Some(pid));
    assert_eq!(pane.dump_viewport().unwrap(), before);
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, 0);
    pane.deliver_user_input(b"still-live\r", cas_mux::UserInputKind::KeyStream).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            pane.drain_output();
            if pane.dump_viewport().unwrap().contains("still-live") { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    pane.kill_tree_force();
}
