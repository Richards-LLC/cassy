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
        no_delivery: None,
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
        agent("id-dead", "old-sup", Shutdown, 7_300),
        agent("id-crashed", "crashed-sup", Active, 3 * 3_600),
        agent("id-idle", "idle-sup", Active, 2 * 3_600),
        agent("id-nopid", "nopid-sup", Stale, 5 * 3_600),
        agent("id-live", "live-sup", Active, 30),
    ];
    // The process probe stands in for the daemon's liveness gate.
    let probe = |agent: &cas_types::Agent| match agent.name.as_str() {
        "crashed-sup" | "old-sup" => OwnerProcess::Dead,
        "nopid-sup" => OwnerProcess::Unknown,
        _ => OwnerProcess::Alive,
    };
    // By id, name and session: shut down, or its process is gone.
    let why = dead_owner_adoption("id-dead", &agents, now, probe).expect("shut down owner");
    assert!(why.contains("old-sup") && why.contains("shut down") && why.contains("2h"), "{why}");
    let why = dead_owner_adoption("crashed-sup", &agents, now, probe).expect("dead process");
    assert!(why.contains("harness process gone"), "{why}");
    assert!(dead_owner_adoption("old-sup-session", &agents, now, probe).is_some());
    // Age is never evidence: an owner idle for 2h with a live process, or
    // stale with no process recorded, keeps its epic.
    assert_eq!(dead_owner_adoption("idle-sup", &agents, now, probe), None);
    assert_eq!(dead_owner_adoption("nopid-sup", &agents, now, probe), None);
    // Live, unknown and empty owners are never adopted.
    assert_eq!(dead_owner_adoption("live-sup", &agents, now, probe), None);
    assert_eq!(dead_owner_adoption("someone-elsewhere", &agents, now, probe), None);
    assert_eq!(dead_owner_adoption("  ", &agents, now, probe), None);
    // Any live registration under the owner's name keeps the owner.
    let mut both = agents.clone();
    both.push(agent("id-dead-2", "old-sup", Active, 5));
    let probe_second_live = |agent: &cas_types::Agent| {
        if agent.id == "id-dead-2" { OwnerProcess::Alive } else { probe(agent) }
    };
    assert_eq!(dead_owner_adoption("old-sup", &both, now, probe_second_live), None);
}

/// The production probe reads the real process table: an idle owner whose
/// registered harness is this very test process is alive, however old its
/// heartbeat; one with no recorded process is unknown.
#[test]
fn cas_7f60_idle_owner_with_live_process_is_not_adopted() {
    let pid = std::process::id();
    let mut idle = agent("id-idle", "idle-sup", cas_types::AgentStatus::Active, 6 * 3_600);
    idle.pid = Some(pid);
    crate::mcp::daemon::stamp_pid_fingerprint(&mut idle, pid);
    assert_eq!(owner_process(&idle), OwnerProcess::Alive);
    assert_eq!(dead_owner_adoption("idle-sup", &[idle], Utc::now(), owner_process), None);
    let nopid = agent("id-nopid", "nopid-sup", cas_types::AgentStatus::Active, 6 * 3_600);
    assert_eq!(owner_process(&nopid), OwnerProcess::Unknown);
    assert_eq!(dead_owner_adoption("nopid-sup", &[nopid], Utc::now(), owner_process), None);
}

struct HandlerClose {
    is_error: bool,
    text: String,
    task: Task,
}

