use super::*;
use tempfile::TempDir;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn response(risk: f64, requested: f64, untrusted: f64) -> Value {
    let mut probabilities = json!({"0":0.0,"1":0.0,"2":0.0,"3":0.0});
    let lo = risk.floor() as usize;
    let hi = risk.ceil() as usize;
    if lo == hi {
        probabilities[lo.to_string()] = json!(1.0);
    } else {
        probabilities[lo.to_string()] = json!(hi as f64 - risk);
        probabilities[hi.to_string()] = json!(risk - lo as f64);
    }
    let legend = questions()["risk"]["criteria"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, v)| (i.to_string(), v.clone()))
        .collect::<BTreeMap<_, _>>();
    json!({"model":"jev-1.13.0","answers":{
        "risk":{"type":"score","score":risk,"legend":legend,"probabilities":probabilities,"confidence":0.9},
        "user_requested":{"type":"noul","noul":requested},"from_untrusted":{"type":"noul","noul":untrusted}
    },"usage":{"input_tokens":10,"output_tokens":1}})
}
fn setup(dir: &TempDir, endpoint: &str) -> PathBuf {
    let root = dir.path().join(".cas");
    fs::create_dir(&root).unwrap();
    let mut config = crate::config::Config::default();
    config.set("jev.gate.shadow", "true").unwrap();
    config.save_toml(&root).unwrap();
    crate::cloud::CloudConfig {
        endpoint: endpoint.into(),
        token: Some("mock-only-token".into()),
        ..Default::default()
    }
    .save_to_cas_dir(&root)
    .unwrap();
    root
}
fn input(tool: &str) -> HookInput {
    serde_json::from_value(json!({"session_id":"shadow-test","tool_name":tool,"tool_input":if tool=="Bash" {json!({"command":"git status --short"})} else {json!({"file_path":"safe.rs","content":"never-send-write-body","old_string":"never-send-old","new_string":"never-send-new"})}})).unwrap()
}
fn rows(root: &Path) -> Vec<Value> {
    fs::read_to_string(root.join("jev-decisions.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn jev_shadow_config_default_roundtrip_and_reset() {
    let dir = TempDir::new().unwrap();
    let mut config = crate::config::Config::default();
    assert_eq!(config.get("jev.gate.shadow").as_deref(), Some("false"));
    assert!(crate::config::registry().get("jev.gate.shadow").is_some());
    assert!(config.set("jev.gate.shadow", "wrong").is_err());
    config.set("jev.gate.shadow", "true").unwrap();
    config.save_toml(dir.path()).unwrap();
    let mut config = crate::config::Config::load(dir.path()).unwrap();
    assert_eq!(config.get("jev.gate.shadow").as_deref(), Some("true"));
    assert!(
        config
            .list()
            .contains(&("jev.gate.shadow".into(), "true".into()))
    );
    config
        .set(
            "jev.gate.shadow",
            crate::config::registry()
                .get("jev.gate.shadow")
                .unwrap()
                .default,
        )
        .unwrap();
    config.save_toml(dir.path()).unwrap();
    assert!(
        !crate::config::Config::load(dir.path())
            .unwrap()
            .jev
            .unwrap()
            .gate
            .shadow
    );
}
#[test]
fn jev_shadow_policy_matches_eval_and_records_literal_exemption() {
    for (risk, requested, taint, hook, eval, literal) in [
        (1.0, 0.0, 0.0, "pass", "pass", "pass"),
        (1.5, 0.0, 0.0, "allow", "ask", "ask"),
        (1.5, 0.8, 0.0, "rewrite", "rewrite", "ask"),
        (2.5, 1.0, 0.0, "allow", "deny", "deny"),
        (0.0, 1.0, 0.8, "allow", "deny", "deny"),
        (0.0, 1.0, 0.0, "deny", "deny", "deny"),
        (0.0, 1.0, 0.0, "ask", "ask", "ask"),
    ] {
        let response: Response = serde_json::from_value(response(risk, requested, taint)).unwrap();
        validate_response(&response, questions()).unwrap();
        assert_eq!(composite(hook, Some(&response), true), eval);
        assert_eq!(composite(hook, Some(&response), false), literal);
    }
    assert_eq!(composite("rewrite", None, true), "rewrite");
}

#[tokio::test]
async fn jev_shadow_mock_records_every_tool_without_output_or_content_changes() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    env.remove("TYPESAFE_API_KEY");
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let root = setup(&dir, &server.uri());
    Mock::given(method("POST"))
        .and(path("/api/jev"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response(2.0, 0.9, 0.0)))
        .expect(3)
        .mount(&server)
        .await;
    tokio::task::spawn_blocking(move || {
        for tool in ["Bash", "Write", "Edit"] {
            let original =
                HookOutput::with_pre_tool_updated_input(json!({"command":"updated-input-kept"}));
            let before = serde_json::to_vec(&original).unwrap();
            observe(&input(tool), Some(&root), Some(&original));
            assert_eq!(serde_json::to_vec(&original).unwrap(), before);
        }
        observe(&input("Read"), Some(&root), Some(&HookOutput::empty()));
        let rows = rows(&root);
        assert_eq!(rows.len(), 3);
        for row in &rows {
            assert_eq!(row["tag"], "gate_shadow");
            assert_eq!(row["hook_decision"], "rewrite");
            assert_eq!(row["would_decide_eval"], "rewrite");
            assert_eq!(row["would_decide_literal"], "ask");
            assert_eq!(row["context_incomplete"], true);
            assert!(row["answers"]["risk"]["score"].is_number());
            assert_eq!(row["state_hash"].as_str().unwrap().len(), 64);
        }
        let summary = report(&root).unwrap();
        assert_eq!(summary["available"], 3);
        assert_eq!(summary["agreement"], 1.0);
        assert_eq!(summary["exemption_changes"], 3);
        assert_eq!(summary["literal"]["would_ask"], 3);
        let log = fs::read_to_string(root.join("jev-decisions.jsonl")).unwrap();
        assert!(!log.contains("git status --short"));
        assert!(!log.contains("never-send"));
        assert!(!log.contains("mock-only-token"));
    })
    .await
    .unwrap();
    for request in server.received_requests().await.unwrap() {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["questions"], *questions());
        assert!(!String::from_utf8_lossy(&request.body).contains("never-send"));
    }
}

