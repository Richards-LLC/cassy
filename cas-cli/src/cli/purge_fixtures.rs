//! `cas purge-test-fixtures` — remove the integration-suite fixture memories
//! that leaked into real Cassy stores (cas-78c8 / GH #156).
//!
//! For months the integration suite wrote its five literal fixture strings into
//! the developer's `~/.cas/cas.db` and the cas-src project database instead of
//! a temp store: 994 of 1696 rows, 58.6% of the corpus. The leak itself is
//! fixed by the `CAS_TEST_PROTECTED_DBS` tripwire in
//! `cas_store::shared_db`; this command cleans up what already landed.
//!
//! Two rules govern the deletion, and both exist because the alternative
//! destroys real memories:
//!
//! * **Exact equality, never `LIKE`.** A `LIKE '%Context test memory entry%'`
//!   would also match a genuine memory that quotes a fixture string while
//!   explaining this very bug. The same rule governs R1 of the cas-b129
//!   migration, and the string list is shared with it rather than copied.
//! * **Dry run by default.** `--apply` is required to delete anything, and
//!   `--apply` first writes a full `VACUUM INTO` snapshot of each database.
//!   That snapshot is a complete, self-consistent SQLite file: if the process
//!   dies mid-delete, the source database is still intact (the delete runs in
//!   one transaction) and the snapshot is a second line of defence.

use std::path::{Path, PathBuf};

use clap::Args;
use rusqlite::{Connection, OpenFlags, params_from_iter};

use crate::memory_migration::routing::FIXTURE_CONTENTS;

#[derive(Debug, Clone, Args)]
pub struct PurgeFixturesArgs {
    /// Actually delete. Without this the command is report-only.
    #[arg(long)]
    pub apply: bool,

    /// Which databases to clean.
    #[arg(long, value_parser = ["project", "global", "both"], default_value = "both")]
    pub scope: String,

    /// Override the project Cassy root (default: the detected one).
    #[arg(long)]
    pub project_root: Option<PathBuf>,

    /// Override the global Cassy root (default: `~/.cas`).
    #[arg(long)]
    pub global_root: Option<PathBuf>,

    /// Where the pre-delete snapshots are written (default: alongside each
    /// database, as `cas.db.fixture-purge-backup`).
    #[arg(long)]
    pub backup_dir: Option<PathBuf>,

    /// Print every row that would be deleted as `db<TAB>id<TAB>content`.
    ///
    /// The per-string counts say how many rows go; this says exactly which.
    /// `--apply` writes the same list to a manifest beside the backup whether
    /// or not this flag is set, so an applied purge always leaves a receipt.
    #[arg(long)]
    pub list_rows: bool,
}

/// One database's share of the work.
#[derive(Debug, Clone)]
pub struct DbPlan {
    pub label: &'static str,
    pub db_path: PathBuf,
    /// `(fixture string, rows matching it exactly)`, in the order of
    /// [`FIXTURE_CONTENTS`].
    pub per_string: Vec<(&'static str, i64)>,
    /// Every row in `entries`, so the report can state what fraction of the
    /// corpus is junk and the apply step can prove it deleted nothing else.
    pub entries_total: i64,
    /// Pending cloud-sync rows pointing at the doomed entries. Left behind,
    /// these are orphans that would push a deleted fixture to the cloud.
    pub sync_queue_rows: i64,
}

impl DbPlan {
    pub fn fixture_total(&self) -> i64 {
        self.per_string.iter().map(|(_, count)| count).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.fixture_total() == 0 && self.sync_queue_rows == 0
    }
}

/// Count the fixture rows in one database without opening it for writing.
///
/// Read-only at the SQLite level (not merely by convention) so a dry run
/// cannot mutate the database it is reporting on, even through an incidental
/// schema migration.
pub fn plan_db(label: &'static str, db_path: &Path) -> anyhow::Result<DbPlan> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| anyhow::anyhow!("open {} read-only: {e}", db_path.display()))?;

    let mut per_string = Vec::with_capacity(FIXTURE_CONTENTS.len());
    for content in FIXTURE_CONTENTS {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM entries WHERE content = ?1",
            [content],
            |row| row.get(0),
        )?;
        per_string.push((content, count));
    }

