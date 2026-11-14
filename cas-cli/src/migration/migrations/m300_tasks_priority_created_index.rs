//! Index the task list's sort order so a full ordered read streams rows
//! instead of sorting every task body in a temp B-tree (cas-5461). On a
//! long-lived store that sort spilled ~100 MB of notes to temp files and held
//! the shared connection for over a second per read. Built in the explicit
//! migration phase, never while a runtime reader opens a populated store
//! (cas-4fe2).

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 300,
    name: "tasks_priority_created_index",
    subsystem: Subsystem::Tasks,
    description: "Index tasks by (priority, created_at DESC) so ordered task lists need no sort",
    up: &[
        "CREATE INDEX IF NOT EXISTS idx_tasks_priority_created ON tasks(priority, created_at DESC)",
    ],
    detect: Some(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_tasks_priority_created'",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn ordered_task_list_streams_without_a_temp_sort() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let project = home.join("project");
            std::fs::create_dir_all(&project).unwrap();
            crate::store::init_cas_dir(&project).unwrap();
            let conn = Connection::open(project.join(".cas").join("cas.db")).unwrap();
            let plan = conn
                .prepare("EXPLAIN QUERY PLAN SELECT * FROM tasks ORDER BY priority, created_at DESC")
                .unwrap()
                .query_map([], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert!(
                !plan.iter().any(|step| step.contains("TEMP B-TREE")),
                "the full task list must not sort its rows: {plan:?}"
            );
        });
    }
}
