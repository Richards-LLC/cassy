//! cas-d5c1 (GH #1023 finding 6): the reviewer's environment is checked
//! before an independent QA round is claimed.
//!
//! In the gabber-studio session, reviews stalled on setup rather than on the
//! product. Codex workers had no `gh` authentication. Worktrees lacked
//! `GABBER_BACKEND_ENV_FILE`, so no staging QA session could be minted. The
//! staging QA account ran out of credits twice mid-run. Each was found
//! partway into a 45-minute round.
//!
//! A project declares what its reviewers need under `[qa]`:
//! - `preflight_gh_token`: a GitHub read token must be available
//!   (`GH_TOKEN`/`GITHUB_TOKEN`, or an authenticated `gh`);
//! - `preflight_env_files`: environment variables that must name a readable
//!   file in the reviewer's environment (the path only, never its content);
//! - `preflight_hook`: a project-owned command that checks, and may
//!   replenish, test-account capacity. Exit 0 means ready; anything else is
//!   a blocker whose first output line is the reason. Cassy assumes no
//!   billing API: the hook owns that.
//!
//! Starting the QA work item runs the preflight. A blocker refuses the start,
//! so the round is not claimed and its deadline is not spent. The report
//! never carries a secret: token values are not read into it, env files are
//! reported by path, and hook output is redacted and truncated.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::config::QaConfig;

/// Bound on the `gh auth status` probe.
const GH_AUTH_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest hook output line carried into the report.
const HOOK_DETAIL_MAX: usize = 300;
/// Environment variables whose values are credentials a GitHub read grant
/// may arrive in.
const GH_TOKEN_VARS: [&str; 2] = ["GH_TOKEN", "GITHUB_TOKEN"];

/// One line of the preflight report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightCheck {
    pub name: String,
    pub ready: bool,
    pub detail: String,
}

/// The whole preflight, in the order the checks ran.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreflightReport {
    pub checks: Vec<PreflightCheck>,
}

impl PreflightReport {
    pub fn is_ready(&self) -> bool {
        self.checks.iter().all(|check| check.ready)
    }

