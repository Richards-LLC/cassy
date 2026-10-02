use super::*;
use tempfile::TempDir;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

fn questions() -> Value {
    json!({"urgent": {"type":"noul", "instructions":"Is this urgent?"}})
}
fn response() -> Value {
    json!({"model":"jev-1.13.0", "answers":{"urgent":{"type":"noul", "noul":0.95}}, "usage":{"input_tokens":296, "output_tokens":20}})
}
fn fixture(server: &MockServer, dir: &TempDir, cloud: bool) -> JevClient {
    JevClient {
        config: JevConfig::default(),
        transport: Ok(Transport {
            endpoint: format!(
                "{}{}",
                server.uri(),
                if cloud { "/api/jev" } else { "/v1/systemone" }
            ),
            token: "test-secret-key".into(),
            cloud,
            team_id: cloud.then(|| "team-a".into()),
            project_id: cloud.then(|| "project-a".into()),
        }),
        log_path: dir.path().join("jev-decisions.jsonl"),
        timeout: Duration::from_secs(2),
    }
}
fn rows(dir: &TempDir) -> Vec<Value> {
    fs::read_to_string(dir.path().join("jev-decisions.jsonl"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}
async fn ask(client: JevClient, advisory: bool) -> Result<Outcome, JevError> {
    tokio::task::spawn_blocking(move || {
        client.ask(&json!("private-state"), &questions(), "test", advisory)
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn jev_proxy_and_direct_http_preserve_answer_shape_and_log_hash() {
    for cloud in [true, false] {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        let client = fixture(&server, &dir, cloud);
        let mut expected =
            json!({"state":"private-state", "questions":questions(), "model":"jev-1.13.0"});
        if cloud {
            expected["team_id"] = json!("team-a");
            expected["project_id"] = json!("project-a");
        }
        Mock::given(method("POST"))
            .and(path(if cloud { "/api/jev" } else { "/v1/systemone" }))
            .and(header("authorization", "Bearer test-secret-key"))
            .and(body_json(expected))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(response())
                    .insert_header("X-Request-Id", "jev-request-123"),
            )
            .expect(1)
            .mount(&server)
            .await;
        assert!(!format!("{client:?}").contains("test-secret-key"));
        assert_eq!(
            serde_json::to_value(ask(client, false).await.unwrap()).unwrap(),
            response()
        );
        let log = rows(&dir);
        assert_eq!(log.len(), 1);
        assert_eq!(log[0]["request_id"], "jev-request-123");
        assert_eq!(log[0]["question_ids"], json!(["urgent"]));
        assert_eq!(log[0]["input_tokens"], 296);
        assert_eq!(log[0]["answers"], response()["answers"]);
        assert_eq!(
            log[0]["state_hash"],
            format!("{:x}", Sha256::digest(b"\"private-state\""))
        );
        let text = fs::read_to_string(dir.path().join("jev-decisions.jsonl")).unwrap();
        assert!(!text.contains("private-state"));
        assert!(!text.contains("test-secret-key"));
        assert!(log[0]["timestamp"].is_string());
        assert!(log[0]["latency_ms"].is_number());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(dir.path().join("jev-decisions.jsonl"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}

#[tokio::test]
async fn jev_batch_logs_each_record_and_enforces_bounds() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response()))
        .expect(2)
        .mount(&server)
        .await;
    let client = fixture(&server, &dir, true);
    let result = tokio::task::spawn_blocking(move || {
        assert!(client.batch(&[], &questions(), "batch", false).is_err());
        assert!(
            client
                .batch(&vec![json!("x"); 51], &questions(), "batch", false)
                .is_err()
        );
        client.batch(
            &[json!("one"), json!({"text":"two"})],
            &questions(),
            "batch",
            false,
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.len(), 2);
    let log = rows(&dir);
    assert_eq!(log.len(), 2);
    assert_ne!(log[0]["state_hash"], log[1]["state_hash"]);
}

#[tokio::test]
async fn jev_retry_429_and_529_is_bounded_and_logs_once() {
    for status in [429, 529] {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).insert_header("Retry-After", "0"))
            .expect(2)
            .up_to_n_times(2)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response()))
            .expect(1)
            .with_priority(2)
            .mount(&server)
            .await;
        assert!(matches!(
            ask(fixture(&server, &dir, true), false).await.unwrap(),
            Outcome::Available(_)
        ));
        assert_eq!(rows(&dir).len(), 1);
        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).insert_header("Retry-After", "0"))
            .expect(3)
            .mount(&server)
            .await;
        assert!(matches!(
            ask(fixture(&server, &dir, true), true).await.unwrap(),
            Outcome::Unavailable { .. }
        ));
        assert_eq!(rows(&dir).len(), 2);
    }
}

