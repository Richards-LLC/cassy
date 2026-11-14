use super::*;
use crate::store::{open_agent_store, open_supervisor_queue_store, open_task_store};
use crate::test_support::TestEnvGuard;
use crate::train_guardrails::notify_train_cuts;
use cas_types::{Agent, AgentRole, TaskStatus, TaskType};
use std::collections::BTreeSet;

const LANE: &str = "epic/release-train-vnext-cas-tr";

fn run(repo: &Path, args: &[&str]) -> String {
    git(repo, &[&["-c", "user.email=t@cas.test", "-c", "user.name=T"][..], args].concat()).unwrap()
}

fn commit(repo: &Path, path: &str) {
    let file = repo.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    if !file.exists() {
        std::fs::write(&file, format!("{path}\n")).unwrap();
    }
    run(repo, &["add", path]);
    run(repo, &["commit", "-qm", path]);
}

fn train_task(id: &str, labels: &[String]) -> Task {
    let mut train = Task::new(id.into(), "Release train vNEXT".into());
    train.task_type = TaskType::Epic;
    train.labels = [vec!["release-train".to_string()], labels.to_vec()].concat();
    train.branch = Some(format!("epic/release-train-vnext-{id}"));
    train.epic_verification_owner = Some("sup-id".into());
    train
}

/// The receipt `train_ci::observe` writes for the lane's current tip.
fn receipt(repo: &Path, root: &Path, outcome: &str) {
    let dir = crate::train_ci::directory(root, "cas-tr").unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    let state = serde_json::json!({"branch": LANE, "head": run(repo, &["rev-parse", LANE]), "outcome": outcome});
    std::fs::write(dir.join("state.json"), state.to_string()).unwrap();
}

