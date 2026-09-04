//! Schema migration system for Cassy
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
    /// Distilled project knowledge (knowledge_pages, knowledge_sources, FTS)
    Knowledge,
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
            Subsystem::Knowledge => "knowledge",
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
        Subsystem::Knowledge,
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
            | Subsystem::Recordings
            | Subsystem::Knowledge => (None, None),
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
/// initialized by an older Cassy version that didn't run every store init),
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
    /// Current schema version (highest contiguous applied migration ID)
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

CREATE TABLE IF NOT EXISTS cas_migration_reconciliations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    migration_id INTEGER NOT NULL,
    migration_name TEXT NOT NULL,
    previous_applied_at TEXT NOT NULL,
    reconciled_at TEXT NOT NULL,
    reason TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_migration_reconciliations_migration
    ON cas_migration_reconciliations(migration_id, id);
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

fn migration_is_detected(conn: &Connection, migration: &Migration) -> bool {
    migration
        .detect
        .and_then(|query| conn.query_row(query, [], |row| row.get::<_, i64>(0)).ok())
        .is_some_and(|detected| detected > 0)
}

fn record_detected_migration(conn: &Connection, migration: &Migration) -> Result<bool> {
    conn.execute(
        "INSERT OR IGNORE INTO cas_migrations (id, name, subsystem, applied_at)
         VALUES (?, ?, ?, ?)",
        params![
            migration.id,
            migration.name,
            migration.subsystem.as_str(),
            "DETECTED"
        ],
    )?;
    let recorded = conn.query_row(
        "SELECT COUNT(*) FROM cas_migrations WHERE id = ?1",
        [migration.id],
        |row| row.get::<_, i64>(0),
    )? > 0;
    Ok(recorded)
}

/// Check migration status for a Cassy directory
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
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;

    check_migrations_on_connection(&conn)
}

/// Check migration status through an already-open connection.
///
/// The migration runner calls this while it owns SQLite's write lock, so a
/// second process cannot observe a half-bootstrapped ledger and derive a
/// stale pending list.
fn check_migrations_on_connection(conn: &Connection) -> Result<MigrationStatus> {
    // Ensure migrations table exists
    ensure_migrations_table(&conn)?;

    let initially_applied = get_applied_migrations(&conn)?;
    let mut applied_ids: std::collections::HashSet<u32> = initially_applied
        .iter()
        .map(|migration| migration.id)
        .collect();

    // Schema detection may fill an unrecorded prefix left by databases that
    // predate the ledger, but it must never jump over a real lower gap. A
    // later migration can be independently detectable because a current store
    // initializer created its table; recording it before the lower migration
    // succeeds would make MAX(id) advertise a schema version the DB does not
    // actually have.
    let mut pending = Vec::new();
    let mut detection_blocked = false;
    for migration in MIGRATIONS {
        if applied_ids.contains(&migration.id) {
            // A ledger row is evidence about what an earlier runner believed;
            // the registered predicate is the source of truth about the
            // current schema. Surface recorded rows whose predicate is now
            // false so the runner can reconcile them safely in registry order.
            if !migration_is_detected(&conn, migration) {
                detection_blocked = true;
                pending.push(migration);
            }
            continue;
        }

        if !detection_blocked && migration_is_detected(&conn, migration) {
            if record_detected_migration(&conn, migration)? {
                applied_ids.insert(migration.id);
                continue;
            }
        }

        detection_blocked = true;
        pending.push(migration);
    }

    // Re-read after prefix detection so callers see the ledger writes made by
    // this check. The reported cursor is the applied registry prefix, not the
    // maximum arbitrary row: stranded higher rows must not hide a lower gap.
    let applied = get_applied_migrations(&conn)?;
    let applied_ids: std::collections::HashSet<u32> = applied
        .iter()
        .filter_map(|applied| {
            MIGRATIONS
                .iter()
                .find(|migration| migration.id == applied.id)
                .filter(|migration| migration_is_detected(&conn, migration))
                .map(|migration| migration.id)
        })
        .collect();
    let current_version = MIGRATIONS
        .iter()
        .take_while(|migration| applied_ids.contains(&migration.id))
        .last()
        .map(|migration| migration.id)
        .unwrap_or(0);
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
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;

    bootstrap_migrations_on_connection(&conn)
}

/// Bootstrap the migration ledger using a caller-owned connection.
///
/// Keeping this on the same connection as the startup lock is important: a
/// concurrent server must not race a ledger bootstrap with its own migration
/// detection and then attempt DDL based on stale observations.
fn bootstrap_migrations_on_connection(conn: &Connection) -> Result<usize> {
    // Ensure migrations table exists
    ensure_migrations_table(&conn)?;

    // Check if already bootstrapped
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM cas_migrations", [], |row| row.get(0))?;

    if count > 0 {
        // Already has migrations recorded, skip bootstrap
        return Ok(0);
    }

    // Detect and record only the already-applied prefix. Once a migration is
    // genuinely absent, later independently detectable schema must wait until
    // that lower gap is repaired by the ordered runner.
    let mut bootstrapped = 0;
    for migration in MIGRATIONS.iter() {
        if !migration_is_detected(&conn, migration) {
            break;
        }
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO cas_migrations (id, name, subsystem, applied_at)
             VALUES (?, ?, ?, ?)",
            params![
                migration.id,
                migration.name,
                migration.subsystem.as_str(),
                "BOOTSTRAP",
            ],
        )?;
        if inserted > 0 {
            bootstrapped += 1;
        }
    }

    Ok(bootstrapped)
}

