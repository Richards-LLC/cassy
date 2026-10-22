//! Isolate host-registry filesystem reads from lifecycle mutation callers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::{
    BoundedRepoError, BoundedRepoProbe, Deadline, bounded_git_layout, bounded_repo_identities,
};

pub(super) const SCAN_BUDGET: Duration = Duration::from_secs(5);
const ROOT_BUDGET: Duration = Duration::from_secs(1);
const NEGATIVE_RETRY: Duration = Duration::from_secs(30);
const MAX_PENDING: usize = 64;

#[derive(Clone)]
pub(super) struct Candidate {
    pub repo_root: PathBuf,
    pub git_common_dir: PathBuf,
    pub identities: Vec<String>,
}

struct PendingCandidate {
    started: Instant,
    // None: running; Ok(None): absent; Err(_): incomplete probe; Ok(Some(_)): fresh evidence.
    result: Mutex<Option<Result<Option<Candidate>, BoundedRepoError>>>,
    ready: Condvar,
}

static PROBES: OnceLock<Mutex<HashMap<PathBuf, Arc<PendingCandidate>>>> = OnceLock::new();

/// Git is killable through the existing process-group runner. Filesystem calls
/// such as canonicalize/open are not: isolate those on a thread and retain its
/// pending entry until it actually finishes, even after the retry TTL. Thus a
/// stalled mount cannot create another blocked thread on each start/close.
pub(super) fn probe(path: &Path, deadline: Deadline) -> Result<Option<Candidate>, String> {
    let pending = {
        let mut probes = PROBES
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        probes.retain(|_, pending| {
            let result = pending
                .result
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match result.as_ref() {
                None => true,
                Some(Ok(None) | Err(_)) => pending.started.elapsed() < NEGATIVE_RETRY,
                Some(Ok(Some(_))) => false, // Healthy identities must always be read fresh.
            }
        });
        if let Some(pending) = probes.get(path) {
            Arc::clone(pending)
        } else {
            let running = probes
                .values()
                .filter(|pending| {
                    pending
                        .result
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .is_none()
                })
                .count();
            if running >= MAX_PENDING {
                return Err("WORK TARGET REPOSITORY UNAVAILABLE: too many known-root filesystem probes are still blocked; retry after storage access recovers".to_string());
            }
            let pending = Arc::new(PendingCandidate {
                started: Instant::now(),
                result: Mutex::new(None),
                ready: Condvar::new(),
            });
            probes.insert(path.to_path_buf(), Arc::clone(&pending));
            let worker = Arc::clone(&pending);
            let path = path.to_path_buf();
            let probe = BoundedRepoProbe::new(Deadline::after(ROOT_BUDGET), ROOT_BUDGET, 1);
            let launched = std::thread::Builder::new()
                .name("known-root-probe".to_string())
                .spawn(move || {
                    let result = std::panic::catch_unwind(|| {
                        if !path.exists() {
                            return Ok(None);
                        }
                        let (repo_root, git_common_dir) = match bounded_git_layout(&path, &probe) {
                            Ok(layout) => layout,
                            Err(BoundedRepoError::Unavailable) => return Ok(None),
                            Err(error) => return Err(error),
                        };
                        let identities = bounded_repo_identities(&repo_root, &probe)?;
                        Ok(Some(Candidate {
                            repo_root,
                            git_common_dir,
                            identities,
                        }))
                    })
                    .unwrap_or(Err(BoundedRepoError::Unavailable));
                    *worker
                        .result
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()) = Some(result);
                    worker.ready.notify_all();
                });
            if launched.is_err() {
                *pending
                    .result
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) =
                    Some(Err(BoundedRepoError::Unavailable));
                pending.ready.notify_all();
            }
            pending
        }
    };
    let budget = ROOT_BUDGET
        .saturating_sub(pending.started.elapsed())
        .min(deadline.remaining());
    let result = pending
        .result
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let (result, _) = pending
        .ready
        .wait_timeout_while(result, budget, |result| result.is_none())
        .unwrap_or_else(|error| error.into_inner());
    // Neither a still-running filesystem read nor a cached Git timeout proves
    // that a repository is absent. Preserve that distinction on every retry.
    result.clone().unwrap_or(Err(BoundedRepoError::TimedOut)).map_err(|_| {
        "WORK TARGET REPOSITORY UNAVAILABLE: known-root probe did not complete; retry after storage access recovers".to_string()
    })
}
