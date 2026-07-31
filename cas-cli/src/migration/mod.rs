//! Schema migration system for CAS
//!
//! Provides versioned, trackable schema migrations that replace ad-hoc
//! ALTER TABLE statements scattered across store init() functions.
//!
//! # Usage
//!
//! ```rust,ignore
//! use cas::migration::{run_migrations, check_migrations, MigrationStatus};
//!
//! // Check for pending migrations
//! let status = check_migrations(&cas_dir)?;
//! println!("{} pending migrations", status.pending.len());
//!
//! // Run all pending migrations
//! run_migrations(&cas_dir, false)?;
//! ```

pub mod detector;
pub mod migrations;

pub use detector::detect_applied_migrations;
pub use migrations::MIGRATIONS;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use std::path::Path;

use crate::error::CasError;

/// Result type for migration operations
pub type Result<T> = std::result::Result<T, CasError>;

/// Subsystem that a migration affects
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subsystem {
    /// Entry storage (entries, metadata, sessions tables)
    Entries,
    /// Task storage (tasks, dependencies tables)
    Tasks,
    /// Rule storage (rules table)
    Rules,
    /// Skill storage (skills table)
    Skills,
    /// Agent coordination (agents, task_leases, lease_history tables)
    Agents,
    /// Entity/knowledge graph (entities, relationships, mentions tables)
    Entities,
    /// Task verification (verifications, verification_issues tables)
    Verification,
    /// Iteration loops (loops table)
    Loops,
    /// Git worktree management (worktrees table)
    Worktrees,
    /// Code analysis (code_files, code_symbols, code_relationships tables)
    Code,
    /// Activity events for sidecar feed
    Events,
    /// Factory recording text search
    Recording,
    /// Terminal recordings for time-travel playback
    Recordings,
    // NOTE: Tracing has its own traces.db file and handles migrations internally
}

impl Subsystem {
    /// Get string representation for storage
    pub fn as_str(&self) -> &'static str {
        match self {
            Subsystem::Entries => "entries",
            Subsystem::Tasks => "tasks",
            Subsystem::Rules => "rules",
            Subsystem::Skills => "skills",
            Subsystem::Agents => "agents",
            Subsystem::Entities => "entities",
            Subsystem::Verification => "verification",
            Subsystem::Loops => "loops",
            Subsystem::Worktrees => "worktrees",
            Subsystem::Code => "code",
            Subsystem::Events => "events",
            Subsystem::Recording => "recording",
            Subsystem::Recordings => "recordings",
        }
    }

    /// Every subsystem that exists today.
    ///
    /// Used by `ensure_base_schemas` to walk the full set during the
    /// migration-runner bootstrap. Keep this in sync with the enum variants.
    pub const ALL: &'static [Subsystem] = &[
        Subsystem::Entries,
        Subsystem::Tasks,
        Subsystem::Rules,
        Subsystem::Skills,
        Subsystem::Agents,
        Subsystem::Entities,
        Subsystem::Verification,
        Subsystem::Loops,
        Subsystem::Worktrees,
        Subsystem::Code,
        Subsystem::Events,
        Subsystem::Recording,
        Subsystem::Recordings,
    ];

    /// Apply this subsystem's base-schema bootstrap DDL to `conn`.
    ///
    /// "Base schema" is the set of `CREATE TABLE IF NOT EXISTS` (+ indexes)
    /// historically created lazily by `Sqlite*Store::init` / `::open`. ALTER
    /// migrations that target a subsystem assume the table already exists, so
    /// the migration runner invokes this before applying pending migrations
    /// on databases that have never had the matching store constructed.
    ///
    /// Subsystems that are fully migration-driven (no inline lazy bootstrap,
    /// e.g. `Recordings`, `Recording`) return `Ok(())` without executing any
    /// statements — their tables are created by migrations themselves.
    ///
    /// All DDL is `CREATE TABLE IF NOT EXISTS` / `CREATE INDEX IF NOT EXISTS`,
    /// so calling this on an already-populated database is a no-op.
    pub fn ensure_base_schema(&self, conn: &Connection) -> Result<()> {
        // Only subsystems whose canonical CREATE TABLE lives in a `Sqlite*Store`
        // constructor / init function (and is therefore tied to "did anyone
        // construct the store this process?") get pre-bootstrapped here.
        //
        // Subsystems that have an explicit `m###_*_create_table` migration in
        // the ledger (Worktrees / Code / Events / Recordings / Recording)
        // are DELIBERATELY excluded — their initial shape is owned by the
        // migration chain itself, and pre-installing the modern post-ALTER
        // shape would break subsequent ALTER migrations that target the
        // historical column layout (e.g. m112 indexes `worktrees.task_id`
        // which was renamed to `epic_id` by m120).
        //
        // The (sentinel_table, schema) pairs below mean: "if `sentinel_table`
        // is missing, install this DDL". When the sentinel table already
        // exists we skip the DDL entirely — the migration chain (ALTER
        // migrations + m###_*_create_table for sibling tables) is the
        // authoritative source from that point on. Re-running an
        // `IF NOT EXISTS` table create is a no-op, but the index statements
        // bundled in the same schema would fail with `no such column: …` on
        // a legacy partial table, so the existence check is load-bearing.
        let (sentinel_table, ddl): (Option<&'static str>, Option<&'static str>) = match self {
            // `Entries` and `Rules` ship as a single SQL bundle in cas-store
            // (entries + rules + metadata + sessions in one batch). We
            // execute it once via Entries; Rules is a no-op.
            Subsystem::Entries => (Some("entries"), Some(cas_store::ENTRIES_RULES_SCHEMA)),
            Subsystem::Rules => (None, None), // covered by Entries
            Subsystem::Tasks => (Some("tasks"), Some(cas_store::TASK_SCHEMA)),
            Subsystem::Skills => (Some("skills"), Some(cas_store::SKILL_SCHEMA)),
            Subsystem::Agents => (Some("agents"), Some(cas_store::AGENT_SCHEMA)),
            Subsystem::Entities => (Some("entities"), Some(cas_store::ENTITY_SCHEMA)),
            Subsystem::Verification => {
                (Some("verifications"), Some(cas_store::VERIFICATION_SCHEMA))
            }
            Subsystem::Loops => (Some("loops"), Some(cas_store::LOOP_SCHEMA)),
            // Migration-driven subsystems: their CREATE TABLE lives in a
            // numbered migration. Skip pre-bootstrap.
            Subsystem::Worktrees
            | Subsystem::Code
            | Subsystem::Events
            | Subsystem::Recording
            | Subsystem::Recordings => (None, None),
        };

        if let (Some(sentinel), Some(sql)) = (sentinel_table, ddl) {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [sentinel],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            if exists == 0 {
                conn.execute_batch(sql)?;
            }
        }
        Ok(())
    }
}