    pub fn render(&self) -> String {
        self.checks
            .iter()
            .map(|check| {
                format!(
                    "- {} {}: {}",
                    if check.ready { "READY" } else { "MISSING" },
                    check.name,
                    check.detail
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// What the hook is told about the round it prepares for.
#[derive(Debug, Clone, Copy)]
pub struct PreflightContext<'a> {
    pub delivery_task: &'a str,
    pub qa_task: &'a str,
    pub head: &'a str,
}

/// Whether the project declared any preflight at all.
pub fn is_configured(qa: &QaConfig) -> bool {
    qa.preflight_gh_token
        || !qa.preflight_env_files.is_empty()
        || qa
            .preflight_hook
            .as_deref()
            .is_some_and(|hook| !hook.trim().is_empty())
}

/// Run the configured checks from the current process environment. `None`
/// when the project declares no preflight.
pub fn run(qa: &QaConfig, repo_root: &Path, ctx: PreflightContext<'_>) -> Option<PreflightReport> {
    run_with_env(qa, repo_root, ctx, &|name| std::env::var(name).ok())
}

/// [`run`] with an injected environment lookup (tests).
pub fn run_with_env(
    qa: &QaConfig,
    repo_root: &Path,
    ctx: PreflightContext<'_>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<PreflightReport> {
    if !is_configured(qa) {
        return None;
    }
    let mut report = PreflightReport::default();
    if qa.preflight_gh_token {
        report.checks.push(gh_read_token_check(repo_root, env));
    }
    for name in qa
        .preflight_env_files
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
    {
        report.checks.push(env_file_check(name, repo_root, env));
    }
    if let Some(hook) = qa
        .preflight_hook
        .as_deref()
        .map(str::trim)
        .filter(|hook| !hook.is_empty())
    {
        report.checks.push(hook_check(
            hook,
            repo_root,
            ctx,
            Duration::from_secs(u64::from(qa.preflight_hook_timeout_secs.max(1))),
            env,
        ));
    }
    Some(report)
}

fn non_empty(env: &dyn Fn(&str) -> Option<String>, name: &str) -> bool {
    env(name).is_some_and(|value| !value.trim().is_empty())
}

fn gh_read_token_check(repo_root: &Path, env: &dyn Fn(&str) -> Option<String>) -> PreflightCheck {
    let name = "GitHub read token".to_string();
    if let Some(var) = GH_TOKEN_VARS.iter().find(|var| non_empty(env, var)) {
        return PreflightCheck {
            name,
            ready: true,
            detail: format!("{var} is set (value not shown)"),
        };
    }
    let gh = env(super::github_gate::GH_BIN_ENV)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "gh".to_string());
    let mut command = Command::new(gh);
    command.args(["auth", "status"]).current_dir(repo_root);
    match run_bounded(command, GH_AUTH_TIMEOUT, &[]) {
        Some((true, _)) => PreflightCheck {
            name,
            ready: true,
            detail: "`gh auth status` reports an authenticated account".to_string(),
        },
        _ => PreflightCheck {
            name,
            ready: false,
            detail: "no GH_TOKEN or GITHUB_TOKEN in this worker's environment and `gh auth status` \
                     is not authenticated. Ask the operator to provision the read-only GitHub grant \
                     for this worker"
                .to_string(),
        },
    }
}

fn env_file_check(
    name: &str,
    repo_root: &Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> PreflightCheck {
    let check = |ready: bool, detail: String| PreflightCheck {
        name: format!("env file {name}"),
        ready,
        detail,
    };
    let Some(value) = env(name)
        .map(|value| value.trim().to_string())
        .filter(|v| !v.is_empty())
    else {
        return check(
            false,
            format!(
                "{name} is not set in this worker's environment; export the path (only the path) \
                 into the reviewer's worktree environment"
            ),
        );
    };
    let path = Path::new(&value);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo_root.join(path)
    };
    // Open it rather than stat it, so an unreadable file is reported too.
    // Nothing is read from it.
    match std::fs::File::open(&path) {
        Ok(file) if file.metadata().is_ok_and(|meta| meta.is_file()) => {
            check(true, format!("{name} → {} (readable)", path.display()))
        }
        Ok(_) => check(false, format!("{name} → {} is not a file", path.display())),
        Err(error) => check(
            false,
            format!(
                "{name} → {} is not readable here: {}",
                path.display(),
                error.kind()
            ),
        ),
    }
}

fn hook_check(
    hook: &str,
    repo_root: &Path,
    ctx: PreflightContext<'_>,
    timeout: Duration,
    env: &dyn Fn(&str) -> Option<String>,
) -> PreflightCheck {
    let mut command = Command::new("sh");
    command
        .args(["-c", hook])
        .current_dir(repo_root)
        .env("CAS_QA_DELIVERY_TASK", ctx.delivery_task)
        .env("CAS_QA_TASK", ctx.qa_task)
        .env("CAS_QA_HEAD", ctx.head);
    let secrets = secret_values(env);
    let name = "test-account capacity hook".to_string();
    match run_bounded(command, timeout, &secrets) {
        Some((true, output)) => PreflightCheck {
            name,
            ready: true,
            detail: first_line(&output).unwrap_or_else(|| "ready (exit 0)".to_string()),
        },
        Some((false, output)) => PreflightCheck {
            name,
            ready: false,
            detail: first_line(&output).unwrap_or_else(|| {
                "the hook exited non-zero without a reason; check the project's QA account"
                    .to_string()
            }),
        },
        None => PreflightCheck {
            name,
            ready: false,
            detail: format!(
                "the hook did not finish within {}s (or could not start)",
                timeout.as_secs()
            ),
        },
    }
}

fn first_line(output: &str) -> Option<String> {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let mut short: String = line.chars().take(HOOK_DETAIL_MAX).collect();
    if line.chars().count() > HOOK_DETAIL_MAX {
        short.push('…');
    }
    Some(short)
}

/// Values that must never reach the report: the GitHub token variables, and
/// any variable whose name marks it as a credential. Short values are left
/// alone so redaction cannot shred ordinary words.
fn secret_values(env: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let mut names: Vec<String> = GH_TOKEN_VARS.iter().map(|name| name.to_string()).collect();
    names.extend(std::env::vars().map(|(name, _)| name).filter(|name| {
        let upper = name.to_ascii_uppercase();
        [
            "TOKEN",
            "SECRET",
            "PASSWORD",
            "API_KEY",
            "PRIVATE_KEY",
            "CREDENTIAL",
        ]
        .iter()
        .any(|marker| upper.contains(marker))
    }));
    names
        .iter()
        .filter_map(|name| env(name))
        .map(|value| value.trim().to_string())
        .filter(|value| value.len() >= 8)
        .collect()
}

/// Replace every known secret value in `text`.
pub fn redact(text: &str, secrets: &[String]) -> String {
    secrets.iter().fold(text.to_string(), |text, secret| {
        text.replace(secret.as_str(), "[redacted]")
    })
}

/// Run with a deadline; `(success, redacted stdout+stderr)`.
fn run_bounded(
    mut command: Command,
    timeout: Duration,
    secrets: &[String],
) -> Option<(bool, String)> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut output = String::new();
                if let Some(mut stdout) = child.stdout.take() {
                    let _ = stdout.read_to_string(&mut output);
                }
                if let Some(mut stderr) = child.stderr.take() {
                    let mut err = String::new();
                    let _ = stderr.read_to_string(&mut err);
                    if !err.trim().is_empty() {
                        output.push('\n');
                        output.push_str(&err);
                    }
                }
                return Some((status.success(), redact(&output, secrets)));
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn ctx() -> PreflightContext<'static> {
        PreflightContext {
            delivery_task: "cas-a6cf",
            qa_task: "cas-qa01",
            head: "aaaa1111",
        }
    }

    fn lookup(vars: &HashMap<&str, String>) -> impl Fn(&str) -> Option<String> + '_ {
        move |name| vars.get(name).cloned()
    }

