//! `cas artifact publish|show|list` end to end (cassy#910, task cas-b72a).
//!
//! The assertions that matter here are the ones a unit test cannot make:
//!
//! * the recorded digest and size agree with `sha256sum` and `stat`, not just
//!   with our own second opinion;
//! * an oversized file is refused *before* any network call, proven by a mock
//!   server that receives nothing;
//! * a path that escapes the task's roots is refused;
//! * after a real three-step upload against a mock, the pre-signed upload URL
//!   appears nowhere in the database file, the command output, or the record.

use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use wiremock::matchers::{method, path as path_matcher};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TASK: &str = "cas-b72a";
/// The signature the mock hands back. If this string ever reaches disk or
/// stdout, a credential leaked.
const UPLOAD_SIGNATURE: &str = "SIGNATUREd00dfeedcafebabe";

fn cas_cmd(root: &Path) -> Command {
    let mut cmd = Command::new(cas::test_paths::cas_binary());
    let home = root.join(".test-home");
    let xdg = root.join(".test-xdg-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    if let Some(host_home) = std::env::var_os("HOME") {
        cmd.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    cmd.env("HOME", home).env("XDG_CONFIG_HOME", xdg);
    cmd.env_remove("CAS_ROOT");
    cmd.env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

struct Project {
    temp: TempDir,
    artifacts_root: std::path::PathBuf,
}

impl Project {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        cas_cmd(temp.path())
            .current_dir(temp.path())
            .args(["init", "--yes"])
            .assert()
            .success();

        // Point the factory artifacts root inside the project's temp tree so
        // the test never writes to the operator's real ~/.cas/artifacts.
        let artifacts_root = temp.path().join("factory-artifacts");
        std::fs::create_dir_all(artifacts_root.join(TASK)).unwrap();
        let config_path = temp.path().join(".cas").join("config.toml");
        let mut config = std::fs::read_to_string(&config_path).unwrap_or_default();
        config.push_str(&format!(
            "\n[factory]\nartifacts_root = \"{}\"\n",
            artifacts_root.display()
        ));
        std::fs::write(&config_path, config).unwrap();

        Self {
            temp,
            artifacts_root,
        }
    }

    fn task_dir(&self) -> std::path::PathBuf {
        self.artifacts_root.join(TASK)
    }

    fn cas_dir(&self) -> std::path::PathBuf {
        self.temp.path().join(".cas")
    }

    fn cmd(&self) -> Command {
        let mut cmd = cas_cmd(self.temp.path());
        cmd.current_dir(self.temp.path());
        cmd
    }

    /// Write cloud credentials pointing at a mock server.
    fn log_in(&self, endpoint: &str) {
        let cloud = serde_json::json!({
            "endpoint": endpoint,
            "token": "test-token",
        });
        std::fs::write(
            self.cas_dir().join("cloud.json"),
            serde_json::to_string_pretty(&cloud).unwrap(),
        )
        .unwrap();
    }
}

/// A small but not trivially-sized PDF-ish file.
fn write_sample(path: &Path) -> Vec<u8> {
    let bytes: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(path, &bytes).unwrap();
    bytes
}

fn system_sha256(path: &Path) -> String {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .expect("sha256sum must be available");
    assert!(output.status.success(), "sha256sum failed");
    String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

fn system_size(path: &Path) -> u64 {
    let output = Command::new("stat")
        .args(["-c", "%s"])
        .arg(path)
        .output()
        .expect("stat must be available");
    assert!(output.status.success(), "stat failed");
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

#[test]
fn publish_records_the_same_digest_and_size_as_sha256sum_and_stat() {
    let project = Project::new();
    let file = project.task_dir().join("brief.pdf");
    write_sample(&file);

    let output = project
        .cmd()
        .args(["--json", "artifact", "publish", "--task", TASK])
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "publish failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let record: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        record["sha256"].as_str().unwrap(),
        system_sha256(&file),
        "the recorded digest must agree with sha256sum"
    );
    assert_eq!(
        record["size_bytes"].as_u64().unwrap(),
        system_size(&file),
        "the recorded size must agree with stat"
    );
    assert_eq!(record["task_id"], TASK);
    assert_eq!(record["name"], "brief.pdf");
    assert_eq!(record["mime"], "application/pdf");
    assert_eq!(
        record["status"], "local",
        "with no cloud credentials the row stays local"
    );
    assert_eq!(record["storage"], "not_logged_in");
    let id = record["artifact_id"].as_str().unwrap();
    assert!(id.starts_with("art-"), "{id}");

    // The record is readable back through both read surfaces.
    project
        .cmd()
        .args(["artifact", "show", id])
        .assert()
        .success()
        .stdout(predicate::str::contains("brief.pdf"))
        .stdout(predicate::str::contains(system_sha256(&file)));
    project
        .cmd()
        .args(["artifact", "list", "--task", TASK])
        .assert()
        .success()
        .stdout(predicate::str::contains(id));
}

#[test]
fn a_file_over_the_ceiling_is_refused_before_any_network_call() {
    // A mock server that would answer `begin` if it were ever asked.
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(async {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_matcher("/api/artifacts/begin"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "artifact_id": "cloud-1",
                "upload_url": "https://example.invalid/put"
            })))
            .expect(0)
            .mount(&server)
            .await;
        server
    });

    let project = Project::new();
    project.log_in(&server.uri());
    let file = project.task_dir().join("huge.bin");
    // 25 MiB + 1 byte.
    std::fs::write(&file, vec![0u8; 25 * 1024 * 1024 + 1]).unwrap();

    let output = project
        .cmd()
        .args(["artifact", "publish", "--task", TASK])
        .arg(&file)
        .output()
        .unwrap();

    assert!(!output.status.success(), "an oversized file must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ceiling") && stderr.contains("Nothing was uploaded"),
        "the refusal must state the ceiling and that nothing was sent: {stderr}"
    );

    // The `.expect(0)` above is verified here: had the ceiling been checked
    // after `begin`, this call count would be 1.
    runtime.block_on(async { server.verify().await });

    project
        .cmd()
        .args(["artifact", "list", "--task", TASK])
        .assert()
        .success()
        .stdout(predicate::str::contains("no artifacts published"));
}