/// The Nov 6 design: a train with fragments is cut by Cassy, inside the
/// cadence, never while paused or queued, and never without the supervisor's
/// notes, which it waits for with one wake. cas-0fc56: the freeze needs no
/// green, because it starts the one train suite; the cut waits for its green.
#[cfg(unix)]
#[test]
fn train_with_fragments_freezes_then_cuts_on_green_within_guards() {
    let _env = TestEnvGuard::temp_home();
    let temp = tempfile::TempDir::new().unwrap();
    let repo = temp.path();
    run(repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join(".gitignore"), ".cas/\nrelease-ran\n").unwrap();
    commit(repo, ".gitignore");
    commit(repo, "scripts/release.sh");
    std::fs::write(repo.join("scripts/release.sh"), "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$CAS_ROOT/../release-ran\"\n").unwrap();
    run(repo, &["update-index", "--chmod=+x", "scripts/release.sh"]);
    std::fs::set_permissions(repo.join("scripts/release.sh"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    run(repo, &["commit", "-qam", "release script"]);
    run(repo, &["checkout", "-qb", LANE]);
    commit(repo, "work.txt");
    run(repo, &["checkout", "-q", "main"]);
    let root = crate::store::init_cas_dir(repo).unwrap();
    let mut supervisor = Agent::new_with_role("sup-id".into(), "supervisor".into(), AgentRole::Supervisor);
    supervisor.factory_session = Some("session".into());
    open_agent_store(&root).unwrap().register(&supervisor).unwrap();
    let store = open_task_store(&root).unwrap();
    store.add(&train_task("cas-tr", &[])).unwrap();
    let now = Utc::now();
    let none = BTreeSet::new();
    let poll = |queued: &BTreeSet<String>| notify_train_cuts(&root, queued, now).unwrap();
    let frozen = || release_freeze::freeze_of(&open_task_store(&root).unwrap().get("cas-tr").unwrap()).is_some();
    let wakes = || open_supervisor_queue_store(&root).unwrap().peek("sup-id", 10).unwrap();

    poll(&none);
    assert!(!frozen(), "no changelog fragments");
    run(repo, &["checkout", "-q", LANE]);
    commit(repo, "changelog.d/cas-x.md");
    run(repo, &["checkout", "-q", "main"]);
    set_paused(&root, "cas-tr", "operator", true).unwrap();
    poll(&none);
    assert!(!frozen(), "paused in Commander");
    set_paused(&root, "cas-tr", "operator", false).unwrap();
    poll(&BTreeSet::from(["factory/other".to_string()]));
    assert!(!frozen(), "a queue run is in flight");

    // Cadence: an earlier train's freeze is the last cut.
    let old = |hours: i64| format!("release-train-frozen:{}:abc", (now - Duration::hours(hours)).timestamp());
    let mut earlier = train_task("cas-old", &[old(1)]);
    earlier.status = TaskStatus::Closed;
    store.add(&earlier).unwrap();
    poll(&none);
    assert!(!frozen(), "inside the 2 h minimum interval");
    earlier.labels = train_task("cas-old", &[old(3)]).labels;
    store.update(&earlier).unwrap();
    let mut config = crate::config::Config::load(&root).unwrap();
    config.set("release.auto_cut_max_per_day", "1").unwrap();
    config.save(&root).unwrap();
    poll(&none);
    assert!(!frozen(), "one cut in 24 h reaches a one-per-day cap");
    config.set("release.auto_cut_max_per_day", "4").unwrap();
    config.save(&root).unwrap();

    poll(&none);
    poll(&none);
    assert!(frozen(), "a ready tip with fragments freezes, starting its train suite");
    assert_eq!(wakes().len(), 1, "a missing notes draft wakes the supervisor once");
    let draft = notes_draft(&root, "cas-tr").unwrap();
    assert!(wakes()[0].payload.contains(&draft.display().to_string()), "{}", wakes()[0].payload);

    std::fs::create_dir_all(draft.parent().unwrap()).unwrap();
    std::fs::write(&draft, "notes\n").unwrap();
    poll(&none);
    receipt(repo, &root, "FAIL");
    poll(&none);
    assert!(!repo.join("release-ran").exists(), "no cut before green, nor on red");
    receipt(repo, &root, "PASS");
    poll(&none);
    let log = draft.with_file_name("auto-cut.log");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !std::fs::read_to_string(&log).unwrap_or_default().contains("RELEASE_EXIT=") {
        assert!(std::time::Instant::now() < deadline, "release.sh never finished");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(std::fs::read_to_string(&log).unwrap().contains("RELEASE_EXIT=0"));
    let ran = std::fs::read_to_string(repo.join("release-ran")).unwrap();
    assert_eq!(ran.trim(), format!("--notes {}", draft.display()));
    poll(&none);
    let notes = open_task_store(&root).unwrap().get("cas-tr").unwrap().notes;
    assert_eq!(notes.matches("AUTO-CUT: started").count(), 1, "one cut per tip");
    assert_eq!(wakes().len(), 1);
}

/// Nov 13: cas-8fb1 had only cas-5a22's fragment when the recorded scope
/// still awaited its other deliveries. Record scope through the real MCP
/// dependency operation, then drive the daemon poll against Git and SQLite.
#[tokio::test]
async fn must_land_scope_prevents_the_partial_train_freeze_cas_8695() {
    use cas_types::{Dependency, DependencyType, WorkTarget};
    use rmcp::handler::server::wrapper::Parameters;

    let mut env = TestEnvGuard::temp_home();
    let temp = tempfile::TempDir::new().unwrap();
    let repo = temp.path();
    let train_id = "cas-8fb1";
    let lane = "epic/release-train-vnext-cas-8fb1";
    run(repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
    commit(repo, ".gitignore");
    commit(repo, "scripts/release.sh");
    run(repo, &["checkout", "-qb", lane]);
    commit(repo, "changelog.d/cas-5a22.md");
    run(repo, &["checkout", "-q", "main"]);
    let root = crate::store::init_cas_dir(repo).unwrap();
    env.set("CAS_ROOT", &root);
    env.set("CAS_AGENT_ROLE", "supervisor");
    let mut supervisor = Agent::new_with_role("sup-id".into(), "supervisor".into(), AgentRole::Supervisor);
    supervisor.factory_session = Some("scope-session".into());
    open_agent_store(&root).unwrap().register(&supervisor).unwrap();
    let store = open_task_store(&root).unwrap();
    store.add(&train_task(train_id, &[])).unwrap();
    let core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
    core.set_agent_id_for_testing("sup-id".into());
    let service = crate::mcp::CasService::new(core, None);
    let required = ["cas-7dd6", "cas-5b88", "cas-c896", "cas-1efb", "cas-3126", "cas-cab9", "cas-870d", "cas-e571"];
    for id in required {
        let mut member = Task::new(id.into(), "Required delivery".into());
        member.status = TaskStatus::AwaitingMerge;
        member.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:fixture".into(), target_branch: lane.into(),
        });
        store.add(&member).unwrap();
        let added = service.task(Parameters(serde_json::from_value(serde_json::json!({
            "action": "dep_add", "id": train_id, "to_id": id, "dep_type": "blocks",
        })).unwrap())).await.unwrap();
        assert!(!added.is_error.unwrap_or(false));
    }
    assert_eq!(store.get_dependencies(train_id).unwrap().len(), required.len());
    assert_eq!(store.get(train_id).unwrap().task_type, TaskType::Epic);
    let mut optional = Task::new("cas-e074".into(), "Optional test sweep".into());
    optional.status = TaskStatus::InProgress;
    store.add(&optional).unwrap();
    store.add_dependency(&Dependency::new(optional.id, train_id.into(), DependencyType::ParentChild)).unwrap();

    let now = Utc::now();
    let poll = || notify_train_cuts(&root, &BTreeSet::new(), now).unwrap();
    let frozen = || release_freeze::freeze_of(&store.get(train_id).unwrap()).is_some();
    poll();
    assert!(!frozen(), "one landed fragment cannot freeze the declared unfinished scope");
    let outcome = attempt(&root, &store.get(train_id).unwrap(), lane,
        &run(repo, &["rev-parse", lane]), false, false, now).unwrap();
    let Outcome::Declined(reason) = outcome else { panic!("unfinished scope was accepted") };
    assert!(required.iter().all(|id| reason.contains(id)), "{reason}");

    let mut cancelled = store.get(required[0]).unwrap();
    cancelled.status = TaskStatus::Cancelled;
    store.update(&cancelled).unwrap();
    let mut retargeted = store.get(required[7]).unwrap();
    retargeted.deliverables.work_target.as_mut().unwrap().target_branch = "epic/release-train-next".into();
    store.update(&retargeted).unwrap();
    for (index, id) in required[1..7].iter().enumerate() {
        let branch = format!("factory/{id}");
        run(repo, &["checkout", "-qb", &branch, lane]);
        commit(repo, &format!("deliveries/{id}.txt"));
        let source = run(repo, &["rev-parse", "HEAD"]);
        run(repo, &["checkout", "-q", lane]);
        run(repo, &["merge", "--no-ff", "-qm", id, &branch]);
        let merged = run(repo, &["rev-parse", "HEAD"]);
        cas_store::record_observed_delivery_merge(&root, id, &branch, lane, &source, &merged, "sup-id").unwrap();
        let mut member = store.get(id).unwrap();
        member.status = TaskStatus::Closed;
        member.deliverables.factory_branch_anchor = Some(source);
        store.update(&member).unwrap();
        run(repo, &["checkout", "-q", "main"]);
        poll();
        assert_eq!(frozen(), index == 5, "freeze only after the last required delivery lands");
    }
    assert_eq!(store.get("cas-e074").unwrap().status, TaskStatus::InProgress);
    assert_eq!(store.get(required[7]).unwrap().status, TaskStatus::AwaitingMerge);
}