    fn qa(configure: impl FnOnce(&mut QaConfig)) -> QaConfig {
        let mut qa = QaConfig::default();
        configure(&mut qa);
        qa
    }

    #[test]
    fn nothing_configured_runs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let vars = HashMap::new();
        assert_eq!(
            run_with_env(&QaConfig::default(), dir.path(), ctx(), &lookup(&vars)),
            None
        );
    }

    #[test]
    fn env_file_missing_unset_and_ready_states() {
        let dir = tempfile::tempdir().unwrap();
        let config = qa(|qa| qa.preflight_env_files = vec!["GABBER_BACKEND_ENV_FILE".to_string()]);

        let unset = HashMap::new();
        let report = run_with_env(&config, dir.path(), ctx(), &lookup(&unset)).unwrap();
        assert!(!report.is_ready());
        assert!(
            report
                .render()
                .contains("GABBER_BACKEND_ENV_FILE is not set"),
            "{}",
            report.render()
        );

        let mut dangling = HashMap::new();
        dangling.insert(
            "GABBER_BACKEND_ENV_FILE",
            "/nonexistent/backend.env".to_string(),
        );
        let report = run_with_env(&config, dir.path(), ctx(), &lookup(&dangling)).unwrap();
        assert!(!report.is_ready());
        assert!(
            report.render().contains("is not readable here"),
            "{}",
            report.render()
        );

        let env_file = dir.path().join("backend.env");
        std::fs::write(&env_file, "STAGING_SECRET=do-not-print-me-123\n").unwrap();
        let mut ready = HashMap::new();
        ready.insert("GABBER_BACKEND_ENV_FILE", env_file.display().to_string());
        let report = run_with_env(&config, dir.path(), ctx(), &lookup(&ready)).unwrap();
        assert!(report.is_ready(), "{}", report.render());
        assert!(report.render().contains("(readable)"));
        assert!(
            !report.render().contains("do-not-print-me"),
            "the env file's content never reaches the report"
        );
    }

    #[test]
    fn gh_read_token_from_env_never_shows_its_value() {
        let dir = tempfile::tempdir().unwrap();
        let config = qa(|qa| qa.preflight_gh_token = true);
        let mut vars = HashMap::new();
        vars.insert("GH_TOKEN", "ghp_verysecretvalue000".to_string());
        let report = run_with_env(&config, dir.path(), ctx(), &lookup(&vars)).unwrap();
        assert!(report.is_ready());
        assert!(
            report
                .render()
                .contains("GH_TOKEN is set (value not shown)")
        );
        assert!(!report.render().contains("ghp_verysecretvalue000"));
    }

    #[cfg(unix)]
    #[test]
    fn gh_read_token_missing_when_gh_is_not_authenticated() {
        let dir = tempfile::tempdir().unwrap();
        let config = qa(|qa| qa.preflight_gh_token = true);
        // A `gh` that reports "not logged in".
        let mut vars = HashMap::new();
        vars.insert(super::super::github_gate::GH_BIN_ENV, "false".to_string());
        let report = run_with_env(&config, dir.path(), ctx(), &lookup(&vars)).unwrap();
        assert!(!report.is_ready());
        assert!(
            report.render().contains("read-only GitHub grant"),
            "{}",
            report.render()
        );
        // An authenticated one.
        vars.insert(super::super::github_gate::GH_BIN_ENV, "true".to_string());
        let report = run_with_env(&config, dir.path(), ctx(), &lookup(&vars)).unwrap();
        assert!(report.is_ready(), "{}", report.render());
    }

    #[cfg(unix)]
    #[test]
    fn capacity_hook_ready_blocked_and_timeout_states() {
        let dir = tempfile::tempdir().unwrap();
        let vars = HashMap::new();
        let with_hook = |hook: &str, timeout: u32| {
            qa(|qa| {
                qa.preflight_hook = Some(hook.to_string());
                qa.preflight_hook_timeout_secs = timeout;
            })
        };

        // Ready, after topping up; the hook sees which round it serves.
        let ready = run_with_env(
            &with_hook(
                "echo \"topped up qa-staging-creator for $CAS_QA_DELIVERY_TASK\"",
                10,
            ),
            dir.path(),
            ctx(),
            &lookup(&vars),
        )
        .unwrap();
        assert!(ready.is_ready(), "{}", ready.render());
        assert!(
            ready
                .render()
                .contains("topped up qa-staging-creator for cas-a6cf")
        );

        // A blocker, with its reason.
        let blocked = run_with_env(
            &with_hook(
                "echo 'qa-staging-creator has 0 credits; top-up refused' >&2; exit 3",
                10,
            ),
            dir.path(),
            ctx(),
            &lookup(&vars),
        )
        .unwrap();
        assert!(!blocked.is_ready());
        assert!(
            blocked.render().contains("0 credits; top-up refused"),
            "{}",
            blocked.render()
        );

        // A hung hook is a blocker, not a hang.
        let hung =
            run_with_env(&with_hook("sleep 5", 1), dir.path(), ctx(), &lookup(&vars)).unwrap();
        assert!(!hung.is_ready());
        assert!(
            hung.render().contains("did not finish within 1s"),
            "{}",
            hung.render()
        );
    }

    #[test]
    fn redaction_removes_known_secret_values() {
        let secrets = vec!["sk_live_abcdef123456".to_string()];
        assert_eq!(
            redact("balance ok for sk_live_abcdef123456", &secrets),
            "balance ok for [redacted]"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hook_output_is_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let mut vars = HashMap::new();
        vars.insert("GH_TOKEN", "ghp_leakyleakyleaky".to_string());
        let config =
            qa(|qa| qa.preflight_hook = Some("echo using ghp_leakyleakyleaky".to_string()));
        let report = run_with_env(&config, dir.path(), ctx(), &lookup(&vars)).unwrap();
        assert!(
            report.render().contains("using [redacted]"),
            "{}",
            report.render()
        );
        assert!(!report.render().contains("ghp_leakyleakyleaky"));
    }
}
