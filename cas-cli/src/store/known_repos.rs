//! Host-scoped repo registry helpers.
//!
//! Thin glue between callers (`cas init`, factory daemon startup, MCP server
//! startup, the `cas known-repos ...` subcommand) and [`SqliteKnownRepoStore`]
//! in `cas-store`. Resolves the host `~/.cas/` directory and exposes
//! non-fatal `register_repo` + a fallible bootstrap that callers on the
//! init path can invoke once to install the schema via the migration
//! machinery.
//!
//! **Schema install is single-site.** Only [`ensure_host_schema`] runs
//! DDL, and it records the m199 migration in `cas_migrations` so the
//! runner stays in sync. Hot-path callers (factory daemon boot, MCP serve
//! boot) open the store without DDL; if the host was never `cas init`'d,
//! the upsert fails silently and the registry stays empty — intended,
//! because a host that has never run `cas init` has nothing to sweep.
//!
//! **Why `dirs::home_dir().join(".cas")` instead of `global_cas_dir()`:**
//! the latter resolves to `~/.config/cas` on Linux / `Application Support`
//! on macOS, which is **not** where the live host CAS state actually lives
//! (sessions, logs, the factory sockets — all under `~/.cas`). Spike A
//! deferred reconciling that inconsistency; this module picks the de-facto
//! root per `ui/factory/session.rs:22-26`.

use std::path::{Path, PathBuf};

use rusqlite::params;
use tracing::{debug, warn};

use crate::migration::ensure_migrations_table;
use crate::migration::migrations::{m199_known_repos, m214_known_repo_bindings};
use crate::store::{KnownRepoStore, SqliteKnownRepoStore};

/// Resolve the host-level `~/.cas/` directory. Falls back to `.cas/` under
/// the current directory if the user's home directory cannot be determined,
/// which should only happen in severely sandboxed test environments.
pub fn host_cas_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    if let Some(protected_home) = std::env::var_os("CAS_TEST_PROTECTED_HOME")
        && home == PathBuf::from(&protected_home)
    {
        panic!(
            "test subprocess resolved the protected host HOME at {}; configure the spawned cas command with an isolated HOME",
            home.display()
        );
    }
    home.join(".cas")
}

/// Install the known-repo registry and host-local binding schemas on
/// `~/.cas/cas.db`, recording both migrations so the normal runner does not
/// see them as pending on the next run.
///
/// This is the **only** code path that issues DDL against the host DB. Safe to
/// call multiple times — both migrations use idempotent table/index creation,
/// and already-recorded migration IDs are skipped.
///
/// Intended callers: `cas init` (once per repo, via `init_cas_dir`) and
/// the `cas known-repos` subcommand itself. Hot startup paths (MCP serve,
/// factory daemon boot) MUST NOT call this — those only upsert.
pub fn ensure_host_schema() -> anyhow::Result<()> {
    let cas_dir = host_cas_dir();
    std::fs::create_dir_all(&cas_dir)?;
    let db_path = cas_dir.join("cas.db");
    let conn = cas_store::shared_db::shared_connection(&db_path)?;
    let conn = conn.lock().unwrap_or_else(|p| p.into_inner());

    ensure_migrations_table(&conn)?;

    for migration in [
        &m199_known_repos::MIGRATION,
        &m214_known_repo_bindings::MIGRATION,
    ] {
        let already_recorded: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM cas_migrations WHERE id = ?1",
                params![migration.id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if already_recorded > 0 {
            continue;
        }

        // Detect whether the table already exists (legacy installs created
        // m199 directly before this helper existed). If so, backfill the
        // migration record; otherwise apply the migration properly.
        let detect_query = migration
            .detect
            .expect("host known-repo migrations must have detect queries");
        let schema_exists: i64 = conn
            .query_row(detect_query, [], |row| row.get(0))
            .unwrap_or(0);
        if schema_exists == 0 {
            for sql in migration.up {
                conn.execute(sql, [])?;
            }
        }

        let ts = if schema_exists > 0 {
            "BOOTSTRAP".to_string()
        } else {
            chrono::Utc::now().to_rfc3339()
        };
        conn.execute(
            "INSERT OR IGNORE INTO cas_migrations (id, name, subsystem, applied_at) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                migration.id,
                migration.name,
                migration.subsystem.as_str(),
                ts
            ],
        )?;
    }
    Ok(())
}

