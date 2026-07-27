use std::io::{self, Write};

use anyhow::{Result, bail};

use crate::cli::factory::FactoryArgs;
use crate::config::{Config, WorktreesConfig};
use crate::store::find_cas_root;
use crate::ui::components::{Formatter, Renderable, StatusLine};
use crate::ui::theme::ActiveTheme;
use crate::worktree::{GitOperations, WorktreeConfig, WorktreeManager};

/// Execute cleanup of worker worktree directories
pub(super) fn execute_cleanup(args: &FactoryArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;

    let worktree_root = args.worktree_root.clone().unwrap_or_else(|| {
        if let Ok(cas_root) = find_cas_root() {
            let config = Config::load(&cas_root).unwrap_or_default();
            config.worktrees().resolve_base_path(&cwd)
        } else {
            WorktreesConfig::default().resolve_base_path(&cwd)
        }
    });

    let theme = ActiveTheme::default();
    let mut stdout = io::stdout();
    let mut fmt = Formatter::stdout(&mut stdout, theme);

    if !worktree_root.exists() {
        fmt.info(&format!(
            "No worktree directory found at: {}",
            worktree_root.display()
        ))?;
        fmt.info("Nothing to clean up.")?;
        return Ok(());
    }

    let config = WorktreeConfig {
        enabled: true,
        base_path: worktree_root.to_string_lossy().to_string(),
        branch_prefix: "factory/".to_string(),
        ..Default::default()
    };
    let mut manager = WorktreeManager::new(&cwd, config)?;

    let entries = std::fs::read_dir(&worktree_root)?;
    let mut worktree_dirs: Vec<(String, std::path::PathBuf)> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && (path.join(".git").exists() || path.join(".git").is_file()) {
            let Some(name) = path.file_name() else {
                continue;
            };
            let name = name.to_string_lossy().to_string();
            let _ = manager.ensure_worker_worktree(&name);
            worktree_dirs.push((name, path));
        }
    }

    if worktree_dirs.is_empty() {
        fmt.info(&format!(
            "No worktree directories found in: {}",
            worktree_root.display()
        ))?;
        fmt.info("Nothing to clean up.")?;
        return Ok(());
    }

    let git = crate::worktree::GitOperations::new(cwd.clone());
    let mut has_uncommitted = false;
    let mut worktree_status: Vec<(String, std::path::PathBuf, bool)> = Vec::new();

    for (name, path) in &worktree_dirs {
        let uncommitted = git.has_uncommitted_changes(path).unwrap_or(false);
        if uncommitted {
            has_uncommitted = true;
        }
        worktree_status.push((name.clone(), path.clone(), uncommitted));
    }

    fmt.info(&format!(
        "Found {} worktree(s) in {}:",
        worktree_status.len(),
        worktree_root.display()
    ))?;
    fmt.newline()?;

    for (name, _path, uncommitted) in &worktree_status {
        let branch_name = manager.branch_name_for_worker(name);
        let warning = if *uncommitted {
            " [WARNING: uncommitted changes]"
        } else {
            ""
        };
        fmt.bullet(&format!("{name} ({branch_name}){warning}"))?;
    }
    fmt.newline()?;

    if args.dry_run {
        StatusLine::info("Dry run - no files will be removed.").render(&mut fmt)?;
        fmt.newline()?;
        fmt.subheading("Would remove:")?;
        for (_name, path, _) in &worktree_status {
            fmt.bullet(&path.display().to_string())?;
        }
        return Ok(());
    }

    if has_uncommitted {
        if !args.force {
            StatusLine::warning("Some worktrees have uncommitted changes.").render(&mut fmt)?;
            fmt.info("Use --force to remove anyway, or commit changes first.")?;
            fmt.newline()?;
            bail!("Cleanup aborted due to uncommitted changes.");
        }
        StatusLine::warning("Forcing cleanup despite uncommitted changes.").render(&mut fmt)?;
        fmt.newline()?;
    }

    if !args.force {
        StatusLine::warning(format!(
            "This will permanently delete {} worktree directories.",
            worktree_status.len()
        ))
        .render(&mut fmt)?;
        write!(io::stdout(), "Continue? [y/N] ")?;
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();

        if input != "y" && input != "yes" {
            StatusLine::info("Cleanup cancelled.").render(&mut fmt)?;
            return Ok(());
        }
    }

    let report = manager.cleanup_workers(true)?;

    // cas-df97: live external symlinks (e.g. a dotfile stow/install step
    // mistakenly run inside a worktree) resolve into a worktree we would
    // otherwise remove — surface it loudly instead of silently orphaning
    // every one of those links. Independent of --force: force means
    // "bypass git dirty-tree protection", not "destroy my $HOME symlinks".
    if !report.external_symlinks_blocked.is_empty() {
        fmt.newline()?;
        StatusLine::warning(format!(
            "{} worktree director{} preserved — live external symlinks point into them:",
            report.external_symlinks_blocked.len(),
            if report.external_symlinks_blocked.len() == 1 { "y" } else { "ies" },
        ))
        .render(&mut fmt)?;
        for warning in &report.external_symlinks_blocked {
            fmt.bullet(&format!("{} ({})", warning.worker_name, warning.path.display()))?;
            for link in &warning.links {
                fmt.bullet(&format!(
                    "  {} -> {}",
                    link.link.display(),
                    link.target.display()
                ))?;
            }
        }
        fmt.newline()?;
    }

    fmt.newline()?;
    fmt.info(&format!(
        "Removed {} worktree directories:",
        report.cleaned.len()
    ))?;
    for name in &report.cleaned {
        fmt.bullet(name)?;
    }

    if worktree_root.read_dir()?.next().is_none() {
        std::fs::remove_dir(&worktree_root)?;
        fmt.newline()?;
        StatusLine::success(format!(
            "Removed empty worktree root: {}",
            worktree_root.display()
        ))
        .render(&mut fmt)?;
    }

    fmt.newline()?;
    StatusLine::success("Cleanup complete.").render(&mut fmt)?;
    Ok(())
}

