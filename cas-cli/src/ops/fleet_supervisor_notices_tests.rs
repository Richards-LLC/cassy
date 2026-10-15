use super::{FocusEpic, focus_epic};
use cas_types::{Agent, AgentRole, Task, TaskStatus, TaskType, WorkTarget};

#[test]
fn ct2_p3_integration_conflict_notifies_both() {
    let home = tempfile::TempDir::new().unwrap();
    let _env =
        crate::test_support::TestEnvGuard::with_vars(&[("HOME", home.path().to_str().unwrap())]);
    let temp = tempfile::TempDir::new().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let agents = crate::store::open_agent_store(&root).unwrap();
    let tasks = crate::store::open_task_store(&root).unwrap();
    for (id, name, session, epic) in [
        ("owner-a", "alpha", "session-a", "cas-lane-a"),
        ("owner-b", "beta", "session-b", "cas-lane-b"),
    ] {
        let mut agent = Agent::new_with_role(id.into(), name.into(), AgentRole::Supervisor);
        agent.factory_session = Some(session.into());
        agents.register(&agent).unwrap();
        let metadata = crate::ui::factory::metadata_path(session);
        std::fs::create_dir_all(metadata.parent().unwrap()).unwrap();
        let data = crate::ui::factory::create_metadata(
            session,
            12345,
            name,
            &[],
            None,
            temp.path().to_str(),
            None,
        );
        std::fs::write(metadata, serde_json::to_string(&data).unwrap()).unwrap();
        let mut task = Task::new(epic.into(), epic.into());
        task.task_type = TaskType::Epic;
        task.status = TaskStatus::InProgress;
        task.branch = Some("epic/shared-integration".into());
        task.epic_verification_owner = Some(id.into());
        task.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:fixture".into(),
            target_branch: "main".into(),
        });
        tasks.add(&task).unwrap();
    }
    focus_epic(
        &root,
        "session-a",
        FocusEpic::Pin {
            epic_id: "cas-lane-a",
            delivery_mode: None,
        },
    )
    .unwrap();
    let queue = crate::store::open_prompt_queue_store(&root).unwrap();
    let rows = queue.peek_all(100).unwrap();
    assert_eq!(
        rows.len(),
        2,
        "overlapping integration lanes must notify both owners"
    );
    for (name, session) in [("alpha", "session-a"), ("beta", "session-b")] {
        let row = rows
            .iter()
            .find(|r| r.target == name)
            .expect("both owners receive a notice");
        assert_eq!(row.factory_session.as_deref(), Some(session));
        assert!(
            row.prompt.contains("cas-lane-a")
                && row.prompt.contains("cas-lane-b")
                && row
                    .prompt
                    .contains(tasks.get("cas-lane-a").unwrap().branch.as_deref().unwrap())
        );
    }
    focus_epic(
        &root,
        "session-a",
        FocusEpic::Pin {
            epic_id: "cas-lane-a",
            delivery_mode: None,
        },
    )
    .unwrap();
    assert_eq!(
        queue.peek_all(100).unwrap().len(),
        2,
        "a repeated focus does not repeat the conflict"
    );
}
