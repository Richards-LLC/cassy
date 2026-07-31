//! CAS instance fixture for e2e testing
//!
//! Provides a fully initialized CAS instance with helper methods for common operations.
//! All data operations use direct SQLite (rusqlite) since CAS CLI subcommands
//! (task, add, rules, etc.) have been removed.
#![allow(dead_code)]

use assert_cmd::Command;
use rusqlite::{Connection, params};
use std::path::PathBuf;
use tempfile::TempDir;

/// A fully initialized CAS instance for testing
pub struct CasInstance {
    /// Temporary directory containing the CAS data
    pub temp_dir: TempDir,
    /// Path to the .cas directory
    pub cas_dir: PathBuf,
}

impl CasInstance {
    /// Create a new CAS command configured for this instance
    pub fn cas_cmd(&self) -> Command {
        let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cas"));
        configure_isolated_home(&mut cmd, self.temp_dir.path());
        cmd.current_dir(self.temp_dir.path());
        cmd.env("CAS_DIR", &self.cas_dir);
        // Clear CAS_ROOT to prevent env pollution from parent shell
        cmd.env_remove("CAS_ROOT");
        cmd.env("CAS_SKIP_FACTORY_TOOLING", "1");
        cmd
    }

    /// Open a connection to the CAS database
    fn open_db(&self) -> Connection {
        Connection::open(self.cas_dir.join("cas.db")).expect("Failed to open CAS database")
    }

    /// Generate a task ID in `cas-XXXX` format (4 hex chars)
    fn gen_task_id() -> String {
        use rand::Rng;
        let mut rng = rand::rng();
        let n: u16 = rng.random();
        format!("cas-{n:04x}")
    }

    /// Generate an entry ID in `YYYY-MM-DD-N` format
    fn gen_entry_id() -> String {
        use rand::Rng;
        let now = chrono::Utc::now();
        let n: u16 = rand::rng().random_range(1..=999);
        format!("{}-{}", now.format("%Y-%m-%d"), n)
    }

    /// Generate a rule ID in `rule-XXX` format
    fn gen_rule_id() -> String {
        use rand::Rng;
        let n: u16 = rand::rng().random_range(1..=999);
        format!("rule-{n:03x}")
    }

    /// Get current timestamp in RFC3339 format
    fn now() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    /// Add a memory entry and return its ID
    pub fn add_memory(&self, content: &str) -> String {
        self.add_memory_with_type(content, "learning")
    }

    /// Add a memory entry with specific type
    pub fn add_memory_with_type(&self, content: &str, entry_type: &str) -> String {
        let conn = self.open_db();
        let id = Self::gen_entry_id();
        let now = Self::now();
        conn.execute(
            "INSERT INTO entries (id, type, content, created, scope) VALUES (?1, ?2, ?3, ?4, 'project')",
            params![id, entry_type, content, now],
        )
        .expect("Failed to insert entry");
        id
    }

    /// Create a task and return its ID
    pub fn create_task(&self, title: &str) -> String {
        self.create_task_with_options(title, None, None, false)
    }

    /// Create a task with options
    pub fn create_task_with_options(
        &self,
        title: &str,
        task_type: Option<&str>,
        priority: Option<u8>,
        start: bool,
    ) -> String {
        let conn = self.open_db();
        let id = Self::gen_task_id();
        let now = Self::now();
        let tt = task_type.unwrap_or("task");
        let prio = priority.unwrap_or(2) as i32;
        let status = if start { "in_progress" } else { "open" };

        conn.execute(
            "INSERT INTO tasks (id, title, status, task_type, priority, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, title, status, tt, prio, now, now],
        )
        .expect("Failed to insert task");
        id
    }

    /// Start a task (set status to in_progress)
    pub fn start_task(&self, task_id: &str) {
        let conn = self.open_db();
        let now = Self::now();
        conn.execute(
            "UPDATE tasks SET status = 'in_progress', updated_at = ?1 WHERE id = ?2",
            params![now, task_id],
        )
        .expect("Failed to start task");
    }