/// Return the table and column targeted by Cassy's static
/// `ALTER TABLE ... ADD COLUMN ...` migration grammar.
///
/// Migration SQL is compiled into the binary rather than supplied by users,
/// and every additive-column migration uses this unquoted five-token prefix.
/// Keeping the parser deliberately narrow means unfamiliar ALTER statements
/// still execute normally instead of being silently skipped.
fn add_column_target(sql: &str) -> Option<(&str, &str)> {
    let mut tokens = sql.split_whitespace();
    let alter = tokens.next()?;
    let table_keyword = tokens.next()?;
    let table = tokens.next()?;
    let add = tokens.next()?;
    let column_keyword = tokens.next()?;
    let column = tokens.next()?;

    (alter.eq_ignore_ascii_case("ALTER")
        && table_keyword.eq_ignore_ascii_case("TABLE")
        && add.eq_ignore_ascii_case("ADD")
        && column_keyword.eq_ignore_ascii_case("COLUMN"))
    .then_some((table, column))
}

/// Apply one migration statement without re-adding a column that is already
/// present in a partially migrated database.
fn apply_migration_statement(conn: &Connection, sql: &str) -> Result<()> {
    if let Some((table, column)) = add_column_target(sql)
        && cas_store::shared_db::column_exists(conn, table, column)
    {
        return Ok(());
    }
    conn.execute(sql, [])?;
    Ok(())
}

/// Whether a migration's statements are safe to replay after its ledger row
/// has already been recorded.
///
/// Recorded-but-undetected repair is deliberately more conservative than a
/// normal forward migration. Cassy can prove additive columns are idempotent by
/// inspecting the target column, and SQLite's `IF NOT EXISTS` grammar proves
/// create statements are non-destructive. Everything else (DROP, rename,
/// UPDATE/backfill, or an unfamiliar future statement) is surfaced as an
/// error instead of being replayed on user data.
fn migration_is_safely_reconcilable(migration: &Migration) -> bool {
    migration.up.iter().all(|sql| {
        if add_column_target(sql).is_some() {
            return true;
        }

        let normalized = sql
            .split_whitespace()
            .map(|token| token.to_ascii_uppercase())
            .collect::<Vec<_>>()
            .join(" ");
        [
            "CREATE TABLE IF NOT EXISTS ",
            "CREATE VIRTUAL TABLE IF NOT EXISTS ",
            "CREATE INDEX IF NOT EXISTS ",
            "CREATE UNIQUE INDEX IF NOT EXISTS ",
        ]
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
    })
}

