//! Operator-granted write roots for the factory workspace contract (cas-3147,
//! GH #1169, GH #1182).
//!
//! The contract normally sanctions only the worktree, the task artifacts root,
//! the scratch root and the harness scratchpad. An operator may add directories
//! outside them: project-wide roots (`[factory] write_roots` in the project's
//! `.cas/config.toml`, each permitting create, edit and delete), or a one-off
//! grant bound to one task with its own modes. This module only matches a
//! resolved path against the policy; who may write the policy is decided
//! elsewhere.

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

/// The paths of a project that no agent may write (GH #1182): the operator
/// policy directory and the config files that hold the write roots.
pub(crate) fn protected_policy_paths(cas_root: &Path) -> Vec<PathBuf> {
    vec![
        cas_root.join("operator"),
        cas_root.join("config.toml"),
        cas_root.join("config.yaml"),
    ]
}

/// Whether `path` is any project's Cassy config or operator policy:
/// `…/.cas/config.toml`, `…/.cas/config.yaml` or under `…/.cas/operator/`.
pub(crate) fn is_cas_policy_path(path: &Path) -> bool {
    let components: Vec<_> = path.components().map(|component| component.as_os_str()).collect();
    components.windows(2).any(|pair| {
        pair[0] == ".cas"
            && (pair[1] == "config.toml" || pair[1] == "config.yaml" || pair[1] == "operator")
    })
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
    /// Paths no agent may write, whatever the roots say: the operator policy
    /// (`.cas/operator/`) and the project config that holds the roots
    /// (`.cas/config.toml`, and the legacy `.cas/config.yaml` it merges).
    pub protected: Vec<PathBuf>,
    /// Project write roots are configured, or the operator policy file exists. Only then are in-place edits
    /// (`sed -i`, `perl -i`) and rename sources judged under the contract;
    /// without it the default contract is exactly as before (cas-3147).
    pub configured: bool,
}

impl WritePolicy {
    pub(crate) fn is_empty(&self) -> bool {
        self.roots.is_empty() && self.protected.is_empty()
    }

    /// The root under which an agent working on `task_ids` may perform `mode`
    /// on `resolved` (already canonicalized for containment), if any.
    pub(crate) fn matching(
        &self,
        resolved: &Path,
        mode: WriteMode,
        task_ids: &HashSet<String>,
    ) -> Option<&WriteRoot> {
        // No root ever admits a Cassy policy file, including another
        // project's, so one project's roots cannot widen another's.
        if is_cas_policy_path(resolved) {
            return None;
        }
        self.in_effect(task_ids).find(|root| {
            root.modes.contains(&mode)
                // A root that cannot be resolved (missing, dangling symlink)
                // admits nothing. Containment is judged on resolved paths, so
                // `..` and symlinks inside the root cannot escape it.
                // Strictly beneath the root: the root directory itself is
                // never deleted, renamed or overwritten through it (GH #1182
                // QA F02).
                && root
                    .path
                    .canonicalize()
                    .is_ok_and(|canonical| resolved.starts_with(&canonical) && resolved != canonical)
        })
    }

    /// Roots that apply to an agent working on `task_ids`: every project root
    /// and the grants bound to those tasks.
    fn in_effect<'a, 'b>(
        &'a self,
        task_ids: &'b HashSet<String>,
    ) -> impl Iterator<Item = &'a WriteRoot> + use<'a, 'b> {
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
