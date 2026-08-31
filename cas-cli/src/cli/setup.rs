//! Guided machine setup.
//!
//! `cas setup` is the front door for a newly installed binary.  It deliberately
//! coordinates the existing commands instead of reimplementing their side
//! effects, while keeping the status model here so a missing optional service
//! or a pending browser ceremony cannot strand the user in the middle of the
//! flow.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;

use super::Cli;
use super::auth::{AuthCommands, LoginArgs};
use super::interactive;
use crate::cloud::{CloudConfig, DeviceConfig};

const PATH_MARKER_BEGIN: &str = "# >>> cassy path >>>";
const PATH_MARKER_END: &str = "# <<< cassy path <<<";

/// Run the complete machine setup flow.
#[derive(Args, Debug, Clone, Default)]
pub struct SetupArgs {
    /// Accept safe defaults without prompting. Authentication still reports
    /// action-needed when no token or existing login is available.
    #[arg(long, short = 'y')]
    pub yes: bool,

    /// Show the fresh-machine plan without changing files or contacting a
    /// service.
    #[arg(long)]
    pub dry_run: bool,

    /// Personal API token for the non-browser login fallback.
    #[arg(long, env = "CAS_CLOUD_TOKEN", hide_env_values = true)]
    pub token: Option<String>,

    /// Cassy Cloud API endpoint used by the token fallback.
    #[arg(
        long,
        env = "CAS_CLOUD_ENDPOINT",
        default_value = "https://petra-stella-cloud.vercel.app"
    )]
    pub endpoint: String,

    /// Initialize and sync this first project after machine setup.
    #[arg(long, value_name = "DIR")]
    pub project: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum StepStatus {
    Ok,
    Skipped,
    ActionNeeded,
}

impl StepStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Skipped => "skipped",
            Self::ActionNeeded => "action-needed",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct SetupStep {
    number: u8,
    name: &'static str,
    status: StepStatus,
    command: String,
    detail: String,
}

impl SetupStep {
    fn new(
        number: u8,
        name: &'static str,
        status: StepStatus,
        command: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            number,
            name,
            status,
            command: command.into(),
            detail: detail.into(),
        }
    }
}

/// Execute all setup steps and always render the final status table.
pub fn execute(args: &SetupArgs, cli: &Cli) -> Result<()> {
    let mut steps = Vec::with_capacity(7);
    steps.push(environment_step(args));
    steps.push(cloud_step(args, cli));
    steps.push(pairing_step(args, cli));
    steps.push(hub_step(args));
    steps.push(viktor_step(args));
    steps.push(project_step(args));

    let final_status = if steps
        .iter()
        .any(|step| step.status == StepStatus::ActionNeeded)
    {
        StepStatus::ActionNeeded
    } else {
        StepStatus::Ok
    };
    let final_detail = if args.dry_run {
        "Plan complete; no changes or external commands were run."
    } else if final_status == StepStatus::Ok {
        "All requested setup steps completed or were already configured."
    } else {
        "Follow the action-needed instructions above, then rerun `cas setup`."
    };
    steps.push(SetupStep::new(
        7,
        "Final status",
        final_status,
        "cas setup",
        final_detail,
    ));

    render_steps(&steps, args.dry_run, cli)
}

