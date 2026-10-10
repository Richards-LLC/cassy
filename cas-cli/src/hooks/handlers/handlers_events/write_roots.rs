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
        self.in_effect(task_ids).find(|root| {
            root.modes.contains(&mode)
                // A root that cannot be resolved (missing, dangling symlink)
                // admits nothing. Containment is judged on resolved paths, so
                // `..` and symlinks inside the root cannot escape it.
                && root
                    .path
                    .canonicalize()
                    .is_ok_and(|canonical| resolved.starts_with(canonical))
        })
    }

    /// Roots that apply to an agent working on `task_ids`: every project root
    /// and the grants bound to those tasks.
    fn in_effect<'a>(
        &'a self,
        task_ids: &'a HashSet<String>,
    ) -> impl Iterator<Item = &'a WriteRoot> + 'a {
        self.roots.iter().filter(move |root| {
            root.task_id
                .as_ref()
                .is_none_or(|task| task_ids.contains(task))
        })
    }

    /// Human-readable list for denial messages: `path (create, edit)`.
    pub(crate) fn describe(&self, task_ids: &HashSet<String>) -> Vec<String> {
        self.in_effect(task_ids)
            .map(|root| {
                let modes = root
                    .modes
                    .iter()
                    .map(|mode| mode.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                match &root.task_id {
                    Some(task) => format!("{} ({modes}; grant for {task})", root.path.display()),
                    None => format!("{} ({modes})", root.path.display()),
                }
            })
            .collect()
    }
}