/// Ensure every subsystem's base schema exists on `conn`.
///
/// This is the fix for cas-bdb9: `apply_pending` / `run_migrations` used to
/// assume that each ALTER migration's target table had already been created
/// by some prior `Sqlite*Store::init`. On databases that have never had the
/// matching store constructed (e.g. `cas doctor --fix` on a `.cas/cas.db`
/// initialized by an older CAS version that didn't run every store init),
/// the ALTER would fail with `no such table: …`. Calling this before the
/// apply loop makes the bootstrap independent of which stores have been
/// touched in the current process. Idempotent.
pub fn ensure_base_schemas(conn: &Connection) -> Result<()> {
    for subsystem in Subsystem::ALL {
        subsystem.ensure_base_schema(conn)?;
    }
    Ok(())
}

impl std::fmt::Display for Subsystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A single schema migration
#[derive(Debug, Clone)]
pub struct Migration {
    /// Unique sequential ID
    pub id: u32,
    /// Machine-readable name (e.g., "add_epoch_to_task_leases")
    pub name: &'static str,
    /// Subsystem this migration affects
    pub subsystem: Subsystem,
    /// Human-readable description
    pub description: &'static str,
    /// SQL statements to apply (forward migration)
    pub up: &'static [&'static str],
    /// Optional detection query - returns > 0 if migration already applied
    /// Used for bootstrap detection of existing databases
    pub detect: Option<&'static str>,
}

/// Record of an applied migration
#[derive(Debug, Clone)]
pub struct AppliedMigration {
    pub id: u32,
    pub name: String,
    pub subsystem: String,
    pub applied_at: DateTime<Utc>,
}

/// Status of the migration system
#[derive(Debug, Clone)]
pub struct MigrationStatus {
    /// Migrations that have been applied
    pub applied: Vec<AppliedMigration>,
    /// Migrations that are pending
    pub pending: Vec<&'static Migration>,
    /// Current schema version (highest applied migration ID)
    pub current_version: u32,
    /// Latest available version
    pub latest_version: u32,
}

