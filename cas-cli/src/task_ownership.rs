//! Proving that a bare task id belongs to this project (cas-6162).
//!
//! Legacy artifact directories (`~/.cas/artifacts/cas-1a2b/`) and simulator
//! names (`cas-1a2b-proof`) carry a task id but no project. Task ids are short,
//! so another project on the same host can have the same id. A bare id counts
//! as this project's only when no other known project's task store holds it;
//! a store that cannot be read makes every bare id unproven.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Other projects' task stores, consulted read-only.
pub trait OtherProjects {
    /// The other projects whose store holds `task_id`. `Err` means ownership
    /// cannot be proven (a store is missing or unreadable).
    fn holders(&self, task_id: &str) -> Result<Vec<String>, String>;
}

/// Task ids of every other known project, loaded once.
pub struct KnownProjects {
    ids: Result<BTreeMap<String, HashSet<String>>, String>,
}

impl KnownProjects {
    /// Every repo in the host registry except the one owning `cas_root`.
    pub fn load(cas_root: &Path) -> Self {
        let repos = crate::store::known_repos::open_host_known_repo_store()
            .and_then(|store| cas_store::KnownRepoStore::list(&store).map_err(Into::into));
        match repos {
            Ok(repos) => Self::from_roots(cas_root, repos.into_iter().map(|repo| repo.path)),
            Err(error) => Self { ids: Err(format!("known-repo registry unreadable: {error}")) },
        }
    }

    /// Other projects from explicit repo roots (each with a `.cas/` store).
    pub fn from_roots(cas_root: &Path, roots: impl IntoIterator<Item = PathBuf>) -> Self {
        let this = std::fs::canonicalize(cas_root).unwrap_or_else(|_| cas_root.to_path_buf());
        let mut ids = BTreeMap::new();
        let mut unreadable = Vec::new();
        for root in roots {
            let other = root.join(".cas");
            if std::fs::canonicalize(&other).is_ok_and(|other| other == this) {
                continue;
            }
            match task_ids(&other.join("cas.db")) {
                Ok(set) => {
                    ids.insert(root.display().to_string(), set);
                }
                Err(error) => unreadable.push(format!("{} ({error})", root.display())),
            }
        }
        if unreadable.is_empty() {
            Self { ids: Ok(ids) }
        } else {
            Self {
                ids: Err(format!(
                    "task stores of other known projects cannot be read: {}; forget missing repos with `cas known-repos` or restore them",
                    unreadable.join(", ")
                )),
            }
        }
    }
}

impl OtherProjects for KnownProjects {
    fn holders(&self, task_id: &str) -> Result<Vec<String>, String> {
        let ids = self.ids.as_ref().map_err(Clone::clone)?;
        Ok(ids
            .iter()
            .filter(|(_, set)| set.contains(task_id))
            .map(|(project, _)| project.clone())
            .collect())
    }
}

/// Every task id in a store. `immutable=1` would skip rows still in the WAL
/// and `mode=ro` can fail on a store in use, so open it without create and
/// with `query_only`, which reads the WAL and cannot write.
fn task_ids(db: &Path) -> Result<HashSet<String>, String> {
    if !db.is_file() {
        return Err("no cas.db".into());
    }
    let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|error| error.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(2)).map_err(|error| error.to_string())?;
    conn.pragma_update(None, "query_only", true).map_err(|error| error.to_string())?;
    let mut statement = conn.prepare("SELECT id FROM tasks").map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|error| error.to_string())
}

/// Whether `name` is exactly a task id (`cas-` and 4 to 16 hex digits).
pub fn is_task_id(name: &str) -> bool {
    name.strip_prefix("cas-")
        .is_some_and(|hex| (4..=16).contains(&hex.len()) && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}
