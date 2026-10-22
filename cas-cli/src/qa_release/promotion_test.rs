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
    tasks.append_trusted_note("cas-rel", &format!("[2026-10-21 12:00] ✅ DECISION {line}")).unwrap();
    let granted = check(&changed, now + Duration::hours(1));
    assert_eq!(granted.state, PromotionState::Success);
    assert!(granted.description().starts_with("RELEASE-GATE OVERRIDE until 2026-10-21 18:00Z: hotfix"));
    assert!(tasks.get("cas-rel").unwrap().notes.contains(&format!("release gate overridden for {changed}")));
    assert_eq!(check(&changed_merged, now).state, PromotionState::Success, "tree-equal commit");
    assert_eq!(check(&unrelated, now).state, PromotionState::Failure, "another commit");
    let expired = now + Duration::hours(6) + Duration::seconds(1);
    assert_eq!(check(&changed, expired).state, PromotionState::Failure, "6h+1s");

    // Statuses through a fake `gh`: every page of open PRs is read, one failed
    // post does not stop the rest, and the freeze posts only what changed.
    let gh_dir = tempfile::tempdir().unwrap();
    let gh = gh_dir.path().join("gh");
    let log = gh_dir.path().join("gh.log");
    std::fs::write(
        &gh,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >>\"$GH_LOG\"\ncase \"$*\" in\n  'api --paginate repos/acme/app/pulls?state=open&base='*) printf '%s' \"$GH_PULLS\" ;;\n  *\"statuses/$GH_FAIL \"*) exit 1 ;;\n  'repo view'*) exit 1 ;;\nesac\n",
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o700)).unwrap();
    env.set(crate::qa_pass::github_gate::GH_BIN_ENV, &gh);
    env.set("GH_LOG", &log);
    env.set("GH_FAIL", "none");
    let pr = |number: u32, sha: &str| format!(r#"[{{"number":{number},"head":{{"sha":"{sha}"}}}}]"#);
    env.set("GH_PULLS", format!("{}\n{}", pr(7, &merged), pr(9, &walked)));
    project.git(&["remote", "add", "origin", "https://github.com/acme/app.git"]);
    let posts = |context: &str| -> Vec<String> {
        let posted: Vec<String> = std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains(&format!("context={context}")))
            .map(|line| {
                let state = line.split("state=").nth(1).unwrap().split(' ').next().unwrap();
                let sha = line.split("statuses/").nth(1).unwrap().split(' ').next().unwrap();
                format!("{} {state}", &sha[..7])
            })
            .collect();
        std::fs::remove_file(&log).ok();
        posted
    };
    let (m, w) = (&merged[..7], &walked[..7]);
    assert_eq!(publish_freeze(&cas, root, &config, true), Ok(2));
    assert_eq!(posts("cassy/release-freeze"), [format!("{m} pending"), format!("{w} pending")]);
    assert_eq!(publish_freeze(&cas, root, &config, true), Ok(0), "unchanged heads are not reposted");
    env.set("GH_FAIL", &walked);
    let failed = publish_freeze(&cas, root, &config, false).unwrap_err();
    assert!(failed.starts_with("1 published, 1 failed: PR #9"), "{failed}");
    env.set("GH_FAIL", "none");
    assert_eq!(publish_freeze(&cas, root, &config, false), Ok(1), "the failed head is retried");
    let released = [format!("{m} success"), format!("{w} success"), format!("{w} success")];
    assert_eq!(posts("cassy/release-freeze"), released, "the failed attempt, then its retry");
    assert_eq!(publish_promotions(&cas, root, &config), Ok(2));
    assert_eq!(posts("cassy/release-gate"), [format!("{m} success"), format!("{w} success")]);
}

/// Only a live registered supervisor's decision note records an override.
/// The same line from a worker's note, a task update (the pre-compact flush
/// path), a sync merge or a plain append grants nothing.
#[tokio::test]
async fn promotion_override_authority() {
    use super::promotion::{PromotionState, evaluate};
    use cas_types::{Agent, AgentRole};

    async fn note_as(cas: &Path, id: &str, role: AgentRole, note: String) -> bool {
        use crate::mcp::tools::TaskNotesRequest;
        use rmcp::handler::server::wrapper::Parameters;
        let agent = Agent::new_with_role(id.into(), id.into(), role);
        crate::store::open_agent_store(cas).unwrap().register(&agent).unwrap();
        let core = crate::mcp::CasCore::with_daemon(cas.to_path_buf(), None, None);
        core.set_agent_id_for_testing(agent.id);
        let request = TaskNotesRequest {
            id: "cas-rel".into(),
            note,
            note_type: "decision".into(),
            supervisor_override: None,
            reason: None,
        };
        core.cas_task_notes(Parameters(request)).await.is_ok()
    }

    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let project = Project::new();
    let root = project.root();
    let forged = project.commit("app.ts", "forged");
    let granted = project.commit("app.ts", "granted");
    let base = tempfile::tempdir().unwrap();
    let cas = base.path().join(".cas");
    std::fs::create_dir_all(&cas).unwrap();
    env.set("CAS_ROOT", &cas);
    let mut settings = crate::config::Config::default();
    settings.set("factory.artifacts_root", base.path().join("artifacts").to_str().unwrap()).unwrap();
    settings.save(&cas).unwrap();
    let tasks = crate::store::open_task_store(&cas).unwrap();
    tasks.init().unwrap();
    crate::store::open_agent_store(&cas).unwrap().init().unwrap();
    tasks.add(&cas_types::Task::new("cas-rel".into(), "Release".into())).unwrap();
    let note = |sha: &str| format!("RELEASE-GATE OVERRIDE sha={sha}: ship it");
    let line = |sha: &str| format!("[{}] ✅ DECISION {}", chrono::Utc::now().format("%Y-%m-%d %H:%M"), note(sha));

    assert!(!note_as(&cas, "worker-1", AgentRole::Standard, note(&granted)).await);
    let mut flushed = tasks.get("cas-rel").unwrap();
    flushed.notes.push_str(&format!("\n{}", line(&forged)));
    tasks.update(&flushed).unwrap();
    let expected = tasks.get("cas-rel").unwrap();
    let mut synced = expected.clone();
    synced.notes.push_str(&format!("\n{}", line(&forged)));
    tasks.update_from_sync(&synced, &expected).unwrap();
    tasks.append_note("cas-rel", &line(&forged)).unwrap();
    assert!(note_as(&cas, "supervisor-1", AgentRole::Supervisor, note(&granted)).await);

    let config = fake_adapter(root, "");
    let check = |sha: &str| evaluate(root, &cas, &config, sha, chrono::Utc::now());
    let passed = check(&granted);
    assert_eq!((passed.state, passed.overridden.is_some()), (PromotionState::Success, true));
    assert_eq!(check(&forged).state, PromotionState::Pending, "{:?}", tasks.get("cas-rel").unwrap().notes);
}
