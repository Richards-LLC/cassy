//! Rendering and naming rules for the server registry surface (cas-7c93, GH #87).
//!
//! End-to-end coverage of the MCP actions lives in
//! `cas-cli/tests/server_registry_mcp_test.rs`; these pin the pure parts, so
//! the "what is listening and who started it" contract is testable without
//! spawning anything.

use super::*;
use chrono::Utc;

fn record(name: &str, shared: bool) -> RegisteredServer {
    RegisteredServer {
        id: format!("srv-{name}-1234-0000abcd"),
        name: name.to_string(),
        command: "npm run dev".to_string(),
        cwd: std::path::PathBuf::from("/repo/app"),
        pid: 1234,
        pgid: Some(1234),
        pid_starttime: Some(42),
        expected_port: Some(5173),
        owner_task: Some("cas-7c93".to_string()),
        owner_worker: Some("young-finch-81".to_string()),
        owner_agent_id: None,
        factory_session: Some("session-a".to_string()),
        shared,
        cgroup: None,
        log_path: None,
        docker: None,
        started_at: Utc::now(),
        state: ServerState::Running,
        ended_at: None,
        ended_detail: None,
    }
}

#[test]
fn listing_answers_what_is_listening_and_who_started_it() {
    let line = render_server_line(&record("web", true), ServerLiveness::Live, &[5173], 0);

    assert!(line.contains("web"), "names the server: {line}");
    assert!(line.contains("pid 1234"));
    assert!(
        line.contains("pgid 1234"),
        "process group is visible: {line}"
    );
    assert!(
        line.contains("live descendants 0"),
        "live descendant count is visible: {line}"
    );
    assert!(line.contains("listening on 5173"), "ports observed: {line}");
    assert!(line.contains("running"));
    assert!(
        line.contains("started by young-finch-81") && line.contains("cas-7c93"),
        "ownership must be visible without ps archaeology: {line}"
    );
    assert!(line.contains("npm run dev"), "the command: {line}");
    assert!(line.contains("/repo/app"), "the cwd: {line}");
}

/// cas-5a88: a real process at the record's pid with a live child, the host
/// state that made the listing test read "live descendants 1" on CI.
#[cfg(target_os = "linux")]
struct HostTreeAtPid(std::process::Child);

#[cfg(target_os = "linux")]
impl HostTreeAtPid {
    fn spawn() -> Self {
        let parent = std::process::Command::new("sh")
            .args(["-c", "sleep 30 & wait"])
            .spawn()
            .expect("spawn sh");
        let mut probe = record("probe", true);
        probe.pid = parent.id();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while server_registry::live_descendant_count(&probe) == 0
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Self(parent)
    }
}

