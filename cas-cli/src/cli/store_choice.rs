//! cas-e1c7: which store an identity- or config-changing command writes when
//! `CAS_ROOT` and the working directory name different Cassy stores.
//!
//! `CAS_ROOT` wins for every read and most writes: factory workers in clones
//! rely on it (cas-b69a). A command that rewrites project identity or config,
//! though, once rewrote cas-src's `[project] canonical_id` when it was run from
//! another project's directory in a factory shell, and the override notice only
//! printed beside the success line. Those commands now refuse up front, before
//! anything is written, until `--store` picks one of the two stores.

use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};

use crate::store::detect::RootConflict;

/// The store a guarded command writes when `CAS_ROOT` and the working
/// directory disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StoreChoice {
    /// The store `CAS_ROOT` names.
    CasRoot,
    /// The store this working directory resolves to.
    Here,
}

/// `--store`, flattened into each guarded command.
#[derive(Args, Debug, Clone, Default)]
pub struct StoreChoiceArgs {
    /// When CAS_ROOT and this directory's .cas are different stores, which one
    /// to write: `here` (this directory's) or `cas-root`. Without it the
    /// command refuses and writes nothing.
    #[arg(long, value_enum, value_name = "STORE")]
    pub store: Option<StoreChoice>,
}

/// The store `command` may write. `resolved` is the store every command
/// resolved (CAS_ROOT when set). With no conflict that is the answer; with one,
/// `choice` must pick, or the command is refused before any write.
pub fn write_root(
    command: &str,
    resolved: &Path,
    choice: Option<StoreChoice>,
    conflict: Option<RootConflict>,
) -> anyhow::Result<PathBuf> {
    let Some(conflict) = conflict else {
        return Ok(resolved.to_path_buf());
    };
    match choice {
        Some(StoreChoice::CasRoot) => Ok(conflict.env_root),
        Some(StoreChoice::Here) => Ok(conflict.cwd_root),
        None => anyhow::bail!(refusal(command, &conflict)),
    }
}

/// The refusal: both stores, nothing written, and the flags that pick one.
pub fn refusal(command: &str, conflict: &RootConflict) -> String {
    format!(
        "`{command}` not run: CAS_ROOT and this directory are different Cassy stores, \
         and this command changes project identity or config.\n\
         \x20 CAS_ROOT store:          {env}\n\
         \x20 this directory's store:  {cwd}\n\
         Nothing was written. Pick the store to write:\n\
         \x20 {command} … --store here       writes {cwd}\n\
         \x20 {command} … --store cas-root   writes {env}\n\
         Or clear the variable for this one command: env -u CAS_ROOT {command} …",
        env = conflict.env_root.display(),
        cwd = conflict.cwd_root.display(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conflict() -> RootConflict {
        RootConflict {
            env_root: PathBuf::from("/work/cas-src/.cas"),
            cwd_root: PathBuf::from("/work/violet_ps/.cas"),
        }
    }

    #[test]
    fn no_conflict_writes_the_resolved_store_whatever_the_flag() {
        let resolved = Path::new("/work/cas-src/.cas");
        for choice in [None, Some(StoreChoice::Here), Some(StoreChoice::CasRoot)] {
            assert_eq!(
                write_root("cas config set", resolved, choice, None).unwrap(),
                resolved
            );
        }
    }

    #[test]
    fn a_conflict_without_store_refuses_naming_both_stores_and_the_flags() {
        let error = write_root(
            "cas cloud project set",
            Path::new("/work/cas-src/.cas"),
            None,
            Some(conflict()),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("/work/cas-src/.cas"), "{error}");
        assert!(error.contains("/work/violet_ps/.cas"), "{error}");
        assert!(error.contains("Nothing was written"), "{error}");
        assert!(error.contains("--store here"), "{error}");
        assert!(error.contains("--store cas-root"), "{error}");
        assert!(
            error.contains("env -u CAS_ROOT cas cloud project set"),
            "{error}"
        );
    }

    #[test]
    fn store_picks_one_side_of_a_conflict() {
        let resolved = Path::new("/work/cas-src/.cas");
        assert_eq!(
            write_root(
                "cas config set",
                resolved,
                Some(StoreChoice::Here),
                Some(conflict())
            )
            .unwrap(),
            PathBuf::from("/work/violet_ps/.cas")
        );
        assert_eq!(
            write_root(
                "cas config set",
                resolved,
                Some(StoreChoice::CasRoot),
                Some(conflict())
            )
            .unwrap(),
            PathBuf::from("/work/cas-src/.cas")
        );
    }
}
