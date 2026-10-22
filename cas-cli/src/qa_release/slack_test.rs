// Included in qa_release::tests: the serve driver's Violet post (cas-efb8).

/// Seals made by `record` (the CLI path, which has no proxy) only note the
/// post; the driver posts each sealed generation once through the driver's
/// Violet transport (a fake here; the upload URL is a loopback double), on a
/// fake clock:
/// - a transient failure retries with a persisted 30 s doubling backoff;
/// - a permanent refusal stops for good;
/// - an attempt that cannot be recorded first is not posted;
/// - a failing tick still posts;
/// - nothing sealed before the channel was set (and over 24 h old) is back-posted;
/// - a corrupt `slack.json` is moved aside.
#[test]
fn slack_post_from_driver() {
    use run::{Record, State};
    use std::future::Future;
    use std::os::unix::fs::PermissionsExt;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    type Reply = Pin<Box<dyn Future<Output = Result<serde_json::Value, crate::artifacts::slack::PostError>> + Send>>;
    const UP: u8 = 0;
    const DOWN: u8 = 1;
    const REFUSED: u8 = 2;

    struct FakeViolet {
        upload: String,
        mode: AtomicU8,
        calls: Mutex<Vec<serde_json::Value>>,
    }
    impl slack::Transport for FakeViolet {
        fn post(&self, args: serde_json::Value) -> Reply {
            use crate::artifacts::slack::PostError;
            let mut calls = self.calls.lock().unwrap();
            let n = calls.len();
            calls.push(args.clone());
            let reply = match self.mode.load(Ordering::SeqCst) {
                DOWN => Err(PostError::new("violet_unavailable", "Violet is down")),
                REFUSED => Err(PostError::new("audience_denied", "the channel's audience refuses this post")),
                _ if args["step"] == "begin" => {
                    Ok(serde_json::json!({"ok": true, "upload_url": self.upload, "file_id": format!("F{n}")}))
                }
                _ => Ok(serde_json::json!({"ok": true,
                    "file": {"file_id": args["file_id"], "size_bytes": args["size_bytes"], "sha256": args["sha256"]},
                    "message": {"message_id": format!("m{n}"), "permalink": "https://slack.test/p/gate"}})),
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
        mode: AtomicU8::new(DOWN),
        calls: Mutex::default(),
    });
    slack::install(violet.clone(), runtime.handle().clone());
    let calls = || violet.calls.lock().unwrap().len();
    let notes = || run::load(&gate.cas, &gate.sha).unwrap().unwrap().notes;
    let t0 = chrono::Utc::now();
    let at = |seconds: i64| run::post_sealed_runs(&gate.cas, t0 + chrono::Duration::seconds(seconds));
    let dir = pack::pack_dir(&gate.cas, &gate.sha).unwrap();
    let record = |verdict: &str, reason: &str| Record {
        row: "support".into(),
        verdict: verdict.into(),
        reason: Some(reason.into()),
        evidence: Some(gate.evidence("support")),
        ..Record::default()
    };

    run::start(&gate.cas, &gate.sha).unwrap();
    run::tick(&gate.cas, &AtomicBool::new(false)).unwrap();
    let owed = run::record(&gate.cas, None, &record("NOT_EXERCISED", "walked later")).unwrap();
    assert_eq!((owed.state, owed.gate.as_ref().map(|g| g.verdict)), (State::Sealed, Some(verdict::GateVerdict::Owed)));
    assert!(owed.notes.iter().any(|n| n.contains("the cas serve driver posts this generation to qa-release")));
    assert_eq!(calls(), 0, "the CLI seal posts nothing");

    // Violet is down: 30 s, then 60 s of persisted backoff; the failure is noted once.
    for (second, want) in [(0, 1), (10, 1), (31, 2), (60, 2), (92, 3)] {
        at(second);
        assert_eq!(calls(), want, "at t0+{second}s");
    }
    assert_eq!(notes().iter().filter(|n| n.contains("Slack post failed (retrying with backoff)")).count(), 1);
    assert_eq!(slack::backoff(7), chrono::Duration::minutes(30), "the backoff caps at 30 minutes");

    violet.mode.store(UP, Ordering::SeqCst);
    at(3600);
    let posted = slack::posted(&dir);
    assert_eq!((posted.len(), posted[0].agent_id.as_str(), posted[0].channel.as_str()), (1, slack::DRIVER_AGENT, "qa-release"));
    assert!(notes().iter().any(|n| n.contains("Slack: posted pack") && n.contains("https://slack.test/p/gate")), "{:?}", notes());
    assert!(violet.calls.lock().unwrap().iter().all(|c| c["channel"] == "qa-release"));
    let before = calls();
    at(7200);
    assert_eq!(calls(), before, "a posted generation never posts again");

    // Generation 2: a permanent refusal stops for good.
    run::record(&gate.cas, Some(&gate.sha), &record("NOT_EXERCISED", "still owed")).unwrap();
    violet.mode.store(REFUSED, Ordering::SeqCst);
    at(7300);
    at(7300 + 3 * 3600);
    assert_eq!(calls(), before + 1, "a permanent refusal is not retried");
    assert!(notes().iter().any(|n| n.contains("Slack post failed (not retrying)")), "{:?}", notes());

    // Generation 3: an attempt that cannot be recorded first is not posted.
    let clean = run::record(&gate.cas, Some(&gate.sha), &record("PASS", "walked")).unwrap();
    assert_eq!(clean.generation, 3);
    violet.mode.store(UP, Ordering::SeqCst);
    let writable = std::fs::metadata(&dir).unwrap().permissions();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    at(8000);
    std::fs::set_permissions(&dir, writable).unwrap();
    assert_eq!(calls(), before + 1, "no record, no post");

    // A failing tick (a live run on a broken registry) still posts.
    let other = gate.project.commit("apps/web/page.ts", "v3");
    run::start(&gate.cas, &other).unwrap();
    std::fs::write(gate.project.root().join("docs/qa/journeys.yaml"), "journeys: [").unwrap();
    assert!(run::drive(&gate.cas, &mut None, &AtomicBool::new(false)).is_err(), "the tick fails");
    assert_eq!(slack::posted(&dir).len(), 2, "generation 3 posted anyway");

    // Nothing sealed before the channel was set, and over 24 h old, back-posts.
    std::fs::remove_file(dir.join(slack::SLACK_FILE)).unwrap();
    let since = serde_json::json!({"channel": "qa-release", "since": t0 + chrono::Duration::hours(47)});
    std::fs::write(gate.cas.join("release-gate").join(slack::CHANNEL_FILE), since.to_string()).unwrap();
    let before = calls();
    at(48 * 3600);
    assert_eq!(calls(), before, "an old pack is not back-posted");

    std::fs::write(dir.join(slack::SLACK_FILE), b"{ not json").unwrap();
    at(48 * 3600 + 60);
    let aside = std::fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("slack.json.corrupt-"));
    assert!(aside, "a corrupt slack.json is moved aside, not overwritten");
    assert!(notes().iter().any(|n| n.contains("did not parse")), "{:?}", notes());
}