/// Open the host-scoped [`SqliteKnownRepoStore`] **without** running DDL.
///
/// Intended for hot startup paths and read-only callers. On a host that has
/// never been `cas init`'d, the `known_repos` table will be absent and any
/// subsequent `upsert`/`list` call will fail. Callers use the non-fatal
/// [`register_repo`] wrapper which swallows those errors; strict callers
/// (the `cas known-repos` subcommand) run [`ensure_host_schema`] first.
pub fn open_host_known_repo_store() -> anyhow::Result<SqliteKnownRepoStore> {
    let cas_dir = host_cas_dir();
    // Create dir even for the no-DDL path so the DB file has a place to
    // live the first time a factory worker tries to register. If the dir
    // already exists this is a no-op.
    std::fs::create_dir_all(&cas_dir)?;
    let store = SqliteKnownRepoStore::open(&cas_dir)?;
    Ok(store)
}

/// Register `repo_path` in the host registry.
///
/// **Non-fatal by design.** Every known call site is a best-effort upsert on
/// a startup hot path (`cas init`, factory daemon boot, MCP server boot);
/// losing the upsert must not break the primary operation. Failures are
/// logged at `warn!` and swallowed. If callers need a fatal variant, use
/// [`open_host_known_repo_store`] + [`KnownRepoStore::upsert`] directly.
pub fn register_repo(repo_path: &Path) {
    if let Err(e) = register_repo_strict(repo_path) {
        warn!(
            path = %repo_path.display(),
            error = %e,
            "failed to register repo in host known_repos registry (non-fatal)",
        );
    } else {
        debug!(path = %repo_path.display(), "registered repo in host known_repos");
    }
}

/// Fallible variant of [`register_repo`]. Use this when you actually want
/// to propagate the error (e.g. a CLI `cas known-repos add` explicitly run
/// by the user). Note: does NOT install schema — run
/// [`ensure_host_schema`] first if the caller is the bootstrap site.
pub fn register_repo_strict(repo_path: &Path) -> anyhow::Result<()> {
    let store = open_host_known_repo_store()?;
    store.upsert(repo_path)?;
    Ok(())
}

