use super::super::*;
use super::*;
use std::process::Command;

fn git(dir: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?} failed");
}

fn commit_file(dir: &std::path::Path, name: &str) {
    std::fs::write(dir.join(name), format!("// {name}\n")).unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-q", "-m", &format!("feat: {name}")]);
}

/// `main` and an old `epic/old` cut from it. `factory/shipped` reached main
/// by merge, `factory/picked` by cherry-pick (new SHAs), and
/// `factory/unlanded` reached neither. None of them reached `epic/old`.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    commit_file(p, "seed.txt");
    git(p, &["branch", "epic/old"]);
    for lane in ["shipped", "picked", "unlanded"] {
        git(p, &["checkout", "-q", "-b", &format!("factory/{lane}"), "main"]);
        commit_file(p, &format!("{lane}-1.rs"));
        commit_file(p, &format!("{lane}-2.rs"));
    }
    git(p, &["checkout", "-q", "main"]);
    git(p, &["merge", "-q", "--no-ff", "-m", "merge shipped", "factory/shipped"]);
    git(p, &["cherry-pick", "factory/picked~1", "factory/picked"]);
    dir
}

fn child(id: &str, lane: &str) -> Task {
    let mut task = Task {
        id: id.to_string(),
        title: format!("child {id}"),
        status: TaskStatus::Closed,
        assignee: Some(lane.to_string()),
        ..Default::default()
    };
    task.deliverables.parked_branch = Some(format!("factory/{lane}"));
    task
}

fn epic() -> Task {
    Task {
        id: "cas-epic".into(),
        title: "epic".into(),
        status: TaskStatus::InProgress,
        task_type: TaskType::Epic,
        ..Default::default()
    }
}

fn req() -> TaskCloseRequest {
    TaskCloseRequest {
        stranded_branch_override: None,
        id: "cas-epic".into(),
        reason: None,
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    }
}

#[test]
fn cas_7f60_child_branch_already_on_main_is_not_stranded() {
    let dir = repo();
    let outcome = run_epic_close_merge_gate(
        &epic(),
        &req(),
        "epic/old",
        dir.path(),
        &[child("cas-c1", "shipped")],
    );
    match outcome {
        EpicCloseGateOutcome::ProceedWithNote(note) => {
            assert!(note.contains("cas-c1") && note.contains("main"), "{note}");
        }
        other => panic!("a lane already on main must not block: {other:?}"),
    }
}

#[test]
fn cas_7f60_cherry_picked_child_branch_is_not_stranded() {
    let dir = repo();
    let outcome = run_epic_close_merge_gate(
        &epic(),
        &req(),
        "epic/old",
        dir.path(),
        &[child("cas-c2", "picked")],
    );
    match outcome {
        EpicCloseGateOutcome::ProceedWithNote(note) => {
            assert!(note.contains("cas-c2") && note.contains("patch"), "{note}");
        }
        other => panic!("patch-equivalent content on main must not block: {other:?}"),
    }
}

#[test]
fn cas_7f60_unlanded_child_branch_is_still_refused() {
    let dir = repo();
    let outcome = run_epic_close_merge_gate(
        &epic(),
        &req(),
        "epic/old",
        dir.path(),
        &[child("cas-c1", "shipped"), child("cas-c3", "unlanded")],
    );
    match outcome {
        EpicCloseGateOutcome::Reject(msg) => {
            assert!(msg.contains("cas-c3"), "{msg}");
            assert!(!msg.contains("cas-c1 ("), "the shipped lane is not listed: {msg}");
        }
        other => panic!("real unlanded content must still block: {other:?}"),
    }
}

#[test]
fn cas_7f60_containment_measures_ancestry_then_patches() {
    let dir = repo();
    let p = dir.path();
    assert_eq!(
        landed_on_default_branch(p, "main", "factory/shipped"),
        Ok(DefaultBranchContainment::Ancestor)
    );
    assert_eq!(
        landed_on_default_branch(p, "main", "factory/picked"),
        Ok(DefaultBranchContainment::PatchEquivalent { commits: 2 })
    );
    assert_eq!(
        landed_on_default_branch(p, "main", "factory/unlanded"),
        Ok(DefaultBranchContainment::Unlanded { patches: 2 })
    );
    assert!(landed_on_default_branch(p, "main", "factory/missing").is_err());
    assert_eq!(default_branch_ref(p).as_deref(), Some("main"));
}

fn agent(id: &str, name: &str, status: cas_types::AgentStatus, silent_secs: i64) -> cas_types::Agent {
    let mut agent = cas_types::Agent::new(id.into(), name.into());
    agent.role = cas_types::AgentRole::Supervisor;
    agent.status = status;
    agent.factory_session = Some(format!("{name}-session"));
    agent.last_heartbeat = Utc::now() - chrono::Duration::seconds(silent_secs);
    agent
}

#[test]
fn cas_7f60_dead_owner_is_adoptable_with_evidence() {
    use cas_types::AgentStatus::*;
    let now = Utc::now();
    let agents = vec![
        agent("id-dead", "old-sup", Shutdown, 7_200),
        agent("id-crashed", "crashed-sup", Active, 3 * 3_600),
        agent("id-live", "live-sup", Active, 30),
        agent("id-slow", "slow-sup", Active, 600),
    ];
    // By id, name and session; shutdown or long-silent rows are dead.
    let why = dead_owner_adoption("id-dead", &agents, now).expect("shutdown owner");
    assert!(why.contains("old-sup") && why.contains("shutdown") && why.contains("2h"), "{why}");
    assert!(dead_owner_adoption("crashed-sup", &agents, now).is_some());
    assert!(dead_owner_adoption("old-sup-session", &agents, now).is_some());
    // Live, briefly silent, unknown and empty owners are never adopted.
    assert_eq!(dead_owner_adoption("live-sup", &agents, now), None);
    assert_eq!(dead_owner_adoption("slow-sup", &agents, now), None);
    assert_eq!(dead_owner_adoption("someone-elsewhere", &agents, now), None);
    assert_eq!(dead_owner_adoption("  ", &agents, now), None);
    // Any live registration under the owner's name keeps the owner.
    let mut both = agents.clone();
    both.push(agent("id-dead-2", "old-sup", Active, 5));
    assert_eq!(dead_owner_adoption("old-sup", &both, now), None);
}