impl MigrationStatus {
    /// Check if there are any pending migrations
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Get count of pending migrations
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

/// Schema for the migrations tracking table
const MIGRATIONS_TABLE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS cas_migrations (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    subsystem TEXT NOT NULL,
    applied_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_migrations_subsystem ON cas_migrations(subsystem);
"#;

/// Ensure the migrations table exists
pub fn ensure_migrations_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(MIGRATIONS_TABLE_SCHEMA)?;
    Ok(())
}

/// Get list of already applied migrations from the database
fn get_applied_migrations(conn: &Connection) -> Result<Vec<AppliedMigration>> {
    let mut stmt =
        conn.prepare("SELECT id, name, subsystem, applied_at FROM cas_migrations ORDER BY id")?;

    let migrations = stmt
        .query_map([], |row| {
            let applied_at_str: String = row.get(3)?;
            let applied_at = DateTime::parse_from_rfc3339(&applied_at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            Ok(AppliedMigration {
                id: row.get(0)?,
                name: row.get(1)?,
                subsystem: row.get(2)?,
                applied_at,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(migrations)
}

/// Check migration status for a CAS directory
pub fn check_migrations(cas_dir: &Path) -> Result<MigrationStatus> {
    let db_path = cas_dir.join("cas.db");

    // If database doesn't exist, all migrations are pending
    if !db_path.exists() {
        return Ok(MigrationStatus {
            applied: vec![],
            pending: MIGRATIONS.iter().collect(),
            current_version: 0,
            latest_version: MIGRATIONS.last().map(|m| m.id).unwrap_or(0),
        });
    }

    let conn = Connection::open(&db_path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;

    // Ensure migrations table exists
    ensure_migrations_table(&conn)?;

    // Get applied migrations
    let applied = get_applied_migrations(&conn)?;
    let applied_ids: std::collections::HashSet<u32> = applied.iter().map(|m| m.id).collect();

    // Find pending migrations
    // Also check detect queries for migrations that may have been applied
    // via schema changes before the migration system was in place
    let pending: Vec<&'static Migration> = MIGRATIONS
        .iter()
        .filter(|m| {
            if applied_ids.contains(&m.id) {
                return false;
            }
            // Check if migration is already applied via schema detection
            if let Some(detect_query) = m.detect {
                let is_applied: i64 = conn
                    .query_row(detect_query, [], |row| row.get(0))
                    .unwrap_or(0);
                if is_applied > 0 {
                    // Migration already applied but not recorded - record it now
                    let _ = conn.execute(
                        "INSERT OR IGNORE INTO cas_migrations (id, name, subsystem, applied_at)
                         VALUES (?, ?, ?, ?)",
                        params![m.id, m.name, m.subsystem.as_str(), "DETECTED"],
                    );
                    return false;
                }
            }
            true
        })
        .collect();

    let current_version = applied.iter().map(|m| m.id).max().unwrap_or(0);
    let latest_version = MIGRATIONS.last().map(|m| m.id).unwrap_or(0);

    Ok(MigrationStatus {
        applied,
        pending,
        current_version,
        latest_version,
    })
}

/// Bootstrap migration tracking for an existing database
///
/// Detects which migrations have already been applied by examining
/// the database schema, and records them as applied.
pub fn bootstrap_migrations(cas_dir: &Path) -> Result<usize> {
    let db_path = cas_dir.join("cas.db");

    if !db_path.exists() {
        return Ok(0);
    }

    let conn = Connection::open(&db_path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;

    // Ensure migrations table exists
    ensure_migrations_table(&conn)?;

    // Check if already bootstrapped
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM cas_migrations", [], |row| row.get(0))?;

    if count > 0 {
        // Already has migrations recorded, skip bootstrap
        return Ok(0);
    }

    // Detect and record already-applied migrations
    let mut bootstrapped = 0;
    for migration in MIGRATIONS.iter() {
        if let Some(detect_query) = migration.detect {
            let is_applied: i64 = conn
                .query_row(detect_query, [], |row| row.get(0))
                .unwrap_or(0);

            if is_applied > 0 {
                conn.execute(
                    "INSERT OR IGNORE INTO cas_migrations (id, name, subsystem, applied_at)
                     VALUES (?, ?, ?, ?)",
                    params![
                        migration.id,
                        migration.name,
                        migration.subsystem.as_str(),
                        "BOOTSTRAP",
                    ],
                )?;
                bootstrapped += 1;
            }
        }
    }

    Ok(bootstrapped)
}

/// Apply a single migration
fn apply_migration(conn: &Connection, migration: &Migration) -> Result<()> {
    // Execute all SQL statements in the migration
    for sql in migration.up {
        conn.execute(sql, [])?;
    }

    // Record that migration was applied
    conn.execute(
        "INSERT INTO cas_migrations (id, name, subsystem, applied_at)
         VALUES (?, ?, ?, ?)",
        params![
            migration.id,
            migration.name,
            migration.subsystem.as_str(),
            Utc::now().to_rfc3339(),
        ],
    )?;

    Ok(())
}

/// Result of running migrations
#[derive(Debug, Clone)]
pub struct MigrationResult {
    /// Number of migrations applied
    pub applied_count: usize,
    /// Names of applied migrations
    pub applied_names: Vec<String>,
    /// Any errors encountered (migration name -> error message)
    pub errors: Vec<(String, String)>,
}

/// Check if the database has been initialized with base schemas.
///
/// Returns true if core tables (entries, rules, tasks) exist,
/// indicating `cas init` has been run.
fn is_db_initialized(conn: &Connection) -> bool {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('entries', 'rules', 'tasks')",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    count >= 3
}

/// Run all pending migrations
///
/// If `dry_run` is true, returns what would be done without applying.
pub fn run_migrations(cas_dir: &Path, dry_run: bool) -> Result<MigrationResult> {
    let db_path = cas_dir.join("cas.db");

    let conn = Connection::open(&db_path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

    // Check that base tables exist (cas init has been run)
    if !is_db_initialized(&conn) {
        return Err(CasError::NotInitialized);
    }

    // Ensure migrations table exists
    ensure_migrations_table(&conn)?;

    // Ensure every subsystem's base schema exists before any ALTER migration
    // runs. Fix for cas-bdb9: `cas doctor --fix` previously failed with
    // `no such table: skills` on databases that had never had
    // `SqliteSkillStore` / `SqliteAgentStore` constructed.
    ensure_base_schemas(&conn)?;

    // Bootstrap if needed (detect already-applied migrations)
    bootstrap_migrations(cas_dir)?;

    // Get pending migrations
    let status = check_migrations(cas_dir)?;

    if dry_run {
        return Ok(MigrationResult {
            applied_count: status.pending.len(),
            applied_names: status.pending.iter().map(|m| m.name.to_string()).collect(),
            errors: vec![],
        });
    }

    let mut result = MigrationResult {
        applied_count: 0,
        applied_names: vec![],
        errors: vec![],
    };

    for migration in status.pending {
        // Run each migration in a transaction
        conn.execute("BEGIN IMMEDIATE", [])?;

        match apply_migration(&conn, migration) {
            Ok(()) => {
                conn.execute("COMMIT", [])?;
                result.applied_count += 1;
                result.applied_names.push(migration.name.to_string());
            }
            Err(e) => {
                conn.execute("ROLLBACK", [])?;
                let reason = e.to_string();
                result
                    .errors
                    .push((migration.name.to_string(), reason.clone()));
                return Err(CasError::MigrationFailed {
                    name: migration.name.to_string(),
                    reason,
                });
            }
        }
    }

    Ok(result)
}

/// Check if there are pending migrations (for startup warning)
pub fn has_pending_migrations(cas_dir: &Path) -> bool {
    check_migrations(cas_dir)
        .map(|status| status.has_pending())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use crate::migration::*;
    use tempfile::TempDir;

    #[test]
    fn test_migrations_table_creation() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("cas.db");
        let conn = Connection::open(&db_path).unwrap();

        ensure_migrations_table(&conn).unwrap();

        // Verify table exists
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='cas_migrations'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_check_migrations_empty_db() {
        let temp = TempDir::new().unwrap();
        let status = check_migrations(temp.path()).unwrap();

        assert_eq!(status.current_version, 0);
        assert!(!status.pending.is_empty());
    }

    #[test]
    fn test_migration_dry_run() {
        // `init_cas_dir` calls `known_repos::register_repo(host_cas_dir)` which
        // writes to `$HOME/.cas/`. Without `with_temp_home`, concurrent sweep
        // tests (`worktree::sweep::*`) see the registration and fail. Wrap so
        // the host registry is isolated to this test's temp HOME.
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let temp = home.join("proj");
            std::fs::create_dir_all(&temp).unwrap();

            // Initialize CAS properly (creates base tables)
            crate::store::init_cas_dir(&temp).unwrap();

            let result = run_migrations(&temp.join(".cas"), true).unwrap();

            // Should report pending but not apply
            // (init_cas_dir already runs migrations, so pending may be 0)
            assert!(result.errors.is_empty());
        });
    }

    #[test]
    fn test_detect_already_applied_migration_via_schema() {
        // Test that migrations are detected as applied even after bootstrap,
        // if the schema change was made before the migration existed.
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("cas.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        ensure_migrations_table(&conn).unwrap();

        // Create a table with a column that a migration would add
        conn.execute_batch("CREATE TABLE test_table (id INTEGER PRIMARY KEY, test_column TEXT);")
            .unwrap();

        // Simulate a migration that's NOT recorded but column exists
        // This is the scenario: schema was updated before migration system existed
        conn.execute(
            "INSERT INTO cas_migrations (id, name, subsystem, applied_at) VALUES (999, 'fake_migration', 'test', 'TEST')",
            [],
        )
        .unwrap();
        drop(conn);

        // Now check_migrations should detect via schema that column exists
        // and NOT return the migration as pending (using detect query)
        // Note: We can't test with actual migrations without more setup,
        // but we can verify the detection mechanism works by checking
        // that the code path is exercised
        let status = check_migrations(temp.path()).unwrap();

        // The key assertion: migrations with detect queries that return > 0
        // should not be in pending, even if not in cas_migrations
        // Since we don't have the actual schema, all real migrations
        // will still be pending, but no errors from duplicate columns
        assert!(!status.applied.is_empty()); // At least our fake migration
    }

    #[test]
    fn test_run_migrations_rejects_uninitialized_db() {
        // run_migrations should refuse to run on a database where
        // cas init hasn't been run (no base tables)
        let temp = TempDir::new().unwrap();

        // Create an empty database with only the migrations table
        let db_path = temp.path().join("cas.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        ensure_migrations_table(&conn).unwrap();
        drop(conn);

        // Should fail with NotInitialized error
        let result = run_migrations(temp.path(), false);
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), CasError::NotInitialized),
            "Expected NotInitialized error"
        );
    }

    /// cas-bdb9: `ensure_base_schemas` on a fresh in-memory connection must
    /// create the canonical tables for every lazy-bootstrap subsystem so that
    /// subsequent ALTER migrations (e.g. m071_skills_add_summary,
    /// m200_agents_add_pid_starttime) never hit "no such table: …".
    #[test]
    fn test_ensure_base_schemas_creates_lazy_subsystem_tables() {
        let conn = Connection::open_in_memory().unwrap();

        // Sanity: a fresh in-memory DB has no user tables.
        let count_before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_before, 0, "fresh in-memory DB should be empty");

        ensure_base_schemas(&conn).expect("ensure_base_schemas should succeed");

        // Every lazy-bootstrap subsystem's primary table must now exist.
        // Subsystems whose canonical CREATE TABLE lives in a numbered
        // migration (Worktrees / Code / Events / Recording / Recordings)
        // are intentionally NOT bootstrapped here — their tables only
        // appear after the migration chain runs.
        let expected = [
            "entries",
            "rules",
            "metadata",
            "sessions", // shipped as part of ENTRIES_RULES_SCHEMA — target of m028/m031/m032/m042/m043/m044
            "tasks",
            "skills",
            "agents",
            "task_leases", // lives in AGENT_SCHEMA (FK to agents + NOT-NULL renewed_at)
            "entities",
            "relationships",
            "entity_mentions",
            "verifications",
            "verification_issues",
            "loops",
        ];

        for table in expected {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "expected table `{table}` to exist after bootstrap");
        }

        // Negative invariant: migration-driven subsystems (Worktrees, Code,
        // Events, Recording, Recordings) must NOT be pre-created by the
        // bootstrap. Their CREATE TABLE shape is owned by the migration
        // ledger and pre-installing the modern post-ALTER shape would break
        // later ALTERs (e.g. m112 indexes `worktrees.task_id`).
        let must_not_exist = [
            "worktrees",
            "code_files",
            "code_symbols",
            "code_relationships",
            "code_memory_links",
            "events",
            "recordings",
            "recording_text",
        ];
        for table in must_not_exist {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                exists, 0,
                "table `{table}` must NOT be pre-created by ensure_base_schemas; \
                 its CREATE TABLE lives in a numbered migration"
            );
        }
    }

    /// cas-bdb9: confirm `task_leases` lands via Agents (FK + NOT-NULL
    /// constraints intact), not via Tasks. Regression guard for fix-round-1
    /// P1 — the old `TASK_SCHEMA` duplicated `task_leases` with a slimmer
    /// shape that silently shadowed `AGENT_SCHEMA`'s definition when
    /// `Subsystem::ALL` iterated `Tasks` (index 1) before `Agents` (index 4),
    /// losing the FK to `agents(id)` and the `renewed_at NOT NULL` constraint.
    #[test]
    fn test_task_leases_lands_with_fk_and_not_null_via_agents() {
        let conn = Connection::open_in_memory().unwrap();
        // Foreign keys are OFF by default on a new connection; turn them on
        // so the FK is actually recorded by sqlite_master inspection.
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

        ensure_base_schemas(&conn).unwrap();

        // FK presence: pragma_foreign_key_list returns one row per FK column.
        let fk_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_foreign_key_list('task_leases') \
                 WHERE \"table\"='agents' AND \"from\"='agent_id' AND \"to\"='id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            fk_count, 1,
            "task_leases must keep its FK to agents(id) ON DELETE CASCADE — \
             AGENT_SCHEMA is the single source of truth"
        );

        // renewed_at must be NOT NULL (AGENT_SCHEMA shape, not the legacy
        // slim TASK_SCHEMA shape).
        let renewed_at_notnull: i64 = conn
            .query_row(
                "SELECT \"notnull\" FROM pragma_table_info('task_leases') WHERE name='renewed_at'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            renewed_at_notnull, 1,
            "task_leases.renewed_at must be NOT NULL — regression on the \
             dual-definition / IF-NOT-EXISTS no-op bug"
        );
    }

    /// cas-bdb9: `ensure_base_schemas` is idempotent — running it twice on
    /// the same connection must not error or create duplicates.
    #[test]
    fn test_ensure_base_schemas_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_base_schemas(&conn).expect("first run should succeed");
        ensure_base_schemas(&conn).expect("second run should be a no-op");

        // Spot-check that exactly one `skills` table exists.
        let skills_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='skills'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(skills_count, 1);
    }

    /// cas-bdb9: `run_migrations` on a CAS dir whose `.cas/cas.db` has only the
    /// minimal base tables (no skills/agents — simulating a DB initialized by
    /// an older CAS version) must succeed end-to-end, with the skills and
    /// agents tables bootstrapped and the ALTER migrations applied cleanly.
    #[test]
    fn test_run_migrations_bootstraps_missing_skills_and_agents_tables() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("cas.db");

        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
            // Seed entries/rules/tasks with their real lazy-bootstrap shape so
            // `is_db_initialized` passes — mirroring the bug-doc scenario where
            // an older CAS version initialized these stores but never touched
            // skills/agents.
            conn.execute_batch(cas_store::ENTRIES_RULES_SCHEMA).unwrap();
            conn.execute_batch(cas_store::TASK_SCHEMA).unwrap();
        }

