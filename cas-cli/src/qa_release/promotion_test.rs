// Included in qa_release::tests: the promotion status `cassy/release-gate`.
/// A CLEAN pack passes its SHA and any tree-equal one; a different tree, a
/// FAILED pack or a prod flag mismatch fails by name; a supervisor override
/// passes only its own commit (or a tree-equal one) for 6 hours; and the
/// freeze and promotion statuses reach GitHub through the `gh` seam.
#[test]
fn promotion() {
    use super::flags::FlagSnapshot;
    use super::pack::{Pack, pack_dir};
    use super::promotion::{PromotionState, evaluate, publish_freeze, publish_promotions};
    use super::verdict::RowVerdict;
    use chrono::{Duration, NaiveDateTime};
    use std::os::unix::fs::PermissionsExt;

    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let project = Project::new();
    let root = project.root();
    let base = tempfile::tempdir().unwrap();
    let cas = base.path().join(".cas");
    std::fs::create_dir_all(&cas).unwrap();
    let tasks = crate::store::open_task_store_local(&cas).unwrap();
    tasks.init().unwrap();
    let mut settings = crate::config::Config::default();
    settings.set("factory.artifacts_root", base.path().join("artifacts").to_str().unwrap()).unwrap();
    settings.set("qa.github_status", "true").unwrap();
    settings.save(&cas).unwrap();
    let config = ReleaseConfig {
        integration_branch: "staging".into(),
        branches: vec!["main".into()],
        ..fake_adapter(root, "FAKE_FLAGS_PROD")
    };
    project.commit(
        "docs/qa/journeys.yaml",
        "journeys:\n  - id: checkout\n    depends_on_flags: [audio.enabled]\n",
    );
    let walked = project.commit("app.ts", "v1");
    project.git(&["commit", "-q", "--allow-empty", "-m", "Merge pull request #7"]);
    let merged = project.git(&["rev-parse", "HEAD"]);
    let changed = project.commit("app.ts", "v2");
    project.git(&["commit", "-q", "--allow-empty", "-m", "Merge pull request #8"]);
    let changed_merged = project.git(&["rev-parse", "HEAD"]);
    let unrelated = project.commit("app.ts", "v3");
    env.set("FAKE_FLAGS_PROD", r#"{"audio.enabled":true}"#);
    let now = NaiveDateTime::parse_from_str("2026-10-21 12:00", "%Y-%m-%d %H:%M").unwrap().and_utc();
    let check = |head: &str, at| evaluate(root, &cas, &config, head, at);

    assert_eq!(check(&walked, now).state, PromotionState::Pending, "no sealed pack yet");
    let seal = |sha: &str, verdict: RowVerdict| {
        let mut pack = Pack::new("cassy", sha, Vec::new());
        pack.rows = vec![row("checkout", "checkout", verdict)];
        pack.flags = Some(FlagSnapshot {
            env: "staging".into(),
            sha: sha.into(),
            values: [("audio.enabled".to_string(), serde_json::json!(true))].into(),
        });
        pack.seal(&pack_dir(&cas, sha).unwrap()).unwrap();
    };
    seal(&walked, RowVerdict::Pass);
    let exact = check(&walked, now);
    assert_eq!((exact.state, exact.walked.as_deref()), (PromotionState::Success, Some(walked.as_str())));
    let equal = check(&merged, now);
    assert_eq!((equal.state, equal.walked.as_deref()), (PromotionState::Success, Some(walked.as_str())));
    assert!(equal.description().contains("tree-equal"), "{}", equal.description());
    let differs = check(&changed, now);
    assert_eq!(differs.state, PromotionState::Failure);
    assert!(differs.description().contains(&format!("the latest pack walked {}", &walked[..12])), "{differs:?}");

    env.set("FAKE_FLAGS_PROD", r#"{"audio.enabled":false}"#);
    assert_eq!(
        check(&walked, now).reasons,
        ["CONFIG_MISMATCH audio.enabled: test=true target=false (checkout)"]
    );
    env.set("FAKE_FLAGS_PROD", r#"{"audio.enabled":true}"#);
    seal(&changed, RowVerdict::Fail { reason: "total wrong".into() });
    assert_eq!(check(&changed, now).reasons, ["FAILED rows: checkout"]);

    // A supervisor override names one commit and lasts 6 hours.
    let line = format!("RELEASE-GATE OVERRIDE sha={changed}: hotfix approved by Daniel");
    assert!(crate::mcp::tools::core::task::caller_note_marker_rejection(&line).is_some());
    assert!(
        crate::mcp::tools::core::task::caller_note_marker_rejection("RELEASE-GATE OVERRIDE sha=<full sha>: why").is_none()
    );
    let mut epic = cas_types::Task::new("cas-rel".into(), "Release".into());
    epic.task_type = cas_types::TaskType::Epic;
    tasks.add(&epic).unwrap();
    tasks.append_note("cas-rel", &format!("[2026-10-21 12:00] ✅ DECISION {line}")).unwrap();
    let granted = check(&changed, now + Duration::hours(1));
    assert_eq!(granted.state, PromotionState::Success);
    assert!(granted.description().starts_with("RELEASE-GATE OVERRIDE until 2026-10-21 18:00Z: hotfix"));
    assert!(tasks.get("cas-rel").unwrap().notes.contains(&format!("release gate overridden for {changed}")));
    assert_eq!(check(&changed_merged, now).state, PromotionState::Success, "tree-equal commit");
    assert_eq!(check(&unrelated, now).state, PromotionState::Failure, "another commit");
    let expired = now + Duration::hours(6) + Duration::seconds(1);
    assert_eq!(check(&changed, expired).state, PromotionState::Failure, "6h+1s");

    // Statuses through a fake `gh`: open PRs listed, statuses posted.
    let gh_dir = tempfile::tempdir().unwrap();
    let gh = gh_dir.path().join("gh");
    let log = gh_dir.path().join("gh.log");
    std::fs::write(
        &gh,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >>\"$GH_LOG\"\ncase \"$1 $2\" in\n  'pr list') printf '[{\"number\":7,\"headRefOid\":\"%s\"}]' \"$GH_HEAD\" ;;\n  'repo view') exit 1 ;;\nesac\n",
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o700)).unwrap();
    env.set(crate::qa_pass::github_gate::GH_BIN_ENV, &gh);
    env.set("GH_LOG", &log);
    env.set("GH_HEAD", &merged);
    project.git(&["remote", "add", "origin", "https://github.com/acme/app.git"]);
    let posts = |context: &str| -> Vec<String> {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains(&format!("context={context}")))
            .map(|line| {
                let state = line.split("state=").nth(1).unwrap().split(' ').next().unwrap();
                format!("{} {state}", line.split(' ').nth(3).unwrap())
            })
            .collect()
    };
    let statuses = format!("repos/acme/app/statuses/{merged}");
    assert_eq!(publish_freeze(&cas, root, &config, true), Ok(1));
    assert_eq!(publish_freeze(&cas, root, &config, false), Ok(1));
    assert_eq!(
        posts("cassy/release-freeze"),
        [format!("{statuses} pending"), format!("{statuses} success")]
    );
    assert!(std::fs::read_to_string(&log).unwrap().contains("pr list --repo acme/app --base staging"));
    assert_eq!(publish_promotions(&cas, root, &config), Ok(1));
    assert_eq!(posts("cassy/release-gate"), [format!("{statuses} success")]);
}
