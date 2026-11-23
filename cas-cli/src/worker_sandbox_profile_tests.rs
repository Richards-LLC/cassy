use super::*;
use crate::test_env_guard::TestEnvGuard;
use cas_types::{AgentRole, Task};
use std::path::PathBuf;

fn paths(sandbox: &serde_json::Value, key: &str) -> Vec<PathBuf> {
    sandbox["filesystem"][key]
        .as_array()
        .unwrap()
        .iter()
        .map(|path| PathBuf::from(path.as_str().unwrap()))
        .collect()
}

/// The same rule Seatbelt applies: under an allowed path and no denied one.
fn writable(sandbox: &serde_json::Value, path: &Path) -> bool {
    paths(sandbox, "allowWrite").iter().any(|allow| path.starts_with(allow))
        && !paths(sandbox, "denyWrite").iter().any(|deny| path.starts_with(deny))
}

fn running(tasks: &dyn cas_store::TaskStore, id: &str, assignee: &str) {
    let mut task = Task::new(id.into(), id.into());
    task.status = TaskStatus::InProgress;
    task.assignee = Some(assignee.into());
    tasks.add(&task).unwrap();
}

/// cas-f157 (plan art-d47c67a6 §2): verifier vdispatch-3941b5cb found the
/// profile granting the project's whole artifact namespace and the staging
/// scratch root. The profile a worker launches with now writes only its own
/// task's artifact dir and its checkout's temporary dir; after the worker's
/// assignment moves, the relaunch profile writes the new task's dir and no
/// longer the old one.
#[test]
fn sandbox_profile_grants_only_the_current_task_and_checkout_scratch() {
    let env = TestEnvGuard::temp_home();
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let root = crate::store::init_cas_dir(&base.join("repo")).unwrap();
    let artifacts_base = base.join("artifacts");
    std::fs::write(
        root.join("config.toml"),
        format!("[factory]\nartifacts_root = \"{}\"\n", artifacts_base.display()),
    )
    .unwrap();
    let artifacts =
        crate::config::resolved_factory_artifact_paths(&root, Some(artifacts_base.to_str().unwrap())).project_root;
    std::fs::create_dir_all(&artifacts).unwrap();
    let checkout = base.join("repo/.cas/worktrees/w");
    let other_checkout = base.join("repo/.cas/worktrees/v");
    std::fs::create_dir_all(&checkout).unwrap();
    let tasks = crate::store::open_task_store(&root).unwrap();
    running(tasks.as_ref(), "cas-own", "w");
    running(tasks.as_ref(), "cas-sibling", "v");

    let profile = |worker: &str| {
        let (mut sandbox, launch) = profile_for(&root, Some("s"), worker).expect("macOS profile");
        let launch: LaunchInputs = serde_json::from_value(launch).unwrap();
        cas_mux::worker_sandbox::apply_worktree(&mut sandbox, &checkout, None, &launch.checkout_scratch(&checkout));
        (sandbox, launch)
    };
    let tmp_root = canonical_spelling(&crate::factory_scratch::test_tmp_root(&root));
    let (sandbox, launch) = profile("w");
    assert_eq!(launch.tasks.as_deref(), Some(&["cas-own".to_string()][..]));
    for (path, expected) in [
        (artifacts.join("cas-own/EVIDENCE.md"), true),
        (artifacts.join("cas-sibling/EVIDENCE.md"), false),
        (artifacts.join("stray.md"), false),
        (canonical_spelling(&crate::factory_scratch::test_tmp_dir(&root, &checkout)).join("f"), true),
        (canonical_spelling(&crate::factory_scratch::test_tmp_dir(&root, &other_checkout)).join("f"), false),
        (tmp_root.join("f"), false),
    ] {
        assert_eq!(writable(&sandbox, &path), expected, "{}", path.display());
    }

    // Reassignment: the own task parks, the sibling moves to this worker.
    let mut own = tasks.get("cas-own").unwrap();
    own.status = TaskStatus::AwaitingMerge;
    tasks.update(&own).unwrap();
    let mut moved = tasks.get("cas-sibling").unwrap();
    moved.assignee = Some("w".into());
    tasks.update(&moved).unwrap();
    let (sandbox, launch) = profile("w");
    assert_eq!(launch.tasks.as_deref(), Some(&["cas-sibling".to_string()][..]));
    assert!(writable(&sandbox, &artifacts.join("cas-sibling/EVIDENCE.md")));
    assert!(!writable(&sandbox, &artifacts.join("cas-own/EVIDENCE.md")));
    drop(env);
}

/// A start the launched profile does not grant queues exactly one recycle of
/// that worker; a start it grants, or an unsandboxed worker's, queues none.
#[test]
fn starting_an_ungranted_task_relaunches_the_sandboxed_worker() {
    let mut env = TestEnvGuard::temp_home();
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let mut worker = Agent::new_with_role("w-id".into(), "w".into(), AgentRole::Worker);
    worker.factory_session = Some("s".into());
    let recycles = || {
        crate::store::open_spawn_queue_store(&root)
            .unwrap()
            .peek(100)
            .unwrap()
            .into_iter()
            .filter(|request| request.action == cas_store::SpawnAction::Recycle)
            .count()
    };

    record_launched_tasks(&mut worker);
    assert!(!relaunch_for_started_task(&root, &worker, "cas-b").unwrap(), "unsandboxed");

    env.set(cas_mux::worker_sandbox::SANDBOX_ENV, cas_mux::worker_sandbox::SHADOW);
    env.set(cas_mux::worker_sandbox::TASKS_ENV, "cas-a");
    record_launched_tasks(&mut worker);
    assert!(!relaunch_for_started_task(&root, &worker, "cas-a").unwrap(), "granted");
    assert_eq!(recycles(), 0);
    assert!(relaunch_for_started_task(&root, &worker, "cas-b").unwrap());
    assert!(relaunch_for_started_task(&root, &worker, "cas-b").unwrap());
    assert_eq!(recycles(), 1);
}
