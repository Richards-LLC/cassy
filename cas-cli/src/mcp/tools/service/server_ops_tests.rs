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
        factory_session: Some("session-a".to_string()),
        shared,
        cgroup: None,
        log_path: None,
        started_at: Utc::now(),
        state: ServerState::Running,
        ended_at: None,
        ended_detail: None,
    }
}

#[test]
fn listing_answers_what_is_listening_and_who_started_it() {
    let line = render_server_line(&record("web", true), ServerLiveness::Live, &[5173]);

    assert!(line.contains("web"), "names the server: {line}");
    assert!(line.contains("pid 1234"));
    assert!(line.contains("listening on 5173"), "ports observed: {line}");
    assert!(line.contains("running"));
    assert!(
        line.contains("started by young-finch-81") && line.contains("cas-7c93"),
        "ownership must be visible without ps archaeology: {line}"
    );
    assert!(line.contains("npm run dev"), "the command: {line}");
    assert!(line.contains("/repo/app"), "the cwd: {line}");
}

#[test]
fn shared_and_private_entries_state_their_teardown_fate() {
    let shared = render_server_line(&record("web", true), ServerLiveness::Live, &[5173]);
    assert!(
        shared.contains("survives worker teardown"),
        "a shared entry must say so: {shared}"
    );

    let private = render_server_line(&record("web", false), ServerLiveness::Live, &[5173]);
    assert!(
        private.contains("dies with its worker"),
        "a private entry must say so: {private}"
    );
}

/// The record's own `Running` claim is never trusted over reality — this is
/// what stops `server_list` from reporting a long-dead pid as a live server.
#[test]
fn a_running_record_whose_process_is_gone_reads_as_dead() {
    let gone = render_server_line(&record("web", true), ServerLiveness::Gone, &[]);
    assert!(gone.contains("dead"), "{gone}");
    assert!(!gone.contains("— running"), "{gone}");

    let reused = render_server_line(&record("web", true), ServerLiveness::Replaced, &[]);
    assert!(
        reused.contains("dead (pid reused)"),
        "pid reuse must be named, not silently reported as dead: {reused}"
    );

    let unverified = render_server_line(&record("web", true), ServerLiveness::Unverifiable, &[]);
    assert!(unverified.contains("unverified"), "{unverified}");
}

/// A port the caller *claimed* must never be presented as a port that is
/// actually bound.
#[test]
fn an_expected_port_is_distinguished_from_an_observed_one() {
    let observed = render_server_line(&record("web", true), ServerLiveness::Live, &[4321]);
    assert!(observed.contains("listening on 4321"));
    assert!(
        !observed.contains("expected"),
        "observation wins when we have it: {observed}"
    );

    let claimed = render_server_line(&record("web", true), ServerLiveness::Live, &[]);
    assert!(
        claimed.contains("expected port 5173 (not bound)"),
        "an unbound expected port must be marked as such: {claimed}"
    );
}

#[test]
fn a_stopped_entry_keeps_its_own_state_label() {
    let mut stopped = record("web", true);
    stopped.state = ServerState::Stopped;
    let line = render_server_line(&stopped, ServerLiveness::Gone, &[]);
    assert!(line.contains("stopped"), "{line}");
    assert!(!line.contains("dead"), "{line}");
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
