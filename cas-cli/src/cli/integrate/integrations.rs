//! Orchestration glue: detect platforms, prompt the user, dispatch to each
//! handler, render results.
//!
//! `cas init` calls [`run`] after its existing setup steps. The same module
//! could be reused by a future `cas integrate all` command. Platform
//! handlers themselves remain testable in isolation — the orchestration
//! layer is the only place that takes the [`super::lock::IntegrateLock`].
//!
//! Owner: task **cas-7417**.

use std::path::Path;

use anyhow::Context;

use super::github::{self, GithubAction};
use super::lock::IntegrateLock;
use super::neon::{self, LiveNeonClient};
use super::types::{IntegrationOutcome, IntegrationStatus};
use super::vercel;

/// Caller-supplied flags that gate the run. Mirrors the `cas init` CLI
/// surface.
#[derive(Debug, Default, Clone)]
pub struct IntegrationFlags {
    /// `--no-integrations`: skip the entire section regardless of detection.
    pub disabled: bool,
    /// `--vercel <projectId>`: pre-seed; auto-confirm without picker.
    pub vercel_project: Option<String>,
    /// `--neon <projectId>`: pre-seed; auto-confirm without picker.
    pub neon_project: Option<String>,
    /// `--github <owner/repo>`: override `git remote -v`.
    pub github_repo: Option<String>,
}

/// UX mode — drives whether prompts fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UxMode {
    /// `cas init` (interactive wizard): prompts allowed.
    Interactive,
    /// `cas init --yes` / `cas init --json`: never prompt; auto-yes if
    /// strong-detected, auto-no otherwise. Same as `--no-integrations`
    /// for never-detected platforms.
    NonInteractive,
}

/// Aggregated result of a [`run`] call. One outcome per platform that was
/// considered (i.e. detected or pre-seeded by a flag).
#[derive(Debug, Default)]
pub struct IntegrationsRun {
    pub outcomes: Vec<IntegrationOutcome>,
    /// Set when `--no-integrations` short-circuited the run.
    pub skipped_globally: bool,
}

/// Top-level entry: detect, prompt-or-auto-confirm, dispatch.
///
/// The lock is taken for the entire run so no parallel `cas integrate` can
/// race the same SKILL files. Per-platform errors are collected into the
/// outcomes vec rather than aborting the run — one platform failing does
/// not prevent the others from configuring.
pub fn run(
    repo_root: &Path,
    flags: &IntegrationFlags,
    ux: UxMode,
) -> anyhow::Result<IntegrationsRun> {
    let mut report = IntegrationsRun::default();
    if flags.disabled {
        report.skipped_globally = true;
        return Ok(report);
    }

    // run() always writes (it only dispatches Init/Refresh), so take an
    // exclusive lock unconditionally. If a future change adds Verify
    // dispatch here, switch to `IntegrateLock::for_action` driven by the
    // strictest action across the plan — the RO-FS-aware helper exists
    // for that day.
    let _lock = IntegrateLock::acquire(repo_root)
        .context("acquiring .cas/integrate.lock")?;

    let plan = build_plan(repo_root, flags);
    if plan.is_empty() {
        return Ok(report);
    }

    for step in plan {
        let outcome = dispatch(repo_root, &step, ux);
        match outcome {
            Ok(o) => report.outcomes.push(o),
            Err(e) => {
                // Capture the error in a synthetic outcome so the renderer
                // surfaces it without aborting the rest of the run.
                let mut o = IntegrationOutcome::new(
                    step.platform_marker(),
                    super::types::IntegrationAction::Init,
                    IntegrationStatus::Skipped,
                );
                o.summary
                    .push(format!("error: {e:#}"));
                report.outcomes.push(o);
            }
        }
    }
    Ok(report)
}

