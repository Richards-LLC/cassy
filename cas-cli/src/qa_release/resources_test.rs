// Included in qa_release::tests: the resource ledger's regressions.
use std::time::{Duration, Instant};

fn metered_registry(yaml: &str) -> crate::qa_registry::Registry {
    let registry: crate::qa_registry::Registry = serde_yaml::from_str(yaml).unwrap();
    for resource in registry.resources.values() {
        resource.limit().unwrap();
    }
    registry
}

/// Two real processes contend on one (env, resource) ledger at the same
/// instant: together they never take more than `per_min`, nor hold more than
/// `concurrency` requests at once. Pipes order the phases; nothing sleeps.
#[test]
fn resources() {
    use crate::qa_registry::Limit;
    use cas_store::ResourceGrant;
    use resources::try_take;
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    const NOW: i64 = 1_800_000_000_000;
    let rate = Limit::Rate { per_min: 6, concurrency: None };
    let held = Limit::Rate { per_min: 100, concurrency: Some(3) };
    let take = |root: &Path, name: &str, limit: Limit, now: i64| {
        try_take(root, "staging", name, limit, 1, now, Duration::from_secs(60)).unwrap().unwrap()
    };

    if let Some(root) = std::env::var_os("CAS_QA_BUCKET_CHILD") {
        let root = std::path::PathBuf::from(root);
        let mut parent = std::io::stdin().lock().lines();
        println!("BUCKET ready");
        assert_eq!(parent.next().unwrap().unwrap(), "go");
        let granted = (0..6)
            .filter(|_| matches!(take(&root, "staging-api", rate, NOW), ResourceGrant::Granted(_)))
            .count();
        // Concurrency permits stay held until the parent has both counts.
        let holding = (0..3)
            .filter(|_| matches!(take(&root, "uploads", held, NOW), ResourceGrant::Granted(_)))
            .count();
        println!("BUCKET {granted} {holding}");
        assert_eq!(parent.next().unwrap().unwrap(), "end");
        return;
    }

    let cas = tempfile::tempdir().unwrap();
    let root = cas.path();
    // Open the ledger once, so the children contend only on takes.
    assert_eq!(cas_store::resource_used(root, "staging", "staging-api", 0).unwrap(), 0);
    let mut children: Vec<_> = (0..2)
        .map(|_| {
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "qa_release::tests::resources", "--nocapture"])
                .env("CAS_QA_BUCKET_CHILD", root)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut outputs: Vec<_> = children
        .iter_mut()
        .map(|child| BufReader::new(child.stdout.take().unwrap()).lines())
        .collect();
    let mut report = |lines: &mut std::io::Lines<BufReader<std::process::ChildStdout>>| loop {
        let line = lines.next().expect("child exited before reporting").unwrap();
        if let Some(rest) = line.strip_prefix("BUCKET ") {
            break rest.to_owned();
        }
    };
    for lines in &mut outputs {
        assert_eq!(report(lines), "ready");
    }
    let mut tell = |line: &str| {
        for child in &mut children {
            writeln!(child.stdin.as_mut().unwrap(), "{line}").unwrap();
        }
    };
    tell("go");
    let counts: Vec<Vec<usize>> = outputs
        .iter_mut()
        .map(|lines| report(lines).split(' ').map(|n| n.parse().unwrap()).collect())
        .collect();
    tell("end");
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    assert_eq!(counts.iter().map(|c| c[0]).sum::<usize>(), 6, "per_min across processes: {counts:?}");
    assert_eq!(counts.iter().map(|c| c[1]).sum::<usize>(), 3, "concurrency across processes: {counts:?}");
    // The window frees exactly one minute after the shared takes.
    let minute = NOW + 60_000;
    assert_eq!(take(root, "staging-api", rate, NOW), ResourceGrant::RetryAt { earliest: minute, latest: minute });
    assert!(matches!(take(root, "staging-api", rate, minute), ResourceGrant::Granted(_)));
    assert_eq!(take(root, "uploads", held, NOW), ResourceGrant::RetryAt { earliest: NOW, latest: minute });
    // Each env has its own bucket.
    assert!(matches!(
        try_take(root, "prod", "staging-api", rate, 1, NOW, Duration::ZERO).unwrap(),
        Some(ResourceGrant::Granted(_))
    ));
}