fn environment_step(args: &SetupArgs) -> SetupStep {
    let home = dirs::home_dir();
    let local_bin = home.as_deref().map(|path| path.join(".local/bin"));
    let path_ready = local_bin.as_deref().is_some_and(path_contains);
    let version = match current_version() {
        Ok(version) => version,
        Err(_) => {
            return SetupStep::new(
                1,
                "Environment",
                StepStatus::ActionNeeded,
                "cas --version",
                "The running Cassy binary could not report its version.",
            );
        }
    };

    let Some(local_bin) = local_bin else {
        return SetupStep::new(
            1,
            "Environment",
            StepStatus::ActionNeeded,
            "cas --version",
            "Cannot determine the home directory; verify the install and PATH manually.",
        );
    };

    if path_ready {
        return SetupStep::new(
            1,
            "Environment",
            StepStatus::Ok,
            "cas --version",
            format!("{} is already on PATH; {version}.", local_bin.display()),
        );
    }

    let Some(rc_file) = shell_rc(home.as_deref().expect("home exists")) else {
        return SetupStep::new(
            1,
            "Environment",
            StepStatus::ActionNeeded,
            "cas --version",
            format!(
                "Add `export PATH=\"{}:$PATH\"` to your shell startup file, then open a new terminal.",
                local_bin.display()
            ),
        );
    };

    if has_path_guard(&rc_file) {
        return SetupStep::new(
            1,
            "Environment",
            StepStatus::Ok,
            "cas --version",
            format!("PATH guard already exists in {}.", rc_file.display()),
        );
    }

    if args.dry_run {
        return SetupStep::new(
            1,
            "Environment",
            StepStatus::Skipped,
            "cas --version",
            format!(
                "{version}; dry-run would offer a PATH guard in {}: `export PATH=\"{}:$PATH\"`.",
                rc_file.display(),
                local_bin.display()
            ),
        );
    }

    let accepted = if args.yes {
        true
    } else {
        match interactive::confirm(
            &format!("Add {} to {}", local_bin.display(), rc_file.display()),
            true,
        ) {
            Ok(value) => value,
            Err(error) => {
                return SetupStep::new(
                    1,
                    "Environment",
                    StepStatus::ActionNeeded,
                    "cas --version",
                    format!(
                        "Could not ask about PATH ({error}); add `export PATH=\"{}:$PATH\"` manually.",
                        local_bin.display()
                    ),
                );
            }
        }
    };

    if !accepted {
        return SetupStep::new(
            1,
            "Environment",
            StepStatus::ActionNeeded,
            "cas --version",
            format!(
                "Add `export PATH=\"{}:$PATH\"` to {} when ready.",
                local_bin.display(),
                rc_file.display()
            ),
        );
    }

    match append_path_guard(&local_bin, &rc_file) {
        Ok(()) => SetupStep::new(
            1,
            "Environment",
            StepStatus::Ok,
            "cas --version",
            format!(
                "{version}; added an idempotent PATH guard to {}; open a new terminal to load it.",
                rc_file.display()
            ),
        ),
        Err(error) => SetupStep::new(
            1,
            "Environment",
            StepStatus::ActionNeeded,
            "cas --version",
            format!(
                "Could not update {} ({error}); add the PATH line manually.",
                rc_file.display()
            ),
        ),
    }
}