    let entries_total: i64 = conn.query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))?;

    let sync_queue_rows = count_sync_queue(&conn)?;

    Ok(DbPlan {
        label,
        db_path: db_path.to_path_buf(),
        per_string,
        entries_total,
        sync_queue_rows,
    })
}

/// Count `sync_queue` rows whose entity is one of the fixture entries.
///
/// Returns 0 when the table does not exist: `sync_queue` arrived in a later
/// migration and an older database is a legitimate target for this purge.
fn count_sync_queue(conn: &Connection) -> anyhow::Result<i64> {
    if !table_exists(conn, "sync_queue")? {
        return Ok(0);
    }
    let placeholders = placeholders(FIXTURE_CONTENTS.len());
    let sql = format!(
        "SELECT COUNT(*) FROM sync_queue WHERE entity_id IN \
         (SELECT id FROM entries WHERE content IN ({placeholders}))"
    );
    Ok(conn.query_row(&sql, params_from_iter(FIXTURE_CONTENTS.iter()), |row| {
        row.get(0)
    })?)
}

/// The exact rows a purge of `db_path` would delete, as
/// `(id, content)` ordered by id.
///
/// Read-only, and computed from the same exact-equality predicate the delete
/// uses, so the manifest cannot describe a different set than the one removed.
pub fn delete_set(db_path: &Path) -> anyhow::Result<Vec<(String, String)>> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let placeholders = placeholders(FIXTURE_CONTENTS.len());
    let sql = format!(
        "SELECT id, content FROM entries WHERE content IN ({placeholders}) ORDER BY id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params_from_iter(FIXTURE_CONTENTS.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Render the delete set as TSV, one row per line, with a header.
fn render_delete_set(plan: &DbPlan, rows: &[(String, String)]) -> String {
    let mut out = String::from("db\tid\tcontent\n");
    let db = plan.db_path.display().to_string();
    for (id, content) in rows {
        out.push_str(&format!("{db}\t{id}\t{content}\n"));
    }
    out
}

fn table_exists(conn: &Connection, name: &str) -> anyhow::Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn placeholders(n: usize) -> String {
    (1..=n)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What one `--apply` actually did.
#[derive(Debug, Clone)]
pub struct PurgeOutcome {
    pub backup_path: PathBuf,
    pub entries_deleted: i64,
    pub sync_queue_deleted: i64,
    pub entries_before: i64,
    pub entries_after: i64,
}

/// Snapshot, delete, then verify.
///
/// The verification is not decoration. This command deletes from a live
/// personal memory store, and "the delete ran without error" is a weaker claim
/// than "exactly the planned rows are gone and every other row survived" —
/// which is what the two post-conditions below assert before returning.
pub fn purge_db(plan: &DbPlan, backup_path: &Path) -> anyhow::Result<PurgeOutcome> {
    if backup_path.exists() {
        anyhow::bail!(
            "backup {} already exists — refusing to overwrite a previous snapshot. \
             Move it aside or pass --backup-dir.",
            backup_path.display()
        );
    }
    if let Some(parent) = backup_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let conn = Connection::open(&plan.db_path)
        .map_err(|e| anyhow::anyhow!("open {}: {e}", plan.db_path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(30))?;

    // `VACUUM INTO` writes a complete, self-consistent copy of the database as
    // of one read transaction. Unlike a file copy it is safe while WAL writes
    // are in flight, and unlike `.dump` it produces a file the purge can be
    // rolled back from with a single `mv`.
    conn.execute("VACUUM INTO ?1", [backup_path.to_string_lossy().as_ref()])
        .map_err(|e| {
            anyhow::anyhow!(
                "backup {} -> {} failed, nothing was deleted: {e}",
                plan.db_path.display(),
                backup_path.display()
            )
        })?;

    let entries_before: i64 =
        conn.query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))?;

    let placeholders = placeholders(FIXTURE_CONTENTS.len());
    let tx = conn.unchecked_transaction()?;

    // Order matters: the sync_queue rows are located through `entries`, so they
    // must be deleted while their entries still exist.
    let sync_queue_deleted = if table_exists(&tx, "sync_queue")? {
        let sql = format!(
            "DELETE FROM sync_queue WHERE entity_id IN \
             (SELECT id FROM entries WHERE content IN ({placeholders}))"
        );
        tx.execute(&sql, params_from_iter(FIXTURE_CONTENTS.iter()))? as i64
    } else {
        0
    };

    let sql = format!("DELETE FROM entries WHERE content IN ({placeholders})");
    let entries_deleted = tx.execute(&sql, params_from_iter(FIXTURE_CONTENTS.iter()))? as i64;

    let remaining: i64 = tx.query_row(
        &format!("SELECT COUNT(*) FROM entries WHERE content IN ({placeholders})"),
        params_from_iter(FIXTURE_CONTENTS.iter()),
        |row| row.get(0),
    )?;
    let entries_after: i64 = tx.query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))?;

    if remaining != 0 {
        anyhow::bail!(
            "{} fixture row(s) still present after the delete in {} — rolled back",
            remaining,
            plan.db_path.display()
        );
    }
    if entries_after != entries_before - entries_deleted {
        anyhow::bail!(
            "{} lost rows the purge did not delete: {entries_before} -> {entries_after} while \
             deleting {entries_deleted} — rolled back",
            plan.db_path.display()
        );
    }

    tx.commit()?;

    Ok(PurgeOutcome {
        backup_path: backup_path.to_path_buf(),
        entries_deleted,
        sync_queue_deleted,
        entries_before,
        entries_after,
    })
}

