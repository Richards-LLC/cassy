//! Epic-scoped staffing pauses for repeated stakeholder defect reports.

#[cfg(test)]
mod tests {
    use crate::mcp::CasCore;
    use crate::mcp::tools::TaskStartRequest;
    use cas_types::{Agent, Dependency, DependencyType, Priority, Task, TaskStatus, TaskType};
    use chrono::{Duration, Utc};
    use rmcp::handler::server::wrapper::Parameters;

    #[tokio::test]
    async fn nth_stakeholder_bug_pauses_only_its_epic() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(project.path()).unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        let tasks = core.open_task_store().unwrap();
        tasks.init().unwrap();
        core.open_agent_store().unwrap().register(&Agent::new("review-caller".into(), "caller".into())).unwrap();
        core.set_agent_id_for_testing("review-caller".into());
        for id in ["epic-risk", "epic-other"] {
            let mut epic = Task::new(id.into(), id.into());
            epic.task_type = TaskType::Epic;
            tasks.add(&epic).unwrap();
        }
        let add_child = |id: &str, epic: &str, hours: i64, priority: i32, stakeholder: bool| {
            let mut task = Task::new(id.into(), id.into());
            task.task_type = TaskType::Bug;
            task.priority = Priority(priority);
            task.created_at = Utc::now() - Duration::hours(hours);
            if stakeholder { task.labels.push("stakeholder-reported".into()); }
            tasks.add(&task).unwrap();
            tasks.add_dependency(&Dependency::new(id.into(), epic.into(), DependencyType::ParentChild)).unwrap();
        };
        for id in ["report-1", "report-2", "report-3", "report-4"] {
            add_child(id, "epic-risk", 1, 0, true);
        }
        // Neither an expired report, a P2 nor an internal finding is the fifth report.
        add_child("expired", "epic-risk", 49, 0, true);
        add_child("minor", "epic-risk", 0, 2, true);
        add_child("internal", "epic-risk", 0, 0, false);
        add_child("first-start", "epic-risk", 0, 2, false);
        let start = |id: &str| TaskStartRequest { id: id.into(), brief: Some(true), force: None };
        core.cas_task_start_with_options(Parameters(start("first-start"))).await.unwrap();
        add_child("report-5", "epic-risk", 0, 1, true);
        add_child("next-start", "epic-risk", 0, 2, false);
        let refusal = core.cas_task_start_with_options(Parameters(start("next-start"))).await
            .expect_err("the fifth stakeholder P0/P1 must stop new starts");
        assert!(refusal.message.contains("design review") && refusal.message.contains("epic-risk")
            && refusal.message.contains("report-5"), "{refusal}");
        assert_eq!(tasks.get("next-start").unwrap().status, TaskStatus::Open);
        add_child("unrelated", "epic-other", 0, 2, false);
        core.cas_task_start_with_options(Parameters(start("unrelated"))).await.unwrap();
    }
}