#[tokio::test]
async fn jev_retry_after_deadline_and_timeout_fail_open() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "60"))
        .expect(1)
        .mount(&server)
        .await;
    let start = Instant::now();
    assert!(matches!(
        ask(fixture(&server, &dir, true), true).await.unwrap(),
        Outcome::Unavailable { .. }
    ));
    assert!(start.elapsed() < Duration::from_secs(2));
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(response())
                .set_delay(Duration::from_millis(150)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut client = fixture(&server, &dir, true);
    client.timeout = Duration::from_millis(30);
    assert!(matches!(
        ask(client, true).await.unwrap(),
        Outcome::Unavailable { .. }
    ));
    assert_eq!(rows(&dir).len(), 2);
    assert_eq!(retry_after("0"), Some(Duration::ZERO));
    assert!(retry_after("Fri, 02 Oct 2026 00:00:00 GMT").is_some());
}

#[tokio::test]
async fn jev_http_errors_and_unreachable_are_typed_and_redacted() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    for status in [401, 422, 503] {
        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(status).set_body_string("test-secret-key private-state"),
            )
            .expect(2)
            .mount(&server)
            .await;
        let result = ask(fixture(&server, &dir, true), true).await.unwrap();
        assert!(matches!(result, Outcome::Unavailable { .. }));
        assert!(
            !serde_json::to_string(&result)
                .unwrap()
                .contains("test-secret-key")
        );
        assert!(matches!(
            ask(fixture(&server, &dir, true), false).await,
            Err(JevError::Unavailable(_))
        ));
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/api/jev", listener.local_addr().unwrap());
    drop(listener);
    let mut client = fixture(&server, &dir, true);
    client.transport.as_mut().unwrap().endpoint = endpoint;
    assert!(matches!(
        ask(client, true).await.unwrap(),
        Outcome::Unavailable { .. }
    ));
    assert_eq!(rows(&dir).len(), 7);
}

#[tokio::test]
async fn jev_invalid_response_redirect_and_disabled_never_succeed() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    for body in [
        json!({"answers":{}}),
        json!({"model":"jev-1.13.0", "answers":{"urgent":{"type":"noul", "noul":2}}, "usage":{"input_tokens":1, "output_tokens":1}}),
    ] {
        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        assert!(matches!(
            ask(fixture(&server, &dir, true), true).await.unwrap(),
            Outcome::Unavailable { .. }
        ));
    }
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", format!("{}/leak", server.uri())),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert!(matches!(
        ask(fixture(&server, &dir, true), true).await.unwrap(),
        Outcome::Unavailable { .. }
    ));
    let mut client = fixture(&server, &dir, true);
    client.config.enabled = false;
    assert!(matches!(
        ask(client, true).await.unwrap(),
        Outcome::Unavailable { .. }
    ));
    assert_eq!(rows(&dir).len(), 4);
}

