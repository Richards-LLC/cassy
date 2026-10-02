use super::*;
use crate::jev::{JevConfig, Response, Transport};
use tempfile::TempDir;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method},
};

fn response(label: &str) -> Value {
    let labels = [
        "real_regression",
        "load_flake_or_timeout",
        "host_or_toolchain_env",
        "known_issue",
        "unknown",
    ];
    let probabilities: std::collections::BTreeMap<_, _> = labels
        .into_iter()
        .map(|l| (l, if l == label { 0.92 } else { 0.02 }))
        .collect();
    json!({"model":"jev-1.13.0", "answers":{
        "failure_class":{"type":"choice","choice":label,"confidence":0.92,"probabilities":probabilities},
        "mentions_touched_change":{"type":"noul","noul":0.17}}, "usage":{"input_tokens":1000,"output_tokens":40}})
}
fn client(server: &MockServer, dir: &TempDir) -> JevClient {
    JevClient {
        config: JevConfig::default(),
        transport: Ok(Transport {
            endpoint: server.uri(),
            token: "fixture-token".into(),
            cloud: false,
            team_id: None,
            project_id: None,
        }),
        log_path: dir.path().join("jev-decisions.jsonl"),
        timeout: Duration::from_secs(2),
    }
}

#[test]
fn failure_block_preserves_names_first_panic_tail_and_utf8_bound() {
    let text = format!(
        "FAIL [0.02s] cas tests::real_regression\nthread 'tests::real_regression' panicked at src/check.rs:7:\nassertion left == right failed\n{}\nFINAL STEP DIAGNOSTIC",
        "🦉".repeat(20_000)
    );
    let state = evidence("sweep", "linux", &text, vec!["src/check.rs".into()]);
    assert!(state.failing_block.len() <= MAX_BLOCK);
    assert!(state.failing_block.contains("tests::real_regression"));
    assert!(state.failing_block.contains("panicked at src/check.rs:7"));
    assert!(state.failing_block.ends_with("FINAL STEP DIAGNOSTIC"));
    assert_eq!(state.touched_paths, ["src/check.rs"]);
    let state = evidence(
        "gate",
        "macos",
        "Authorization: Bearer fixture-secret\napi_key=key-secret\nrealpath: illegal option -- m",
        vec![],
    );
    assert!(!state.failing_block.contains("fixture-secret"));
    assert!(!state.failing_block.contains("key-secret"));
    assert_eq!(state.known_issue_hints.len(), 1);
}

#[test]
fn failure_log_reads_bounded_windows_and_real_git_paths() {
    let repo = TempDir::new().unwrap();
    let run = |args: &[&str]| {
        assert!(
            Command::new("git")
                .current_dir(repo.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        )
    };
    run(&["init", "-q"]);
    run(&[
        "-c",
        "user.name=fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "--allow-empty",
        "-qm",
        "base",
    ]);
    std::fs::write(repo.path().join("touched.rs"), "change").unwrap();
    run(&["add", "."]);
    run(&[
        "-c",
        "user.name=fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "-qm",
        "change",
    ]);
    assert_eq!(touched_paths(repo.path(), "HEAD^", "HEAD"), ["touched.rs"]);
    assert!(touched_paths(repo.path(), "missing-ref", "HEAD").is_empty());
    let log = repo.path().join("failure.log");
    std::fs::write(
        &log,
        format!(
            "FAIL test::original\nthread original panicked\n{}\nSTEP TAIL",
            "noise\n".repeat(100_000)
        ),
    )
    .unwrap();
    let state = log_evidence("sweep", &log, "failed", vec![]);
    assert!(state.failing_block.len() <= MAX_BLOCK);
    assert!(state.failing_block.contains("panicked"));
    assert!(state.failing_block.ends_with("STEP TAIL"));
}

#[tokio::test]
async fn failure_mock_labels_and_noul_log_without_changing_status() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let state = evidence(
        "sweep",
        "macos",
        "realpath: illegal option -- m\ntest result: 97 passed; 7 failed",
        vec!["scripts/cache.sh".into()],
    );
    Mock::given(method("POST"))
        .and(body_json(
            json!({"state":state,"questions":questions(),"model":"jev-1.13.0"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(response("host_or_toolchain_env")))
        .expect(1)
        .mount(&server)
        .await;
    let client = client(&server, &dir);
    let label = tokio::task::spawn_blocking(move || classify(&client, &state))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        label,
        "Jev: host_or_toolchain_env (0.92); touched-change mention=0.17 (advisory)"
    );
    let row: Value = serde_json::from_str(
        std::fs::read_to_string(dir.path().join("jev-decisions.jsonl"))
            .unwrap()
            .trim(),
    )
    .unwrap();
    assert_eq!(row["caller"], "failure:sweep");
    assert_eq!(
        row["question_ids"],
        json!(["failure_class", "mentions_touched_change"])
    );
    assert_eq!(
        row["answers"]["failure_class"]["choice"],
        "host_or_toolchain_env"
    );
    assert!(row.get("state").is_none());
}

#[tokio::test]
async fn failure_unavailable_or_wrong_shape_omits_label_silently() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    for body in [
        ResponseTemplate::new(503),
        ResponseTemplate::new(200).set_body_json(json!({"model":"invalid"})),
    ] {
        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(body)
            .expect(1)
            .mount(&server)
            .await;
        let client = client(&server, &dir);
        assert!(
            tokio::task::spawn_blocking(move || classify(
                &client,
                &evidence("worker-check", "linux", "failure", vec![])
            ))
            .await
            .unwrap()
            .is_none()
        );
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("jev-decisions.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    let mut response: Response = serde_json::from_value(response("unknown")).unwrap();
    response
        .answers
        .insert("failure_class".into(), Answer::Noul { noul: 0.9 });
    assert!(annotation(&Outcome::Available(response)).is_none());
}

#[tokio::test]
async fn failure_advisory_deadline_bounds_retry_and_never_waives_failure() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "10"))
        .expect(1)
        .mount(&server)
        .await;
    let client = client(&server, &dir);
    let start = Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        classify(&client, &evidence("gate", "linux", "FAIL", vec![]))
    })
    .await
    .unwrap();
    assert!(result.is_none());
    assert!(start.elapsed() < Duration::from_secs(5));
}