fn cloud_step(args: &SetupArgs, cli: &Cli) -> SetupStep {
    let mut config = CloudConfig::load_effective();
    if !config.is_logged_in() {
        if args.dry_run || (cli.json && args.token.is_none()) || (args.token.is_none() && args.yes)
        {
            let command = if args.token.is_some() {
                "cas login --token <API-TOKEN>"
            } else {
                "cas login"
            };
            return SetupStep::new(
                2,
                "Cloud login + team",
                StepStatus::ActionNeeded,
                command,
                "Browser-first login is ready; use the token command when a browser is unavailable.",
            );
        }

        let login = LoginArgs {
            token: args.token.clone(),
            endpoint: args.endpoint.clone(),
            no_browser: false,
        };
        if let Err(error) = super::auth::execute(&AuthCommands::Login(login), cli) {
            return SetupStep::new(
                2,
                "Cloud login + team",
                StepStatus::ActionNeeded,
                "cas login",
                format!("Login did not complete: {error}"),
            );
        }
        config = CloudConfig::load_effective();
        if !config.is_logged_in() {
            return SetupStep::new(
                2,
                "Cloud login + team",
                StepStatus::ActionNeeded,
                "cas login",
                "Login did not produce a stored session; rerun `cas login` and finish the browser ceremony.",
            );
        }
    }

    let mut user_config = match CloudConfig::load_user() {
        Ok(config) => config,
        Err(error) => {
            return SetupStep::new(
                2,
                "Cloud login + team",
                StepStatus::ActionNeeded,
                "cas cloud team show",
                format!("Logged in, but cached team membership could not be read: {error}"),
            );
        }
    };

    if let Some(default_id) = user_config.default_team_id.as_deref()
        && user_config.teams.iter().any(|team| team.id == default_id)
    {
        return SetupStep::new(
            2,
            "Cloud login + team",
            StepStatus::Ok,
            "cas cloud team show",
            format!("Logged in; default team is {}.", default_id),
        );
    }

    if user_config.teams.is_empty() {
        return SetupStep::new(
            2,
            "Cloud login + team",
            StepStatus::Skipped,
            "cas cloud team show",
            "Logged in with no cached team memberships; personal scope remains available.",
        );
    }

    if user_config.teams.len() == 1 {
        let team = &user_config.teams[0];
        if args.dry_run {
            return SetupStep::new(
                2,
                "Cloud login + team",
                StepStatus::Skipped,
                format!("cas cloud team default {}", team.slug),
                format!("Dry-run would select the only cached team, {}.", team.name),
            );
        }
        user_config.default_team_id = Some(team.id.clone());
        return match user_config.save_user() {
            Ok(()) => SetupStep::new(
                2,
                "Cloud login + team",
                StepStatus::Ok,
                format!("cas cloud team default {}", team.slug),
                format!("Selected the only cached team, {}.", team.name),
            ),
            Err(error) => SetupStep::new(
                2,
                "Cloud login + team",
                StepStatus::ActionNeeded,
                format!("cas cloud team default {}", team.slug),
                format!("Could not save the default team: {error}"),
            ),
        };
    }

    if args.dry_run || args.yes {
        let choices = user_config
            .teams
            .iter()
            .map(|team| team.slug.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return SetupStep::new(
            2,
            "Cloud login + team",
            StepStatus::ActionNeeded,
            "cas cloud team default <slug>",
            format!("Choose one cached team before syncing: {choices}."),
        );
    }

    let options = user_config
        .teams
        .iter()
        .map(|team| format!("{} ({})", team.name, team.slug))
        .collect::<Vec<_>>();
    let option_refs = options.iter().map(String::as_str).collect::<Vec<_>>();
    let selection = match interactive::select("Choose the default Cassy Cloud team", &option_refs) {
        Ok(selection) => selection,
        Err(error) => {
            return SetupStep::new(
                2,
                "Cloud login + team",
                StepStatus::ActionNeeded,
                "cas cloud team default <slug>",
                format!("Team selection needs attention: {error}"),
            );
        }
    };
    let team = &user_config.teams[selection];
    user_config.default_team_id = Some(team.id.clone());
    match user_config.save_user() {
        Ok(()) => SetupStep::new(
            2,
            "Cloud login + team",
            StepStatus::Ok,
            format!("cas cloud team default {}", team.slug),
            format!("Selected {}.", team.name),
        ),
        Err(error) => SetupStep::new(
            2,
            "Cloud login + team",
            StepStatus::ActionNeeded,
            format!("cas cloud team default {}", team.slug),
            format!("Could not save the selected team: {error}"),
        ),
    }
}

fn pairing_step(args: &SetupArgs, cli: &Cli) -> SetupStep {
    let registered = DeviceConfig::load().ok().flatten().is_some();
    if registered {
        return SetupStep::new(
            3,
            "Machine pairing",
            StepStatus::Ok,
            "cas device register",
            "This machine is already registered; Commander pairing remains browser-led.",
        );
    }

    if !CloudConfig::load_effective().is_logged_in() {
        return SetupStep::new(
            3,
            "Machine pairing",
            StepStatus::Skipped,
            "cas device register",
            "Login is required first. After registration, open Commander and choose Pair this machine.",
        );
    }

    if args.dry_run || cli.json {
        return SetupStep::new(
            3,
            "Machine pairing",
            StepStatus::Skipped,
            "cas device register",
            "Dry-run leaves registration untouched; then open Commander and choose Pair this machine.",
        );
    }

    if !args.yes {
        match interactive::confirm("Register this machine with Cassy Cloud", true) {
            Ok(false) => {
                return SetupStep::new(
                    3,
                    "Machine pairing",
                    StepStatus::Skipped,
                    "cas device register",
                    "Registration skipped. Run it later, then open Commander and choose Pair this machine.",
                );
            }
            Err(error) => {
                return SetupStep::new(
                    3,
                    "Machine pairing",
                    StepStatus::ActionNeeded,
                    "cas device register",
                    format!("Could not ask about registration: {error}"),
                );
            }
            Ok(true) => {}
        }
    }

    match run_cas_capture(&["device", "register"], None) {
        Ok(output) if output.status.success() => SetupStep::new(
            3,
            "Machine pairing",
            StepStatus::Ok,
            "cas device register",
            "Machine registered. Open Commander and choose Pair this machine to finish browser pairing.",
        ),
        Ok(output) => SetupStep::new(
            3,
            "Machine pairing",
            StepStatus::ActionNeeded,
            "cas device register",
            format!("Registration failed: {}", summarize_output(&output)),
        ),
        Err(error) => SetupStep::new(
            3,
            "Machine pairing",
            StepStatus::ActionNeeded,
            "cas device register",
            format!("Could not run registration: {error}"),
        ),
    }
}

fn hub_step(args: &SetupArgs) -> SetupStep {
    if args.dry_run {
        return SetupStep::new(
            4,
            "Hub service",
            StepStatus::Skipped,
            "cas hub service install",
            "Dry-run would install and start the platform-native user service.",
        );
    }

    if let Ok(output) = run_cas_capture(&["--json", "hub", "service", "status"], None) {
        if output.status.success() && service_is_ready(&output) {
            return SetupStep::new(
                4,
                "Hub service",
                StepStatus::Ok,
                "cas hub service install",
                "Hub service is already installed and active.",
            );
        }
    }

    match run_cas_capture(&["--json", "hub", "service", "install"], None) {
        Ok(output) if output.status.success() && service_is_ready(&output) => SetupStep::new(
            4,
            "Hub service",
            StepStatus::Ok,
            "cas hub service install",
            "Hub service is installed and started (the command is idempotent).",
        ),
        Ok(output) => SetupStep::new(
            4,
            "Hub service",
            StepStatus::ActionNeeded,
            "cas hub service install",
            format!("Hub service needs attention: {}", summarize_output(&output)),
        ),
        Err(error) => SetupStep::new(
            4,
            "Hub service",
            StepStatus::ActionNeeded,
            "cas hub service install",
            format!("Could not run the native hub service command: {error}"),
        ),
    }
}

fn service_is_ready(output: &Output) -> bool {
    serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .ok()
        .and_then(|report| {
            Some(report.get("installed")?.as_bool()? && report.get("active")?.as_bool()?)
        })
        .unwrap_or(false)
}

fn viktor_step(args: &SetupArgs) -> SetupStep {
    let configured = std::env::var_os("VIKTOR_API_KEY").is_some()
        || super::viktor::load_machine_credential()
            .ok()
            .flatten()
            .is_some();
    if configured {
        return SetupStep::new(
            5,
            "Viktor key",
            StepStatus::Ok,
            "cas viktor key",
            "Viktor credentials are already configured for this machine.",
        );
    }
    if args.dry_run || args.yes {
        return SetupStep::new(
            5,
            "Viktor key",
            StepStatus::Skipped,
            "cas viktor key",
            "Optional; run the command later with an operator-issued key if Viktor is needed.",
        );
    }

    match interactive::confirm("Configure the optional Viktor gateway now", false) {
        Ok(false) => SetupStep::new(
            5,
            "Viktor key",
            StepStatus::Skipped,
            "cas viktor key",
            "Skipped; run `cas viktor key` later with an operator-issued key.",
        ),
        Err(error) => SetupStep::new(
            5,
            "Viktor key",
            StepStatus::ActionNeeded,
            "cas viktor key",
            format!("Could not ask about the optional key: {error}"),
        ),
        Ok(true) => match run_cas_inherited(&["viktor", "key"], None) {
            Ok(status) if status.success() => SetupStep::new(
                5,
                "Viktor key",
                StepStatus::Ok,
                "cas viktor key",
                "Viktor key was saved for this machine.",
            ),
            Ok(status) => SetupStep::new(
                5,
                "Viktor key",
                StepStatus::ActionNeeded,
                "cas viktor key",
                format!("Viktor key command exited with {status}."),
            ),
            Err(error) => SetupStep::new(
                5,
                "Viktor key",
                StepStatus::ActionNeeded,
                "cas viktor key",
                format!("Could not run Viktor key setup: {error}"),
            ),
        },
    }
}

fn project_step(args: &SetupArgs) -> SetupStep {
    let Some(project) = args.project.as_deref() else {
        return SetupStep::new(
            6,
            "First project",
            StepStatus::Skipped,
            "cas setup --project <DIR>",
            "Optional; pass --project DIR to initialize and sync a first project.",
        );
    };
    let command = format!("cas setup --project {}", project.display());
    if !project.is_dir() {
        return SetupStep::new(
            6,
            "First project",
            StepStatus::ActionNeeded,
            command,
            format!(
                "Project directory {} does not exist yet.",
                project.display()
            ),
        );
    }
    if args.dry_run {
        return SetupStep::new(
            6,
            "First project",
            StepStatus::Skipped,
            format!(
                "cd {} && cas init --yes && cas cloud sync",
                project.display()
            ),
            "Dry-run would initialize the project and sync it when logged in.",
        );
    }

    let init = match run_cas_capture(&["init", "--yes"], Some(project)) {
        Ok(output) => output,
        Err(error) => {
            return SetupStep::new(
                6,
                "First project",
                StepStatus::ActionNeeded,
                command,
                format!("Could not initialize the project: {error}"),
            );
        }
    };
    if !init.status.success() || !project.join(".cas").is_dir() {
        return SetupStep::new(
            6,
            "First project",
            StepStatus::ActionNeeded,
            "cas init --yes",
            format!("Project initialization failed: {}", summarize_output(&init)),
        );
    }

    if !CloudConfig::load_effective().is_logged_in() {
        return SetupStep::new(
            6,
            "First project",
            StepStatus::Skipped,
            "cas cloud sync",
            "Project initialized; sync skipped until `cas login` is complete.",
        );
    }
    match run_cas_capture(&["cloud", "sync"], Some(project)) {
        Ok(output) if output.status.success() => SetupStep::new(
            6,
            "First project",
            StepStatus::Ok,
            "cas init --yes && cas cloud sync",
            format!(
                "Project initialized and synced: {}",
                summarize_output(&output)
            ),
        ),
        Ok(output) => SetupStep::new(
            6,
            "First project",
            StepStatus::ActionNeeded,
            "cas cloud sync",
            format!(
                "Project initialized, but sync needs attention: {}",
                summarize_output(&output)
            ),
        ),
        Err(error) => SetupStep::new(
            6,
            "First project",
            StepStatus::ActionNeeded,
            "cas cloud sync",
            format!("Project initialized, but sync could not run: {error}"),
        ),
    }
}

fn render_steps(steps: &[SetupStep], dry_run: bool, cli: &Cli) -> Result<()> {
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "command": "setup",
                "dry_run": dry_run,
                "steps": steps,
            }))?
        );
        return Ok(());
    }

    println!(
        "\nCassy setup{}",
        if dry_run {
            " (dry-run; no changes)"
        } else {
            ""
        }
    );
    println!("\nSetup status");
    println!("{:<4} {:<14} {:<28} Details", "Step", "Status", "Command");
    println!("{}", "─".repeat(100));
    for step in steps {
        println!(
            "{:<4} {:<14} {:<28} {}",
            step.number,
            step.status.label(),
            step.command,
            step.detail
        );
    }
    Ok(())
}