/// Render one database's plan as the per-string table the operator reviews
/// before authorizing `--apply`.
pub fn render_plan(plan: &DbPlan) -> String {
    let mut out = format!(
        "{} {}\n  {} entr{} total\n",
        plan.label,
        plan.db_path.display(),
        plan.entries_total,
        if plan.entries_total == 1 { "y" } else { "ies" }
    );
    for (content, count) in &plan.per_string {
        out.push_str(&format!("  {count:>6}  {content}\n"));
    }
    let total = plan.fixture_total();
    let percent = if plan.entries_total > 0 {
        (total as f64 / plan.entries_total as f64) * 100.0
    } else {
        0.0
    };
    out.push_str(&format!(
        "  {total:>6}  TOTAL fixture rows ({percent:.1}% of this database)\n"
    ));
    if plan.sync_queue_rows > 0 {
        out.push_str(&format!(
            "  {:>6}  pending sync_queue row(s) referencing them (also deleted)\n",
            plan.sync_queue_rows
        ));
    }
    out
}

/// Default snapshot location for `db_path`, honouring `--backup-dir`.
///
/// `label` prefixes the name under `--backup-dir` because every Cassy database
/// is called `cas.db`: without it the global snapshot and the project snapshot
/// collide in a shared directory, and the second purge would abort on the
/// "backup already exists" rail after the first had already deleted.
fn backup_path_for(label: &str, db_path: &Path, backup_dir: Option<&Path>) -> PathBuf {
    let file_name = db_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "cas.db".to_string());
    let backup_name = format!("{file_name}.fixture-purge-backup");
    match backup_dir {
        Some(dir) => dir.join(format!("{label}-{backup_name}")),
        None => db_path.with_file_name(backup_name),
    }
}

