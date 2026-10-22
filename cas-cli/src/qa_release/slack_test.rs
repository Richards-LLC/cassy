// Included in qa_release::tests: the serve driver's Violet post (cas-efb8).

/// A seal posts the pack to `qa.release.slack_channel` through the driver's
/// Violet transport (a fake here; the upload URL is a loopback double), records
/// the receipt in the run notes and `slack.json`, and the same generation
/// never posts twice.
#[test]
fn slack_post_at_seal() {
    use run::{Record, State};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    struct FakeViolet {
        upload: String,
        calls: Mutex<Vec<serde_json::Value>>,
    }
    impl slack::Transport for FakeViolet {
        fn post(
            &self,
            args: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, crate::artifacts::slack::PostError>> + Send>> {
            let mut calls = self.calls.lock().unwrap();
            let n = calls.len();
            calls.push(args.clone());
            let reply = if args["step"] == "begin" {
                serde_json::json!({"ok": true, "upload_url": self.upload, "file_id": format!("F{n}")})
            } else {
                serde_json::json!({"ok": true,
                    "file": {"file_id": args["file_id"], "size_bytes": args["size_bytes"], "sha256": args["sha256"]},
                    "message": {"message_id": format!("m{n}"), "permalink": "https://slack.test/p/gate"}})
            };
            Box::pin(async move { Ok(reply) })
        }
    }

    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let gate = run_support::gate(
        &mut env,
        &[
            ("suites", r#"{"e2e":{"journeys":["checkout","web"],"wall_clock_mins":5}}"#),
            ("slack_channel", "qa-release"),
        ],
    );
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let upload = runtime.block_on(async {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;
        server
    });
    let violet = std::sync::Arc::new(FakeViolet { upload: format!("{}/upload", upload.uri()), calls: Mutex::default() });
    slack::install(violet.clone(), runtime.handle().clone());

    run::start(&gate.cas, &gate.sha).unwrap();
    run::tick(&gate.cas, &AtomicBool::new(false)).unwrap();
    let record = Record {
        row: "support".into(),
        verdict: "PASS".into(),
        evidence: Some(gate.evidence("support")),
        ..Record::default()
    };
    let sealed = run::record(&gate.cas, None, &record).unwrap();
    assert_eq!(sealed.state, State::Sealed);
    assert!(
        sealed.notes.iter().any(|n| n.contains("Slack: posted pack") && n.contains("https://slack.test/p/gate")),
        "{:?}",
        sealed.notes
    );
    let dir = pack::pack_dir(&gate.cas, &gate.sha).unwrap();
    let posted = slack::posted(&dir);
    assert_eq!(posted.len(), 1);
    assert_eq!((posted[0].agent_id.as_str(), posted[0].channel.as_str()), (slack::DRIVER_AGENT, "qa-release"));
    let calls = violet.calls.lock().unwrap().clone();
    assert!(!calls.is_empty() && calls.iter().all(|c| c["channel"] == "qa-release"), "{calls:?}");

    // The same generation never posts again.
    let config = configured(&gate.cas).unwrap();
    assert!(slack::post_sealed(&config, &dir).is_empty());
    assert_eq!(violet.calls.lock().unwrap().len(), calls.len());
}
