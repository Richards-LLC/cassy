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
//! on macOS, which is **not** where the live host Cassy state actually lives
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

/// Why a path is disposable by construction for host-wide discovery.
///
/// These paths may still be registered because local startup and explicit
/// bindings need a complete host registry. Update discovery and cloud-facing
/// sweeps apply this classification before acting on registered roots.
///
/// ROOT CAUSE this exists for (cas-647c): every registry row is treated as a
/// live host project by the sweep and doctor surfaces. A closed mecha-cassy
/// task copied a whole CAS root to `~/.cas/artifacts/cas-1bfb/fresh-proxy` as
/// an isolated proxy-health fixture and ran `cas serve` inside it, which
/// auto-registered the copy. `cas doctor` then opened the fixture's 10-table
/// database, found no `tasks`, and reported a project DB that "could NOT be
/// read" — an amber row with no command that cleared it, because the path
/// still existed so `prune-missing` would not remove it.
///
/// All three classes below are normally *copies* of a project created and
/// abandoned by tooling, not checkouts an operator works in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrySkip {
    /// Under the resolved `[factory] artifacts_root`.
    Artifacts(PathBuf),
    /// Under `~/.cas/scratch`, the sanctioned home for disposable roots.
    Scratch(PathBuf),
    /// Any initialized project root below a system temporary directory.
    Temp(PathBuf),
    /// A `.cas` below another registered project's durable artifact tree.
    NestedArtifacts {
        project: PathBuf,
        artifacts: PathBuf,
    },
    /// An initialized store whose identity would be derived only from its
    /// containing folder. Such roots are usually container directories or
    /// copied fixtures, not operator-owned projects.
    BareFolderIdentity(PathBuf),
}

impl RegistrySkip {
    /// The disposable root the path was matched against.
    pub fn base(&self) -> &Path {
        match self {
            Self::Artifacts(base) | Self::Scratch(base) | Self::Temp(base) => base,
            Self::NestedArtifacts { artifacts, .. } => artifacts,
            Self::BareFolderIdentity(path) => path,
        }
    }

    /// Operator-readable justification, always naming the base it matched so
    /// the decision can be checked rather than trusted.
    pub fn reason(&self) -> String {
        match self {
            Self::Artifacts(base) => format!(
                "below the configured [factory] artifacts_root {}",
                base.display()
            ),
            Self::Scratch(base) => format!("below the scratch root {}", base.display()),
            Self::Temp(base) => format!("a disposable temp root at {}", base.display()),
            Self::NestedArtifacts { project, artifacts } => format!(
                "below registered project {}'s artifacts directory {}",
                project.display(),
                artifacts.display()
            ),
            Self::BareFolderIdentity(path) => format!(
                "{} has no [project] canonical_id pin and no git origin remote, so its cloud identity would be a bare folder name",
                path.display()
            ),
        }
    }
}

/// Classify `repo_path` against the disposable roots. `None` means it is an
/// ordinary project checkout and may be registered.
///
/// Paths are compared in both their literal and canonical spellings, in both
/// directions, so a symlinked platform temp dir (macOS `/var` ->
/// `/private/var`) and a not-yet-created path both classify correctly.
pub fn registry_skip(repo_path: &Path) -> Option<RegistrySkip> {
    let candidates = path_spellings(repo_path);

    for root in artifacts_roots(repo_path) {
        if let Some(base) = matching_base(&candidates, &root) {
            return Some(RegistrySkip::Artifacts(base));
        }
    }
    if let Some(base) = matching_base(&candidates, &crate::temp_hygiene::scratch_root_base()) {
        return Some(RegistrySkip::Scratch(base));
    }
    if let Some(skip) = temp_root(&candidates) {
        return Some(skip);
    }
    let cas_root = repo_path.join(".cas");
    if cas_root.is_dir()
        && crate::cloud::canonical_id_from_config_toml(&cas_root).is_none()
        && crate::cloud::git_origin_url(&cas_root).is_none()
    {
        return Some(RegistrySkip::BareFolderIdentity(repo_path.to_path_buf()));
    }
    None
}

/// Classify a registry path with the additional rule that an initialized
/// `.cas` below another registered project's artifact directory is not itself
/// a project. The parent list is supplied by the caller so this remains
/// deterministic and does not recursively read the host registry.
pub fn registry_skip_for_known_roots(
    repo_path: &Path,
    known_roots: &[PathBuf],
) -> Option<RegistrySkip> {
    if let Some(skip) = registry_skip(repo_path) {
        return Some(skip);
    }

    let candidate = path_spellings(repo_path);
    for project in known_roots {
        let project = canonical_path(project);
        let artifacts = project.join(".cas/artifacts");
        if candidate.iter().any(|path| {
            path_spellings(&artifacts)
                .iter()
                .any(|base| path.starts_with(base) && path != base)
        }) {
            return Some(RegistrySkip::NestedArtifacts { project, artifacts });
        }
    }
    None
}

