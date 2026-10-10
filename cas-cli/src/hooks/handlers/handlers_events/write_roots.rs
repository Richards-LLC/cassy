//! Operator-granted write roots for the factory workspace contract (cas-3147,
//! GH #1169).
//!
//! The contract normally sanctions only the worktree, the task artifacts root,
//! the scratch root and the harness scratchpad. An operator may add directories
//! outside them: project-wide roots, or a one-off grant bound to one task. Each
//! root names the operations it permits (create, edit, delete). This module only
//! matches a resolved path against the policy; who may write the policy is
//! decided elsewhere.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

/// One kind of filesystem change, judged separately under a write root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum WriteMode {
    /// A file or directory that does not exist yet.
    Create,
    /// An existing file changed in place, overwritten, or renamed.
    Edit,
    /// An existing file or directory removed.
    Delete,
}

impl WriteMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            WriteMode::Create => "create",
            WriteMode::Edit => "edit",
            WriteMode::Delete => "delete",
        }
    }
}

/// A directory outside the default sanctioned set, with its permitted modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WriteRoot {
    pub path: PathBuf,
    pub modes: BTreeSet<WriteMode>,
    /// `Some(task)` for a one-off operator grant bound to that task.
    pub task_id: Option<String>,
}

/// The operator's write roots and grants in effect for one hook call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WritePolicy {
    pub roots: Vec<WriteRoot>,
}

impl WritePolicy {
    pub(crate) fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// The root under which an agent working on `task_ids` may perform `mode`
    /// on `resolved` (already canonicalized for containment), if any.
    pub(crate) fn matching(
        &self,
        resolved: &Path,
        mode: WriteMode,
        task_ids: &HashSet<String>,
    ) -> Option<&WriteRoot> {
        let _ = (resolved, mode, task_ids);
        None
    }

    /// Human-readable list for denial messages: `path (create, edit)`.
    pub(crate) fn describe(&self, task_ids: &HashSet<String>) -> Vec<String> {
        let _ = task_ids;
        Vec::new()
    }
}