/// Describe a host-registry failure as an infrastructure problem, including
/// bounded local process evidence when SQLite reports lock contention.
pub fn host_registry_write_error(repo_path: &Path, error: &anyhow::Error) -> String {
    let error_text = error.to_string();
    let locked = error_text.contains("database is locked")
        || error_text.contains("database table is locked");
    let db_path = host_cas_dir().join("cas.db");
    let holders = locked
        .then(|| open_holder_pids(&db_path))
        .unwrap_or_default();
    let holder_text = if holders.is_empty() {
        "holding PID could not be identified".to_string()
    } else {
        format!(
            "holding PID{}: {}",
            if holders.len() == 1 { "" } else { "s" },
            holders
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    if locked {
        format!(
            "HOST REGISTRY UNAVAILABLE: failed to register {} in {}: {error_text}; {holder_text}. Inspect with `fuser -v {db}` and `ps -o pid,ppid,stat,etime,wchan:20,cmd -p <PID>`; stop the owning worker or send SIGTERM, then SIGKILL only if the orphan remains wedged, and retry.",
            repo_path.display(),
            db_path.display(),
            db = db_path.display(),
        )
    } else {
        format!(
            "HOST REGISTRY UNAVAILABLE: failed to register {} in {}: {error_text}. Check ownership and writability of the host registry, then retry.",
            repo_path.display(),
            db_path.display(),
        )
    }
}

#[cfg(target_os = "linux")]
fn open_holder_pids(db_path: &Path) -> Vec<u32> {
    let db_path = db_path
        .canonicalize()
        .unwrap_or_else(|_| db_path.to_path_buf());
    let wal_path = PathBuf::from(format!("{}-wal", db_path.display()));
    let shm_path = PathBuf::from(format!("{}-shm", db_path.display()));
    let Ok(processes) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut holders = Vec::new();
    for process in processes.flatten() {
        let Some(pid) = process
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(process.path().join("fd")) else {
            continue;
        };
        let holds_registry = fds.flatten().any(|fd| {
            std::fs::read_link(fd.path())
                .is_ok_and(|target| target == db_path || target == wal_path || target == shm_path)
        });
        if holds_registry {
            holders.push(pid);
        }
    }
    holders.sort_unstable();
    holders.dedup();
    holders
}

#[cfg(not(target_os = "linux"))]
fn open_holder_pids(_db_path: &Path) -> Vec<u32> {
    Vec::new()
}

/// PIDs with the host registry database, WAL, or shared-memory file open.
/// Used by `gc_report` to surface CAS children that survived their worker.
pub(crate) fn host_registry_open_pids() -> Vec<u32> {
    open_holder_pids(&host_cas_dir().join("cas.db"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;

    #[test]
    fn host_cas_dir_follows_home() {
        TestEnvGuard::run_with_temp_home(|home| {
            let resolved = host_cas_dir();
            assert_eq!(resolved, home.join(".cas"));
        });
    }

    #[test]
    #[should_panic(expected = "test subprocess resolved the protected host HOME")]
    fn host_cas_dir_guard_rejects_protected_test_home() {
        let mut guard = TestEnvGuard::temp_home();
        let protected_home = guard.home().to_path_buf();
        guard.set("CAS_TEST_PROTECTED_HOME", &protected_home);

        let _ = host_cas_dir();
    }

    #[test]
    fn register_repo_strict_creates_host_dir_and_inserts() {
        TestEnvGuard::run_with_temp_home(|home| {
            let repo = home.join("myproject");
            std::fs::create_dir_all(&repo).unwrap();

            // Bootstrap schema first — mirrors the `cas init` contract.
            ensure_host_schema().unwrap();
            register_repo_strict(&repo).unwrap();

            let store = open_host_known_repo_store().unwrap();
            let list = store.list().unwrap();
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].path, repo.canonicalize().unwrap());
            assert!(home.join(".cas/cas.db").exists());
        });
    }

    #[test]
    fn register_repo_is_non_fatal_on_missing_schema() {
        // Pre-schema: register_repo must NOT panic and must NOT abort;
        // the warn-and-swallow contract is what the hot boot path depends on.
        TestEnvGuard::run_with_temp_home(|home| {
            let repo = home.join("pre-init-repo");
            std::fs::create_dir_all(&repo).unwrap();
            // Schema intentionally not installed.
            register_repo(&repo); // expect no panic, no abort
        });
    }

    #[test]
    fn locked_registry_error_names_holder_pid_and_is_not_input_rejection() {
        TestEnvGuard::run_with_temp_home(|home| {
            ensure_host_schema().unwrap();
            let db_path = home.join(".cas/cas.db");
            let _open_holder = rusqlite::Connection::open(&db_path).unwrap();
            let repo = home.join("repo");
            let message = host_registry_write_error(
                &repo,
                &anyhow::anyhow!("database error: database is locked"),
            );

            assert!(message.starts_with("HOST REGISTRY UNAVAILABLE:"));
            assert!(!message.contains("WORK TARGET REJECTED"));
            assert!(message.contains(&format!("holding PID: {}", std::process::id())));
            assert!(message.contains("fuser -v"));
            assert!(message.contains("SIGTERM"));
            assert!(message.contains("SIGKILL"));
        });
    }

    #[test]
    fn ensure_host_schema_records_migration_and_is_idempotent() {
        TestEnvGuard::run_with_temp_home(|home| {
            ensure_host_schema().unwrap();
            // m199 row must be present.
            let db = home.join(".cas/cas.db");
            let conn = rusqlite::Connection::open(&db).unwrap();
            let id: i64 = conn
                .query_row("SELECT id FROM cas_migrations WHERE id = 199", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(id, 199);
            let binding_id: i64 = conn
                .query_row("SELECT id FROM cas_migrations WHERE id = 214", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(binding_id, 214);

            // Running twice must not double-insert.
            ensure_host_schema().unwrap();
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM cas_migrations WHERE id = 199",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "ensure_host_schema must be idempotent");
        });
    }

    #[test]
    fn ensure_host_schema_backfills_when_table_preexists() {
        // Simulates a host that installed under the pre-fix code which
        // created the table via raw DDL without the migrations row.
        TestEnvGuard::run_with_temp_home(|home| {
            let cas_dir = home.join(".cas");
            std::fs::create_dir_all(&cas_dir).unwrap();
            let db = cas_dir.join("cas.db");
            let conn = rusqlite::Connection::open(&db).unwrap();
            for sql in m199_known_repos::MIGRATION.up {
                conn.execute(sql, []).unwrap();
            }
            drop(conn);
            // Now install the schema via the migration-aware path — it
            // must see the table, NOT re-run the DDL, AND record the row.
            ensure_host_schema().unwrap();
            let conn = rusqlite::Connection::open(&db).unwrap();
            let applied_at: String = conn
                .query_row(
                    "SELECT applied_at FROM cas_migrations WHERE id = 199",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(applied_at, "BOOTSTRAP");
        });
    }
}