    /// Add a note to a task
    pub fn add_task_note(&self, task_id: &str, note: &str, _note_type: &str) {
        let conn = self.open_db();
        let now = Self::now();
        // Append note to existing notes field
        conn.execute(
            "UPDATE tasks SET notes = CASE WHEN notes = '' THEN ?1 ELSE notes || '\n' || ?1 END, updated_at = ?2 WHERE id = ?3",
            params![note, now, task_id],
        )
        .expect("Failed to add task note");
    }

    /// Close a task
    pub fn close_task(&self, task_id: &str) {
        let conn = self.open_db();
        let now = Self::now();
        conn.execute(
            "UPDATE tasks SET status = 'closed', closed_at = ?1, close_reason = 'Test complete', updated_at = ?1 WHERE id = ?2",
            params![now, task_id],
        )
        .expect("Failed to close task");
    }

    /// Create a rule and return its ID
    pub fn create_rule(&self, content: &str) -> String {
        let conn = self.open_db();
        let id = Self::gen_rule_id();
        let now = Self::now();
        conn.execute(
            "INSERT INTO rules (id, content, created, status, scope) VALUES (?1, ?2, ?3, 'draft', 'project')",
            params![id, content, now],
        )
        .expect("Failed to insert rule");
        id
    }

    /// Mark a rule as helpful
    pub fn mark_rule_helpful(&self, rule_id: &str) {
        let conn = self.open_db();
        conn.execute(
            "UPDATE rules SET helpful_count = helpful_count + 1, status = 'proven' WHERE id = ?1",
            params![rule_id],
        )
        .expect("Failed to mark rule helpful");
    }

    /// Mark a memory as helpful
    pub fn mark_helpful(&self, entry_id: &str) {
        let conn = self.open_db();
        conn.execute(
            "UPDATE entries SET helpful_count = helpful_count + 1 WHERE id = ?1",
            params![entry_id],
        )
        .expect("Failed to mark entry helpful");
    }

    /// Mark a memory as harmful
    pub fn mark_harmful(&self, entry_id: &str) {
        let conn = self.open_db();
        conn.execute(
            "UPDATE entries SET harmful_count = harmful_count + 1 WHERE id = ?1",
            params![entry_id],
        )
        .expect("Failed to mark entry harmful");
    }

    /// Archive an entry
    pub fn archive(&self, entry_id: &str) {
        let conn = self.open_db();
        conn.execute(
            "UPDATE entries SET archived = 1 WHERE id = ?1",
            params![entry_id],
        )
        .expect("Failed to archive entry");
    }

    /// Sync rules to .claude/rules/ (still uses CLI — this command works)
    pub fn sync_rules(&self) {
        // Rules sync is part of the MCP server, not a standalone CLI command.
        // For tests, we can skip this or use the serve command.
        // Most hook tests don't need this functionality.
    }

    /// Search for entries (returns IDs)
    pub fn search(&self, query: &str) -> Vec<String> {
        let conn = self.open_db();
        let mut stmt = conn
            .prepare("SELECT id FROM entries WHERE content LIKE ?1 AND archived = 0")
            .expect("Failed to prepare search query");
        let pattern = format!("%{query}%");
        let ids: Vec<String> = stmt
            .query_map(params![pattern], |row| row.get(0))
            .expect("Failed to search entries")
            .filter_map(|r| r.ok())
            .collect();
        ids
    }