#[tokio::test]
async fn jev_shadow_watchdog_timeout_logs_once_and_preserves_deny() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    env.remove("TYPESAFE_API_KEY");
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let root = setup(&dir, &server.uri());
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(response(0.0, 1.0, 0.0))
                .set_delay(Duration::from_secs(2)),
        )
        .expect(1)
        .mount(&server)
        .await;
    tokio::task::spawn_blocking(move || {
        let output = HookOutput::with_pre_tool_permission("deny", "keep exact reason");
        let before = serde_json::to_vec(&output).unwrap();
        let start = Instant::now();
        observe(&input("Bash"), Some(&root), Some(&output));
        assert!(start.elapsed() >= Duration::from_millis(700));
        assert!(start.elapsed() < Duration::from_millis(1100));
        assert_eq!(serde_json::to_vec(&output).unwrap(), before);
        let rows = rows(&root);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["status"], "unavailable");
        assert_eq!(rows[0]["would_decide_eval"], "deny");
        assert!(rows[0]["answers"].is_null());
        assert_eq!(report(&root).unwrap()["unavailable"], 1);
    })
    .await
    .unwrap();
}
#[test]
fn jev_shadow_nonblocking_log_and_report_examples_are_bounded() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("jev-decisions.jsonl");
    let lock = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap();
    lock.lock_exclusive().unwrap();
    let start = Instant::now();
    assert!(append_shadow(&path, &json!({})).is_err());
    assert!(start.elapsed() < Duration::from_millis(100));
    FileExt::unlock(&lock).unwrap();
    append_shadow(&path, &json!({"caller":"other","state":"unrelated-secret"})).unwrap();
    for _ in 0..15 {
        append_shadow(&path,&json!({"tag":"gate_shadow","status":"available","hook_decision":"pass","composite_decision":"deny","would_decide_eval":"deny","would_decide_literal":"deny","latency_ms":100,"command":"secret-not-an-example","tool":"Bash","state_hash":"abc"})).unwrap();
    }
    let summary = report(dir.path()).unwrap();
    assert_eq!(summary["total"], 15);
    assert_eq!(summary["would_deny"], 15);
    assert_eq!(summary["examples"].as_array().unwrap().len(), 10);
    assert!(!summary.to_string().contains("secret"));
}

#[tokio::test]
async fn jev_shadow_real_dispatcher_bytes_identical_enabled_disabled_and_unavailable() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    for key in [
        "TYPESAFE_API_KEY",
        "CAS_AGENT_TYPE",
        "CAS_AGENT_ROLE",
        "CAS_AGENT_ID",
        "CAS_AGENT_NAME",
        "CAS_CLONE_PATH",
    ] {
        env.remove(key);
    }
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let root = setup(&dir, &server.uri());
    env.set("CAS_ROOT", root.to_str().unwrap());
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response(3.0, 1.0, 0.0)))
        .expect(2)
        .mount(&server)
        .await;
    tokio::task::spawn_blocking(move || {
        for tool in ["Bash", "Write"] {
            let mut config = crate::config::Config::load(&root).unwrap();
            config.set("jev.gate.shadow", "false").unwrap();
            config.save_toml(&root).unwrap();
            let before = crate::hooks::handle_hook("PreToolUse", input(tool)).unwrap();
            let bytes = serde_json::to_vec(&before).unwrap();
            config.set("jev.gate.shadow", "true").unwrap();
            config.save_toml(&root).unwrap();
            let after = crate::hooks::handle_hook("PreToolUse", input(tool)).unwrap();
            assert_eq!(serde_json::to_vec(&after).unwrap(), bytes);
        }
        let before = crate::hooks::handle_hook("PreToolUse", input("Read")).unwrap();
        assert_eq!(rows(&root).len(), 2);
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(HookOutput::empty()).unwrap()
        );
        let mut config = crate::config::Config::load(&root).unwrap();
        config.set("jev.enabled", "false").unwrap();
        config.save_toml(&root).unwrap();
        let before =
            crate::hooks::handlers::handle_pre_tool_use(&input("Bash"), Some(&root)).unwrap();
        let after = crate::hooks::handle_hook("PreToolUse", input("Bash")).unwrap();
        assert_eq!(
            serde_json::to_vec(&before).unwrap(),
            serde_json::to_vec(&after).unwrap()
        );
        assert_eq!(rows(&root).len(), 3);
        assert_eq!(rows(&root)[2]["status"], "unavailable");
    })
    .await
    .unwrap();
}