#[test]
fn jev_config_accessors_roundtrip_and_transport_resolution() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    env.remove("TYPESAFE_API_KEY");
    let dir = TempDir::new().unwrap();
    let mut config = crate::config::Config::default();
    for (key, value) in [
        ("jev.model", "jev-1.13.0"),
        ("jev.key_file", "key.txt"),
        ("jev.enabled", "false"),
    ] {
        assert!(crate::config::registry().get(key).is_some());
        config.set(key, value).unwrap();
        assert_eq!(config.get(key).as_deref(), Some(value));
        assert!(config.list().contains(&(key.into(), value.into())));
    }
    assert!(config.set("jev.model", "").is_err());
    assert!(config.set("jev.enabled", "no").is_err());
    config.save_toml(dir.path()).unwrap();
    let loaded = crate::config::Config::load(dir.path()).unwrap();
    assert_eq!(loaded.get("jev.key_file").as_deref(), Some("key.txt"));
    assert!(!JevClient::from_project(dir.path()).unwrap().config.enabled);
    fs::write(dir.path().join("key.txt"), "file-secret\n").unwrap();
    let direct = resolve_transport(dir.path(), loaded.jev.as_ref().unwrap()).unwrap();
    assert!(!direct.cloud);
    assert_eq!(direct.token, "file-secret");
    assert_eq!(direct.endpoint, DIRECT_ENDPOINT);
    env.set("TYPESAFE_API_KEY", "env-secret");
    assert_eq!(
        resolve_transport(dir.path(), loaded.jev.as_ref().unwrap())
            .unwrap()
            .token,
        "env-secret"
    );
    env.remove("TYPESAFE_API_KEY");
    config.set("jev.key_file", "").unwrap();
    config.set("jev.enabled", "true").unwrap();
    config.save_toml(dir.path()).unwrap();
    let cloud = crate::cloud::CloudConfig {
        endpoint: "https://cloud.example".into(),
        token: Some("cloud-secret".into()),
        team_id: Some("team-123".into()),
        ..Default::default()
    };
    cloud.save_to_cas_dir(dir.path()).unwrap();
    let proxy = JevClient::from_project(dir.path()).unwrap();
    let transport = proxy.transport.as_ref().unwrap();
    assert!(transport.cloud);
    assert_eq!(transport.endpoint, "https://cloud.example/api/jev");
    assert_eq!(transport.token, "cloud-secret");
    assert_eq!(transport.team_id.as_deref(), Some("team-123"));
    assert!(!format!("{proxy:?}").contains("cloud-secret"));
}

#[test]
fn jev_validation_probabilities_log_failure_and_concurrent_append() {
    assert!(validate_input(&json!(null), &questions()).is_err());
    assert!(validate_input(&json!("x"), &json!({})).is_err());
    let questions = json!({"choice":{"type":"choice", "instructions":"Pick", "criteria":{"a":null,"b":"other"}}, "score":{"type":"score", "instructions":"Rate", "criteria":["low","high"]}});
    validate_input(&json!({}), &questions).unwrap();
    let mut reply: Response = serde_json::from_value(json!({"model":"jev-1.13.0", "answers":{
        "choice":{"type":"choice","choice":"a","probabilities":{"a":0.8,"b":0.2},"confidence":0.7},
        "score":{"type":"score","score":0.8,"legend":{"0":"low","1":"high"},"probabilities":{"0":0.2,"1":0.8},"confidence":0.7}},"usage":{"input_tokens":5,"output_tokens":1}})).unwrap();
    validate_response(&reply, &questions).unwrap();
    if let Answer::Choice { probabilities, .. } = reply.answers.get_mut("choice").unwrap() {
        probabilities.insert("a".into(), 2.0);
    }
    assert!(validate_response(&reply, &questions).is_err());
    let dir = TempDir::new().unwrap();
    std::thread::scope(|scope| {
        for i in 0..20 {
            let path = dir.path().join("jev-decisions.jsonl");
            scope.spawn(move || append_log(&path, &json!({"index":i})).unwrap());
        }
    });
    assert_eq!(rows(&dir).len(), 20);
    let client = JevClient {
        config: JevConfig::default(),
        transport: Err("unavailable".into()),
        log_path: dir.path().join("missing/log"),
        timeout: CALL_TIMEOUT,
    };
    assert!(matches!(
        client
            .ask(&json!("x"), &super::tests::questions(), "test", true)
            .unwrap(),
        Outcome::Unavailable { .. }
    ));
    assert!(matches!(
        client.ask(&json!("x"), &super::tests::questions(), "test", false),
        Err(JevError::DecisionLog(_))
    ));
}

