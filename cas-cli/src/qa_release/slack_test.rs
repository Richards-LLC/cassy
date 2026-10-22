// Included in qa_release::tests: the serve driver's Violet post (cas-efb8).

/// Seals made by `record` (the CLI path, which has no proxy) only note the
/// post; the driver posts each sealed generation through the driver's Violet
/// transport (a fake here; the upload URL is a loopback double), on a fake
/// clock, never spamming the channel. One generation per case:
/// 1. Violet down: persisted 30 s doubling backoff, then the post and its permalink.
/// 2. The second file fails: one top-level message, recorded partial, never retried.
/// 3. A permanent refusal stops.
/// 4. An attempt that cannot be recorded first is not posted, and an
///    in-flight attempt after a restart is not retried.
/// 5. A post whose record fails to save is not reposted; its permalink is noted.
/// 6. A failing tick still posts.
///
/// Then: an unreadable pack is noted once, a channel change resets the mark
/// so nothing old back-posts, and a corrupt `slack.json` is moved aside.
#[test]
fn slack_post_from_driver() {
    use run::{Record, State};
    use sha2::Digest;
    use std::future::Future;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    type Reply = Pin<Box<dyn Future<Output = Result<serde_json::Value, crate::artifacts::slack::PostError>> + Send>>;
    const UP: u8 = 0;
    const DOWN: u8 = 1;
    const REFUSED: u8 = 2;
    const FAIL_SECOND: u8 = 3;
    const SAVE_FAILS: u8 = 4;

    struct FakeViolet {
        upload: String,
        mode: AtomicU8,
        dir: PathBuf,
        calls: Mutex<Vec<serde_json::Value>>,
    }
    impl slack::Transport for FakeViolet {
        fn post(&self, args: serde_json::Value) -> Reply {
            use crate::artifacts::slack::PostError;
            let mut calls = self.calls.lock().unwrap();
            let n = calls.len();
            calls.push(args.clone());
            let begin = args["step"] == "begin";
            let reply = match self.mode.load(Ordering::SeqCst) {
                DOWN => Err(PostError::new("violet_unavailable", "Violet is down")),
                REFUSED => Err(PostError::new("audience_denied", "the channel's audience refuses this post")),
                FAIL_SECOND if begin && args["filename"] != "pack.json" => {
                    Err(PostError::new("violet_unavailable", "Violet dropped the reply"))
                }
                _ if begin => Ok(serde_json::json!({"ok": true, "upload_url": self.upload, "file_id": format!("F{n}")})),
                mode => {
                    if mode == SAVE_FAILS {
                        // Slack accepted the post; the record write that follows fails.
                        std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o555)).unwrap();
                    }
                    Ok(serde_json::json!({"ok": true,
                        "file": {"file_id": args["file_id"], "size_bytes": args["size_bytes"], "sha256": args["sha256"]},
                        "message": {"message_id": format!("m{n}"), "permalink": format!("https://slack.test/p/{n}")}}))
                }
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
    let dir = pack::pack_dir(&gate.cas, &gate.sha).unwrap();
    let violet = std::sync::Arc::new(FakeViolet {
        upload: format!("{}/upload", upload.uri()),
        mode: AtomicU8::new(DOWN),
        dir: dir.clone(),
        calls: Mutex::default(),
    });
    slack::install(violet.clone(), runtime.handle().clone());
    let calls = || violet.calls.lock().unwrap().len();
    let top_level = || violet.calls.lock().unwrap().iter().filter(|c| c["step"] == "begin" && c["filename"] == "pack.json").count();
    let notes = || run::load(&gate.cas, &gate.sha).unwrap().unwrap().notes;
    let noted = |text: &str| notes().iter().filter(|n| n.contains(text)).count();
    let t0 = chrono::Utc::now();
    let at = |hours: f64| run::post_sealed_runs(&gate.cas, t0 + chrono::Duration::milliseconds((hours * 3_600_000.0) as i64));
    let set_mode = |mode| violet.mode.store(mode, Ordering::SeqCst);
    let chmod = |path: &Path, mode| std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    // A real PNG frame, so each bundle uploads a second file after pack.json.
    let evidence = gate.project.root().join("evidence-png");
    std::fs::create_dir_all(&evidence).unwrap();
    std::fs::write(evidence.join("walk.png"), b"\x89PNG\r\n\x1a\nframe").unwrap();
    let record = |verdict: &str, reason: &str| {
        let input = Record {
            row: "support".into(),
            verdict: verdict.into(),
            reason: Some(reason.into()),
            evidence: Some(evidence.clone()),
            ..Record::default()
        };
        run::record(&gate.cas, Some(&gate.sha), &input).unwrap()
    };

    run::start(&gate.cas, &gate.sha).unwrap();
    run::tick(&gate.cas, &AtomicBool::new(false)).unwrap();
    let g1 = record("NOT_EXERCISED", "generation 1");
    assert_eq!((g1.state, g1.gate.as_ref().map(|g| g.verdict)), (State::Sealed, Some(verdict::GateVerdict::Owed)));
    assert!(g1.notes.iter().any(|n| n.contains("the cas serve driver posts this generation to qa-release")));
    assert_eq!(calls(), 0, "the CLI seal posts nothing");

    // 1. Violet down: calls at 0, 31 s and 92 s only; the failure is noted once.
    for (seconds, want) in [(0.0, 1), (10.0, 1), (31.0, 2), (60.0, 2), (92.0, 3)] {
        at(seconds / 3600.0);
        assert_eq!(calls(), want, "at t0+{seconds}s");
    }
    assert_eq!(noted("Slack post failed (retrying with backoff)"), 1);
    assert_eq!(slack::backoff(7), chrono::Duration::minutes(30), "the backoff caps at 30 minutes");
    set_mode(UP);
    at(1.0);
    let posted = slack::posted(&dir);
    assert_eq!((posted.len(), posted[0].agent_id.as_str(), posted[0].channel.as_str()), (1, slack::DRIVER_AGENT, "qa-release"));
    assert!(!posted[0].partial && posted[0].files >= 2, "{posted:?}");
    assert_eq!(noted(&format!("Slack: posted pack {}", &posted[0].pack_sha256[..12])), 1);
    assert!(notes().iter().any(|n| n.contains(&posted[0].permalink)));

    // 2. The second file fails: one top-level message, recorded partial, final.
    record("NOT_EXERCISED", "generation 2");
    set_mode(FAIL_SECOND);
    let before = top_level();
    at(2.0);
    at(5.0);
    assert_eq!(top_level(), before + 1, "one top-level message across ticks");
    let partial = slack::posted(&dir).pop().unwrap();
    assert!(partial.partial && partial.missing.iter().any(|m| m.ends_with("walk.png")), "{partial:?}");
    assert_eq!(noted("Slack: partial post"), 1);

    // 3. A permanent refusal stops.
    record("NOT_EXERCISED", "generation 3");
    set_mode(REFUSED);
    let before = calls();
    at(6.0);
    at(9.0);
    assert_eq!(calls(), before + 1, "a permanent refusal is not retried");
    assert_eq!(noted("Slack post failed (not retrying)"), 1);

    // 4. No record, no post; an in-flight attempt after a restart is never retried.
    record("NOT_EXERCISED", "generation 4");
    set_mode(UP);
    chmod(&dir, 0o555);
    at(10.0);
    chmod(&dir, 0o755);
    assert_eq!(calls(), before + 1, "no record, no post");
    let sha = format!("{:x}", sha2::Sha256::digest(std::fs::read(dir.join(pack::PACK_FILE)).unwrap()));
    let mut ledger: slack::Ledger = serde_json::from_slice(&std::fs::read(dir.join(slack::SLACK_FILE)).unwrap()).unwrap();
    ledger.attempts.push(slack::Attempt {
        pack_sha256: sha.clone(),
        attempts: 1,
        next_at: t0,
        last_error: String::new(),
        stopped: false,
        in_flight: true,
    });
    std::fs::write(dir.join(slack::SLACK_FILE), serde_json::to_vec(&ledger).unwrap()).unwrap();
    at(11.0);
    at(12.0);
    assert_eq!(calls(), before + 1, "an interrupted post is not retried");
    assert_eq!(noted("Slack post stopped, not retrying: an earlier post was interrupted"), 1);

    // 5. Posted, but the record cannot be saved: never reposted; permalink noted.
    record("NOT_EXERCISED", "generation 5");
    set_mode(SAVE_FAILS);
    let before = top_level();
    let pack_sha = || format!("{:x}", sha2::Sha256::digest(std::fs::read(dir.join(pack::PACK_FILE)).unwrap()));
    let g5 = pack_sha();
    at(13.0);
    chmod(&dir, 0o755);
    set_mode(UP);
    at(14.0);
    assert_eq!(top_level(), before + 1, "a post whose record failed is not reposted");
    assert_eq!(noted("this process will not repost it"), 1);
    assert_eq!(noted("Slack: posted pack"), 2, "the receipt of the second post is noted");
    let g5_note = notes().into_iter().find(|n| n.contains(&format!("Slack: posted pack {}", &g5[..12]))).unwrap();
    assert!(g5_note.contains("to qa-release as release-gate-driver: https://slack.test/p/"), "{g5_note}");

    // 6. A failing tick (a live run on a broken registry) still posts.
    let clean = record("PASS", "generation 6");
    assert_eq!((clean.gate.as_ref().map(|g| g.verdict), clean.generation), (Some(verdict::GateVerdict::Clean), 6));
    let other = gate.project.commit("apps/web/page.ts", "v3");
    run::start(&gate.cas, &other).unwrap();
    let registry = gate.project.root().join("docs/qa/journeys.yaml");
    let valid = std::fs::read(&registry).unwrap();
    std::fs::write(&registry, "journeys: [").unwrap();
    let before = top_level();
    let g6 = pack_sha();
    assert!(run::drive(&gate.cas, &mut None, &AtomicBool::new(false)).is_err(), "the tick fails");
    assert_eq!(top_level(), before + 1, "generation 6 posted anyway");
    let finished = slack::posted(&dir).into_iter().find(|p| p.pack_sha256 == g6).expect("generation 6 recorded");
    assert!(!finished.partial && finished.files >= 2 && finished.permalink.starts_with("https://slack.test/p/"), "{finished:?}");
    std::fs::write(&registry, valid).unwrap();

    // An unreadable pack is noted once, not every tick.
    let pack_json = dir.join(pack::PACK_FILE);
    chmod(&pack_json, 0o000);
    at(15.0);
    at(16.0);
    chmod(&pack_json, 0o644);
    assert_eq!(noted("pack.json is unreadable"), 1);

    assert!(violet.calls.lock().unwrap().iter().all(|c| c["channel"] == "qa-release"), "every call went to slack_channel");

    // A new channel resets the mark: nothing sealed before it and over 24 h old back-posts.
    let mut config = crate::config::Config::load(&gate.cas).unwrap();
    config.set("qa.release.slack_channel", "qa-other").unwrap();
    config.save(&gate.cas).unwrap();
    std::fs::remove_file(dir.join(slack::SLACK_FILE)).unwrap();
    let before = calls();
    at(48.0);
    let mark: serde_json::Value =
        serde_json::from_slice(&std::fs::read(gate.cas.join("release-gate").join(slack::CHANNEL_FILE)).unwrap()).unwrap();
    assert_eq!(mark["channel"], "qa-other");
    assert_eq!(calls(), before, "an old pack is not back-posted to the new channel");

    std::fs::write(dir.join(slack::SLACK_FILE), b"{ not json").unwrap();
    at(49.0);
    let aside = std::fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("slack.json.corrupt-"));
    assert!(aside, "a corrupt slack.json is moved aside, not overwritten");
    assert!(noted("did not parse") >= 1);
}