/// Check if the current worktree is behind its sync target branch.
pub(super) fn execute_check_staleness(branch: Option<&str>, fetch: bool) -> Result<()> {
    use std::process::Command;

    let cwd = std::env::current_dir()?;

    let git_check = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(&cwd)
        .output();
    match git_check {
        Ok(output) if output.status.success() => {}
        _ => return Ok(()),
    }

    let sync_ref = resolve_sync_ref(&cwd, branch)?;

    if fetch {
        if let Some(remote) = remote_for_ref(&cwd, &sync_ref)? {
            let status = Command::new("git")
                .args(["fetch", &remote])
                .current_dir(&cwd)
                .status()?;

            if !status.success() {
                let mut stderr = io::stderr();
                let theme = ActiveTheme::default();
                let mut fmt = Formatter::stdout(&mut stderr, theme);
                StatusLine::warning(format!("Could not fetch from {remote} (offline?)"))
                    .render(&mut fmt)?;
            }
        }
    }

    let output = Command::new("git")
        .args(["rev-list", "--count", &format!("HEAD..{sync_ref}")])
        .current_dir(&cwd)
        .output()?;

    if !output.status.success() {
        return Ok(());
    }

    let behind_count: usize = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0);

    if behind_count > 0 {
        let mut stderr = io::stderr();
        let theme = ActiveTheme::default();
        let mut fmt = Formatter::stdout(&mut stderr, theme);
        fmt.newline()?;
        StatusLine::warning("STALE WORKTREE").render(&mut fmt)?;
        fmt.separator()?;
        fmt.write_primary(&format!(
            "This worktree is {behind_count} commit(s) behind {sync_ref}."
        ))?;
        fmt.newline()?;
        fmt.write_primary("You may be missing code from other workers' merged changes.")?;
        fmt.newline()?;
        fmt.newline()?;
        fmt.info("To sync: cas factory sync")?;
        fmt.separator()?;
        fmt.newline()?;
    }

    Ok(())
}