/// Preflight need is Σ consumes × runs × cells; a balance shortfall is
/// granted in `grant_envs` and refused elsewhere, naming the resource, the
/// need and the balance. Quota refusals are local and make no call.
#[test]
fn resources_preflight() {
    use resources::{PlannedRow, preflight};
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let project = tempfile::tempdir().unwrap();
    let cas = tempfile::tempdir().unwrap();
    let log = cas.path().join("calls.log");
    let config = fake_adapter(project.path(), "FAKE_BALANCE,FAKE_GRANT,FAKE_CALL_LOG,FAKE_ERROR_KIND");
    assert_eq!(config.grant_envs, ["staging"], "grant defaults to staging only");
    env.set("FAKE_CALL_LOG", &log);
    let registry = metered_registry(
        "resources:\n  staging-api: {kind: rate, per_min: 60, concurrency: 4}\n  credits: {kind: balance}\n  visitor-audits: {kind: quota, per_day: 3}\njourneys:\n  - id: audit\n    consumes: {staging-api: 12, credits: 10, visitor-audits: 1}\n",
    );
    let audit = &registry.journeys[0];
    let run = |target: &str, cells: u32| {
        let rows = [PlannedRow { journey: audit, runs: 2, cells }];
        preflight(project.path(), cas.path(), &config, target, &"a".repeat(40), &registry.resources, &rows)
    };
    let calls = || std::fs::read_to_string(&log).unwrap_or_default();

    let refusal = run("staging", 2).unwrap_err().to_string();
    assert_eq!(refusal, "RESOURCE SHORTFALL: visitor-audits needs 4, 3 of 3 left in 24h on staging");
    assert!(calls().is_empty(), "a local refusal makes no adapter call");

    env.set("FAKE_BALANCE", r#"{"credits":50}"#);
    let plan = run("staging", 1).unwrap();
    assert_eq!(plan.need["credits"], 20);
    assert_eq!(plan.need["staging-api"], 24);
    assert!(plan.granted.is_empty() && !calls().contains(r#""verb":"grant""#));

    env.set("FAKE_BALANCE", r#"{"credits":5}"#);
    env.set("FAKE_GRANT", r#"{"credits":20}"#);
    let plan = run("staging", 1).unwrap();
    assert_eq!((plan.granted["credits"], plan.balance["credits"]), (15, 20));
    assert!(calls().contains(r#""args":{"amounts":{"credits":15}}"#), "{}", calls());

    std::fs::remove_file(&log).unwrap();
    let refusal = run("prod", 1).unwrap_err().to_string();
    assert_eq!(
        refusal,
        "RESOURCE SHORTFALL: credits needs 20, balance 5 on prod; prod is not in qa.release.grant_envs"
    );
    assert!(!calls().contains(r#""verb":"grant""#), "grant is refused outside grant_envs");
    env.set("FAKE_GRANT", r#"{"credits":12}"#);
    let refusal = run("staging", 1).unwrap_err().to_string();
    assert!(refusal.ends_with("needs 20, balance 12 on staging; grant did not cover it"), "{refusal}");

    env.set("FAKE_ERROR_KIND", "insufficient");
    let blocked = run("staging", 1).unwrap_err().blocked().expect("402 is BLOCKED_EXTERNAL");
    assert_eq!(blocked.provider, "fake-health");
}

/// A 429 retries once after `retry_after_s` only when the wall clock allows;
/// it and a 402 are BLOCKED_EXTERNAL. Every request, retries included, is
/// metered, and a full bucket refuses before calling the adapter.
#[test]
fn resources_throttle() {
    use resources::{Meter, MeteredError, call_metered};
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let project = tempfile::tempdir().unwrap();
    let cas = tempfile::tempdir().unwrap();
    let log = cas.path().join("calls.log");
    let config = fake_adapter(project.path(), "FAKE_ERROR_KIND,FAKE_RETRY_AFTER,FAKE_CALL_LOG");
    env.set("FAKE_CALL_LOG", &log);
    let registry = metered_registry(
        "resources:\n  staging-api: {kind: rate, per_min: 3, concurrency: 1}\n  credits: {kind: balance}\njourneys:\n  - id: audit\n    consumes: {staging-api: 1}\n  - id: checkout\n    consumes: {credits: 5}\n",
    );
    let sha = "a".repeat(40);
    let run = |journey: &crate::qa_registry::Journey, clock_secs: u64| {
        let meter = Meter {
            cas_root: cas.path(),
            env: "staging",
            resources: &registry.resources,
            deadline: Instant::now() + Duration::from_secs(clock_secs),
        };
        let request = Request { env: "staging", sha: &sha, journey: Some(&journey.id), ..Request::default() };
        call_metered(project.path(), &config, &meter, journey, Verb::Run, request)
    };
    let calls = || std::fs::read_to_string(&log).map_or(0, |log| log.lines().count());
    let (audit, checkout) = (&registry.journeys[0], &registry.journeys[1]);

    env.set("FAKE_ERROR_KIND", "rate_limited");
    match run(audit, 5) {
        Err(MeteredError::Blocked(blocked)) => {
            assert!(blocked.to_string().starts_with("BLOCKED_EXTERNAL{provider=fake-health"));
        }
        other => panic!("expected BLOCKED_EXTERNAL, got {other:?}"),
    }
    assert_eq!(calls(), 1, "retry_after_s 30 does not fit a 5s clock");
    env.set("FAKE_RETRY_AFTER", "0");
    assert!(matches!(run(audit, 5), Err(MeteredError::Blocked(_))));
    assert_eq!(calls(), 3, "exactly one retry, after the first request's hold ended");
    assert!(matches!(run(audit, 5), Err(MeteredError::Budget { resource }) if resource == "staging-api"));
    assert_eq!(calls(), 3, "a full bucket refuses before the adapter");

    env.set("FAKE_ERROR_KIND", "insufficient");
    assert!(matches!(run(checkout, 5), Err(MeteredError::Blocked(b)) if b.provider == "fake-health"));
    assert_eq!(calls(), 4, "a 402 is not retried");
}