fn current_version() -> Result<String> {
    let output = run_cas_capture(&["--version"], None)?;
    if !output.status.success() {
        anyhow::bail!("cas --version exited with {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn current_binary() -> Result<PathBuf> {
    std::env::current_exe().context("resolve the running cas binary")
}

fn run_cas_capture(args: &[&str], cwd: Option<&Path>) -> Result<Output> {
    let mut command = Command::new(current_binary()?);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command.output().context("run a Cassy setup command")
}

fn run_cas_inherited(args: &[&str], cwd: Option<&Path>) -> Result<std::process::ExitStatus> {
    let mut command = Command::new(current_binary()?);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command
        .status()
        .context("run an interactive Cassy setup command")
}

fn summarize_output(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{} {}", stdout.trim(), stderr.trim());
    let combined = combined.split_whitespace().collect::<Vec<_>>().join(" ");
    if combined.is_empty() {
        format!("exit status {}", output.status)
    } else {
        combined.chars().take(240).collect()
    }
}

fn path_contains(dir: &Path) -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|entry| entry == dir))
        .unwrap_or(false)
}

fn shell_rc(home: &Path) -> Option<PathBuf> {
    let shell = std::env::var_os("SHELL")
        .and_then(|shell| shell.into_string().ok())
        .and_then(|shell| PathBuf::from(shell).file_name().map(|name| name.to_owned()))
        .and_then(|name| name.into_string().ok())
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "zsh".to_string()
            } else {
                "bash".to_string()
            }
        });
    match shell.as_str() {
        "zsh" => Some(
            std::env::var_os("ZDOTDIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.to_path_buf())
                .join(".zshenv"),
        ),
        "bash" => Some(if home.join(".bashrc").exists() {
            home.join(".bashrc")
        } else {
            home.join(".profile")
        }),
        _ => None,
    }
}