/// The 2026-10-16 cleanup shape through the real handler: an epic owned by a
/// dead supervisor session, whose branch `epic/gone` exists neither locally
/// nor on origin, with one terminal child whose lane is deleted and whose
/// recorded delivery is `child_on_main` ? on origin/main : nowhere on main.
/// The live supervisor closes it with no receipt, no owner transfer and no
/// override.
#[cfg(unix)]
async fn close_finished_epic(child_on_main: bool) -> HandlerClose {
    use crate::mcp::CasService;
    use crate::store::{open_agent_store, open_rule_store, open_skill_store, open_store, open_task_store};
    use cas_types::{Agent, AgentRole, AgentStatus, Dependency, DependencyType};

    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let bare = tempfile::tempdir().unwrap();
    git(bare.path(), &["init", "-q", "--bare"]);
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    commit_file(p, "seed.txt");
    git(p, &["remote", "add", "origin", bare.path().to_str().unwrap()]);
    git(p, &["push", "-q", "origin", "main"]);
    git(p, &["checkout", "-q", "-b", "factory/alpha"]);
    commit_file(p, "delivery.rs");
    let anchor = {
        let out = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(p).output().unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    git(p, &["checkout", "-q", "main"]);
    if child_on_main {
        git(p, &["merge", "-q", "--no-ff", "-m", "ship alpha", "factory/alpha"]);
        git(p, &["push", "-q", "origin", "main"]);
    }
    // The lane is gone locally and was never on origin; the anchor object
    // stays in the repository, as it does after a squash or merge.
    git(p, &["branch", "-D", "factory/alpha"]);
    if !child_on_main {
        // Keep the unlanded commit reachable so it is measurable.
        git(p, &["tag", "unlanded-alpha", &anchor]);
    }

    let cas_dir = p.join(".cas");
    std::fs::create_dir_all(&cas_dir).unwrap();
    env.set("CAS_ROOT", &cas_dir);
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    std::fs::write(cas_dir.join("config.toml"), "[verification]\nenabled = false\n").unwrap();
    open_store(&cas_dir).unwrap().init().unwrap();
    open_rule_store(&cas_dir).unwrap().init().unwrap();
    open_skill_store(&cas_dir).unwrap().init().unwrap();
    let store = open_task_store(&cas_dir).unwrap();
    store.init().unwrap();
    let agents = open_agent_store(&cas_dir).unwrap();
    agents.init().unwrap();
    let mut dead = Agent::new_with_role("old-sup-id".into(), "old-sup".into(), AgentRole::Supervisor);
    dead.status = AgentStatus::Shutdown;
    dead.last_heartbeat = Utc::now() - chrono::Duration::hours(3);
    agents.register(&dead).unwrap();
    let actor = "cas-7f60-supervisor";
    agents
        .register(&Agent::new_with_role(actor.into(), "live-sup".into(), AgentRole::Supervisor))
        .unwrap();
    let core = CasCore::with_daemon(cas_dir.clone(), None, None);
    core.set_agent_id_for_testing(actor.into());
    let service = CasService::new(core, None);

    let epic = Task {
        id: "cas-7f60-epic".to_string(),
        title: "finished epic".to_string(),
        task_type: TaskType::Epic,
        status: TaskStatus::Open,
        branch: Some("epic/gone".to_string()),
        epic_verification_owner: Some("old-sup-id".to_string()),
        risk: vec![TaskRisk::None],
        ..Default::default()
    };
    store.add(&epic).unwrap();
    let mut child = Task {
        id: "cas-7f60-child".to_string(),
        title: "shipped child".to_string(),
        status: TaskStatus::Closed,
        assignee: Some("alpha".to_string()),
        ..Default::default()
    };
    child.deliverables.factory_branch_anchor = Some(anchor);
    child.deliverables.parked_branch = Some("factory/alpha".to_string());
    store.add(&child).unwrap();
    store
        .add_dependency(&Dependency::new(child.id.clone(), epic.id.clone(), DependencyType::ParentChild))
        .unwrap();

    let request = serde_json::from_value(serde_json::json!({
        "action": "close",
        "id": epic.id,
        "reason": "all children terminal; content shipped to main",
    }))
    .unwrap();
    let (is_error, text) = match service.task(Parameters(request)).await {
        Ok(response) => (
            response.is_error == Some(true),
            response
                .content
                .iter()
                .filter_map(|content| match &content.raw {
                    rmcp::model::RawContent::Text(text) => Some(text.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        Err(error) => (true, error.message.to_string()),
    };
    HandlerClose {
        is_error,
        text,
        task: store.get(&epic.id).unwrap(),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn cas_7f60_finished_epic_closes_with_dead_owner_and_gone_branch() {
    let run = close_finished_epic(true).await;
    assert!(!run.is_error, "{}", run.text);
    assert_eq!(run.task.status, TaskStatus::Closed, "{}", run.text);
    assert_eq!(
        run.task.epic_verification_owner.as_deref(),
        Some("cas-7f60-supervisor"),
        "the live supervisor adopted the epic"
    );
    // pin: the adoption is logged with the dead owner's evidence.
    assert!(run.task.notes.contains("adopted") && run.task.notes.contains("old-sup"), "{}", run.task.notes);
    // pin: the gone epic branch is judged on the default branch, on the record.
    assert!(run.task.notes.contains("epic/gone"), "{}", run.task.notes);
}

#[cfg(unix)]
#[tokio::test]
async fn cas_7f60_gone_branch_with_unlanded_child_is_still_refused() {
    let run = close_finished_epic(false).await;
    assert_ne!(run.task.status, TaskStatus::Closed, "{}", run.text);
    assert!(run.text.contains("cas-7f60-child"), "{}", run.text);
}

/// cas-04c6: a gone epic branch with nothing recorded to prove stays
/// fail-closed; only anchored, terminal children make it measurable.
#[test]
fn cas_7f60_gone_branch_needs_anchored_terminal_children() {
    let anchored = |id: &str| {
        let mut task = child(id, "lane");
        task.deliverables.factory_branch_anchor = Some("a".repeat(40));
        task
    };
    assert!(!children_prove_delivery(&[]), "no children: nothing to prove");
    assert!(children_prove_delivery(&[anchored("cas-a")]));
    let mut open = anchored("cas-open");
    open.status = TaskStatus::Open;
    assert!(!children_prove_delivery(&[anchored("cas-a"), open]), "open child");
    let mut unanchored_code = child("cas-code", "lane");
    unanchored_code.deliverables.files_changed = vec!["child.rs".into()];
    assert!(
        !children_prove_delivery(&[anchored("cas-a"), unanchored_code]),
        "code the gate cannot measure"
    );
    let ops_only = child("cas-ops", "lane");
    assert!(!children_prove_delivery(&[ops_only.clone()]), "no anchor at all");
    assert!(children_prove_delivery(&[anchored("cas-a"), ops_only]));
}
