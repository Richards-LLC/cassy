//! `cas release doctor` and `cas release setup` (cas-9f61).
//!
//! Both run the source checkout's `scripts/release_env.py`, the one resolver
//! that assembly-proof, lane compile, the release gate and the release train
//! share. Keeping the logic in that script means the CLI, the shell tools and
//! the Python tools cannot drift apart.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use clap::Args;

const RESOLVER: &str = "scripts/release_env.py";

/// Options shared by `cas release doctor` and `cas release setup`.
#[derive(Args, Debug, Clone)]
pub struct ReleaseEnvArgs {
    /// Release checkout to inspect (default: the current directory's checkout)
    #[arg(long)]
    pub worktree: Option<PathBuf>,
}

/// Which resolver command to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseEnvCommand {
    /// Check every release prerequisite in seconds; one fix command per gap.
    Doctor,
    /// Persist discovered Zig and per-filesystem scratch bases in the profile.
    Setup,
}

impl ReleaseEnvCommand {
    fn name(self) -> &'static str {
        match self {
            Self::Doctor => "doctor",
            Self::Setup => "setup",
        }
    }
}

/// The checkout root and its resolver script, or a message naming what is missing.
pub fn resolver_for(start: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let output = Command::new("git")
        .arg("-C")
        .arg(start)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .with_context(|| format!("could not run git in {}", start.display()))?;
    if !output.status.success() {
        bail!(
            "{} is not inside a Git checkout; run from a Cassy source checkout or pass --worktree",
            start.display()
        );
    }
    let root = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let script = root.join(RESOLVER);
    if !script.is_file() {
        bail!(
            "{} has no {RESOLVER}; `cas release doctor` and `cas release setup` run from a Cassy source checkout",
            root.display()
        );
    }
    Ok((root, script))
}

pub fn execute(command: ReleaseEnvCommand, args: &ReleaseEnvArgs) -> anyhow::Result<()> {
    let start = match &args.worktree {
        Some(path) => path.clone(),
        None => std::env::current_dir().context("could not determine the current directory")?,
    };
    let (root, script) = resolver_for(&start)?;
    let status = Command::new("python3")
        .arg(&script)
        .arg(command.name())
        .arg("--worktree")
        .arg(&root)
        .status()
        .with_context(|| format!("could not run python3 {}", script.display()))?;
    if !status.success() {
        // The resolver already printed each gap and its fix command.
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn resolver_is_found_from_a_nested_directory_of_the_checkout() {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "-q"]);
        std::fs::create_dir_all(temp.path().join("scripts")).unwrap();
        std::fs::write(temp.path().join(RESOLVER), "# resolver\n").unwrap();
        let nested = temp.path().join("cas-cli/src");
        std::fs::create_dir_all(&nested).unwrap();
        let (root, script) = resolver_for(&nested).unwrap();
        assert_eq!(
            root.canonicalize().unwrap(),
            temp.path().canonicalize().unwrap()
        );
        assert!(script.ends_with(RESOLVER));
    }

    #[test]
    fn a_checkout_without_the_resolver_names_the_requirement() {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "-q"]);
        let error = resolver_for(temp.path()).unwrap_err().to_string();
        assert!(error.contains("scripts/release_env.py"), "{error}");
        assert!(error.contains("Cassy source checkout"), "{error}");
    }
}