fn has_path_guard(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|content| content.contains(PATH_MARKER_BEGIN) && content.contains(PATH_MARKER_END))
        .unwrap_or(false)
}

fn append_path_guard(dir: &Path, path: &Path) -> Result<()> {
    if has_path_guard(path) {
        return Ok(());
    }
    if path.exists() && fs::symlink_metadata(path)?.file_type().is_symlink() {
        anyhow::bail!("refusing to follow symlinked shell startup file")
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("create shell startup directory")?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    write!(
        file,
        "\n{PATH_MARKER_BEGIN}\n# Added by `cas setup`; keeps the canonical Cassy install on PATH.\ncase \":$PATH:\" in\n  *\":{}:\"*) ;;\n  *) export PATH=\"{}:$PATH\" ;;\nesac\n{PATH_MARKER_END}\n",
        dir.display(),
        dir.display()
    )?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_serializes_with_action_needed_spelling() {
        assert_eq!(
            serde_json::to_string(&StepStatus::ActionNeeded).unwrap(),
            "\"action-needed\""
        );
    }

    #[test]
    fn path_guard_is_idempotently_detected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(".bashrc");
        append_path_guard(Path::new("/tmp/cas-bin"), &path).unwrap();
        assert!(has_path_guard(&path));
        let before = fs::read_to_string(&path).unwrap();
        append_path_guard(Path::new("/tmp/cas-bin"), &path).unwrap();
        assert_eq!(before, fs::read_to_string(&path).unwrap());
    }
}