#[tokio::test]
async fn jev_batch_deadline_logs_all_unavailable_without_more_requests() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(response())
                .set_delay(Duration::from_millis(150)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = fixture(&server, &dir, true);
    let start = Instant::now();
    let outcomes = tokio::task::spawn_blocking(move || {
        client.batch_until(
            &[json!("first"), json!("second"), json!("third")],
            &questions(),
            "batch",
            true,
            Instant::now() + Duration::from_millis(30),
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(outcomes.len(), 3);
    assert!(
        outcomes
            .iter()
            .all(|o| matches!(o, Outcome::Unavailable { .. }))
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(rows(&dir).len(), 3);
}

#[tokio::test]
async fn jev_files_mock_http_globs_secrets_ignores_binary_and_hash_only() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::create_dir(root.join("creds")).unwrap();
    fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
    fs::write(root.join("src/one.rs"), "private-file-one").unwrap();
    fs::write(root.join("src/two.rs"), "private-file-two").unwrap();
    fs::write(root.join("ignored.txt"), "ignored-private").unwrap();
    fs::write(root.join(".env.local"), "secret-token").unwrap();
    fs::write(root.join("server.pem"), "private-key").unwrap();
    fs::write(root.join("creds/password.txt"), "password-token").unwrap();
    fs::write(root.join("binary.dat"), [0, 1, 2]).unwrap();
    fs::write(root.join("invalid.dat"), [0xff, 0xfe]).unwrap();
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response()))
        .expect(2)
        .mount(&server)
        .await;
    let client = fixture(&server, &dir, false);
    let result = tokio::task::spawn_blocking(move || {
        client.files(
            &root,
            &FilesOptions {
                paths: vec!["ignored.txt".into()],
                globs: vec!["**/*".into(), "src/*.rs".into()],
                ..Default::default()
            },
            &questions(),
            "test:files",
            false,
        )
    })
    .await
    .unwrap()
    .unwrap();
    let encoded = serde_json::to_value(&result).unwrap();
    let available: Vec<_> = encoded["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["status"] == "available")
        .collect();
    assert_eq!(available.len(), 2);
    assert_eq!(available[0]["path"], "src/one.rs");
    assert_eq!(available[0]["answers"]["urgent"]["noul"], 0.95);
    for name in [".env.local", "server.pem", "creds/password.txt"] {
        assert!(
            encoded["files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["path"] == name && r["reason"] == "secret path")
        );
    }
    assert!(
        encoded
            .to_string()
            .contains("ignored by file selection rules")
    );
    assert!(encoded.to_string().contains("binary file"));
    assert!(encoded.to_string().contains("binary or non-UTF-8 file"));
    let requests = server.received_requests().await.unwrap();
    for request in &requests {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert!(
            body["state"]["content"]
                .as_str()
                .unwrap()
                .starts_with("private-file-")
        );
        assert!(body["state"]["path"].as_str().unwrap().starts_with("src/"));
    }
    let log = fs::read_to_string(dir.path().join("jev-decisions.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 2);
    assert!(rows(&dir).iter().all(|r| r["caller"] == "test:files"));
    for token in [
        "private-file-one",
        "private-file-two",
        "secret-token",
        "private-key",
        "password-token",
        "ignored-private",
    ] {
        assert!(!encoded.to_string().contains(token));
        assert!(!log.contains(token));
        assert!(
            !requests
                .iter()
                .any(|r| String::from_utf8_lossy(&r.body).contains(token)
                    && !token.starts_with("private-file-"))
        );
    }
}

#[tokio::test]
async fn jev_files_caps_truncate_utf8_and_bound_selection() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.txt"), "abésecret-after-cap").unwrap();
    fs::write(dir.path().join("b.txt"), "never-read").unwrap();
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response()))
        .expect(1)
        .mount(&server)
        .await;
    let client = fixture(&server, &dir, false);
    let result = tokio::task::spawn_blocking(move || {
        for options in [
            FilesOptions {
                max_files: 0,
                paths: vec!["a.txt".into()],
                ..Default::default()
            },
            FilesOptions {
                max_files: 51,
                paths: vec!["a.txt".into()],
                ..Default::default()
            },
            FilesOptions {
                max_bytes: 0,
                paths: vec!["a.txt".into()],
                ..Default::default()
            },
            FilesOptions {
                max_bytes: MAX_FILE_BYTES + 1,
                paths: vec!["a.txt".into()],
                ..Default::default()
            },
            FilesOptions::default(),
        ] {
            assert!(
                client
                    .files(dir.path(), &options, &questions(), "caps", false)
                    .is_err()
            );
        }
        client.files(
            dir.path(),
            &FilesOptions {
                globs: vec!["*.txt".into()],
                max_files: 1,
                max_bytes: 3,
                ..Default::default()
            },
            &questions(),
            "caps",
            false,
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert!(result.limit_reached);
    let encoded = serde_json::to_value(result).unwrap();
    assert_eq!(encoded["files"].as_array().unwrap().len(), 1);
    assert_eq!(encoded["files"][0]["truncated"], true);
    let requests = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(
        body["state"]["content"],
        "ab\n[Jev: file truncated at byte cap]"
    );
    assert!(!String::from_utf8_lossy(&requests[0].body).contains("secret-after-cap"));
}

#[tokio::test]
async fn jev_files_directory_recursion_and_no_matches() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("src/nested")).unwrap();
    fs::write(dir.path().join("src/a.txt"), "first").unwrap();
    fs::write(dir.path().join("src/nested/b.txt"), "second").unwrap();
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response()))
        .expect(3)
        .mount(&server)
        .await;
    let client = fixture(&server, &dir, false);
    tokio::task::spawn_blocking(move || {
        let mut options = FilesOptions {
            paths: vec!["src".into()],
            ..Default::default()
        };
        assert_eq!(
            client
                .files(dir.path(), &options, &questions(), "dirs", false)
                .unwrap()
                .files
                .len(),
            1
        );
        options.recursive = true;
        assert_eq!(
            client
                .files(dir.path(), &options, &questions(), "dirs", false)
                .unwrap()
                .files
                .len(),
            2
        );
        options.paths.clear();
        options.globs = vec!["missing/*.txt".into()];
        assert!(
            client
                .files(dir.path(), &options, &questions(), "dirs", false)
                .unwrap()
                .files
                .is_empty()
        );
        options.globs = vec!["[".into()];
        assert!(
            client
                .files(dir.path(), &options, &questions(), "dirs", false)
                .is_err()
        );
    })
    .await
    .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn jev_files_refuses_outside_and_symlinked_secrets_before_http() {
    use std::os::unix::fs::symlink;
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("private.txt"), "outside-token").unwrap();
    fs::write(dir.path().join(".env"), "inside-secret").unwrap();
    symlink(
        outside.path().join("private.txt"),
        dir.path().join("escape.txt"),
    )
    .unwrap();
    symlink(dir.path().join(".env"), dir.path().join("alias.txt")).unwrap();
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response()))
        .expect(0)
        .mount(&server)
        .await;
    let client = fixture(&server, &dir, false);
    tokio::task::spawn_blocking(move || {
        let result = client
            .files(
                dir.path(),
                &FilesOptions {
                    paths: vec![
                        outside.path().join("private.txt").to_string_lossy().into(),
                        "../private.txt".into(),
                        "escape.txt".into(),
                        "alias.txt".into(),
                        ".env".into(),
                    ],
                    globs: vec!["../**/*".into()],
                    ..Default::default()
                },
                &questions(),
                "refused",
                false,
            )
            .unwrap();
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["files"].as_array().unwrap().len(), 6);
        assert_eq!(
            value["files"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["reason"] == "outside project root")
                .count(),
            4
        );
        assert_eq!(
            value["files"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["reason"] == "secret path")
                .count(),
            2
        );
        assert!(!value.to_string().contains("outside-token"));
        assert!(!value.to_string().contains("inside-secret"));
        assert!(!dir.path().join("jev-decisions.jsonl").exists());
    })
    .await
    .unwrap();
}