/// Repair a migration that is present in `cas_migrations` but whose detection
/// predicate is false, preserving the original ledger marker in an append-only
/// reconciliation receipt.
fn reconcile_recorded_migration(conn: &Connection, migration: &Migration) -> Result<()> {
    let recorded: (String, String, String) = conn.query_row(
        "SELECT name, subsystem, applied_at FROM cas_migrations WHERE id = ?1",
        [migration.id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let (recorded_name, recorded_subsystem, previous_applied_at) = recorded;

    if recorded_name != migration.name || recorded_subsystem != migration.subsystem.as_str() {
        return Err(CasError::MigrationFailed {
            name: migration.name.to_string(),
            reason: format!(
                "recorded migration identity mismatch: ledger has {recorded_name}/{recorded_subsystem}"
            ),
        });
    }

    if !migration_is_safely_reconcilable(migration) {
        return Err(CasError::MigrationFailed {
            name: migration.name.to_string(),
            reason: "recorded migration predicate is false, but its SQL is not provably safe to replay; manual repair is required".to_string(),
        });
    }

    for sql in migration.up {
        apply_migration_statement(conn, sql)?;
    }

    if !migration_is_detected(conn, migration) {
        return Err(CasError::MigrationFailed {
            name: migration.name.to_string(),
            reason: "recorded migration predicate remained false after safe replay".to_string(),
        });
    }

    let reconciled_at = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO cas_migration_reconciliations
             (migration_id, migration_name, previous_applied_at, reconciled_at, reason)
         VALUES (?1, ?2, ?3, ?4, 'recorded predicate was false')",
        params![
            migration.id,
            migration.name,
            previous_applied_at,
            reconciled_at
        ],
    )?;
    conn.execute(
        "UPDATE cas_migrations SET applied_at = ?1 WHERE id = ?2",
        params![reconciled_at, migration.id],
    )?;
    Ok(())
}

/// Apply a single migration
fn apply_migration(conn: &Connection, migration: &Migration) -> Result<()> {
    // Execute all SQL statements in the migration. Each ADD COLUMN statement
    // is guarded independently because a migration-level detect query cannot
    // distinguish every possible mixed-schema state.
    for sql in migration.up {
        apply_migration_statement(conn, sql)?;
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

/// Assign the current project's canonical identity to legacy task rows that
/// predate m241. A nullable column is intentional: a caller may have a
/// database without enough project configuration to derive an identity, and
/// such rows must remain excluded from project-scoped task surfaces until an
/// operator resolves them.
fn backfill_task_origin_project(conn: &Connection, cas_dir: &Path) -> Result<()> {
    if !cas_store::shared_db::column_exists(conn, "tasks", "origin_project") {
        return Ok(());
    }
    let Some(project_id) = crate::cloud::resolve_canonical_id(cas_dir) else {
        return Ok(());
    };
    conn.execute(
        "UPDATE tasks SET origin_project = ?1
         WHERE origin_project IS NULL OR trim(origin_project) = ''",
        [project_id],
    )?;
    Ok(())
}

/// Run all pending migrations
///
/// If `dry_run` is true, returns what would be done without applying.
pub fn run_migrations(cas_dir: &Path, dry_run: bool) -> Result<MigrationResult> {
    let db_path = cas_dir.join("cas.db");

    let conn = Connection::open(&db_path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

    // Check that base tables exist (cas init has been run)
    if !is_db_initialized(&conn) {
        return Err(CasError::NotInitialized);
    }

    // Serialize the base-schema bootstrap and status snapshot. Without this,
    // two freshly spawned MCP servers can independently bootstrap/detect the
    // ledger and retain stale pending lists before either reaches its first
    // per-migration BEGIN IMMEDIATE.
    conn.execute("BEGIN IMMEDIATE", [])?;
    let status = (|| {
        ensure_migrations_table(&conn)?;

        // Ensure every subsystem's base schema exists before any ALTER
        // migration runs. Fix for cas-bdb9: `cas doctor --fix` previously
        // failed with `no such table: skills` on databases that had never had
        // `SqliteSkillStore` / `SqliteAgentStore` constructed.
        ensure_base_schemas(&conn)?;
        bootstrap_migrations_on_connection(&conn)?;
        check_migrations_on_connection(&conn)
    })();
    let status = match status {
        Ok(status) => {
            conn.execute("COMMIT", [])?;
            status
        }
        Err(error) => {
            let _ = conn.execute("ROLLBACK", []);
            return Err(error);
        }
    };

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

        let already_recorded = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM cas_migrations WHERE id = ?1)",
            [migration.id],
            |row| row.get::<_, i64>(0),
        )? > 0;

        // A concurrent runner may have applied this migration after our
        // serialized status snapshot but before we acquired this migration's
        // write lock. It is complete, not a recorded-but-broken migration to
        // reconcile, so leave its ledger receipt untouched and move on.
        if already_recorded && migration_is_detected(&conn, migration) {
            conn.execute("COMMIT", [])?;
            continue;
        }

        if already_recorded {
            match reconcile_recorded_migration(&conn, migration) {
                Ok(()) => {
                    conn.execute("COMMIT", [])?;
                    result.applied_count += 1;
                    result.applied_names.push(migration.name.to_string());
                    continue;
                }
                Err(error) => {
                    conn.execute("ROLLBACK", [])?;
                    let reason = error.to_string();
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

        // `check_migrations` deliberately stops schema detection at the first
        // real gap. Once every lower migration has committed, preserve the
        // existing detection behavior for a later schema that was already
        // installed out of band instead of replaying its DDL.
        if migration_is_detected(&conn, migration) {
            match record_detected_migration(&conn, migration) {
                Ok(true) => {
                    conn.execute("COMMIT", [])?;
                    continue;
                }
                Ok(false) => {
                    conn.execute("ROLLBACK", [])?;
                    let reason =
                        "schema was detected but its ledger row could not be recorded".to_string();
                    result
                        .errors
                        .push((migration.name.to_string(), reason.clone()));
                    return Err(CasError::MigrationFailed {
                        name: migration.name.to_string(),
                        reason,
                    });
                }
                Err(error) => {
                    conn.execute("ROLLBACK", [])?;
                    let reason = error.to_string();
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

    backfill_task_origin_project(&conn, cas_dir)?;

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
    use cas_store::{CommitLinkStore, KnowledgeStore};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Every migration from `first` upward, minus the ids a fixture has already
    /// recorded.
    ///
    /// Derived from the registry rather than written out: these guards are
    /// about the *cursor* — a false ledger entry must order all later work
    /// behind it — not about which ids exist this week. Spelling the list
    /// literally made every one of them fail the moment a migration was added
    /// (m249-m252 left all three red), which trains readers to ignore them.
    /// The property still bites: a cursor that skipped or reordered anything
    /// would not match this sequence.
    fn pending_ids_from(first: u32, already_recorded: &[u32]) -> Vec<u32> {
        MIGRATIONS
            .iter()
            .map(|migration| migration.id)
            .filter(|id| *id >= first && !already_recorded.contains(id))
            .collect()
    }

    fn prepare_v225_knowledge_gap(home: &Path, stranded_later_ledger: bool) -> PathBuf {
        let project = home.join(if stranded_later_ledger {
            "stranded-v225"
        } else {
            "fresh-v225"
        });
        std::fs::create_dir_all(&project).unwrap();
        crate::store::init_cas_dir(&project).unwrap();
        let cas_dir = project.join(".cas");
        let conn = Connection::open(cas_dir.join("cas.db")).unwrap();

        // Recreate the exact commit_links + knowledge_pages shapes shipped at
        // schema v225. The released ledger marked m225 BOOTSTRAP while
        // commit_links was absent; m143 then created this legacy table later
        // in the same run, leaving m225 recorded but its predicate false.
        // The current store initializer deliberately cannot add columns to an
        // existing table, while the later m227-m229 tables remain present and
        // therefore satisfy their detection predicates.
        conn.execute_batch(
            "DROP TABLE commit_links;
             CREATE TABLE commit_links (
                 commit_hash TEXT PRIMARY KEY,
                 session_id TEXT NOT NULL,
                 agent_id TEXT NOT NULL,
                 branch TEXT NOT NULL,
                 message TEXT NOT NULL,
                 files_changed TEXT NOT NULL,
                 prompt_ids TEXT NOT NULL,
                 committed_at TEXT NOT NULL,
                 author TEXT NOT NULL,
                 scope TEXT NOT NULL DEFAULT 'project'
             );
             INSERT INTO commit_links
                 (commit_hash, session_id, agent_id, branch, message,
                  files_changed, prompt_ids, committed_at, author, scope)
             VALUES ('legacy-v225', 'session-v225', 'agent-v225', 'main',
                     'legacy observed commit', '[]', '[]',
                     '2026-01-01T00:00:00Z', 'Cassy', 'project');
             UPDATE cas_migrations SET applied_at = 'BOOTSTRAP' WHERE id = 225;
             DROP INDEX IF EXISTS idx_knowledge_pages_rel_path;
             DROP INDEX IF EXISTS idx_knowledge_pages_type;
             DROP INDEX IF EXISTS idx_knowledge_pages_pending_embedding;
             DROP TABLE knowledge_pages;
             CREATE TABLE knowledge_pages (
                 row_id INTEGER PRIMARY KEY AUTOINCREMENT,
                 id TEXT NOT NULL UNIQUE,
                 page_type TEXT NOT NULL,
                 title TEXT NOT NULL,
                 rel_path TEXT NOT NULL,
                 snippet TEXT NOT NULL DEFAULT '',
                 locked INTEGER NOT NULL DEFAULT 0,
                 sources_json TEXT NOT NULL DEFAULT '[]',
                 created_at TEXT NOT NULL,
                 updated_at TEXT NOT NULL,
                 pending_embedding INTEGER NOT NULL DEFAULT 1
             );
             CREATE UNIQUE INDEX idx_knowledge_pages_rel_path
                 ON knowledge_pages(rel_path);
             CREATE INDEX idx_knowledge_pages_type
                 ON knowledge_pages(page_type);
             CREATE INDEX idx_knowledge_pages_pending_embedding
                 ON knowledge_pages(updated_at) WHERE pending_embedding = 1;
             INSERT INTO knowledge_pages
                 (id, page_type, title, rel_path, created_at, updated_at)
             VALUES ('cas-kn-v225', 'architecture', 'Legacy page',
                     'architecture/legacy.md', '2026-01-01T00:00:00Z',
                     '2026-01-01T00:00:00Z');
             DELETE FROM cas_migrations WHERE id >= 226;",
        )
        .unwrap();

        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM cas_migrations", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            173,
            "released v225 ledger fixture must retain the exact row count"
        );
        assert_eq!(
            conn.query_row("SELECT MAX(id) FROM cas_migrations", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            225
        );
        assert!(!cas_store::shared_db::column_exists(
            &conn,
            "commit_links",
            "link_method"
        ));
        assert_eq!(
            conn.query_row(
                "SELECT applied_at FROM cas_migrations WHERE id = 225",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "BOOTSTRAP"
        );

        if stranded_later_ledger {
            for id in [227, 228, 229] {
                let migration = MIGRATIONS
                    .iter()
                    .find(|migration| migration.id == id)
                    .unwrap();
                conn.execute(
                    "INSERT INTO cas_migrations (id, name, subsystem, applied_at)
                     VALUES (?1, ?2, ?3, 'DETECTED')",
                    params![id, migration.name, migration.subsystem.as_str()],
                )
                .unwrap();
            }
        }
        drop(conn);

        // Production store opening upgrades the additive projection in place:
        // m225-era rows remain readable as local pages until the migration
        // runner reconciles the ledger and records the repair.
        let store = cas_store::SqliteKnowledgeStore::open(&cas_dir).unwrap();
        let pages = store
            .list_pages()
            .expect("legacy v225 projection must remain readable");
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].id, "cas-kn-v225");
        assert_eq!(pages[0].origin, cas_store::KnowledgePageOrigin::Local);
        assert_eq!(pages[0].origin_project_id, None);
        drop(store);

        cas_dir
    }

    fn assert_repaired_v225_knowledge_gap(cas_dir: &Path, expected_m226_ledger: &str) {
        let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
        for id in [225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244, 245, 246] {
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM cas_migrations WHERE id = ?1",
                    [id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                1,
                "migration {id} must be recorded exactly once"
            );
        }
        assert_eq!(
            conn.query_row(
                "SELECT applied_at FROM cas_migrations WHERE id = 226",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            expected_m226_ledger,
            "m226 preserves the fixture's truthful ledger attribution"
        );
        assert!(cas_store::shared_db::column_exists(
            &conn,
            "commit_links",
            "link_method"
        ));
        assert_eq!(
            conn.query_row(
                "SELECT previous_applied_at FROM cas_migration_reconciliations
                 WHERE migration_id = 225",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "BOOTSTRAP",
            "the false released ledger marker must remain auditable"
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM commit_links WHERE commit_hash = 'legacy-v225'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1,
            "additive reconciliation must preserve legacy provenance rows"
        );
        drop(conn);

        let links = cas_store::SqliteCommitLinkStore::open(cas_dir).unwrap();
        let legacy = links
            .get("legacy-v225")
            .expect("production provenance read must succeed")
            .expect("legacy provenance row must survive");
        assert_eq!(legacy.session_id, "session-v225");
        assert_eq!(legacy.link_method, None);

        let store = cas_store::SqliteKnowledgeStore::open(cas_dir).unwrap();
        let pages = store
            .list_pages()
            .expect("production knowledge listing must work after repair");
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].origin, cas_store::KnowledgePageOrigin::Local);
        assert_eq!(pages[0].origin_project_id, None);

        let status = check_migrations(cas_dir).unwrap();
        assert_eq!(
            status.current_version,
            MIGRATIONS.last().expect("registry is never empty").id
        );
        assert!(status.pending.is_empty());
        let second = run_migrations(cas_dir, false).unwrap();
        assert_eq!(second.applied_count, 0, "repeated open must be idempotent");
    }

    #[test]
    fn detected_later_migrations_wait_for_missing_lower_migration() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let cas_dir = prepare_v225_knowledge_gap(home, false);

            let status = check_migrations(&cas_dir).unwrap();
            assert_eq!(
                status.current_version, 224,
                "a false recorded m225 must stop the truthful contiguous cursor"
            );
            assert_eq!(
                status
                    .pending
                    .iter()
                    .map(|migration| migration.id)
                    .collect::<Vec<_>>(),
                pending_ids_from(225, &[]),
                "recorded m225 and missing m226 must order all later work behind them"
            );

            let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM cas_migrations WHERE id > 225",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                0,
                "status detection must not strand later ledger rows past a gap"
            );
            drop(conn);

            let first = run_migrations(&cas_dir, false).unwrap();
            assert_eq!(first.applied_count, 1);
            assert_eq!(first.applied_names, ["commit_links_link_method"]);

            let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
            for id in [227, 228, 229] {
                assert_eq!(
                    conn.query_row(
                        "SELECT applied_at FROM cas_migrations WHERE id = ?1",
                        [id],
                        |row| row.get::<_, String>(0),
                    )
                    .unwrap(),
                    "DETECTED",
                    "later migration {id} should retain schema detection after m226 succeeds"
                );
            }
            drop(conn);

            assert_repaired_v225_knowledge_gap(&cas_dir, "DETECTED");
        });
    }

    #[test]
    fn bootstrap_detection_stops_at_first_unapplied_migration() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let cas_dir = prepare_v225_knowledge_gap(home, false);
            let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
            conn.execute("DELETE FROM cas_migrations", []).unwrap();
            drop(conn);

            assert_eq!(bootstrap_migrations(&cas_dir).unwrap(), 172);
            let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
            assert_eq!(
                conn.query_row("SELECT MAX(id) FROM cas_migrations", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                224
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM cas_migrations WHERE id > 225",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                0
            );
        });
    }

    #[test]
    fn stranded_later_ledger_repairs_missing_lower_migration() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let cas_dir = prepare_v225_knowledge_gap(home, true);

            let status = check_migrations(&cas_dir).unwrap();
            assert_eq!(
                status.current_version, 224,
                "max ledger id 229 must not hide the false recorded m225 entry"
            );
            assert_eq!(
                status
                    .pending
                    .iter()
                    .map(|migration| migration.id)
                    .collect::<Vec<_>>(),
                pending_ids_from(225, &[227, 228, 229])
            );

            let first = run_migrations(&cas_dir, false).unwrap();
            assert_eq!(first.applied_count, 1);
            assert_eq!(first.applied_names, ["commit_links_link_method"]);
            assert_repaired_v225_knowledge_gap(&cas_dir, "DETECTED");
        });
    }

    #[test]
    fn all_absent_parent_bootstrap_shortcuts_are_reconciled() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let cas_dir = prepare_v225_knowledge_gap(home, false);
            let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
            conn.execute("DROP TABLE knowledge_page_tombstones", [])
                .unwrap();
            for id in [226, 227] {
                let migration = MIGRATIONS
                    .iter()
                    .find(|migration| migration.id == id)
                    .unwrap();
                conn.execute(
                    "INSERT INTO cas_migrations (id, name, subsystem, applied_at)
                     VALUES (?1, ?2, ?3, 'BOOTSTRAP')",
                    params![id, migration.name, migration.subsystem.as_str()],
                )
                .unwrap();
            }
            drop(conn);

            let status = check_migrations(&cas_dir).unwrap();
            assert_eq!(status.current_version, 224);
            assert_eq!(
                status
                    .pending
                    .iter()
                    .map(|migration| migration.id)
                    .collect::<Vec<_>>(),
                pending_ids_from(225, &[226])
            );

            let first = run_migrations(&cas_dir, false).unwrap();
            assert_eq!(first.applied_count, 2);
            assert_eq!(
                first.applied_names,
                ["commit_links_link_method", "knowledge_page_tombstones"]
            );

            let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM cas_migration_reconciliations
                     WHERE migration_id IN (225, 226, 227)",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                2,
                "only migrations still absent after store-open repair need audit receipts"
            );
            drop(conn);

            assert_repaired_v225_knowledge_gap(&cas_dir, "BOOTSTRAP");
        });
    }

    #[test]
    fn recorded_additive_migration_reconciles_with_an_audit_receipt() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_migrations_table(&conn).unwrap();
        conn.execute("CREATE TABLE sample (id INTEGER PRIMARY KEY)", [])
            .unwrap();
        let migration = Migration {
            id: 999_998,
            name: "sample_add_value",
            subsystem: Subsystem::Tasks,
            description: "synthetic recorded-but-undetected additive migration",
            up: &["ALTER TABLE sample ADD COLUMN value TEXT"],
            detect: Some("SELECT COUNT(*) FROM pragma_table_info('sample') WHERE name = 'value'"),
        };
        conn.execute(
            "INSERT INTO cas_migrations (id, name, subsystem, applied_at)
             VALUES (?1, ?2, ?3, 'BOOTSTRAP')",
            params![migration.id, migration.name, migration.subsystem.as_str()],
        )
        .unwrap();

        conn.execute("BEGIN IMMEDIATE", []).unwrap();
        reconcile_recorded_migration(&conn, &migration).unwrap();
        conn.execute("COMMIT", []).unwrap();

        assert!(migration_is_detected(&conn, &migration));
        assert_eq!(
            conn.query_row(
                "SELECT previous_applied_at FROM cas_migration_reconciliations
                 WHERE migration_id = ?1",
                [migration.id],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "BOOTSTRAP"
        );
    }

    #[test]
    fn recorded_destructive_migration_is_never_replayed() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_migrations_table(&conn).unwrap();
        conn.execute("CREATE TABLE irreplaceable (id INTEGER PRIMARY KEY)", [])
            .unwrap();
        let migration = Migration {
            id: 999_999,
            name: "destructive_repair",
            subsystem: Subsystem::Tasks,
            description: "synthetic destructive migration",
            up: &["DROP TABLE irreplaceable"],
            detect: Some("SELECT 0"),
        };
        conn.execute(
            "INSERT INTO cas_migrations (id, name, subsystem, applied_at)
             VALUES (?1, ?2, ?3, 'BOOTSTRAP')",
            params![migration.id, migration.name, migration.subsystem.as_str()],
        )
        .unwrap();

        conn.execute("BEGIN IMMEDIATE", []).unwrap();
        let error = reconcile_recorded_migration(&conn, &migration)
            .expect_err("destructive replay must be refused");
        conn.execute("ROLLBACK", []).unwrap();

        assert!(error.to_string().contains("not provably safe to replay"));
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'irreplaceable'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM cas_migration_reconciliations
                 WHERE migration_id = ?1",
                [migration.id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn m241_backfills_blank_task_origins_from_current_project() {
        let home = TempDir::new().unwrap();
        let project = home.path().join("accounting");
        let cas_dir = project.join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();
        let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (
                 id TEXT PRIMARY KEY,
                 origin_project TEXT
             );
             INSERT INTO tasks (id, origin_project) VALUES
                 ('legacy-null', NULL),
                 ('legacy-blank', '  '),
                 ('already-assigned', 'acme/other');
             CREATE INDEX idx_tasks_origin_project ON tasks(origin_project);",
        )
        .unwrap();

        let expected = crate::cloud::resolve_canonical_id(&cas_dir).unwrap();
        super::backfill_task_origin_project(&conn, &cas_dir).unwrap();
        let origins = conn
            .prepare("SELECT id, origin_project FROM tasks ORDER BY id")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(
            origins,
            vec![
                ("already-assigned".to_string(), "acme/other".to_string()),
                ("legacy-blank".to_string(), expected.clone()),
                ("legacy-null".to_string(), expected),
            ]
        );
    }

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
        // Keep the temporary HOME because migration tests exercise other
        // host-scoped paths and must never observe developer state.
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let temp = home.join("proj");
            std::fs::create_dir_all(&temp).unwrap();

            // Initialize Cassy properly (creates base tables)
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
            assert_eq!(
                exists, 1,
                "expected table `{table}` to exist after bootstrap"
            );
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

    /// cas-bdb9: `run_migrations` on a Cassy dir whose `.cas/cas.db` has only the
    /// minimal base tables (no skills/agents — simulating a DB initialized by
    /// an older Cassy version) must succeed end-to-end, with the skills and
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
            // an older Cassy version initialized these stores but never touched
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
        assert!(
            cas_store::shared_db::column_exists(&conn, "commit_links", "link_method"),
            "m225 must wait for m143 to create commit_links, then add link_method"
        );
        assert!(!matches!(
            conn.query_row(
                "SELECT applied_at FROM cas_migrations WHERE id = 225",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
            .as_str(),
            "BOOTSTRAP" | "DETECTED"
        ));
    }

    #[test]
    fn concurrent_runners_apply_a_pending_migration_once_without_lock_errors() {
        let temp = TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        {
            let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
            conn.execute("ALTER TABLE tasks DROP COLUMN terminal_outcome", [])
                .unwrap();
            conn.execute("DELETE FROM cas_migrations WHERE id = 233", [])
                .unwrap();
        }

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let barrier = std::sync::Arc::clone(&barrier);
                let cas_dir = cas_dir.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    run_migrations(&cas_dir, false)
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle
                .join()
                .expect("migration thread must not panic")
                .expect("concurrent migration runner must not return a lock error");
        }

        let status = check_migrations(&cas_dir).unwrap();
        assert!(status.pending.is_empty());
        let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM cas_migrations WHERE id = 233",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1,
            "the concurrent stale runner must not duplicate the migration ledger receipt"
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM cas_migration_reconciliations WHERE migration_id = 233",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0,
            "a runner that loses the race must skip the now-complete migration, not reconcile it"
        );
    }

    /// cas-cbf1: the knowledge store lands on a DB that predates it — the
    /// acceptance criterion "migration applies cleanly on existing DBs".
    #[test]
    fn test_run_migrations_creates_knowledge_store_on_legacy_db() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("cas.db");

        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
            // A DB from an older Cassy version: entries/rules/tasks only.
            conn.execute_batch(cas_store::ENTRIES_RULES_SCHEMA).unwrap();
            conn.execute_batch(cas_store::TASK_SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO tasks (id, title, status, created_at, updated_at)
                 VALUES ('cas-old1', 'pre-existing task', 'open', '2026-01-01T00:00:00Z',
                         '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        }

        let result = run_migrations(temp.path(), false).expect("migration must apply cleanly");
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        let conn = Connection::open(&db_path).unwrap();
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name IN
                 ('knowledge_pages', 'knowledge_sources', 'knowledge_pages_fts')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 3, "all three knowledge tables must exist");

        // The FTS index must be usable (contentless_delete needs SQLite 3.43+).
        conn.execute(
            "INSERT INTO knowledge_pages_fts (rowid, title, snippet, body)
             VALUES (1, 'T', 'S', 'body text')",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM knowledge_pages_fts WHERE rowid = 1", [])
            .unwrap();

        // Pre-existing data is untouched.
        let task_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE id = 'cas-old1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(task_count, 1);
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
            // Properly initialize Cassy (runs every store init).
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
            let result = run_migrations(&cas_dir, false).expect("run_migrations should succeed");
            assert!(result.errors.is_empty());

            let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
            let sentinel_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM skills WHERE id='sentinel'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                sentinel_count, 1,
                "pre-existing data must survive bootstrap"
            );
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
    fn test_multi_alter_migrations_are_individually_idempotent_from_mixed_states() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE worktrees (
                 id TEXT PRIMARY KEY,
                 change_id TEXT
             );
             CREATE TABLE verifications (
                 id TEXT PRIMARY KEY,
                 task_id TEXT NOT NULL,
                 provenance TEXT NOT NULL DEFAULT 'legacy',
                 created_at TEXT NOT NULL
             );
             CREATE TABLE verification_dispatches (
                 id TEXT PRIMARY KEY,
                 receipt_id TEXT
             );",
        )
        .unwrap();

        // Static audit: these are the only migrations with multiple ALTER
        // statements. Seed one column from each migration, then apply every
        // statement twice through the production runner helper.
        let multi_alter_migrations: Vec<&Migration> = MIGRATIONS
            .iter()
            .filter(|migration| matches!(migration.id, 130 | 210 | 213))
            .collect();
        assert_eq!(multi_alter_migrations.len(), 3);
        for migration in multi_alter_migrations {
            for _ in 0..2 {
                for sql in migration.up {
                    apply_migration_statement(&conn, sql).unwrap_or_else(|error| {
                        panic!(
                            "{} statement must be individually repeat-safe: {sql}: {error}",
                            migration.name
                        )
                    });
                }
            }
        }

        for (table, column) in [
            ("worktrees", "change_id"),
            ("worktrees", "workspace_name"),
            ("worktrees", "has_conflicts"),
            ("verifications", "provenance"),
            ("verifications", "capability_id"),
            ("verifications", "issuer_agent_id"),
            ("verification_dispatches", "receipt_id"),
            ("verification_dispatches", "delivery_transaction_id"),
            ("verification_capabilities", "dispatch_id"),
            ("verifications", "dispatch_id"),
        ] {
            assert!(
                cas_store::shared_db::column_exists(&conn, table, column),
                "missing {table}.{column} after mixed-state convergence"
            );
        }
    }

    #[test]
    fn test_mixed_m213_schema_converges_through_m214_and_is_repeat_safe() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let project = home.join("proj");
            std::fs::create_dir_all(&project).unwrap();
            crate::store::init_cas_dir(&project).unwrap();
            let cas_dir = project.join(".cas");
            let db_path = cas_dir.join("cas.db");

            {
                let conn = Connection::open(&db_path).unwrap();
                conn.execute_batch(
                    "DROP INDEX IF EXISTS idx_verification_capabilities_dispatch;
                     DROP INDEX IF EXISTS idx_verifications_dispatch;
                     ALTER TABLE verification_capabilities DROP COLUMN dispatch_id;
                     ALTER TABLE verifications DROP COLUMN dispatch_id;
                     DROP TABLE known_repo_bindings;
                     DELETE FROM cas_migrations WHERE id IN (213, 214);",
                )
                .unwrap();

                for column in ["receipt_id", "delivery_transaction_id"] {
                    assert_eq!(
                        conn.query_row(
                            "SELECT COUNT(*) FROM pragma_table_info('verification_dispatches')
                             WHERE name = ?1",
                            [column],
                            |row| row.get::<_, i64>(0),
                        )
                        .unwrap(),
                        1,
                        "live failure shape requires existing dispatch column {column}"
                    );
                }
                for table in ["verification_capabilities", "verifications"] {
                    assert_eq!(
                        conn.query_row(
                            &format!(
                                "SELECT COUNT(*) FROM pragma_table_info('{table}')
                                 WHERE name = 'dispatch_id'"
                            ),
                            [],
                            |row| row.get::<_, i64>(0),
                        )
                        .unwrap(),
                        0,
                        "live failure shape requires legacy {table}"
                    );
                }
            }

            let first = run_migrations(&cas_dir, false)
                .expect("mixed m213 schema must converge instead of duplicating receipt_id");
            assert!(
                first
                    .applied_names
                    .iter()
                    .any(|name| name == "verification_proof_boundaries"),
                "m213 must be applied, not falsely detected: {:?}",
                first.applied_names
            );
            assert!(
                first
                    .applied_names
                    .iter()
                    .any(|name| name == "known_repo_bindings"),
                "m214 must run after repaired m213: {:?}",
                first.applied_names
            );

            let conn = Connection::open(&db_path).unwrap();
            for (table, column) in [
                ("verification_dispatches", "receipt_id"),
                ("verification_dispatches", "delivery_transaction_id"),
                ("verification_capabilities", "dispatch_id"),
                ("verifications", "dispatch_id"),
            ] {
                assert_eq!(
                    conn.query_row(
                        &format!(
                            "SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"
                        ),
                        [column],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                    1,
                    "missing converged {table}.{column}"
                );
            }
            for migration_id in [213, 214] {
                assert_eq!(
                    conn.query_row(
                        "SELECT COUNT(*) FROM cas_migrations WHERE id = ?1",
                        [migration_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                    1,
                    "migration {migration_id} must be recorded exactly once"
                );
            }
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'table' AND name = 'known_repo_bindings'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                1,
                "m214 table must exist after mixed-state repair"
            );
            drop(conn);

            let second = run_migrations(&cas_dir, false)
                .expect("repeated mixed-state repair must be a no-op");
            assert_eq!(second.applied_count, 0);
        });
    }

    // =========================================================================
    // cas-d9c7: a migration renamed/renumbered in flight leaves its old name in
    // the ledger under a different id. `cas_migrations.name` is UNIQUE, so the
    // new id can never be recorded and the phase dies on every run — observed
    // on gabber-studio, stuck at 252 with 2 pending forever.
    // =========================================================================

    /// A fully migrated store rewound to the exact gabber-studio ledger shape:
    /// row 250 carries `history_embedding_error` (the pre-renumber name) while
    /// the registry has m250=quarantined_rows and m253=history_embedding_error.
    fn renamed_ledger_fixture(temp: &TempDir, drop_embedding_error_column: bool) -> PathBuf {
        let project = temp.path().join("renamed-ledger");
        std::fs::create_dir_all(&project).unwrap();
        crate::store::init_cas_dir(&project).unwrap();
        let cas_dir = project.join(".cas");
        let conn = Connection::open(cas_dir.join("cas.db")).unwrap();

        // m254's table must be absent so the run has real work to apply after
        // the reconciliation, not just rows to record.
        conn.execute_batch("DROP TABLE IF EXISTS code_index_skipped_files;")
            .unwrap();
        if drop_embedding_error_column {
            // The name matches but the schema is NOT there: the migration must
            // be applied, never assumed from the ledger row.
            conn.execute_batch("ALTER TABLE history_docs DROP COLUMN embedding_error;")
                .unwrap();
        }

        conn.execute("DELETE FROM cas_migrations WHERE id >= 250", [])
            .unwrap();
        conn.execute(
            "INSERT INTO cas_migrations (id, name, subsystem, applied_at)
             VALUES (250, 'history_embedding_error', 'code', 'DETECTED')",
            [],
        )
        .unwrap();
        for (id, name, subsystem) in [
            (251u32, "sync_revisions", "tasks"),
            (252, "sync_conflicts_add_revisions", "tasks"),
        ] {
            conn.execute(
                "INSERT INTO cas_migrations (id, name, subsystem, applied_at)
                 VALUES (?1, ?2, ?3, 'DETECTED')",
                params![id, name, subsystem],
            )
            .unwrap();
        }
        cas_dir
    }

    fn ledger_name(conn: &Connection, id: u32) -> Option<String> {
        conn.query_row(
            "SELECT name FROM cas_migrations WHERE id = ?1",
            [id],
            |row| row.get::<_, String>(0),
        )
        .ok()
    }

    #[test]
    fn renamed_ledger_row_is_reconciled_and_the_phase_completes() {
        let temp = TempDir::new().unwrap();
        let cas_dir = renamed_ledger_fixture(&temp, false);

        // The wedge, before the fix: the phase fails and nothing advances.
        let result = run_migrations(&cas_dir, false)
            .expect("a renamed ledger row must not fail the migration phase");
        assert!(
            result.errors.is_empty(),
            "no migration should error: {:?}",
            result.errors
        );

        let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
        assert_eq!(
            ledger_name(&conn, 250).as_deref(),
            Some("quarantined_rows"),
            "id 250 must end up owned by the registry migration that holds it"
        );
        assert_eq!(
            ledger_name(&conn, 253).as_deref(),
            Some("history_embedding_error"),
            "the renamed migration must be recorded under its current id"
        );
        assert_eq!(
            ledger_name(&conn, 254).as_deref(),
            Some("code_index_skipped_files"),
            "later pending migrations must still be applied"
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM cas_migrations WHERE name = 'history_embedding_error'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1,
            "the stale row must be gone, not duplicated"
        );

        let status = check_migrations(&cas_dir).unwrap();
        assert_eq!(status.pending_count(), 0, "pending: {:?}", status.pending);
        assert_eq!(
            status.current_version,
            MIGRATIONS.last().unwrap().id,
            "the store must reach the latest schema version"
        );

        let reconciliation: (i64, String, String) = conn
            .query_row(
                "SELECT migration_id, migration_name, reason FROM cas_migration_reconciliations
                 WHERE migration_name = 'history_embedding_error' ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("the rename must be recorded as a reconciliation");
        assert_eq!(reconciliation.0, 253);
        assert!(
            reconciliation.2.contains("250") && reconciliation.2.contains("253"),
            "the reconciliation must name both ids: {}",
            reconciliation.2
        );

        let second = run_migrations(&cas_dir, false).expect("repair must be idempotent");
        assert_eq!(second.applied_count, 0, "second run must be a no-op");
    }

    #[test]
    fn ledger_name_match_without_schema_applies_the_migration() {
        let temp = TempDir::new().unwrap();
        let cas_dir = renamed_ledger_fixture(&temp, true);

        run_migrations(&cas_dir, false).expect("the phase must complete");

        let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
        assert!(
            cas_store::shared_db::column_exists(&conn, "history_docs", "embedding_error"),
            "a name match with the schema absent must APPLY the migration, not assume it"
        );
        assert_eq!(
            ledger_name(&conn, 253).as_deref(),
            Some("history_embedding_error")
        );
        assert_eq!(check_migrations(&cas_dir).unwrap().pending_count(), 0);
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