    /// Get task as JSON
    pub fn get_task_json(&self, task_id: &str) -> serde_json::Value {
        let conn = self.open_db();
        let mut stmt = conn
            .prepare("SELECT id, title, description, status, priority, task_type, assignee, created_at, updated_at FROM tasks WHERE id = ?1")
            .expect("Failed to prepare task query");
        stmt.query_row(params![task_id], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "title": row.get::<_, String>(1)?,
                "description": row.get::<_, String>(2)?,
                "status": row.get::<_, String>(3)?,
                "priority": row.get::<_, i32>(4)?,
                "task_type": row.get::<_, String>(5)?,
                "assignee": row.get::<_, Option<String>>(6)?,
                "created_at": row.get::<_, String>(7)?,
                "updated_at": row.get::<_, String>(8)?,
            }))
        })
        .expect("Failed to get task JSON")
    }
}

/// Create a new CAS instance for testing
pub fn new_cas_instance() -> CasInstance {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let cas_dir = temp_dir.path().join(".cas");

    // Initialize CAS (this now runs migrations automatically)
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cas"));
    configure_isolated_home(&mut cmd, temp_dir.path());
    cmd.current_dir(temp_dir.path());
    // Clear CAS_ROOT to prevent env pollution from parent shell
    cmd.env_remove("CAS_ROOT");
    cmd.args(["init", "--yes"]);
    let output = cmd.output().expect("Failed to run cas init");

    assert!(
        output.status.success(),
        "cas init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    CasInstance { temp_dir, cas_dir }
}

fn configure_isolated_home(cmd: &mut Command, root: &std::path::Path) {
    let home = root.join(".test-home");
    let xdg = root.join(".test-xdg-config");
    std::fs::create_dir_all(&home).expect("create isolated test HOME");
    std::fs::create_dir_all(&xdg).expect("create isolated test XDG_CONFIG_HOME");
    if let Some(host_home) = std::env::var_os("HOME") {
        cmd.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    cmd.env("HOME", home).env("XDG_CONFIG_HOME", xdg);
}

/// Create a CAS instance with sample data preloaded
pub fn new_cas_with_data() -> CasInstance {
    let cas = new_cas_instance();

    // Add some sample memories
    cas.add_memory("Sample learning about Rust ownership");
    cas.add_memory_with_type("User prefers dark mode", "preference");

    // Add a sample task
    cas.create_task("Sample task for testing");

    // Add a sample rule
    cas.create_rule("Always write tests for new features");

    cas
}

// Helper functions to extract IDs from command output

pub(crate) fn extract_entry_id(output: &str) -> Option<String> {
    // Match patterns like "Created entry: 2026-01-15-123" or "2026-01-15-123"
    let re = regex::Regex::new(r"(\d{4}-\d{2}-\d{2}-\d+)").ok()?;
    re.captures(output)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
}

pub(crate) fn extract_task_id(output: &str) -> Option<String> {
    // Match patterns like "Created task: cas-1234" or "cas-1234"
    let re = regex::Regex::new(r"(cas-[a-f0-9]{4})").ok()?;
    re.captures(output)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
}

pub(crate) fn extract_rule_id(output: &str) -> Option<String> {
    // Match patterns like "Created rule: rule-001" or "rule-001"
    let re = regex::Regex::new(r"(rule-[a-f0-9]+)").ok()?;
    re.captures(output)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use crate::fixtures::cas_instance::{extract_entry_id, extract_rule_id, extract_task_id};

    #[test]
    fn test_extract_entry_id() {
        assert_eq!(
            extract_entry_id("Created entry: 2026-01-15-123"),
            Some("2026-01-15-123".to_string())
        );
        assert_eq!(
            extract_entry_id("Entry 2026-01-15-456 stored"),
            Some("2026-01-15-456".to_string())
        );
    }

    #[test]
    fn test_extract_task_id() {
        assert_eq!(
            extract_task_id("Created task: cas-1a2b"),
            Some("cas-1a2b".to_string())
        );
        assert_eq!(
            extract_task_id("Task cas-abcd started"),
            Some("cas-abcd".to_string())
        );
    }

    #[test]
    fn test_extract_rule_id() {
        assert_eq!(
            extract_rule_id("Created rule: rule-001"),
            Some("rule-001".to_string())
        );
    }
}