#[derive(Debug, Clone)]
enum Step {
    Vercel {
        mode: StepMode,
        /// Pre-seeded `prj_*` from `--vercel <id>` if any. Threaded into
        /// `vercel::init_with_preseed`, which bypasses list_projects +
        /// fuzzy-match and validates the id via `client.get_project`.
        preseed_project: Option<String>,
    },
    Neon {
        mode: StepMode,
        /// Pre-seeded Neon project ID from `--neon <id>` if any. Threaded
        /// into `neon::InitChoices.project_id`, which forces the handler to
        /// use this project without prompting.
        preseed_project: Option<String>,
    },
    Github {
        mode: StepMode,
        /// `--github OWNER/REPO`. Plumbed through to `GithubAction::{Init,Refresh}.repo`.
        repo: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepMode {
    /// Run `init` against an unconfigured target.
    Init,
    /// SKILL.md already populated → run `refresh` (preserve keep blocks).
    Refresh,
}

impl Step {
    fn platform_marker(&self) -> super::types::Platform {
        match self {
            Step::Vercel { .. } => super::types::Platform::Vercel,
            Step::Neon { .. } => super::types::Platform::Neon,
            Step::Github { .. } => super::types::Platform::Github,
        }
    }
}

fn build_plan(repo_root: &Path, flags: &IntegrationFlags) -> Vec<Step> {
    let mut plan = Vec::new();

    if vercel::detect_vercel(repo_root).detected() || flags.vercel_project.is_some() {
        plan.push(Step::Vercel {
            mode: skill_mode(repo_root, ".claude/skills/vercel-deployments/SKILL.md"),
            preseed_project: flags.vercel_project.clone(),
        });
    }

    if neon::detect(repo_root).detected() || flags.neon_project.is_some() {
        plan.push(Step::Neon {
            mode: skill_mode(repo_root, ".claude/skills/neon-database/SKILL.md"),
            preseed_project: flags.neon_project.clone(),
        });
    }

    // GitHub: queue the step when the user explicitly passed `--github`,
    // OR `.git` exists AND a github.com remote is detected. We
    // short-circuit on `--github` so the user's explicit override never
    // pays for a `git remote -v` subprocess. For non-github remotes
    // (gitlab/bitbucket/codeberg/self-hosted-gitea) we skip the step
    // entirely instead of dispatching a Skipped roundtrip through the
    // handler. detect_repo errors are surfaced via stderr (rather than
    // silently dropped) so a misconfigured git is visible.
    let queue_github = if flags.github_repo.is_some() {
        true
    } else {
        match github::detect_repo(repo_root) {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(e) => {
                eprintln!(
                    "  Integrations: skipping GitHub plan check (`git remote -v` failed: {e:#})"
                );
                false
            }
        }
    };
    if queue_github {
        plan.push(Step::Github {
            mode: skill_mode(repo_root, ".claude/skills/github-repo/SKILL.md"),
            repo: flags.github_repo.clone(),
        });
    }

    plan
}

/// True if the candidate SKILL.md exists and is non-empty.
fn skill_mode(repo_root: &Path, rel: &str) -> StepMode {
    let p = repo_root.join(rel);
    if p.is_file() {
        match std::fs::metadata(&p) {
            Ok(md) if md.len() > 0 => StepMode::Refresh,
            _ => StepMode::Init,
        }
    } else {
        StepMode::Init
    }
}

fn dispatch(
    repo_root: &Path,
    step: &Step,
    ux: UxMode,
) -> anyhow::Result<IntegrationOutcome> {
    // Confirmation prompt — fires only in Interactive mode. Default Yes for
    // Init steps (the user is bootstrapping a project); default No for
    // Refresh prompts so re-running `cas init` on an existing project never
    // silently re-renders SKILL files the user might have hand-edited.
    if ux == UxMode::Interactive {
        let (label, default_yes) = match step {
            Step::Vercel { mode, .. } => match mode {
                StepMode::Init => ("Configure Vercel integration?", true),
                StepMode::Refresh => ("Refresh Vercel integration?", false),
            },
            Step::Neon { mode, .. } => match mode {
                StepMode::Init => ("Configure Neon integration?", true),
                StepMode::Refresh => ("Refresh Neon integration?", false),
            },
            Step::Github { mode, .. } => match mode {
                StepMode::Init => ("Configure GitHub integration?", true),
                StepMode::Refresh => ("Refresh GitHub integration?", false),
            },
        };
        let confirmed = inquire::Confirm::new(label)
            .with_default(default_yes)
            .prompt()
            .unwrap_or(default_yes);
        if !confirmed {
            let mut o = IntegrationOutcome::new(
                step.platform_marker(),
                super::types::IntegrationAction::Init,
                IntegrationStatus::Skipped,
            );
            o.summary.push("user declined".to_string());
            return Ok(o);
        }
    }

    match step {
        Step::Vercel { mode, preseed_project } => match mode {
            StepMode::Init => {
                let client = vercel::default_client();
                vercel::init_with_preseed(
                    repo_root,
                    client.as_ref(),
                    preseed_project.as_deref(),
                )
            }
            StepMode::Refresh => {
                let client = vercel::default_client();
                // Default refresh: preserve keep block (do not re-fetch IDs).
                vercel::refresh(repo_root, client.as_ref(), false)
            }
        },
        Step::Neon { mode, preseed_project } => {
            let client = LiveNeonClient::new();
            match mode {
                StepMode::Init => {
                    let choices = neon::InitChoices {
                        project_id: preseed_project.clone(),
                        ..Default::default()
                    };
                    neon::init(repo_root, &client, choices)
                }
                StepMode::Refresh => neon::refresh(
                    repo_root,
                    &client,
                    neon::RefreshOpts::default(),
                ),
            }
        }
        Step::Github { mode, repo } => match mode {
            StepMode::Init => {
                github::execute(GithubAction::Init { repo: repo.clone() })
            }
            StepMode::Refresh => {
                github::execute(GithubAction::Refresh { repo: repo.clone() })
            }
        },
    }
}

/// Render an aggregated run for stdout. Used by `cas init`.
pub fn render(report: &IntegrationsRun) {
    if report.skipped_globally {
        println!("  Integrations: skipped (--no-integrations)");
        return;
    }
    if report.outcomes.is_empty() {
        println!("  Integrations: no platforms detected");
        return;
    }
    println!("  Integrations:");
    for o in &report.outcomes {
        println!(
            "    {} {}: {}",
            o.platform.as_str(),
            o.action.as_str(),
            o.status.as_str()
        );
        for s in &o.summary {
            println!("      {s}");
        }
    }
    println!("    → run `cas integrate <platform> refresh` later to update IDs");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tmp_repo() -> TempDir {
        let t = TempDir::new().unwrap();
        // Make it look enough like a project to satisfy locate_repo_root
        // sentinel checks (cas-7417 also tightens those).
        std::fs::create_dir_all(t.path().join(".git")).unwrap();
        t
    }

    #[test]
    fn run_with_disabled_flag_short_circuits() {
        let t = tmp_repo();
        let flags = IntegrationFlags {
            disabled: true,
            ..Default::default()
        };
        let r = run(t.path(), &flags, UxMode::NonInteractive).unwrap();
        assert!(r.skipped_globally);
        assert!(r.outcomes.is_empty());
        // Lock file is NOT created on the disabled path — skip is cheap.
        assert!(!t.path().join(".cas/integrate.lock").exists());
    }

    #[test]
    fn run_with_no_signals_does_not_panic() {
        let t = tmp_repo();
        let flags = IntegrationFlags::default();
        // Smoke: orchestration runs to completion without panicking. The
        // github handler is invoked because .git/ exists in the tmp_repo;
        // its outcome (Skipped, Stale, or Err captured-as-Skipped) is
        // implementation-detail of the github handler, not the integrations
        // contract.
        let r = run(t.path(), &flags, UxMode::NonInteractive).unwrap();
        assert!(!r.skipped_globally);
    }

    #[test]
    fn run_creates_lockfile_on_non_disabled_path() {
        let t = tmp_repo();
        let flags = IntegrationFlags::default();
        let _ = run(t.path(), &flags, UxMode::NonInteractive).unwrap();
        // Lockfile is created (and held) inside run; once `run` returns
        // the file persists but the OS lock has been released.
        assert!(t.path().join(".cas/integrate.lock").exists());
    }

    #[test]
    fn render_handles_disabled_and_empty_runs() {
        // Smoke: just ensure render doesn't panic.
        render(&IntegrationsRun {
            skipped_globally: true,
            ..Default::default()
        });
        render(&IntegrationsRun::default());
    }

    #[test]
    fn build_plan_picks_refresh_when_skill_md_already_populated() {
        let t = tmp_repo();
        // Vercel signal + populated SKILL.md.
        std::fs::write(t.path().join("vercel.json"), "{}").unwrap();
        let claude_skill = t
            .path()
            .join(".claude/skills/vercel-deployments/SKILL.md");
        std::fs::create_dir_all(claude_skill.parent().unwrap()).unwrap();
        std::fs::write(&claude_skill, "# already").unwrap();

        let flags = IntegrationFlags::default();
        let plan = build_plan(t.path(), &flags);
        let vercel_step = plan
            .iter()
            .find(|s| matches!(s, Step::Vercel { .. }))
            .expect("vercel step should be planned");
        assert!(matches!(
            vercel_step,
            Step::Vercel { mode: StepMode::Refresh, .. }
        ));
    }

    #[test]
    fn build_plan_includes_vercel_step_when_only_flag_set() {
        let t = tmp_repo();
        // No vercel.json, no @vercel deps — but flag is set.
        let flags = IntegrationFlags {
            vercel_project: Some("prj_x".to_string()),
            ..Default::default()
        };
        let plan = build_plan(t.path(), &flags);
        let vercel_step = plan
            .iter()
            .find(|s| matches!(s, Step::Vercel { .. }))
            .expect("flag should force the step into the plan");
        match vercel_step {
            Step::Vercel { preseed_project, .. } => {
                assert_eq!(preseed_project.as_deref(), Some("prj_x"));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn build_plan_includes_neon_step_when_only_flag_set() {
        let t = tmp_repo();
        let flags = IntegrationFlags {
            neon_project: Some("np_x".to_string()),
            ..Default::default()
        };
        let plan = build_plan(t.path(), &flags);
        match plan.iter().find(|s| matches!(s, Step::Neon { .. })) {
            Some(Step::Neon { preseed_project, .. }) => {
                assert_eq!(preseed_project.as_deref(), Some("np_x"));
            }
            other => panic!("expected Neon step, got {other:?}"),
        }
    }

    #[test]
    fn run_in_non_interactive_mode_does_not_prompt() {
        // The binary installs this before dispatch; this standalone library test
        // exercises reqwest without passing through main.
        let _ = rustls::crypto::ring::default_provider().install_default();
        // Smoke: run() in NonInteractive mode must complete without ever
        // calling inquire::Confirm (which would block on stdin in CI).
        // The fact that this test runs to completion under `cargo test`
        // confirms the property — Confirm::prompt would error/block
        // without a tty. We assert non-skip-globally + completion.
        let t = tmp_repo();
        std::fs::write(t.path().join("vercel.json"), "{}").unwrap();
        let r = run(t.path(), &IntegrationFlags::default(), UxMode::NonInteractive)
            .expect("non-interactive run should not block on stdin");
        assert!(!r.skipped_globally);
    }

    #[test]
    fn build_plan_skips_github_when_remote_is_not_github() {
        // A repo with .git/ but a gitlab origin must NOT queue a Github
        // step — the handler would only Skip it internally.
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path();
        // Initialize a real git repo with a non-github origin.
        let git_init = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(repo)
            .status();
        if git_init.map(|s| !s.success()).unwrap_or(true) {
            // git not installed; skip on platforms without it.
            return;
        }
        let _ = std::process::Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                "https://gitlab.com/owner/repo.git",
            ])
            .current_dir(repo)
            .status();

        let plan = build_plan(repo, &IntegrationFlags::default());
        let has_github = plan.iter().any(|s| matches!(s, Step::Github { .. }));
        assert!(
            !has_github,
            "gitlab remote must not queue a Github step: {plan:?}"
        );
    }

    #[test]
    fn build_plan_includes_github_when_remote_is_github() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path();
        let git_init = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(repo)
            .status();
        if git_init.map(|s| !s.success()).unwrap_or(true) {
            return;
        }
        let _ = std::process::Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widgets.git",
            ])
            .current_dir(repo)
            .status();

        let plan = build_plan(repo, &IntegrationFlags::default());
        assert!(
            plan.iter().any(|s| matches!(s, Step::Github { .. })),
            "github.com remote must queue a Github step: {plan:?}"
        );
    }

    #[test]
    fn build_plan_threads_github_repo_flag_into_step() {
        let t = tmp_repo();
        let flags = IntegrationFlags {
            github_repo: Some("acme/widgets".to_string()),
            ..Default::default()
        };
        let plan = build_plan(t.path(), &flags);
        match plan.iter().find(|s| matches!(s, Step::Github { .. })) {
            Some(Step::Github { repo, .. }) => {
                assert_eq!(repo.as_deref(), Some("acme/widgets"));
            }
            other => panic!("expected Github step, got {other:?}"),
        }
    }
}