/// Sync the current worktree to its sync target.
pub(super) fn execute_sync(branch: Option<&str>) -> Result<()> {
    use std::process::Command;

    let cwd = std::env::current_dir()?;
    let sync_ref = resolve_sync_ref(&cwd, branch)?;

    let theme = ActiveTheme::default();
    let mut stdout = io::stdout();
    let mut fmt = Formatter::stdout(&mut stdout, theme);

    fmt.info(&format!("Syncing to {sync_ref}..."))?;

    if let Some(remote) = remote_for_ref(&cwd, &sync_ref)? {
        let status = Command::new("git")
            .args(["fetch", &remote])
            .current_dir(&cwd)
            .status()?;

        if !status.success() {
            bail!("Failed to fetch from {remote}");
        }
    }

    let status = Command::new("git")
        .args(["rebase", &sync_ref])
        .current_dir(&cwd)
        .status()?;

    if !status.success() {
        bail!("Rebase failed against {sync_ref}. Check `git log` for conflicts.");
    }

    let output = Command::new("git")
        .args(["log", "--oneline", "-1"])
        .current_dir(&cwd)
        .output()?;

    if output.status.success() {
        let head = String::from_utf8_lossy(&output.stdout);
        StatusLine::success(format!("Synced to: {}", head.trim())).render(&mut fmt)?;
    }

    Ok(())
}

/// Detect the target branch to check against.
fn detect_target_branch(cwd: &std::path::Path) -> Result<String> {
    use std::process::Command;

    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()?;

    if output.status.success() {
        let current_branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if current_branch.starts_with("factory/") {
            let output = Command::new("git")
                .args(["branch", "--list", "epic/*"])
                .current_dir(cwd)
                .output()?;

            if output.status.success() {
                let epic_branches = String::from_utf8_lossy(&output.stdout);
                if let Some(epic_branch) = epic_branches.lines().last() {
                    let epic_branch = epic_branch.trim().trim_start_matches("* ");
                    if !epic_branch.is_empty() {
                        return Ok(epic_branch.to_string());
                    }
                }
            }
        }
    }

    let repo_root = GitOperations::detect_repo_root(cwd)?;
    let git = GitOperations::new(repo_root);
    Ok(git.detect_default_branch())
}

fn resolve_sync_ref(cwd: &std::path::Path, branch: Option<&str>) -> Result<String> {
    if let Some(branch) = branch {
        return Ok(branch.to_string());
    }

    if let Some(upstream) = current_upstream(cwd)? {
        return Ok(upstream);
    }

    detect_target_branch(cwd)
}

fn current_upstream(cwd: &std::path::Path) -> Result<Option<String>> {
    use std::process::Command;

    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "@{upstream}"])
        .current_dir(cwd)
        .output()?;

    if !output.status.success() {
        return Ok(None);
    }

    let upstream = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if upstream.is_empty() {
        Ok(None)
    } else {
        Ok(Some(upstream))
    }
}

fn remote_for_ref(cwd: &std::path::Path, reference: &str) -> Result<Option<String>> {
    use std::process::Command;

    let Some(candidate) = reference.split('/').next() else {
        return Ok(None);
    };
    if candidate.is_empty() {
        return Ok(None);
    }

    let output = Command::new("git")
        .args(["remote"])
        .current_dir(cwd)
        .output()?;

    if !output.status.success() {
        return Ok(None);
    }

    let remotes = String::from_utf8_lossy(&output.stdout);
    let found = remotes.lines().any(|line| line.trim() == candidate);
    if found {
        Ok(Some(candidate.to_string()))
    } else {
        Ok(None)
    }
}
