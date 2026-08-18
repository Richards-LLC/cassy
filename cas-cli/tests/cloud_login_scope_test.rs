//! End-to-end scope tests for `cas login` (cas-046d, Ben's field report #3/#4).
//!
//! Two behaviours are pinned by driving the real binary:
//!
//!  1. `cas login --token` works from a directory that is not a CAS project.
//!     It used to abort with "CAS not initialized — run cas init", because the
//!     credential was written to `<project>/.cas/cloud.json`.
//!  2. That one login serves every project on the machine: a second, freshly
//!     created project reports the user as logged in without logging in again.
//!
//! The cloud is stood up as a one-thread canned HTTP server so the test never
//! touches the network and never depends on a live endpoint.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// Serve canned `200 {"teams":[]}` responses until the returned handle is
/// dropped. Returns the bound `http://127.0.0.1:<port>` base URL.
fn spawn_canned_cloud() -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind canned cloud");
    let endpoint = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());

    let handle = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            if !serve_one(stream) {
                break;
            }
        }
    });

    (endpoint, handle)
}

/// Answer one request. Returns false once the shutdown request is seen.
fn serve_one(mut stream: TcpStream) -> bool {
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return true;
    }
    // Drain headers so the client sees a complete exchange.
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) if line == "\r\n" || line == "\n" => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }

    let body = r#"{"teams":[],"default_team_id":null}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
    !request_line.contains("/__shutdown")
}

fn stop_canned_cloud(endpoint: &str, handle: std::thread::JoinHandle<()>) {
    let addr = endpoint.trim_start_matches("http://");
    if let Ok(mut stream) = TcpStream::connect(addr) {
        let _ = stream.write_all(b"GET /__shutdown HTTP/1.1\r\nHost: x\r\n\r\n");
        let _ = stream.flush();
    }
    let _ = handle.join();
}

/// A `cas` invocation confined to `home`: no real `~/.cas`, no ambient
/// `CAS_ROOT`, and cwd under the caller's control.
fn cas_command(home: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cas"));
    cmd.current_dir(cwd)
        .env("HOME", home)
        .env("CAS_USER_CLOUD_JSON", home.join(".cas").join("cloud.json"))
        .env_remove("CAS_ROOT")
        .env_remove("CAS_CLOUD_TOKEN")
        .env_remove("CAS_CLOUD_ENDPOINT");
    cmd
}

#[test]
fn token_login_outside_a_project_succeeds_and_serves_every_project() {
    let (endpoint, server) = spawn_canned_cloud();

    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home");
    let outside = temp.path().join("outside-any-project");
    let project_b = temp.path().join("project-b");
    std::fs::create_dir_all(home.join(".cas")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::create_dir_all(project_b.join(".cas")).unwrap();

    // 1. Log in from a directory that is not a CAS project.
    let login = cas_command(&home, &outside)
        .args(["login", "--token", "test-token", "--endpoint", &endpoint])
        .output()
        .expect("run cas login");

    let stderr = String::from_utf8_lossy(&login.stderr).to_string();
    assert!(
        login.status.success(),
        "`cas login --token` outside a project must succeed; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("not initialized"),
        "login must not demand `cas init` when credentials are user-level; stderr: {stderr}"
    );

    let user_config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join(".cas").join("cloud.json")).unwrap())
            .unwrap();
    assert_eq!(
        user_config["token"].as_str(),
        Some("test-token"),
        "the credential belongs in ~/.cas/cloud.json"
    );

    // 2. A different project, never logged in to, is already authenticated.
    let whoami = cas_command(&home, &project_b)
        .arg("whoami")
        .output()
        .expect("run cas whoami");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&whoami.stdout),
        String::from_utf8_lossy(&whoami.stderr)
    );
    assert!(
        whoami.status.success(),
        "a second project must inherit the machine-wide login; output: {combined}"
    );
    assert!(
        !combined.contains("Not logged in"),
        "a second project must not be asked to log in again; output: {combined}"
    );

    // 3. Logout is machine-wide too.
    let logout = cas_command(&home, &project_b)
        .arg("logout")
        .output()
        .expect("run cas logout");
    assert!(logout.status.success());
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join(".cas").join("cloud.json")).unwrap())
            .unwrap();
    assert!(
        after["token"].is_null(),
        "logout must clear the user-level credential, got {after}"
    );

    stop_canned_cloud(&endpoint, server);
}