        // Confirm the precondition: skills and agents do NOT exist yet.
        let conn = Connection::open(&db_path).unwrap();
        let lazy_tables_before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('skills', 'agents')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            lazy_tables_before, 0,
            "skills/agents should be absent before run_migrations"
        );
        drop(conn);

        let result = run_migrations(temp.path(), false);
        assert!(
            result.is_ok(),
            "run_migrations should succeed after base-schema bootstrap, got: {:?}",
            result.err()
        );

        // After run_migrations the skills AND agents tables must exist.
        let conn = Connection::open(&db_path).unwrap();
        let lazy_tables_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('skills', 'agents')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            lazy_tables_after, 2,
            "skills and agents must both exist after run_migrations bootstrap"
        );
    }

    /// cas-bdb9: running migrations a second time on the same already-
    /// bootstrapped DB is a no-op (no errors, no duplicate apply).
    #[test]
    fn test_run_migrations_is_idempotent_after_bootstrap() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("cas.db");

        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
            conn.execute_batch(cas_store::ENTRIES_RULES_SCHEMA).unwrap();
            conn.execute_batch(cas_store::TASK_SCHEMA).unwrap();
        }

        let first = run_migrations(temp.path(), false).expect("first run should succeed");
        let second = run_migrations(temp.path(), false).expect("second run should be a no-op");

        assert!(first.errors.is_empty());
        assert!(second.errors.is_empty());
        // Without this assertion `bootstrap_migrations` auto-detecting every
        // migration as already applied would let the test silently pass.
        assert!(
            first.applied_count > 0,
            "first run should apply at least one migration after base-schema bootstrap; \
             a 0-count would mean bootstrap_migrations falsely flagged every migration as applied"
        );
        assert_eq!(
            second.applied_count, 0,
            "second migration run should apply nothing"
        );
        let conn = Connection::open(&db_path).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM cas_migrations WHERE id = 215",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1,
            "m215 must be recorded exactly once across repeated migration runs"
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'known_repo_bindings'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1,
            "m214 host-binding schema must survive repeated migration runs"
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'verification_handoffs'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1,
            "m215 sealed-handoff table must survive repeated migration runs"
        );
    }

    /// cas-bdb9: pre-existing DB where stores HAVE been constructed continues
    /// to migrate correctly — the additive bootstrap must not corrupt or
    /// reset existing data.
    #[test]
    fn test_run_migrations_with_preexisting_stores_unchanged() {
        // Use `with_temp_home` to isolate the host known_repos registry that
        // `init_cas_dir` writes to — otherwise this test pollutes the shared
        // process-level $HOME and races with other tests (e.g.
        // `worktree::sweep::tests::sweep_all_known_iterates_registry_and_flags_unhealthy`).
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let temp = home.join("proj");
            std::fs::create_dir_all(&temp).unwrap();
            // Properly initialize CAS (runs every store init).
            crate::store::init_cas_dir(&temp).unwrap();
            let cas_dir = temp.join(".cas");

            // Insert a sentinel row to confirm data is preserved.
            {
                let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
                // The skills table already exists thanks to SqliteSkillStore::init().
                conn.execute(
                    "INSERT OR IGNORE INTO skills (id, name, created_at, updated_at) VALUES ('sentinel', 'sentinel', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                    [],
                )
                .unwrap();
            }

            // Run migrations again — should not error, sentinel row must survive.
            let result =
                run_migrations(&cas_dir, false).expect("run_migrations should succeed");
            assert!(result.errors.is_empty());

            let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
            let sentinel_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM skills WHERE id='sentinel'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(sentinel_count, 1, "pre-existing data must survive bootstrap");
        });
    }

    #[test]
    fn test_verifier_migrations_survive_serve_before_update_on_legacy_db() {
        fn table_shape(
            conn: &Connection,
            table: &str,
        ) -> (
            Vec<(String, String, i64, Option<String>, i64)>,
            Vec<(String, i64, i64)>,
        ) {
            let mut columns = conn
                .prepare(&format!(
                    "SELECT name, type, \"notnull\", dflt_value, pk
                     FROM pragma_table_info('{table}')"
                ))
                .unwrap()
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            columns.sort();

            let mut indexes = conn
                .prepare(&format!(
                    "SELECT name, \"unique\", partial
                     FROM pragma_index_list('{table}')"
                ))
                .unwrap()
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            indexes.sort();
            (columns, indexes)
        }

        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let project = home.join("proj");
            std::fs::create_dir_all(&project).unwrap();
            crate::store::init_cas_dir(&project).unwrap();
            let cas_dir = project.join(".cas");
            let db_path = cas_dir.join("cas.db");

            {
                let conn = Connection::open(&db_path).unwrap();
                conn.execute(
                    "INSERT INTO verifications
                     (id, task_id, verification_type, provenance, status, summary,
                      files_reviewed, created_at)
                     VALUES ('ver-legacy-upgrade', 'cas-legacy', 'task', 'legacy',
                             'approved', 'preserve this row', '[]',
                             '2026-01-01T00:00:00Z')",
                    [],
                )
                .unwrap();

                // Recreate the pre-m210 authority shape while retaining a real
                // legacy row. Remove side tables so the next store open
                // exercises the same eager schema initialization as `cas serve`.
                conn.execute_batch(
                    "DROP INDEX IF EXISTS idx_verification_dispatches_active_task;
                     DROP INDEX IF EXISTS idx_verification_dispatches_task;
                     DROP INDEX IF EXISTS idx_verification_capabilities_task;
                     DROP TABLE verification_dispatches;
                     DROP TABLE verification_capabilities;
                     ALTER TABLE verifications DROP COLUMN issuer_agent_id;
                     ALTER TABLE verifications DROP COLUMN capability_id;
                     ALTER TABLE verifications DROP COLUMN provenance;
                     DELETE FROM cas_migrations WHERE id IN (210, 211);",
                )
                .unwrap();
            }

            // Production-equivalent serve startup: current store DDL creates
            // the authority side tables but cannot alter the legacy primary
            // table. Historically that made m210's later CREATE TABLE collide.
            let store = cas_store::SqliteVerificationStore::open(&cas_dir)
                .expect("serve-first verification store init");
            drop(store);

            let before = Connection::open(&db_path).unwrap();
            assert_eq!(
                before
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('verifications')
                         WHERE name = 'provenance'",
                        [],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                0,
                "serve-first store open must leave the legacy primary table unmigrated"
            );
            assert_eq!(
                before
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master
                         WHERE type = 'table'
                           AND name IN ('verification_capabilities',
                                        'verification_dispatches')",
                        [],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                2,
                "serve-first store open must reproduce the side-table collision precondition"
            );
            drop(before);

            let first = run_migrations(&cas_dir, false)
                .expect("serve-first legacy verifier upgrade must succeed");
            assert!(
                first
                    .applied_names
                    .iter()
                    .any(|name| name == "verifier_authority"),
                "m210 must apply rather than being incorrectly bootstrapped: {:?}",
                first.applied_names
            );

            let conn = Connection::open(&db_path).unwrap();
            let (summary, provenance): (String, String) = conn
                .query_row(
                    "SELECT summary, provenance FROM verifications
                     WHERE id = 'ver-legacy-upgrade'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(summary, "preserve this row");
            assert_eq!(provenance, "legacy");
            for index in [
                "idx_verification_capabilities_task",
                "idx_verification_dispatches_task",
                "idx_verification_dispatches_active_task",
            ] {
                assert_eq!(
                    conn.query_row(
                        "SELECT COUNT(*) FROM sqlite_master
                         WHERE type = 'index' AND name = ?1",
                        [index],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                    1,
                    "missing authority index {index}"
                );
            }

            // Compare against the production bootstrap path, including
            // indexes that are intentionally installed only after current
            // exact-boundary columns are known to exist. Executing the static
            // schema alone cannot safely do that against a serve-first legacy
            // primary table.
            let fresh_dir = TempDir::new().unwrap();
            let fresh_store = cas_store::SqliteVerificationStore::open(fresh_dir.path())
                .expect("fresh verification store init");
            drop(fresh_store);
            let fresh = Connection::open(fresh_dir.path().join("cas.db")).unwrap();
            for table in [
                "verifications",
                "verification_capabilities",
                "verification_dispatches",
            ] {
                assert_eq!(
                    table_shape(&conn, table),
                    table_shape(&fresh, table),
                    "serve-first upgraded `{table}` shape must match a fresh current store"
                );
            }
            drop(conn);

            let second =
                run_migrations(&cas_dir, false).expect("repeated verifier upgrade must be a no-op");
            assert_eq!(second.applied_count, 0);
        });
    }

    #[test]
    fn test_failing_migration_rolls_back_cleanly() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("cas.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        ensure_migrations_table(&conn).unwrap();

        // Create base tables so migration flow is considered initialized.
        conn.execute_batch(
            "CREATE TABLE entries (id TEXT PRIMARY KEY);
             CREATE TABLE rules (id TEXT PRIMARY KEY);
             CREATE TABLE tasks (id TEXT PRIMARY KEY);",
        )
        .unwrap();

        let failing = Migration {
            id: 999_999,
            name: "test_failing_migration",
            subsystem: Subsystem::Tasks,
            description: "test migration that should fail and roll back",
            up: &[
                "CREATE TABLE should_not_exist (id INTEGER PRIMARY KEY)",
                "THIS IS INVALID SQL",
            ],
            detect: None,
        };

        conn.execute("BEGIN IMMEDIATE", []).unwrap();
        let result = apply_migration(&conn, &failing);
        assert!(result.is_err(), "migration should fail");
        conn.execute("ROLLBACK", []).unwrap();

        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='should_not_exist'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_exists, 0, "failed migration should be rolled back");

        let recorded: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM cas_migrations WHERE id = ?",
                [failing.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(recorded, 0, "failed migration must not be recorded");
    }
}
