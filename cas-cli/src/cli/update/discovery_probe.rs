//! One bounded observation per root for host-wide refresh discovery. A blocked
//! open/stat/read_dir cannot be cancelled; retain its pending entry instead of
//! creating another blocked thread on every refresh. Git children are killable.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::config::{RetiredConfigNotice, RetiredNoticeScope};
use crate::store::known_repos::{self, RegistrySkip};

const ROOT_BUDGET: Duration = Duration::from_secs(2);
const NEGATIVE_RETRY: Duration = Duration::from_secs(30);
const MAX_PENDING: usize = 64;
pub(super) const TIMEOUT: &str =
    "repository discovery timed out; check filesystem access and retry";

#[derive(Clone)]
pub(super) struct Observation {
    pub root: PathBuf,
    pub ready: bool,
    pub has_store: bool,
    pub project_root: bool,
    pub git_root: bool,
    pub skip: Option<RegistrySkip>,
    pub artifacts: Vec<PathBuf>,
    pub inode: Option<(u64, u64)>,
    pub children: Vec<PathBuf>,
    notices: Vec<RetiredConfigNotice>,
}

impl Observation {
    fn read(path: &Path) -> Self {
        let mut notices = RetiredNoticeScope::begin(|_, _| {}, false);
        let root = super::canonical_path(path);
        let has_cas = root.join(".cas").is_dir();
        let ready = root.is_dir() && has_cas;
        let has_store = root.join(".cas/cas.db").is_file();
        let skip = has_cas.then(|| known_repos::registry_skip(&root)).flatten();
        let project_root = has_cas && known_repos::is_project_root(&root);
        let git_root = has_cas && known_repos::git_project_root(&root);
        let artifacts = root.join(".cas/artifacts");
        let artifacts = vec![artifacts.clone(), super::canonical_path(&artifacts)];
        let inode = known_repos::project_inode_key(&root);
        let archive = dirs::home_dir().map(|home| super::canonical_path(&home.join("Archive")));
        let children = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let kind = entry.file_type().ok()?;
                if !kind.is_dir() || kind.is_symlink() {
                    return None;
                }
                let path = entry.path();
                let name = path.file_name()?.to_str()?;
                if name.starts_with('.')
                    || matches!(
                        name,
                        "node_modules"
                            | "target"
                            | "venv"
                            | "dist"
                            | "build"
                            | "vendor"
                            | "Library"
                            | "snap"
                    )
                    || archive.as_ref() == Some(&super::canonical_path(&path))
                {
                    None
                } else {
                    Some(path)
                }
            })
            .collect();
        Self {
            root,
            ready,
            has_store,
            project_root,
            git_root,
            skip,
            artifacts,
            inode,
            children,
            notices: notices.take(),
        }
    }
}

struct Pending {
    started: Instant,
    result: Mutex<Option<Option<Observation>>>,
    ready: Condvar,
}
static PROBES: OnceLock<Mutex<HashMap<PathBuf, Arc<Pending>>>> = OnceLock::new();

fn probe(path: &Path) -> Option<Observation> {
    let pending = {
        let mut probes = PROBES
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        probes.retain(|_, pending| {
            match pending
                .result
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                None => true,
                Some(None) => pending.started.elapsed() < NEGATIVE_RETRY,
                Some(Some(_)) => false, // Successful observations must always be fresh.
            }
        });
        if let Some(pending) = probes.get(path) {
            Arc::clone(pending)
        } else {
            if probes
                .values()
                .filter(|p| p.result.lock().unwrap_or_else(|e| e.into_inner()).is_none())
                .count()
                >= MAX_PENDING
            {
                return None;
            }
            let pending = Arc::new(Pending {
                started: Instant::now(),
                result: Mutex::new(None),
                ready: Condvar::new(),
            });
            probes.insert(path.to_path_buf(), Arc::clone(&pending));
            let worker = Arc::clone(&pending);
            let path = path.to_path_buf();
            let launched = std::thread::Builder::new()
                .name("refresh-root-probe".into())
                .spawn(move || {
                    let result = std::panic::catch_unwind(|| Observation::read(&path))
                        .ok()
                        .filter(|_| worker.started.elapsed() <= ROOT_BUDGET);
                    *worker.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
                    worker.ready.notify_all();
                });
            if launched.is_err() {
                *pending.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(None);
                pending.ready.notify_all();
            }
            pending
        }
    };
    let remaining = ROOT_BUDGET.saturating_sub(pending.started.elapsed());
    let result = pending.result.lock().unwrap_or_else(|e| e.into_inner());
    let (result, _) = pending
        .ready
        .wait_timeout_while(result, remaining, |r| r.is_none())
        .unwrap_or_else(|e| e.into_inner());
    result.clone().flatten()
}

/// One pass shares observations across registry, scan and explicit-current-root
/// paths. In particular the scanner never retries a root the registry timed out.
#[derive(Default)]
pub(super) struct Inspector {
    roots: HashMap<PathBuf, Option<Observation>>,
}
impl Inspector {
    pub fn observe(&mut self, path: &Path) -> Option<Observation> {
        if let Some(result) = self.roots.get(path) {
            return result.clone();
        }
        let result = probe(path);
        if let Some(observation) = &result {
            RetiredNoticeScope::adopt(
                observation.notices.clone(),
                super::replay_retired_config_notices,
            );
            self.roots.insert(observation.root.clone(), result.clone());
        }
        self.roots.insert(path.to_path_buf(), result.clone());
        result
    }
}