/// A database this invocation operates on: `(label, path)`.
type Target = (&'static str, PathBuf);

/// Resolve which databases this invocation reads.
///
/// Mirrors `memory_migrate::resolve_sources`, including its rail: a redirected
/// project root means "rehearsing against a copy", so silently defaulting the
/// global side to the live `~/.cas` would delete from live data.
fn resolve_targets(
    args: &PurgeFixturesArgs,
    cas_root: &Path,
    home_dir: Option<PathBuf>,
) -> anyhow::Result<(Vec<Target>, Vec<String>)> {
    let project_root = args
        .project_root
        .clone()
        .unwrap_or_else(|| cas_root.to_path_buf());
    let mut targets: Vec<Target> = Vec::new();
    let mut notes = Vec::new();

    if args.scope != "global" {
        let db = project_root.join("cas.db");
        if db.is_file() {
            targets.push(("project", db));
        } else {
            notes.push(format!("(no project database at {} — skipping)", db.display()));
        }
    }

    if args.scope != "project" {
        if args.project_root.is_some() && args.global_root.is_none() {
            anyhow::bail!(
                "--project-root redirects the project side to {}, but the global side would \
                 still default to ~/.cas — the LIVE global database. Pass --global-root <copy> \
                 as well, or --scope project.",
                project_root.display()
            );
        }
        let global_root = args
            .global_root
            .clone()
            .or_else(|| home_dir.map(|home| home.join(".cas")));
        match global_root {
            // Compared against `project_root` so a project that *is* ~/.cas is
            // not purged twice under two labels.
            Some(root) if root != project_root && root.join("cas.db").is_file() => {
                targets.push(("global", root.join("cas.db")));
            }
            Some(root) => notes.push(format!(
                "(no separate global database at {} — skipping)",
                root.display()
            )),
            None => notes
                .push("(cannot resolve a home directory — skipping the global database)".into()),
        }
    }

    if targets.is_empty() {
        anyhow::bail!("no database to purge (scope = {})", args.scope);
    }

    Ok((targets, notes))
}

pub fn execute(args: &PurgeFixturesArgs, cas_root: &Path) -> anyhow::Result<()> {
    let (targets, notes) = resolve_targets(args, cas_root, dirs::home_dir())?;
    for note in &notes {
        println!("{note}");
    }

    println!(
        "{} — matching {} fixture string(s) by EXACT equality (never LIKE)\n",
        if args.apply { "APPLY" } else { "DRY RUN" },
        FIXTURE_CONTENTS.len()
    );

    let mut plans = Vec::new();
    for (label, db_path) in &targets {
        plans.push(plan_db(label, db_path)?);
    }

    for plan in &plans {
        print!("{}", render_plan(plan));
        println!();
    }

    if args.list_rows {
        for plan in &plans {
            print!("{}", render_delete_set(plan, &delete_set(&plan.db_path)?));
        }
        println!();
    }

    let grand_total: i64 = plans.iter().map(DbPlan::fixture_total).sum();
    println!("{grand_total} fixture row(s) across {} database(s)", plans.len());

    if !args.apply {
        println!();
        println!("DRY RUN — nothing was deleted. Re-run with --apply to execute.");
        return Ok(());
    }

    if plans.iter().all(DbPlan::is_empty) {
        println!();
        println!("Nothing to delete — no backup was taken.");
        return Ok(());
    }

    println!();
    for plan in &plans {
        if plan.is_empty() {
            println!("{}: already clean, skipped", plan.db_path.display());
            continue;
        }
        let backup = backup_path_for(plan.label, &plan.db_path, args.backup_dir.as_deref());

        // Written before a single row is deleted: if the process dies mid-purge
        // the manifest still names every row that was in scope, so the backup
        // can be reconciled against it rather than trusted blindly.
        let manifest_path = backup.with_extension("manifest.tsv");
        if let Some(parent) = manifest_path.parent() {
            // `--backup-dir` may name a directory that does not exist yet, and
            // the manifest is written before `purge_db` creates it.
            std::fs::create_dir_all(parent)?;
        }
        let rows = delete_set(&plan.db_path)?;
        std::fs::write(&manifest_path, render_delete_set(plan, &rows))
            .map_err(|e| anyhow::anyhow!("write manifest {}: {e}", manifest_path.display()))?;

        let outcome = purge_db(plan, &backup)?;
        println!(
            "{}: deleted {} entr{} ({} -> {}) and {} sync_queue row(s)\n  backup: {}",
            plan.db_path.display(),
            outcome.entries_deleted,
            if outcome.entries_deleted == 1 {
                "y"
            } else {
                "ies"
            },
            outcome.entries_before,
            outcome.entries_after,
            outcome.sync_queue_deleted,
            outcome.backup_path.display()
        );
        println!("  manifest: {}", manifest_path.display());
    }

    println!();
    println!(
        "Backups are complete SQLite databases — restore one with \
         `mv <backup> <cas.db>` if anything looks wrong."
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a store with `real` genuine rows plus one row per fixture string,
    /// plus a decoy that *contains* a fixture string without equalling it.
    fn seed(db_path: &Path, real: usize) {
        let conn = Connection::open(db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE entries (id TEXT PRIMARY KEY, content TEXT NOT NULL, created TEXT NOT NULL);
             CREATE TABLE sync_queue (id INTEGER PRIMARY KEY, entity_type TEXT, entity_id TEXT);",
        )
        .unwrap();
        for i in 0..real {
            conn.execute(
                "INSERT INTO entries (id, content, created) VALUES (?1, ?2, '2026-01-01')",
                (format!("real-{i}"), format!("a genuine memory number {i}")),
            )
            .unwrap();
        }
        for (i, content) in FIXTURE_CONTENTS.iter().enumerate() {
            conn.execute(
                "INSERT INTO entries (id, content, created) VALUES (?1, ?2, '2026-01-01')",
                (format!("fixture-{i}"), *content),
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sync_queue (entity_type, entity_id) VALUES ('entry', ?1)",
                [format!("fixture-{i}")],
            )
            .unwrap();
        }
        // The row that a LIKE-based purge would destroy: a real memory that
        // quotes a fixture string while documenting this very bug.
        conn.execute(
            "INSERT INTO entries (id, content, created) VALUES ('quote', ?1, '2026-01-01')",
            [format!(
                "the suite leaked rows whose content is exactly \"{}\" into the real store",
                FIXTURE_CONTENTS[0]
            )],
        )
        .unwrap();
    }

    #[test]
    fn plan_counts_each_fixture_string_exactly_once() {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("cas.db");
        seed(&db, 3);

        let plan = plan_db("test", &db).unwrap();

        assert_eq!(plan.per_string.len(), FIXTURE_CONTENTS.len());
        for (content, count) in &plan.per_string {
            assert_eq!(*count, 1, "unexpected count for {content}");
        }
        assert_eq!(plan.fixture_total(), FIXTURE_CONTENTS.len() as i64);
        // 3 genuine + 5 fixture + 1 quoting decoy
        assert_eq!(plan.entries_total, 9);
        assert_eq!(plan.sync_queue_rows, FIXTURE_CONTENTS.len() as i64);
    }

    #[test]
    fn dry_run_plan_does_not_mutate_the_database() {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("cas.db");
        seed(&db, 3);

        let before = plan_db("test", &db).unwrap();
        let after = plan_db("test", &db).unwrap();

        assert_eq!(before.entries_total, after.entries_total);
        assert_eq!(before.fixture_total(), after.fixture_total());
    }

    #[test]
    fn apply_deletes_only_exact_matches_and_keeps_the_quoting_row() {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("cas.db");
        seed(&db, 3);

        let plan = plan_db("test", &db).unwrap();
        let backup = temp.path().join("cas.db.backup");
        let outcome = purge_db(&plan, &backup).unwrap();

        assert_eq!(outcome.entries_deleted, FIXTURE_CONTENTS.len() as i64);
        assert_eq!(outcome.sync_queue_deleted, FIXTURE_CONTENTS.len() as i64);

        let after = plan_db("test", &db).unwrap();
        assert_eq!(after.fixture_total(), 0);
        assert_eq!(after.sync_queue_rows, 0);
        // 3 genuine + the decoy that merely quotes a fixture string.
        assert_eq!(after.entries_total, 4);

        let conn = Connection::open(&db).unwrap();
        let quoted: i64 = conn
            .query_row("SELECT COUNT(*) FROM entries WHERE id = 'quote'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(quoted, 1, "a LIKE-based purge would have eaten this row");
    }

    #[test]
    fn backup_is_a_complete_restorable_database() {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("cas.db");
        seed(&db, 3);

        let plan = plan_db("test", &db).unwrap();
        let backup = temp.path().join("cas.db.backup");
        purge_db(&plan, &backup).unwrap();

        // The snapshot still has every row the live database just lost.
        let restored = plan_db("test", &backup).unwrap();
        assert_eq!(restored.entries_total, 9);
        assert_eq!(restored.fixture_total(), FIXTURE_CONTENTS.len() as i64);
    }

    #[test]
    fn apply_refuses_to_overwrite_an_existing_backup() {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("cas.db");
        seed(&db, 3);
        let backup = temp.path().join("cas.db.backup");
        std::fs::write(&backup, b"previous snapshot").unwrap();

        let plan = plan_db("test", &db).unwrap();
        let err = purge_db(&plan, &backup).unwrap_err().to_string();
        assert!(err.contains("already exists"), "unexpected error: {err}");

        // And the refusal was total: nothing was deleted.
        assert_eq!(
            plan_db("test", &db).unwrap().fixture_total(),
            FIXTURE_CONTENTS.len() as i64
        );
    }

    #[test]
    fn the_delete_set_names_exactly_the_rows_the_purge_removes() {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("cas.db");
        seed(&db, 3);

        let rows = delete_set(&db).unwrap();
        assert_eq!(rows.len(), FIXTURE_CONTENTS.len());
        // The manifest is the receipt an operator reconciles the backup
        // against, so it must not claim a row the delete leaves behind.
        assert!(
            rows.iter().all(|(id, _)| id.starts_with("fixture-")),
            "delete set named a non-fixture row: {rows:?}"
        );

        let plan = plan_db("test", &db).unwrap();
        purge_db(&plan, &temp.path().join("b")).unwrap();
        assert!(delete_set(&db).unwrap().is_empty());
    }

    #[test]
    fn the_rendered_delete_set_is_tsv_with_one_line_per_row() {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("cas.db");
        seed(&db, 3);

        let plan = plan_db("test", &db).unwrap();
        let rendered = render_delete_set(&plan, &delete_set(&db).unwrap());
        let lines: Vec<&str> = rendered.lines().collect();

        assert_eq!(lines[0], "db\tid\tcontent");
        assert_eq!(lines.len(), FIXTURE_CONTENTS.len() + 1);
        assert!(lines[1..].iter().all(|line| line.matches('\t').count() == 2));
    }

    #[test]
    fn purge_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("cas.db");
        seed(&db, 3);

        let plan = plan_db("test", &db).unwrap();
        purge_db(&plan, &temp.path().join("first.backup")).unwrap();

        let second = plan_db("test", &db).unwrap();
        assert!(second.is_empty());
        let outcome = purge_db(&second, &temp.path().join("second.backup")).unwrap();
        assert_eq!(outcome.entries_deleted, 0);
        assert_eq!(outcome.entries_after, 4);
    }

    #[test]
    fn missing_sync_queue_table_is_not_an_error() {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("cas.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE entries (id TEXT PRIMARY KEY, content TEXT NOT NULL, created TEXT NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO entries VALUES ('f', ?1, '2026-01-01')",
            [FIXTURE_CONTENTS[0]],
        )
        .unwrap();
        drop(conn);

        let plan = plan_db("test", &db).unwrap();
        assert_eq!(plan.sync_queue_rows, 0);
        let outcome = purge_db(&plan, &temp.path().join("b")).unwrap();
        assert_eq!(outcome.entries_deleted, 1);
        assert_eq!(outcome.sync_queue_deleted, 0);
    }

    #[test]
    fn redirected_project_root_refuses_to_default_the_global_side_to_live() {
        let temp = TempDir::new().unwrap();
        let copy_root = temp.path().join("copy");
        std::fs::create_dir_all(&copy_root).unwrap();

        let args = PurgeFixturesArgs {
            apply: true,
            scope: "both".into(),
            project_root: Some(copy_root),
            global_root: None,
            backup_dir: None,
            list_rows: false,
        };
        let err = resolve_targets(&args, temp.path(), Some(temp.path().to_path_buf()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("--global-root"), "unexpected error: {err}");
    }

    #[test]
    fn default_backup_sits_beside_the_database() {
        let path = backup_path_for("project", Path::new("/tmp/proj/.cas/cas.db"), None);
        assert_eq!(
            path,
            PathBuf::from("/tmp/proj/.cas/cas.db.fixture-purge-backup")
        );
    }

    #[test]
    fn backup_dir_disambiguates_two_databases_with_the_same_file_name() {
        let dir = Path::new("/tmp/backups");
        // Every Cassy database is called `cas.db`, and both of these live in a
        // directory called `.cas` — only the label tells them apart.
        let a = backup_path_for("global", Path::new("/home/u/.cas/cas.db"), Some(dir));
        let b = backup_path_for("project", Path::new("/home/u/proj/.cas/cas.db"), Some(dir));
        assert_ne!(a, b);
        assert!(a.starts_with(dir) && b.starts_with(dir));
    }
}