#[cfg(target_os = "linux")]
impl Drop for HostTreeAtPid {
    fn drop(&mut self) {
        let _ = std::process::Command::new("pkill")
            .args(["-P", &self.0.id().to_string()])
            .status();
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// cas-5a88: the rendered descendant count is the caller's, never whatever
/// the host has at the record's pid.
#[cfg(target_os = "linux")]
#[test]
fn listing_ignores_the_host_process_tree_at_the_record_pid_cas_5a88() {
    let host = HostTreeAtPid::spawn();
    let mut adversarial = record("web", true);
    adversarial.pid = host.0.id();
    assert!(
        server_registry::live_descendant_count(&adversarial) > 0,
        "fixture: the host has a live child at pid {}",
        adversarial.pid
    );
    let line = render_server_line(&adversarial, ServerLiveness::Live, &[5173], 0);
    let observed = render_server_line(&adversarial, ServerLiveness::Live, &[5173], 3);
    let gone = render_server_line(&adversarial, ServerLiveness::Gone, &[], 3);
    drop(host);
    assert!(line.contains("live descendants 0"), "{line}");
    assert!(observed.contains("live descendants 3"), "the caller's count: {observed}");
    assert!(gone.contains("live descendants 0"), "a dead server counts none: {gone}");
}

#[test]
fn shared_and_private_entries_state_their_teardown_fate() {
    let shared = render_server_line(&record("web", true), ServerLiveness::Live, &[5173], 0);
    assert!(
        shared.contains("survives worker teardown"),
        "a shared entry must say so: {shared}"
    );

    let private = render_server_line(&record("web", false), ServerLiveness::Live, &[5173], 0);
    assert!(
        private.contains("dies with its worker"),
        "a private entry must say so: {private}"
    );
}

/// The record's own `Running` claim is never trusted over reality — this is
/// what stops `server_list` from reporting a long-dead pid as a live server.
#[test]
fn a_running_record_whose_process_is_gone_reads_as_dead() {
    let gone = render_server_line(&record("web", true), ServerLiveness::Gone, &[], 0);
    assert!(gone.contains("dead"), "{gone}");
    assert!(!gone.contains("— running"), "{gone}");

    let reused = render_server_line(&record("web", true), ServerLiveness::Replaced, &[], 0);
    assert!(
        reused.contains("dead (pid reused)"),
        "pid reuse must be named, not silently reported as dead: {reused}"
    );

    let unverified = render_server_line(&record("web", true), ServerLiveness::Unverifiable, &[], 0);
    assert!(unverified.contains("unverified"), "{unverified}");
}

/// A port the caller *claimed* must never be presented as a port that is
/// actually bound.
#[test]
fn an_expected_port_is_distinguished_from_an_observed_one() {
    let observed = render_server_line(&record("web", true), ServerLiveness::Live, &[4321], 0);
    assert!(observed.contains("listening on 4321"));
    assert!(
        !observed.contains("expected"),
        "observation wins when we have it: {observed}"
    );

    let claimed = render_server_line(&record("web", true), ServerLiveness::Live, &[], 0);
    assert!(
        claimed.contains("expected port 5173 (not bound)"),
        "an unbound expected port must be marked as such: {claimed}"
    );
}

#[test]
fn a_stopped_entry_keeps_its_own_state_label() {
    let mut stopped = record("web", true);
    stopped.state = ServerState::Stopped;
    let line = render_server_line(&stopped, ServerLiveness::Gone, &[], 0);
    assert!(line.contains("stopped"), "{line}");
    assert!(!line.contains("dead"), "{line}");
}

#[test]
fn cas_ced2_server_rows_are_single_line_and_bounded() {
    let mut server = record("web", true);
    server.name = "界\n".repeat(2000);
    server.id = "界\r".repeat(2000);
    server.command = "界\t".repeat(2000);
    server.cwd = std::path::PathBuf::from("界\n".repeat(2000));
    server.owner_worker = Some("界\n".repeat(2000));
    server.owner_task = Some("界\n".repeat(2000));
    let ports: Vec<_> = (1..=1000).collect();
    let line = render_server_line(&server, ServerLiveness::Live, &ports, 0);
    assert_eq!(line.lines().count(), 1, "a server must occupy one line");
    assert!(line.len() <= 512, "row emitted {} bytes", line.len());
    assert!(!line.contains(['\r', '\t']));
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn cas_ced2_server_list_caps_running_and_excludes_unverified() {
    use crate::mcp::{CasCore, CasService};
    use rmcp::model::RawContent;

    let temp = tempfile::TempDir::new().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let service = CasService::new(CasCore::with_daemon(root.clone(), None, None), None);
    let pid = std::process::id();
    let starttime =
        crate::mcp::daemon::read_pid_starttime(pid).expect("current process fingerprint");
    for i in 0..25 {
        let mut server = record(&format!("live-{i}"), true);
        server.pid = pid;
        server.pgid = None;
        server.pid_starttime = Some(starttime);
        server_registry::write_record(&root, &server).unwrap();
    }
    let mut unverified = record("unverified-server", true);
    unverified.pid = pid;
    unverified.pid_starttime = None;
    server_registry::write_record(&root, &unverified).unwrap();
    let text = |result: CallToolResult| {
        result
            .content
            .into_iter()
            .filter_map(|c| match c.raw {
                RawContent::Text(text) => Some(text.text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let req = serde_json::from_value(serde_json::json!({"action": "server_list"})).unwrap();
    let listed = text(service.factory_server_list(req).await.unwrap());
    assert_eq!(
        listed.lines().filter(|line| line.starts_with("  ")).count(),
        20,
        "{listed}"
    );
    assert!(listed.contains("Showing 20 of 25 matches"), "{listed}");
    assert!(!listed.contains("unverified-server"), "{listed}");

    let req = serde_json::from_value(
        serde_json::json!({"action": "server_list", "status": "unverified"}),
    )
    .unwrap();
    let listed = text(service.factory_server_list(req).await.unwrap());
    assert!(
        listed.contains("unverified-server") && listed.contains("unverified"),
        "{listed}"
    );
    assert_eq!(
        listed.lines().filter(|line| line.starts_with("  ")).count(),
        1,
        "{listed}"
    );
}

#[test]
fn default_names_come_from_the_command_and_are_filename_safe() {
    assert_eq!(default_server_name("npm run dev"), "npm-run");
    assert_eq!(default_server_name("cargo watch -x run"), "cargo-watch");
    assert_eq!(
        default_server_name("./scripts/serve.sh"),
        "scripts-serve-sh"
    );
    // Nothing usable in the command still yields a usable name.
    assert_eq!(default_server_name("///"), "server");
    assert_eq!(default_server_name(""), "server");
    assert!(!default_server_name("../../etc/passwd").contains('/'));
}

#[test]
fn cas_7694_names_ignore_environment_prefixes() {
    for command in [
        "PORT=4000 npm run dev",
        "env -i PORT=4000 npm run dev",
        "PORT=4000 LABEL='hello world' npm run dev",
    ] {
        assert_eq!(
            default_server_name(command),
            default_server_name("npm run dev"),
            "{command}"
        );
    }
}

#[test]
fn cas_9723_server_ownership_rejects_identity_and_session_reuse() {
    use crate::types::{Agent, AgentRole};
    let mut server = record("owned", true);
    let mut caller = Agent::new_with_role(
        "worker-id".into(),
        "young-finch-81".into(),
        AgentRole::Worker,
    );
    caller.factory_session = server.factory_session.clone();
    assert!(
        owns_server(&server, &caller),
        "legacy matching name and session remain stoppable"
    );
    caller.factory_session = Some("different-session".into());
    assert!(!owns_server(&server, &caller));
    caller.factory_session = server.factory_session.clone();
    server.owner_agent_id = Some(caller.id.clone());
    assert!(owns_server(&server, &caller));
    caller.id = "replacement-id".into();
    assert!(
        !owns_server(&server, &caller),
        "same-name replacement cannot stop the original owner's server"
    );
    server.owner_worker = None;
    server.owner_agent_id = None;
    assert!(
        !owns_server(&server, &caller),
        "unowned legacy entries are supervisor-only"
    );
}
