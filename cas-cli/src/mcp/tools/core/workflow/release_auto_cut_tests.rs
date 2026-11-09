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
    std::fs::write(&file, format!("{path}\n")).unwrap();
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

/// The Nov 6 design: a green exact tip with fragments is cut by Cassy, inside
/// the cadence, never while red, paused or queued, and never without the
/// supervisor's notes, which it waits for with one wake.
#[cfg(unix)]
#[test]
fn green_train_with_fragments_auto_cuts_within_guards_and_waits_for_notes() {
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

    receipt(repo, &root, "PASS");
    poll(&none);
    assert!(!frozen(), "green, but no changelog fragments");
    run(repo, &["checkout", "-q", LANE]);
    commit(repo, "changelog.d/cas-x.md");
    run(repo, &["checkout", "-q", "main"]);
    receipt(repo, &root, "FAIL");
    poll(&none);
    assert!(!frozen(), "a red tip never cuts");
    receipt(repo, &root, "PASS");
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
    assert!(frozen(), "green tip with fragments freezes for its cut");
    assert_eq!(wakes().len(), 1, "a missing notes draft wakes the supervisor once");
    let draft = notes_draft(&root, "cas-tr").unwrap();
    assert!(wakes()[0].payload.contains(&draft.display().to_string()), "{}", wakes()[0].payload);
    assert!(!repo.join("release-ran").exists(), "no cut without notes");

    std::fs::write(&draft, "notes\n").unwrap();
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
