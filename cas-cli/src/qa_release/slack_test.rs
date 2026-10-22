// Included in qa_release::tests: the serve driver's Violet post (cas-efb8).

/// Seals made by `record` (the CLI path, which has no proxy) only note the
/// post; the driver's tick posts each sealed generation missing from
/// `slack.json` through the driver's Violet transport (a fake here; the
/// upload URL is a loopback double). A failed post is noted once and retried
/// each tick, an OWED reseal posts its new generation, a posted generation
/// never posts again, and a corrupt `slack.json` is moved aside.
#[test]
fn slack_post_from_driver() {
    use run::{Record, State};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    type Reply = Pin<Box<dyn Future<Output = Result<serde_json::Value, crate::artifacts::slack::PostError>> + Send>>;

    struct FakeViolet {
        upload: String,
        down: AtomicBool,
        calls: Mutex<Vec<serde_json::Value>>,
    }
    impl slack::Transport for FakeViolet {
        fn post(&self, args: serde_json::Value) -> Reply {
            let mut calls = self.calls.lock().unwrap();
            let n = calls.len();
            calls.push(args.clone());
            let reply = if self.down.load(Ordering::SeqCst) {
                Err(crate::artifacts::slack::PostError::new("violet_unavailable", "Violet is down"))
            } else if args["step"] == "begin" {
                Ok(serde_json::json!({"ok": true, "upload_url": self.upload, "file_id": format!("F{n}")}))
            } else {
                Ok(serde_json::json!({"ok": true,
                    "file": {"file_id": args["file_id"], "size_bytes": args["size_bytes"], "sha256": args["sha256"]},
                    "message": {"message_id": format!("m{n}"), "permalink": "https://slack.test/p/gate"}}))
            };
            Box::pin(async move { reply })
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
    let violet = std::sync::Arc::new(FakeViolet {
        upload: format!("{}/upload", upload.uri()),
        down: AtomicBool::new(true),
        calls: Mutex::default(),
    });
    slack::install(violet.clone(), runtime.handle().clone());
    let (cancel, mut lease) = (AtomicBool::new(false), None);
    let drive = |lease: &mut Option<std::fs::File>| assert!(run::drive(&gate.cas, lease, &cancel).unwrap());
    let dir = pack::pack_dir(&gate.cas, &gate.sha).unwrap();
    let record = |verdict: &str| Record {
        row: "support".into(),
        verdict: verdict.into(),
        cause: Some(verdict::NotExercisedCause::OperatorSkip),
        reason: Some("walked by hand later".into()),
        evidence: Some(gate.evidence("support")),
        ..Record::default()
    };

    run::start(&gate.cas, &gate.sha).unwrap();
    drive(&mut lease);
    let owed = run::record(&gate.cas, None, &record("NOT_EXERCISED")).unwrap();
    assert_eq!((owed.state, owed.gate.as_ref().map(|g| g.verdict)), (State::Sealed, Some(verdict::GateVerdict::Owed)));
    assert!(owed.notes.iter().any(|n| n.contains("the cas serve driver posts this generation to qa-release")));
    assert!(slack::posted(&dir).is_empty() && violet.calls.lock().unwrap().is_empty(), "the CLI seal posts nothing");

    // Violet is down: each tick retries, and the failure is noted once.
    drive(&mut lease);
    drive(&mut lease);
    assert_eq!(violet.calls.lock().unwrap().len(), 2, "one attempt per tick");
    let notes = run::load(&gate.cas, &gate.sha).unwrap().unwrap().notes;
    assert_eq!(notes.iter().filter(|n| n.contains("Slack post failed")).count(), 1, "{notes:?}");
    assert!(slack::posted(&dir).is_empty());

    violet.down.store(false, Ordering::SeqCst);
    drive(&mut lease);
    let posted = slack::posted(&dir);
    assert_eq!(posted.len(), 1);
    assert_eq!((posted[0].agent_id.as_str(), posted[0].channel.as_str()), (slack::DRIVER_AGENT, "qa-release"));
    assert!(violet.calls.lock().unwrap().iter().all(|c| c["channel"] == "qa-release"));

    // The OWED reseal is a new generation: posted once on the next tick.
    let clean = run::record(&gate.cas, None, &record("PASS")).unwrap();
    assert_eq!((clean.gate.as_ref().map(|g| g.verdict), clean.generation), (Some(verdict::GateVerdict::Clean), 2));
    drive(&mut lease);
    assert_eq!(slack::posted(&dir).len(), 2);
    let calls = violet.calls.lock().unwrap().len();
    drive(&mut lease);
    assert_eq!(violet.calls.lock().unwrap().len(), calls, "a posted generation never posts again");

    std::fs::write(dir.join(slack::SLACK_FILE), b"{ not json").unwrap();
    drive(&mut lease);
    let aside = std::fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("slack.json.corrupt-"));
    assert!(aside, "a corrupt slack.json is moved aside, not overwritten");
    let notes = run::load(&gate.cas, &gate.sha).unwrap().unwrap().notes;
    assert!(notes.iter().any(|n| n.contains("did not parse")), "{notes:?}");
}