#[test]
fn a_path_escaping_the_publishable_roots_is_refused() {
    let project = Project::new();

    // A symlink inside the task directory pointing at a file outside both roots.
    let outside_dir = TempDir::new().unwrap();
    let outside = outside_dir.path().join("elsewhere.pdf");
    std::fs::write(&outside, b"not yours").unwrap();
    let link = project.task_dir().join("innocent.pdf");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    #[cfg(not(unix))]
    return;

    let output = project
        .cmd()
        .args(["artifact", "publish", "--task", TASK])
        .arg(&link)
        .output()
        .unwrap();
    assert!(!output.status.success(), "an escaping symlink must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("outside this task's publishable roots"),
        "{stderr}"
    );

    // A plain path outside the roots is refused the same way.
    let output = project
        .cmd()
        .args(["artifact", "publish", "--task", TASK])
        .arg(&outside)
        .output()
        .unwrap();
    assert!(!output.status.success());

    // The credential cache is never publishable, even though it is inside the
    // checkout.
    let cloud_json = project.cas_dir().join("cloud.json");
    std::fs::write(&cloud_json, "{\"token\":\"real-secret\"}").unwrap();
    let output = project
        .cmd()
        .args(["artifact", "publish", "--task", TASK])
        .arg(&cloud_json)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "publishing the token cache must fail"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("never publishable"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_committed_upload_never_persists_or_prints_the_signed_upload_url() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (server, upload_url) = runtime.block_on(async {
        let server = MockServer::start().await;
        let upload_url = format!(
            "{}/object/put?X-Amz-Signature={UPLOAD_SIGNATURE}&X-Amz-Expires=900",
            server.uri()
        );
        Mock::given(method("POST"))
            .and(path_matcher("/api/artifacts/begin"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "artifact_id": "cloud-42",
                "upload_url": upload_url,
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path_matcher("/object/put"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path_matcher("/api/artifacts/cloud-42/complete"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "url": "https://cloud.example/a/cloud-42"
            })))
            .expect(1)
            .mount(&server)
            .await;
        (server, upload_url)
    });

    let project = Project::new();
    project.log_in(&server.uri());
    let file = project.task_dir().join("report.pdf");
    let bytes = write_sample(&file);

    let output = project
        .cmd()
        .args(["--json", "artifact", "publish", "--task", TASK])
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "publish failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let record: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(record["status"], "committed");
    assert_eq!(record["storage"], "committed");
    assert_eq!(record["cloud_url"], "https://cloud.example/a/cloud-42");

    runtime.block_on(async {
        server.verify().await;
        let uploads: Vec<_> = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|request| request.url.path() == "/object/put")
            .collect();
        assert_eq!(uploads.len(), 1);
        assert!(
            uploads[0].headers.get("authorization").is_none(),
            "the Cassy bearer must never reach the object store"
        );
        assert_eq!(uploads[0].body, bytes, "the whole file must arrive");
    });

    // The credential must not survive anywhere the operator or a later reader
    // can reach it.
    assert!(
        !stdout.contains(UPLOAD_SIGNATURE),
        "the signed upload URL leaked into stdout"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains(UPLOAD_SIGNATURE),
        "the signed upload URL leaked into stderr"
    );

    let show = project
        .cmd()
        .args(["artifact", "show", record["artifact_id"].as_str().unwrap()])
        .output()
        .unwrap();
    let shown = String::from_utf8_lossy(&show.stdout);
    assert!(
        !shown.contains(UPLOAD_SIGNATURE),
        "the signed upload URL leaked into `artifact show`: {shown}"
    );
    assert!(
        shown.contains("cloud-42"),
        "the server's artifact id is recorded: {shown}"
    );

    // Search the raw database bytes, not just the API: a column we forgot
    // about would still be caught here.
    for name in ["cas.db", "cas.db-wal", "cas.db-shm"] {
        let db = project.cas_dir().join(name);
        if !db.exists() {
            continue;
        }
        let raw = std::fs::read(&db).unwrap();
        assert!(
            !contains_bytes(&raw, UPLOAD_SIGNATURE.as_bytes()),
            "the signed upload URL was persisted into {name}"
        );
        assert!(
            !contains_bytes(&raw, b"test-token"),
            "the bearer token was persisted into {name}"
        );
    }
    assert!(
        upload_url.contains(UPLOAD_SIGNATURE),
        "sanity: the mock really did hand back a signed URL"
    );
}

#[test]
fn storage_that_is_not_live_still_yields_a_citable_record_and_exits_zero() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(async {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_matcher("/api/artifacts/begin"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not implemented yet"))
            .expect(1)
            .mount(&server)
            .await;
        server
    });

    let project = Project::new();
    project.log_in(&server.uri());
    let file = project.task_dir().join("brief.pdf");
    write_sample(&file);

    let output = project
        .cmd()
        .args(["artifact", "publish", "--task", TASK])
        .arg(&file)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "a not-live endpoint must not fail the publish: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Cloud storage is not live yet"),
        "the operator must be told why: {stdout}"
    );
    let id = stdout
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .last()
        .unwrap();
    assert!(
        id.starts_with("art-"),
        "the record id must still be printed so the message lane can cite it: {stdout}"
    );

    runtime.block_on(async { server.verify().await });

    project
        .cmd()
        .args(["artifact", "show", id])
        .assert()
        .success()
        .stdout(predicate::str::contains("status  local"));
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