/// Every artifacts root this process could plausibly mean: the default
/// (`~/.cas/artifacts`), whatever the candidate's own `.cas/config.toml`
/// declares (an artifacts copy carries the config it was copied from), and
/// whatever the ambient project store declares. Registering is rare enough
/// that checking all three is cheaper than being wrong.
fn artifacts_roots(repo_path: &Path) -> Vec<PathBuf> {
    let mut configured: Vec<Option<String>> = vec![None];
    let mut candidate_roots = vec![repo_path.join(".cas")];
    if let Ok(ambient) = crate::store::detect::find_cas_root() {
        candidate_roots.push(ambient);
    }
    for cas_dir in candidate_roots {
        if let Ok(config) = crate::config::Config::load(&cas_dir) {
            let value = config.factory().artifacts_root;
            if value.is_some() && !configured.contains(&value) {
                configured.push(value);
            }
        }
    }
    let mut roots = Vec::new();
    for value in configured {
        let root = crate::config::resolved_factory_artifacts_root(value.as_deref());
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

/// Literal and canonical spellings of one path, deduplicated.
fn path_spellings(path: &Path) -> Vec<PathBuf> {
    let mut out = vec![path.to_path_buf()];
    if let Ok(canonical) = path.canonicalize()
        && canonical != out[0]
    {
        out.push(canonical);
    }
    out
}

/// The base, if any spelling of the candidate lies at or below any spelling of
/// it. Returns the base's literal spelling so the logged reason matches what
/// the operator configured.
fn matching_base(candidates: &[PathBuf], base: &Path) -> Option<PathBuf> {
    path_spellings(base)
        .into_iter()
        .any(|spelling| candidates.iter().any(|c| c.starts_with(&spelling)))
        .then(|| base.to_path_buf())
}

/// Any directory under the system temporary roots is disposable for host-wide
/// discovery. A temp checkout can still be used explicitly as the current
/// project, but it must never be swept merely because a scan found it.
fn temp_root(candidates: &[PathBuf]) -> Option<RegistrySkip> {
    for base in path_spellings(&std::env::temp_dir()) {
        for candidate in candidates {
            let Ok(rest) = candidate.strip_prefix(&base) else {
                continue;
            };
            if rest.components().next().is_some() {
                return Some(RegistrySkip::Temp(base.to_path_buf()));
            }
        }
    }
    // `std::env::temp_dir()` is normally `/tmp`; include the explicit macOS
    // spelling and `/var/tmp` because discovery is also used on remote hosts.
    for base in [
        Path::new("/tmp"),
        Path::new("/var/tmp"),
        Path::new("/private/tmp"),
        Path::new("/private/var/tmp"),
    ] {
        for candidate in candidates {
            if candidate.starts_with(base) && candidate != base {
                return Some(RegistrySkip::Temp(base.to_path_buf()));
            }
        }
    }
    None
}

fn canonical_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
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
/// Registration deliberately does not apply [`registry_skip`]: local startup
/// and explicit bindings must be able to read back every path they register.
/// Host-wide discovery applies the disposable-root policy when it sweeps this
/// registry instead.
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
/// Used by `gc_report` to surface Cassy children that survived their worker.
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
            let external =
                tempfile::tempdir_in(crate::test_paths::runtime_fixture_parent()).unwrap();
            let repo = external.path().join("myproject");
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
        TestEnvGuard::run_with_temp_home(|_| {
            let external =
                tempfile::tempdir_in(crate::test_paths::runtime_fixture_parent()).unwrap();
            let repo = external.path().join("pre-init-repo");
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
            #[cfg(target_os = "linux")]
            assert!(message.contains(&format!("holding PID: {}", std::process::id())));
            #[cfg(not(target_os = "linux"))]
            assert!(message.contains("holding PID could not be identified"));
            assert!(message.contains("fuser -v"));
            assert!(message.contains("SIGTERM"));
            assert!(message.contains("SIGKILL"));
        });
    }

    /// cas-647c: the measured incident. A closed mecha-cassy task copied a CAS
    /// root to `~/.cas/artifacts/cas-1bfb/fresh-proxy` as a proxy-health
    /// fixture and ran `cas serve` inside it, which auto-registered the copy as
    /// a host project. `cas doctor` then opened its 10-table database, found no
    /// `tasks`, and warned about a project DB that "could NOT be read" — with
    /// no command that cleared it, because the path existed so `prune-missing`
    /// refused to touch it.
    #[test]
    fn registered_disposable_roots_remain_available_for_local_use_cas_c8ab() {
        TestEnvGuard::run_with_temp_home(|home| {
            ensure_host_schema().unwrap();
            let fixture = home.join(".cas/artifacts/cas-1bfb/fresh-proxy");
            std::fs::create_dir_all(fixture.join(".cas")).unwrap();
            let external =
                tempfile::tempdir_in(crate::test_paths::runtime_fixture_parent()).unwrap();
            let real = external.path().join("myproject");
            std::fs::create_dir_all(&real).unwrap();

            register_repo(&fixture);
            register_repo_strict(&real).unwrap();

            let paths: Vec<PathBuf> = open_host_known_repo_store()
                .unwrap()
                .list()
                .unwrap()
                .into_iter()
                .map(|repo| repo.path)
                .collect();
            assert_eq!(paths.len(), 2);
            assert!(paths.contains(&fixture.canonicalize().unwrap()));
            assert!(paths.contains(&real.canonicalize().unwrap()));
            assert!(matches!(
                registry_skip(&fixture),
                Some(RegistrySkip::Artifacts(_))
            ));
            assert!(registry_skip(&real).is_none());
        });
    }

    /// Local registration is intentionally independent from the discovery
    /// guard. Explicitly registered roots remain visible in known_repos so
    /// local startup and binding flows can resolve them; update discovery
    /// applies `registry_skip_for_known_roots` when it sweeps the registry.
    #[test]
    fn register_repo_strict_registers_disposable_root_for_local_use_cas_c8ab() {
        TestEnvGuard::run_with_temp_home(|home| {
            ensure_host_schema().unwrap();
            let scratch = home.join(".cas/scratch/fresh-proxy");
            std::fs::create_dir_all(scratch.join(".cas")).unwrap();

            register_repo_strict(&scratch).unwrap();
            register_repo_strict(&scratch).unwrap();

            let store = open_host_known_repo_store().unwrap();
            assert_eq!(store.count().unwrap(), 1);
            assert_eq!(
                store.list().unwrap()[0].path,
                scratch.canonicalize().unwrap()
            );
            assert!(matches!(
                registry_skip(&scratch),
                Some(RegistrySkip::Scratch(_))
            ));
            assert!(
                registry_skip(&scratch)
                    .unwrap()
                    .reason()
                    .contains("scratch")
            );
        });
    }

    /// A `[factory] artifacts_root` pointed somewhere other than the default
    /// must be honoured — the skip follows configuration, not a hardcoded path.
    #[test]
    fn registry_skip_follows_a_configured_artifacts_root_cas_647c() {
        TestEnvGuard::run_with_temp_home(|home| {
            let artifacts = home.join("durable-artifacts");
            std::fs::create_dir_all(&artifacts).unwrap();
            std::fs::write(
                home.join(".cas/config.toml"),
                format!(
                    "[factory]\nartifacts_root = {:?}\n",
                    artifacts.display().to_string()
                ),
            )
            .unwrap();
            let fixture = artifacts.join("cas-1bfb/fresh-proxy");
            std::fs::create_dir_all(&fixture).unwrap();

            assert!(matches!(
                registry_skip(&fixture),
                Some(RegistrySkip::Artifacts(_))
            ));
            // The default location stays covered too, not replaced.
            assert!(matches!(
                registry_skip(&home.join(".cas/artifacts/cas-9999/copy")),
                Some(RegistrySkip::Artifacts(_))
            ));
        });
    }

    /// Every root under `$TMPDIR` is disposable for host-wide discovery. The
    /// updater must not mistake test/probe/container directories for projects.
    #[test]
    fn registry_skip_names_all_temp_roots_cas_647c() {
        TestEnvGuard::run_with_temp_home(|home| {
            let temp = std::env::temp_dir();
            assert!(matches!(
                registry_skip(&temp.join("cas-probe-comm-1234/root")),
                Some(RegistrySkip::Temp(_))
            ));
            assert!(matches!(
                registry_skip(&temp.join("custom-wt-abcd/erin")),
                Some(RegistrySkip::Temp(_))
            ));
            assert!(matches!(
                registry_skip(&temp.join("ordinary-project")),
                Some(RegistrySkip::Temp(_))
            ));
            // The temp HOME itself is also outside the project registry's
            // intended durable roots.
            assert!(matches!(registry_skip(home), Some(RegistrySkip::Temp(_))));
            assert!(matches!(
                registry_skip(&home.join("myproject")),
                Some(RegistrySkip::Temp(_))
            ));
        });
    }

    #[test]
    fn registry_skip_rejects_bare_folder_identity_and_nested_artifacts() {
        TestEnvGuard::run_with_temp_home(|_| {
            // Keep this fixture outside the temporary HOME and the system
            // temporary root. A bare `.cas` folder under either root is
            // intentionally classified as disposable. The runtime fixture
            // parent also works when an archived test runs outside any Git
            // checkout, while keeping the path on the runner's durable root.
            let fixture_parent = crate::test_paths::runtime_fixture_parent();
            let bare = tempfile::Builder::new()
                .prefix("cas-1d41-bare-")
                .tempdir_in(&fixture_parent)
                .unwrap();
            std::fs::create_dir_all(bare.path().join(".cas")).unwrap();
            assert!(matches!(
                registry_skip(bare.path()),
                Some(RegistrySkip::BareFolderIdentity(_))
            ));

            let project = PathBuf::from("/home/u/registered-project");
            let copy = project.join(".cas/artifacts/cas-1d41/container");
            assert!(matches!(
                registry_skip_for_known_roots(&copy, &[project]),
                Some(RegistrySkip::NestedArtifacts { .. })
            ));
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
