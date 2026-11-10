use super::*;
use cas_types::{Agent, AgentRole, Task, TaskStatus};
use serde_json::json;

const SOURCE: &str = "https://example.slack.com/archives/C123/p1710000000000001";

struct World {
    _temp: tempfile::TempDir,
    root: std::path::PathBuf,
    now: DateTime<Utc>,
}

impl World {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        std::fs::write(root.join("config.toml"), "[project]\ncanonical_id = 'fixture'\n").unwrap();
        let mut supervisor = Agent::new("sup".into(), "supervisor".into());
        supervisor.role = AgentRole::Supervisor;
        supervisor.factory_session = Some("session".into());
        crate::store::open_agent_store(&root).unwrap().register(&supervisor).unwrap();
        let now = Utc::now();
        let journal = super::super::external_work::SqliteRequestJournal::open(&root).unwrap();
        journal.prepare("claim", "claim", &json!({"project_id":"fixture","external_ref":SOURCE})).unwrap();
        journal.record_result("claim", &json!({"code":"acquired","receipt":{
            "resource":{"id":"resource","state":"claimed"},
            "visible_owner":{"registration_id":"sup"},
            "expires_at":(now + chrono::Duration::hours(1)).to_rfc3339()
        }}), None).unwrap();
        let mut task = Task::new("cas-source".into(), "source work".into());
        task.external_ref = Some(SOURCE.into());
        crate::store::open_task_store(&root).unwrap().add(&task).unwrap();
        Self { _temp:temp, root, now }
    }

    fn status(&self, status: TaskStatus) {
        let store = crate::store::open_task_store(&self.root).unwrap();
        let mut task = store.get("cas-source").unwrap();
        task.status = status;
        store.update(&task).unwrap();
    }
}

#[test]
fn source_reaction_lifecycle_reconciles_source_posts() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let world = World::new();
    let mut observed = vec!["eyes".to_string(), "pick".to_string()];
    // A closed task without staging evidence must retain pick. A reopened
    // task reverses status; cancellation clears only the bot's status marks.
    for (step, status, expected) in [
        (0, TaskStatus::Open, Some("eyes")),
        (1, TaskStatus::InProgress, Some("pick")),
        (2, TaskStatus::AwaitingMerge, Some("pick")),
        (3, TaskStatus::Closed, Some("pick")),
        (4, TaskStatus::Open, Some("eyes")),
        (5, TaskStatus::Cancelled, None),
    ] {
        world.status(status);
        let mut calls = Vec::new();
        reconcile(&world.root, "sup", "session", world.now + chrono::Duration::minutes(step), &mut |tool, args| {
            calls.push((tool.to_string(), args.clone()));
            if tool == "violet_read" {
                return Ok(json!({"ok":true,"channel":{"id":"C123"},"complete":true,"messages":[{
                    "message_id":"1710000000.000001","permalink":SOURCE,
                    "reactions":observed.iter().map(|name| json!({"name":name,"count":1,"users":["BOT"]})).collect::<Vec<_>>()
                }]}));
            }
            assert_eq!(tool, "violet_post");
            assert_eq!(args["kind"], "reaction");
            assert_eq!(args["channel"], "C123");
            assert_eq!(args["message_id"], "1710000000.000001");
            let mark = args["reaction"].as_str().unwrap().to_string();
            if args["action"] == "remove" { observed.retain(|name| name != &mark); }
            else if !observed.contains(&mark) { observed.push(mark.clone()); }
            Ok(json!({"ok":true,"reaction":{"name":mark,"action":args["action"],"changed":true}}))
        }).unwrap();
        assert!(!calls.is_empty(), "step {step}: no lifecycle reconciliation");
        assert_eq!(observed, expected.into_iter().map(str::to_string).collect::<Vec<_>>(), "step {step}");
    }
}
