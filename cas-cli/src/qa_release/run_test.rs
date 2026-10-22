// Included in qa_release::tests: the release-gate run's regressions.
mod run_support {
    use super::super::test_project::Project;
    use std::path::PathBuf;

    pub(super) const REGISTRY: &str = "version: 1
resources:
  credits: {kind: balance}
journeys:
  - id: checkout
    status: live
    owners: [apps/backend/**]
    spec: e2e/checkout.spec.ts
    release_always: true
    consumes: {credits: 5}
  - id: web
    status: live
    owners: [apps/web/**]
    spec: e2e/web.spec.ts
  - id: settings
    status: live
    owners: [apps/settings/**]
    spec: e2e/settings.spec.ts
  - id: support
    status: live
    owners: [apps/web/**]
";

    /// A project on `staging` one web commit past `main`, with the fake
    /// adapter serving that commit and `[qa.release]` saved in its store.
    pub(super) struct Gate {
        pub(super) project: Project,
        pub(super) cas: PathBuf,
        pub(super) log: PathBuf,
        pub(super) sha: String,
    }

    pub(super) fn gate(env: &mut crate::test_support::TestEnvGuard, settings: &[(&str, &str)]) -> Gate {
        let project = Project::new();
        project.commit(".gitignore", ".cas/\n");
        project.commit("docs/qa/journeys.yaml", REGISTRY);
        project.commit("apps/backend/api.ts", "v1");
        project.git(&["checkout", "-qb", "staging"]);
        let sha = project.commit("apps/web/page.ts", "v2");
        let cas = crate::store::init_cas_dir(project.root()).unwrap();
        let mut config = crate::config::Config::default();
        let defaults = [
            ("adapter", "./scripts/qa/fake-adapter"),
            (
                "env",
                "FAKE_SERVED_SHA,FAKE_BALANCE,FAKE_CALL_LOG,FAKE_RUN_FAIL,FAKE_RUN_ERROR_KIND",
            ),
            ("integration_branch", "staging"),
            ("branches", "main"),
            ("components", r#"{"backend":["apps/backend"]}"#),
            ("timeout_secs", "10"),
        ];
        for (key, value) in defaults.iter().chain(settings) {
            config.set(&format!("qa.release.{key}"), value).unwrap();
        }
        config.save(&cas).unwrap();
        let log = cas.join("calls.log");
        env.set("FAKE_SERVED_SHA", &sha);
        env.set("FAKE_BALANCE", r#"{"credits":10}"#);
        env.set("FAKE_CALL_LOG", &log);
        Gate { project, cas, log, sha }
    }

    impl Gate {
        pub(super) fn calls(&self) -> usize {
            std::fs::read_to_string(&self.log).map_or(0, |log| log.lines().count())
        }

        /// A directory holding one evidence frame.
        pub(super) fn evidence(&self, name: &str) -> PathBuf {
            let dir = self.project.root().join(format!("evidence-{name}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("walk.png"), name).unwrap();
            dir
        }
    }
}

fn verdicts(run: &run::Run) -> Vec<(String, &'static str)> {
    let mut rows: Vec<_> = run.rows.iter().map(|r| (r.id.clone(), r.verdict.word())).collect();
    rows.sort();
    rows
}

/// Scope is the release_always journey plus the journeys the range impacts;
/// start is idempotent per SHA and refuses a second SHA; the freeze holds
/// until the last manual row seals the pack CLEAN.
#[test]
fn run_scope_freeze_seal() {
    use run::{Record, State, active_freeze};
    use std::sync::atomic::AtomicBool;
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let gate = run_support::gate(&mut env, &[("suites", r#"{"e2e":{"journeys":["checkout","web"],"wall_clock_mins":5,"concurrency":2}}"#)]);
    let config = configured(&gate.cas).unwrap();
    let frozen = |cas: &Path| active_freeze(cas, Some(&config), "staging", chrono::Utc::now());

    let started = run::start(&gate.cas, &gate.sha).unwrap();
    let mut scope: Vec<_> = started.plan.iter().map(|p| (p.id.as_str(), p.why.as_str(), p.manual)).collect();
    scope.sort();
    assert_eq!(
        scope,
        [("checkout", "release_always", false), ("support", "impacted", true), ("web", "impacted", false)],
        "settings is neither main nor impacted"
    );
    assert_eq!(started.resources.need["credits"], 5, "resource preflight ran once over the plan");
    let calls = gate.calls();
    assert_eq!(run::start(&gate.cas, &gate.sha).unwrap(), started);
    assert_eq!(gate.calls(), calls, "a repeated start makes no adapter call");
    let other = gate.project.commit("apps/web/page.ts", "v3");
    let refusal = run::start(&gate.cas, &other).unwrap_err().to_string();
    assert!(refusal.contains("abort it first"), "{refusal}");
    assert_eq!(frozen(&gate.cas).map(|f| f.sha), Some(gate.sha.clone()));
    assert!(active_freeze(&gate.cas, Some(&config), "main", chrono::Utc::now()).is_none());

    // One server holds the driver lease; another takes it over once it exits.
    let no = AtomicBool::new(false);
    let mut holder = None;
    assert!(run::drive(&gate.cas, &mut holder, &no).unwrap(), "the first server leases and ticks");
    let mut standby = None;
    assert!(!run::drive(&gate.cas, &mut standby, &no).unwrap(), "a held lease is not shared");
    drop(holder);
    assert!(run::drive(&gate.cas, &mut standby, &no).unwrap(), "the lease is taken over");
    let walked = run::load(&gate.cas, &gate.sha).unwrap().unwrap();
    assert_eq!(walked.state, State::Manual);
    assert_eq!(verdicts(&walked), [("checkout".into(), "PASS"), ("web".into(), "PASS")]);
    assert!(frozen(&gate.cas).is_some(), "manual rows keep the freeze");

    let record = Record {
        row: "support".into(),
        verdict: "PASS".into(),
        evidence: Some(gate.evidence("support")),
        minutes: Some(7),
        ..Record::default()
    };
    let sealed = run::record(&gate.cas, None, &record).unwrap();
    assert_eq!(sealed.state, State::Sealed);
    assert!(frozen(&gate.cas).is_none(), "seal lifts the freeze");
    let pack = pack::Pack::load(&pack::pack_dir(&gate.cas, &gate.sha).unwrap()).unwrap();
    assert_eq!(pack.gate.verdict, verdict::GateVerdict::Clean);
    assert_eq!(pack.rows.iter().find(|r| r.id == "support").unwrap().manual_minutes, Some(7));
    let json = run::status_json(&gate.cas, &sealed).unwrap();
    assert_eq!((json["sha"].as_str(), json["hand_walk"]["total_minutes"].as_u64()), (Some(gate.sha.as_str()), Some(7)));
    assert_eq!(pack.media.len(), 3, "one frame per row, digested");
}

/// A shutdown between rows leaves them for the next tick and keeps the suite
/// clock; an expired clock times its rows out; the OWED pack reseals as a
/// new generation when an owed row is re-walked, and CLEAN is final.
#[test]
fn run_restart_timeout_record() {
    use run::{Record, State};
    use std::sync::atomic::AtomicBool;
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let gate = run_support::gate(
        &mut env,
        &[(
            "suites",
            r#"{"a":{"journeys":["checkout"],"wall_clock_mins":5},"b":{"journeys":["web"],"wall_clock_mins":1}}"#,
        )],
    );
    run::start(&gate.cas, &gate.sha).unwrap();
    run::tick(&gate.cas, &AtomicBool::new(true)).unwrap();
    let stopped = run::load(&gate.cas, &gate.sha).unwrap().unwrap();
    assert!(stopped.state == State::Suites && stopped.rows.is_empty(), "{stopped:?}");
    let clock = stopped.suites[0].started_at;

    // Suite b's clock ran out before the restart.
    let path = gate.cas.join("release-gate").join(format!("{}.json", gate.sha));
    let mut file: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let ago = chrono::Utc::now() - chrono::Duration::minutes(2);
    file["suites"].as_array_mut().unwrap().push(serde_json::json!({"name": "b", "started_at": ago}));
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    run::tick(&gate.cas, &AtomicBool::new(false)).unwrap();
    let resumed = run::load(&gate.cas, &gate.sha).unwrap().unwrap();
    assert_eq!(resumed.suites[0].started_at, clock, "a restart keeps the suite clock");
    assert_eq!(verdicts(&resumed), [("checkout".into(), "PASS"), ("web".into(), "NOT_EXERCISED")]);
    assert!(resumed.rows.iter().any(|r| r.verdict.to_string().starts_with("NOT_EXERCISED{timed_out}")));

    let record = |row: &str, verdict: &str| Record {
        row: row.into(),
        verdict: verdict.into(),
        evidence: Some(gate.evidence(row)),
        ..Record::default()
    };
    let owed = run::record(&gate.cas, Some(&gate.sha), &record("support", "PASS")).unwrap();
    assert_eq!(owed.gate.as_ref().map(|g| g.verdict), Some(verdict::GateVerdict::Owed));
    let clean = run::record(&gate.cas, Some(&gate.sha), &record("web", "PASS")).unwrap();
    assert_eq!((clean.gate.as_ref().map(|g| g.verdict), clean.generation), (Some(verdict::GateVerdict::Clean), 2));
    let dir = pack::pack_dir(&gate.cas, &gate.sha).unwrap();
    assert_eq!(pack::Pack::load(&dir).unwrap().gate.verdict, verdict::GateVerdict::Clean);
    let first: pack::Pack = serde_json::from_slice(&std::fs::read(dir.join("history/pack-1.json")).unwrap()).unwrap();
    assert_eq!(first.gate.verdict, verdict::GateVerdict::Owed, "the OWED generation is kept");
    let refusal = run::record(&gate.cas, Some(&gate.sha), &record("web", "PASS")).unwrap_err().to_string();
    assert!(refusal.contains("which is final"), "{refusal}");
}

/// Preflight refuses before the freeze and persists nothing; a hand-edited
/// component that `cas config set` would refuse is refused at load; a 429 on
/// `run` goes through the metered call and leaves the row BLOCKED_EXTERNAL.
#[test]
fn run_preflight_refusals() {
    use std::sync::atomic::AtomicBool;
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let gate = run_support::gate(
        &mut env,
        &[("grant_envs", ""), ("suites", r#"{"e2e":{"journeys":["checkout","web"],"wall_clock_mins":5}}"#)],
    );
    env.set("FAKE_BALANCE", r#"{"credits":1}"#);
    let refusal = run::start(&gate.cas, &gate.sha).unwrap_err().to_string();
    assert_eq!(
        refusal,
        "RESOURCE SHORTFALL: credits needs 5, balance 1 on staging; staging is not in qa.release.grant_envs"
    );
    assert!(run::load(&gate.cas, &gate.sha).unwrap().is_none(), "a refused preflight persists nothing");

    env.set("FAKE_BALANCE", r#"{"credits":10}"#);
    env.set("FAKE_RUN_ERROR_KIND", "rate_limited");
    run::start(&gate.cas, &gate.sha).unwrap();
    run::tick(&gate.cas, &AtomicBool::new(false)).unwrap();
    let blocked = run::load(&gate.cas, &gate.sha).unwrap().unwrap();
    assert_eq!(verdicts(&blocked), [("checkout".into(), "BLOCKED_EXTERNAL"), ("web".into(), "BLOCKED_EXTERNAL")]);
    run::abort(&gate.cas, None, "provider outage").unwrap();

    // A driver that first ticks after the TTL walks nothing: every row times
    // out and the run seals. A row recorded after that seal reseals.
    let runs = |gate: &run_support::Gate| {
        std::fs::read_to_string(&gate.log).unwrap().matches(r#""verb":"run""#).count()
    };
    run::start(&gate.cas, &gate.sha).unwrap();
    let path = gate.cas.join("release-gate").join(format!("{}.json", gate.sha));
    let pack = pack::pack_dir(&gate.cas, &gate.sha).unwrap();
    let staged = |pack: &Path| {
        std::fs::read_dir(pack).map_or(0, |d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(".staged-")).count())
    };
    // Row ids and arguments are refused before any evidence is copied.
    let mut bad = run::Record { row: "..".into(), verdict: "PASS".into(), evidence: Some(gate.evidence("bad")), ..run::Record::default() };
    let refusal = run::record(&gate.cas, Some(&gate.sha), &bad).unwrap_err().to_string();
    assert!(refusal.contains("and not `.` or `..`"), "{refusal}");
    bad.row = "support".into();
    bad.verdict = "FAIL".into();
    assert_eq!(run::record(&gate.cas, Some(&gate.sha), &bad).unwrap_err().to_string(), "a FAIL needs --reason");
    assert!(!pack.join("media/support").exists() && staged(&pack) == 0, "a refused record copies nothing");
    // The TTL tick times support out between record's two admits: the write
    // step refuses and the staged evidence is removed.
    let race = path.clone();
    run::tests_hook::set_before_write(move || {
        let mut file: serde_json::Value = serde_json::from_slice(&std::fs::read(&race).unwrap()).unwrap();
        file["rows"].as_array_mut().unwrap().push(serde_json::json!({"id": "support", "journey": "support",
            "verdict": {"verdict": "NOT_EXERCISED", "cause": "timed_out", "detail": "not walked before the freeze ended"}}));
        std::fs::write(&race, serde_json::to_vec(&file).unwrap()).unwrap();
    });
    let early = run::Record { row: "support".into(), verdict: "PASS".into(), evidence: Some(gate.evidence("early")), ..run::Record::default() };
    let refusal = run::record(&gate.cas, Some(&gate.sha), &early).unwrap_err().to_string();
    assert_eq!(refusal, "the TTL timed row support out; retry after the run seals to reseal it");
    assert!(!pack.join("media/support").exists() && staged(&pack) == 0, "the write-step refusal leaves no evidence");
    let mut file: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    file["freeze_until"] = serde_json::json!(chrono::Utc::now() - chrono::Duration::minutes(1));
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    let before = runs(&gate);
    run::tick(&gate.cas, &AtomicBool::new(false)).unwrap();
    let expired = run::load(&gate.cas, &gate.sha).unwrap().unwrap();
    assert_eq!(runs(&gate), before, "no suite starts after the TTL");
    assert_eq!(expired.state, run::State::Sealed);
    assert!(expired.rows.len() == 3 && expired.rows.iter().all(|r| r.verdict.to_string().starts_with("NOT_EXERCISED{timed_out}")));
    let record = run::Record {
        row: "support".into(),
        verdict: "PASS".into(),
        evidence: Some(gate.evidence("support")),
        ..run::Record::default()
    };
    let refusal = run::record(&gate.cas, None, &record).unwrap_err().to_string();
    assert!(refusal.contains("was sealed by the TTL; retry the record with --sha"), "{refusal}");
    let resealed = run::record(&gate.cas, Some(&gate.sha), &record).unwrap();
    assert_eq!(resealed.generation, 2);
    assert!(!resealed.ttl_sealed, "a reseal clears the TTL retry hint");
    assert_eq!(resealed.row("support").unwrap().verdict.word(), "PASS");

    // A corrupt run file, or one naming another SHA, is reported, never sliced.
    let dir = gate.cas.join("release-gate");
    std::fs::write(dir.join(format!("{}.json", "b".repeat(40))), "{not json").unwrap();
    std::fs::copy(&path, dir.join(format!("{}.json", "c".repeat(40)))).unwrap();
    let bad: Vec<String> = run::unreadable(&gate.cas).iter().map(|(_, e)| e.clone()).collect();
    assert!(bad.len() == 2 && bad.iter().any(|e| e.contains("not its own")), "{bad:?}");
    assert!(run::active(&gate.cas).unwrap().is_none());
    let until = chrono::Utc::now();
    assert!(run::Freeze { sha: "abc".into(), until }.refusal("staging").contains("for abc holds"));
    let wide = format!("a{}", "é".repeat(12));
    assert_eq!(short(&wide), format!("a{}", "é".repeat(11)), "cuts on a char boundary");
    let push = coverage::PrCoverage { pr: None, commit: wide.clone(), journeys: vec![], rows: vec![], manual_row: None };
    assert_eq!(push.subject(), format!("push a{}", "é".repeat(6)));
    assert!(crate::config::Config::default().set("qa.release.gate_env", "stag ing").is_err());

    let path = gate.cas.join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap().replace(r#""apps/backend""#, r#""../outside""#);
    std::fs::write(&path, text).unwrap();
    let refusal = run::start(&gate.cas, &gate.sha).unwrap_err().to_string();
    assert!(
        refusal.starts_with("RELEASE CONFIG INVALID: qa.release.components.backend path \"../outside\" must stay inside the repository"),
        "{refusal}"
    );
}
