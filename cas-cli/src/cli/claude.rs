//! `cas claude` — launch the CAS factory with a Claude supervisor on an
//! explicitly selected account profile.
//!
//! `cas claude alt` is the operator-facing spelling for "run CAS, supervised by
//! Claude, signed in as my alt subscription". It is the Claude sibling of
//! `cas codex` / `cas grok`, with one extra positional: the account profile.
//!
//! Named-account selection exports both `CLAUDE_CONFIG_DIR` and Claude Code's
//! secure storage selector before the factory starts. `main` clears both to use
//! Claude Code's default `~/.claude` behavior. Panes inherit that boundary
//! (`cas-pty` never calls `env_clear`), so the supervisor runs on the chosen
//! account, and `spawn_workers` captures the same config directory as
//! `requester_config_dir` so workers land on that account too.
//!
//! On macOS, Claude Code stores OAuth credentials in Keychain. An unscoped
//! process uses the legacy `Claude Code-credentials` item; a named secure
//! storage directory uses `Claude Code-credentials-<path hash>`. The main
//! profile intentionally retains the legacy item for compatibility, while
//! named profiles receive distinct hashed items. Claude Code falls back to
//! `<secure-storage-dir>/.credentials.json` when Keychain is unavailable.
//!
//! `--bare` keeps the plain "just open Claude Code on this profile" launcher.

use std::ffi::OsString;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use clap::{Args, FromArgMatches, Subcommand};
use serde::Deserialize;

use super::Cli;
use super::factory::FactoryArgs;

/// Arguments for `cas claude [profile] [factory-args...]`.
#[derive(Args, Clone, Debug)]
#[command(subcommand_precedence_over_arg = true)]
pub struct ClaudeArgs {
    #[command(subcommand)]
    pub command: Option<ClaudeCommand>,

    /// Account profile: `main` maps to ~/.claude; any other name maps to ~/.claude-<name>.
    ///
    /// Omit to use whichever account the current environment already selects.
    pub profile: Option<String>,

    /// List detected account profiles with login state and exit.
    #[arg(long = "list-profiles")]
    pub list_profiles: bool,

    /// Launch plain Claude Code on this profile instead of the CAS factory.
    #[arg(long = "bare")]
    pub bare: bool,

    /// Remaining arguments: `cas factory` flags, or Claude Code flags with `--bare`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<OsString>,
}

#[derive(Subcommand, Clone, Debug)]
pub enum ClaudeCommand {
    /// Sign in to exactly one Claude account profile.
    Login {
        /// Account profile: `main` maps to ~/.claude; any other name maps to ~/.claude-<name>.
        profile: String,

        /// Remaining arguments passed to `claude auth login`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoginState {
    LoggedIn,
    LoggedOut,
    Unknown,
}

#[derive(Debug, Eq, PartialEq)]
struct Profile {
    name: String,
    directory: PathBuf,
    login_state: LoginState,
    active: bool,
}

#[derive(Deserialize)]
struct ClaudeAuthStatus {
    #[serde(rename = "loggedIn")]
    logged_in: bool,
}

/// Resolve a convention-based profile name under `home`.
pub(crate) fn resolve_profile_dir(home: &Path, profile: &str) -> PathBuf {
    if profile == "main" {
        home.join(".claude")
    } else {
        home.join(format!(".claude-{profile}"))
    }
}

fn scan_profiles(home: &Path, active_dir: Option<&Path>) -> io::Result<Vec<Profile>> {
    scan_profiles_with(home, active_dir, probe_login_state)
}

/// Sibling directories that share the `.claude-` prefix without being accounts.
///
/// Lock and scratch directories are created next to a profile by other tooling
/// (`~/.claude-support@example.com.lock`). Listing one as a selectable account
/// offers the operator a choice that cannot work.
const NON_ACCOUNT_SUFFIXES: [&str; 5] = [".lock", ".tmp", ".bak", ".old", ".backup"];

/// Whether a `.claude-<name>` directory names a real account profile.
fn is_account_profile_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let lowered = name.to_ascii_lowercase();
    if NON_ACCOUNT_SUFFIXES
        .iter()
        .any(|suffix| lowered.ends_with(suffix))
    {
        return false;
    }
    // Dotted scratch names like `foo.bak.1777306474` are not accounts either,
    // but `support@petrastella.io` must survive: only reject a marker segment.
    !lowered
        .rsplit('.')
        .any(|segment| matches!(segment, "lock" | "tmp" | "bak" | "old" | "backup"))
}

fn scan_profiles_with(
    home: &Path,
    active_dir: Option<&Path>,
    mut login_state_for: impl FnMut(&str, &Path) -> LoginState,
) -> io::Result<Vec<Profile>> {
    let mut profiles = vec![profile_for(
        "main",
        home.join(".claude"),
        active_dir,
        &mut login_state_for,
    )];

    if let Ok(entries) = std::fs::read_dir(home) {
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(file_name) = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            let Some(profile_name) = file_name.strip_prefix(".claude-") else {
                continue;
            };
            if !is_account_profile_name(profile_name) {
                continue;
            }
            profiles.push(profile_for(
                profile_name,
                path,
                active_dir,
                &mut login_state_for,
            ));
        }
    }

    profiles[1..].sort_by(|left, right| left.name.cmp(&right.name));
    Ok(profiles)
}

fn profile_for(
    name: &str,
    directory: PathBuf,
    active_dir: Option<&Path>,
    login_state_for: &mut impl FnMut(&str, &Path) -> LoginState,
) -> Profile {
    Profile {
        name: name.to_string(),
        login_state: login_state_for(name, &directory),
        active: active_dir.map_or(name == "main", |active| active == directory),
        directory,
    }
}

/// Ask Claude Code itself whether the exact profile credential is usable.
///
/// File existence is not sufficient: macOS normally uses Keychain, and a
/// stale `.credentials.json` may contain an expired or revoked credential.
fn probe_login_state(profile: &str, profile_dir: &Path) -> LoginState {
    let mut command = Command::new("claude");
    configure_profile_command(&mut command, profile, profile_dir);
    command.args(["auth", "status", "--json"]);

    let Ok(output) = command.output() else {
        return LoginState::Unknown;
    };
    let Ok(status) = serde_json::from_slice::<ClaudeAuthStatus>(&output.stdout) else {
        return LoginState::Unknown;
    };
    if status.logged_in {
        LoginState::LoggedIn
    } else {
        LoginState::LoggedOut
    }
}

fn profile_listing(home: &Path, active_dir: Option<&Path>) -> Result<String> {
    let profiles = scan_profiles(home, active_dir)
        .with_context(|| format!("could not inspect Claude profiles under {}", home.display()))?;
    let mut output = String::from(
        "Usage: cas claude <profile> [factory-args...]\n\nDetected Claude profiles:\n",
    );

    for profile in profiles {
        let login = match profile.login_state {
            LoginState::LoggedIn => "logged in",
            LoginState::LoggedOut => "not logged in",
            LoginState::Unknown => "login state unknown",
        };
        let active = if profile.active { " (active)" } else { "" };
        output.push_str(&format!(
            "  {} — {} ({login}){active}\n",
            profile.name,
            profile.directory.display(),
        ));
    }

    Ok(output)
}

/// Bind a Claude subprocess to one profile's config and credential store.
fn configure_profile_command(command: &mut Command, profile: &str, profile_dir: &Path) {
    if profile == "main" {
        // Claude's default, unscoped process owns ~/.claude and the legacy
        // Keychain item. Removing both selectors also prevents an ambient alt
        // profile from leaking into an explicit `main` selection.
        command
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CLAUDE_SECURESTORAGE_CONFIG_DIR");
    } else {
        command
            .env("CLAUDE_CONFIG_DIR", profile_dir)
            .env("CLAUDE_SECURESTORAGE_CONFIG_DIR", profile_dir);
    }
    command
        // Explicit account selection must win over inherited credentials.
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .env_remove("CLAUDE_CODE_OAUTH_REFRESH_TOKEN")
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR");
}

/// Return whether a bare launch should ask the user to select an account.
///
/// A profile explicitly named by the caller always wins. We must also leave
/// pipe/script launches alone: a prompt there would either hang or consume
/// caller input that belongs to Claude Code.
/// Gate on DETECTED accounts, not confirmed-logged-in ones.
///
/// Counting only `LoggedIn` made every probe failure silently degrade to
/// "launch the default account": a missing `claude` binary at probe time, an
/// auth-output shape change, or a keychain timeout all return `Unknown`, drop
/// the profile from the count, and skip the prompt with no indication. With
/// more than one account on disk the operator must be asked regardless of what
/// the probe could determine.
fn should_prompt_for_profile(
    explicit_profile: Option<&str>,
    profiles: &[Profile],
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
) -> bool {
    explicit_profile.is_none() && stdin_is_terminal && stdout_is_terminal && profiles.len() > 1
}

/// One selectable row in the account picker.
#[derive(Clone, Debug, Eq, PartialEq)]
enum ProfileChoice {
    Existing { name: String, label: String },
    NewLogin,
}

impl std::fmt::Display for ProfileChoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileChoice::Existing { label, .. } => formatter.write_str(label),
            ProfileChoice::NewLogin => formatter.write_str("+ Log in a new account…"),
        }
    }
}

fn login_state_label(state: LoginState) -> &'static str {
    match state {
        LoginState::LoggedIn => "logged in",
        LoginState::LoggedOut => "not logged in",
        LoginState::Unknown => "login state unknown",
    }
}

/// Build the picker rows: every detected profile, then the new-login entry.
///
/// Nothing is filtered out here. A logged-out or unknown account stays
/// selectable and is labelled as such, so the operator sees the real state of
/// the box instead of a silently shortened list.
fn profile_choices(profiles: &[Profile]) -> Vec<ProfileChoice> {
    let mut choices: Vec<ProfileChoice> = profiles
        .iter()
        .map(|profile| ProfileChoice::Existing {
            name: profile.name.clone(),
            label: format!(
                "{} — {}{}",
                profile.name,
                login_state_label(profile.login_state),
                if profile.active { ", active" } else { "" }
            ),
        })
        .collect();
    choices.push(ProfileChoice::NewLogin);
    choices
}

/// Index of the account the environment currently points at, for cursor start.
fn active_choice_index(profiles: &[Profile]) -> usize {
    profiles.iter().position(|profile| profile.active).unwrap_or(0)
}

/// Show the whole list when it plausibly fits.
///
/// inquire's default page is 7. This operator has 7 accounts, which pushed
/// "log in a new account" below the fold — an option you must scroll to find
/// is an option most people never discover.
fn picker_page_size(choice_count: usize) -> usize {
    choice_count.clamp(7, 20)
}

/// Prompt for an account. Returns the chosen profile name.
fn prompt_for_profile(home: &Path, profiles: &[Profile]) -> Result<String> {
    let choices = profile_choices(profiles);
    let starting_cursor = active_choice_index(profiles);
    let page_size = picker_page_size(choices.len());

    let selection = inquire::Select::new("Choose Claude account", choices)
        .with_starting_cursor(starting_cursor)
        .with_page_size(page_size)
        .with_help_message("This selection applies only to this Claude session")
        .prompt()
        .context("Claude account selection cancelled")?;

    match selection {
        ProfileChoice::Existing { name, .. } => Ok(name),
        ProfileChoice::NewLogin => create_and_log_in_new_profile(home),
    }
}

/// Whether an operator-entered account name can become `~/.claude-<name>`.
fn validate_new_profile_name(name: &str) -> std::result::Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("enter an email address or account name".to_string());
    }
    if trimmed == "main" {
        return Err("`main` is the default ~/.claude profile; pick another name".to_string());
    }
    if trimmed
        .chars()
        .any(|c| c.is_whitespace() || c == '/' || c == '\\' || c == '\0')
    {
        return Err("account name cannot contain whitespace or path separators".to_string());
    }
    if trimmed.starts_with('.') {
        return Err("account name cannot start with a dot".to_string());
    }
    if !is_account_profile_name(trimmed) {
        return Err("that name is reserved for lock/scratch directories".to_string());
    }
    Ok(())
}

/// Prompt for an email, create the profile, seed shared config, run login.
///
/// The login runs as a child process rather than replacing this one, so a
/// successful login lands the operator straight into the account they just
/// created instead of making them re-run the command.
fn create_and_log_in_new_profile(home: &Path) -> Result<String> {
    let entered = inquire::Text::new("Email for the new Claude account")
        .with_help_message("creates ~/.claude-<email> and runs the Claude login flow")
        .with_validator(|input: &str| {
            Ok(match validate_new_profile_name(input) {
                Ok(()) => inquire::validator::Validation::Valid,
                Err(message) => {
                    inquire::validator::Validation::Invalid(
                        inquire::validator::ErrorMessage::Custom(message),
                    )
                }
            })
        })
        .prompt()
        .context("new Claude account entry cancelled")?;
    let profile = entered.trim().to_string();
    let profile_dir = resolve_profile_dir(home, &profile);

    std::fs::create_dir_all(&profile_dir).with_context(|| {
        format!(
            "could not create Claude profile directory {}",
            profile_dir.display()
        )
    })?;

    let seeding = seed_profile_from_main(home, &profile_dir)?;
    report_seeding(&seeding);

    eprintln!("Logging in Claude account config: {}", profile_dir.display());
    let mut command = build_claude_command(
        &profile,
        &profile_dir,
        &[OsString::from("auth"), OsString::from("login")],
    );
    let status = command
        .status()
        .context("failed to run `claude auth login` for the new account")?;
    if !status.success() {
        anyhow::bail!(
            "`claude auth login` did not complete for {}; the profile directory was created and seeded, so `cas claude login {profile}` can finish it",
            profile_dir.display()
        );
    }

    Ok(profile)
}

/// Shared configuration surface symlinked into a freshly created profile.
///
/// These are the files an account needs to be *equipped* — the same set whose
/// absence produced the "alt profiles lack hooks/skills" reports (cas-5b96),
/// plus the team directory that has to live under CLAUDE_CONFIG_DIR (cas-3585).
const SHARED_PROFILE_ENTRIES: [&str; 8] = [
    "agents",
    "skills",
    "commands",
    "hooks",
    "workflows",
    "output-styles",
    "settings.json",
    "CLAUDE.md",
];

/// Never linked or copied: credentials and per-account identity/history state.
///
/// Sharing any of these across profiles would defeat the point of separate
/// accounts. `.credentials.json` and the securestorage scoping are the account
/// identity itself; `.claude.json`, history, sessions and projects are that
/// account's own record. `settings.local.json` is deliberately private too --
/// the `.local` convention means machine/account-specific overrides.
const PRIVATE_PROFILE_ENTRIES: [&str; 12] = [
    ".credentials.json",
    ".claude.json",
    "settings.local.json",
    "history.jsonl",
    "sessions",
    "projects",
    "session-env",
    "shell-snapshots",
    "statsig",
    "telemetry",
    "stats-cache.json",
    "backups",
];

#[derive(Debug, Default, Eq, PartialEq)]
struct SeedingReport {
    linked: Vec<String>,
    already_linked: Vec<String>,
    skipped_existing: Vec<String>,
    missing_in_main: Vec<String>,
}

/// Idempotently symlink the shared config surface from `~/.claude`.
///
/// Re-running is safe and never clobbers: an entry the operator later replaced
/// with their own real file or a different link is reported and left alone.
fn seed_profile_from_main(home: &Path, profile_dir: &Path) -> Result<SeedingReport> {
    let main = home.join(".claude");
    let mut report = SeedingReport::default();
    if main == profile_dir || !main.is_dir() {
        return Ok(report);
    }

    for entry in SHARED_PROFILE_ENTRIES {
        debug_assert!(
            !PRIVATE_PROFILE_ENTRIES.contains(&entry),
            "credential/identity state must never be shared"
        );
        let source = main.join(entry);
        if !source.exists() {
            report.missing_in_main.push(entry.to_string());
            continue;
        }
        let target = profile_dir.join(entry);
        match std::fs::symlink_metadata(&target) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                symlink_entry(&source, &target).with_context(|| {
                    format!("could not link {} into {}", entry, profile_dir.display())
                })?;
                report.linked.push(entry.to_string());
            }
            Err(error) => return Err(error).context(format!("could not inspect {entry}")),
            Ok(metadata) => {
                if metadata.file_type().is_symlink()
                    && std::fs::read_link(&target).is_ok_and(|existing| existing == source)
                {
                    report.already_linked.push(entry.to_string());
                } else {
                    report.skipped_existing.push(entry.to_string());
                }
            }
        }
    }
    Ok(report)
}

#[cfg(unix)]
fn symlink_entry(source: &Path, target: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(not(unix))]
fn symlink_entry(source: &Path, target: &Path) -> io::Result<()> {
    if source.is_dir() {
        std::os::windows::fs::symlink_dir(source, target)
    } else {
        std::os::windows::fs::symlink_file(source, target)
    }
}

fn report_seeding(report: &SeedingReport) {
    if !report.linked.is_empty() {
        eprintln!("Seeded shared config from ~/.claude: {}", report.linked.join(", "));
    }
    if !report.skipped_existing.is_empty() {
        eprintln!(
            "Left existing entries untouched: {}",
            report.skipped_existing.join(", ")
        );
    }
    eprintln!("Credentials and account identity remain private to this profile.");
}

/// Build the command used for a `--bare` profile launch without executing it.
pub(crate) fn build_claude_command(
    profile: &str,
    profile_dir: &Path,
    args: &[OsString],
) -> Command {
    let mut command = Command::new("claude");
    configure_profile_command(&mut command, profile, profile_dir);
    command.args(args);
    command
}

/// Warn about a profile directory that Claude will have to bootstrap or log in.
fn warn_about_profile_state(profile: &str, profile_dir: &Path) {
    if !profile_dir.is_dir() {
        eprintln!(
            "Note: {} does not exist yet; Claude will create it.",
            profile_dir.display()
        );
    }
    if probe_login_state(profile, profile_dir) == LoginState::LoggedOut {
        eprintln!(
            "Note: {} is not logged in yet; run `cas claude login {profile}`.",
            profile_dir.display()
        );
    }
}

fn set_profile_env(profile: &str, profile_dir: &Path) {
    // SAFETY: every caller runs before telemetry creates background threads.
    unsafe {
        if profile == "main" {
            std::env::remove_var("CLAUDE_CONFIG_DIR");
            std::env::remove_var("CLAUDE_SECURESTORAGE_CONFIG_DIR");
        } else {
            std::env::set_var("CLAUDE_CONFIG_DIR", profile_dir);
            std::env::set_var("CLAUDE_SECURESTORAGE_CONFIG_DIR", profile_dir);
        }
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
        std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");
        std::env::remove_var("CLAUDE_CODE_OAUTH_REFRESH_TOKEN");
        std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR");
    }
}

/// Export the selected account into this process before anything spawns a
/// thread or a pane.
///
/// This MUST run before `initialize_telemetry` — `std::env::set_var` in a
/// multi-threaded process is UB, and telemetry spawns a background thread. The
/// factory supervisor pane and every `spawn_workers` request then inherit the
/// value through ordinary process environment inheritance.
pub fn apply_profile_env(args: &ClaudeArgs) -> Result<()> {
    if args.command.is_some() || args.list_profiles {
        return Ok(());
    }

    let home = dirs::home_dir().context("cannot determine home directory for Claude profiles")?;

    let profile = match args.profile.as_deref() {
        // An explicitly named account is the no-prompt fast path.
        Some(profile) => profile.to_string(),
        None => {
            let active_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
            let profiles = scan_profiles(&home, active_dir.as_deref()).with_context(|| {
                format!("could not inspect Claude profiles under {}", home.display())
            })?;
            if should_prompt_for_profile(
                None,
                &profiles,
                io::stdin().is_terminal(),
                io::stdout().is_terminal(),
            ) {
                prompt_for_profile(&home, &profiles)?
            } else {
                // Non-interactive, or only one account exists: leave the
                // ambient environment exactly as the caller set it.
                return Ok(());
            }
        }
    };

    let profile_dir = resolve_profile_dir(&home, &profile);
    warn_about_profile_state(&profile, &profile_dir);
    eprintln!("Using Claude account config: {}", profile_dir.display());

    set_profile_env(&profile, &profile_dir);
    // `execute_bare` runs later in the same process; record the choice so the
    // operator is asked once, not once per launch path.
    let _ = SELECTED_PROFILE.set(profile);

    Ok(())
}

/// The account chosen during `apply_profile_env`, for later launch paths.
static SELECTED_PROFILE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Run `cas claude`: factory with a Claude supervisor by default, plain Claude
/// Code under `--bare`, profile listing under `--list-profiles`.
pub fn execute(args: &ClaudeArgs, cli: &Cli, cas_root: Option<&Path>) -> Result<()> {
    if let Some(command) = &args.command {
        return execute_command(command);
    }

    if args.list_profiles {
        let home =
            dirs::home_dir().context("cannot determine home directory for Claude profiles")?;
        let active_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
        print!("{}", profile_listing(&home, active_dir.as_deref())?);
        return Ok(());
    }

    if args.bare {
        return execute_bare(args);
    }

    // `apply_profile_env` already exported CLAUDE_CONFIG_DIR for this process.
    let mut factory_args = parse_factory_args(&args.args);
    factory_args.supervisor_cli = "claude".to_string();
    factory_args.supervisor_cli_explicit = true;
    super::factory::execute(&factory_args, cli, cas_root)
}

/// Parse the trailing arguments as `cas factory` flags.
///
/// Kept separate from `ClaudeArgs` because `FactoryArgs` carries a subcommand,
/// which clap cannot disambiguate from the leading `profile` positional.
fn parse_factory_args(args: &[OsString]) -> FactoryArgs {
    let command = FactoryArgs::augment_args(clap::Command::new("cas claude"));
    let matches = match command.try_get_matches_from(
        std::iter::once(OsString::from("cas claude")).chain(args.iter().cloned()),
    ) {
        Ok(matches) => matches,
        Err(err) => err.exit(),
    };
    match FactoryArgs::from_arg_matches(&matches) {
        Ok(parsed) => parsed,
        Err(err) => err.exit(),
    }
}

/// Plain Claude Code launch. On Unix this replaces the CAS process with Claude.
fn execute_bare(args: &ClaudeArgs) -> Result<()> {
    let home = dirs::home_dir().context("cannot determine home directory for Claude profiles")?;
    // `apply_profile_env` already ran the picker for this process; reuse its
    // answer rather than asking a second time on the way to the same launch.
    // `apply_profile_env` already resolved and announced the account for both
    // the explicit and the picked case. Only the silent fallback below — no
    // explicit profile and no prompt (non-TTY, or a single account) — still
    // needs its own announcement, which is what it printed before this change.
    let (profile, already_announced) = match args.profile.as_deref() {
        Some(profile) => (profile.to_string(), true),
        None => match SELECTED_PROFILE.get() {
            Some(picked) => (picked.clone(), true),
            None => ("main".to_string(), false),
        },
    };
    let profile_dir = resolve_profile_dir(&home, &profile);

    if !already_announced {
        warn_about_profile_state(&profile, &profile_dir);
        eprintln!("Using Claude account config: {}", profile_dir.display());
    }

    let mut command = build_claude_command(&profile, &profile_dir, &args.args);
    exec_claude(&mut command)
}

fn execute_command(command: &ClaudeCommand) -> Result<()> {
    match command {
        ClaudeCommand::Login { profile, args } => {
            let home =
                dirs::home_dir().context("cannot determine home directory for Claude profiles")?;
            let profile_dir = resolve_profile_dir(&home, profile);
            std::fs::create_dir_all(&profile_dir).with_context(|| {
                format!(
                    "could not create Claude profile directory {}",
                    profile_dir.display()
                )
            })?;
            eprintln!(
                "Logging in Claude account config: {}",
                profile_dir.display()
            );

            let login_args = std::iter::once(OsString::from("auth"))
                .chain(std::iter::once(OsString::from("login")))
                .chain(args.iter().cloned())
                .collect::<Vec<_>>();
            let mut command = build_claude_command(profile, &profile_dir, &login_args);
            exec_claude(&mut command)
        }
    }
}

#[cfg(unix)]
fn exec_claude(command: &mut Command) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let error = command.exec();
    if error.kind() == io::ErrorKind::NotFound {
        anyhow::bail!("could not find `claude` on PATH; install Claude Code or add it to PATH")
    }
    Err(error).context("failed to launch `claude`")
}

#[cfg(not(unix))]
fn exec_claude(_command: &mut Command) -> Result<()> {
    anyhow::bail!("`cas claude --bare` is supported on Unix only")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use tempfile::TempDir;

    #[test]
    fn resolves_main_and_named_profiles_under_home() {
        let home = Path::new("/tmp/test-home");

        assert_eq!(resolve_profile_dir(home, "main"), home.join(".claude"));
        assert_eq!(resolve_profile_dir(home, "alt"), home.join(".claude-alt"));
        assert_eq!(resolve_profile_dir(home, "work"), home.join(".claude-work"));
    }

    #[test]
    fn scans_main_and_named_directories_with_login_and_active_state() {
        let home = TempDir::new().unwrap();
        let main = home.path().join(".claude");
        let alt = home.path().join(".claude-alt");
        let work = home.path().join(".claude-work");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&alt).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        let profiles = scan_profiles_with(home.path(), Some(alt.as_path()), |name, _| {
            if name == "alt" {
                LoginState::LoggedIn
            } else {
                LoginState::LoggedOut
            }
        })
        .unwrap();

        assert_eq!(
            profiles,
            vec![
                Profile {
                    name: "main".to_string(),
                    directory: main,
                    login_state: LoginState::LoggedOut,
                    active: false,
                },
                Profile {
                    name: "alt".to_string(),
                    directory: alt,
                    login_state: LoginState::LoggedIn,
                    active: true,
                },
                Profile {
                    name: "work".to_string(),
                    directory: work,
                    login_state: LoginState::LoggedOut,
                    active: false,
                },
            ]
        );
    }

    #[test]
    fn bare_launch_command_sets_profile_scrubs_api_key_and_forwards_args() {
        let args = vec![OsString::from("--continue"), OsString::from("--verbose")];
        let command = build_claude_command("alt", Path::new("/tmp/.claude-alt"), &args);

        assert_eq!(command.get_program(), OsStr::new("claude"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            args.iter().collect::<Vec<_>>()
        );
        let envs = command.get_envs().collect::<Vec<_>>();
        assert!(envs.contains(&(
            OsStr::new("CLAUDE_CONFIG_DIR"),
            Some(OsStr::new("/tmp/.claude-alt"))
        )));
        assert!(envs.contains(&(
            OsStr::new("CLAUDE_SECURESTORAGE_CONFIG_DIR"),
            Some(OsStr::new("/tmp/.claude-alt"))
        )));
        assert!(envs.contains(&(OsStr::new("ANTHROPIC_API_KEY"), None)));
        assert!(envs.contains(&(OsStr::new("ANTHROPIC_AUTH_TOKEN"), None)));
        assert!(envs.contains(&(OsStr::new("CLAUDE_CODE_OAUTH_TOKEN"), None)));
        assert!(envs.contains(&(OsStr::new("CLAUDE_CODE_OAUTH_REFRESH_TOKEN"), None)));
        assert!(envs.contains(&(OsStr::new("CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR"), None)));
    }

    #[test]
    fn main_profile_clears_profile_selectors_for_the_legacy_default_store() {
        let command = build_claude_command("main", Path::new("/tmp/.claude"), &[]);
        let envs = command.get_envs().collect::<Vec<_>>();

        assert!(envs.contains(&(OsStr::new("CLAUDE_CONFIG_DIR"), None)));
        assert!(envs.contains(&(OsStr::new("CLAUDE_SECURESTORAGE_CONFIG_DIR"), None)));
    }

    #[test]
    fn bare_launch_prompts_only_for_interactive_ambiguous_account_selection() {
        let profiles = vec![
            Profile {
                name: "main".to_string(),
                directory: PathBuf::from("/tmp/.claude"),
                login_state: LoginState::LoggedIn,
                active: false,
            },
            Profile {
                name: "alt".to_string(),
                directory: PathBuf::from("/tmp/.claude-alt"),
                login_state: LoginState::LoggedIn,
                active: false,
            },
        ];

        assert!(should_prompt_for_profile(None, &profiles, true, true));
        assert!(!should_prompt_for_profile(None, &profiles, false, true));
        assert!(!should_prompt_for_profile(None, &profiles, true, false));
        assert!(!should_prompt_for_profile(
            Some("alt"),
            &profiles,
            true,
            true
        ));
        assert!(!should_prompt_for_profile(None, &profiles[..1], true, true));
    }

    fn profile(name: &str, state: LoginState, active: bool) -> Profile {
        Profile {
            name: name.to_string(),
            directory: PathBuf::from(format!("/tmp/.claude-{name}")),
            login_state: state,
            active,
        }
    }

    #[test]
    fn scan_skips_lock_and_scratch_directories_but_keeps_dotted_emails() {
        let home = TempDir::new().unwrap();
        for dir in [
            ".claude",
            ".claude-support@petrastella.io",
            ".claude-support@petrastella.io.lock",
            ".claude-alt.bak",
            ".claude-work.tmp",
            ".claude-",
        ] {
            std::fs::create_dir_all(home.path().join(dir)).unwrap();
        }

        let profiles =
            scan_profiles_with(home.path(), None, |_, _| LoginState::LoggedIn).unwrap();
        let names: Vec<_> = profiles.iter().map(|p| p.name.as_str()).collect();

        assert_eq!(names, vec!["main", "support@petrastella.io"]);
    }

    #[test]
    fn account_name_predicate_accepts_emails_and_rejects_markers() {
        assert!(is_account_profile_name("support@petrastella.io"));
        assert!(is_account_profile_name("daniel@petrastella.io"));
        assert!(is_account_profile_name("alt"));
        assert!(!is_account_profile_name(""));
        assert!(!is_account_profile_name("support@petrastella.io.lock"));
        assert!(!is_account_profile_name("skills.bak.1777306474"));
        assert!(!is_account_profile_name("work.TMP"));
    }

    #[test]
    fn prompt_fires_on_detected_accounts_even_when_the_probe_could_not_confirm_login() {
        // The regression that made the operator's picker vanish: probes that
        // return Unknown or LoggedOut must not silently shrink the count to one.
        let unknown = vec![
            profile("main", LoginState::LoggedIn, true),
            profile("alt", LoginState::Unknown, false),
        ];
        let logged_out = vec![
            profile("main", LoginState::LoggedIn, true),
            profile("alt", LoginState::LoggedOut, false),
        ];
        let all_unknown = vec![
            profile("main", LoginState::Unknown, true),
            profile("alt", LoginState::Unknown, false),
        ];

        assert!(should_prompt_for_profile(None, &unknown, true, true));
        assert!(should_prompt_for_profile(None, &logged_out, true, true));
        assert!(should_prompt_for_profile(None, &all_unknown, true, true));
    }

    #[test]
    fn explicit_profile_and_non_tty_paths_never_prompt() {
        let profiles = vec![
            profile("main", LoginState::LoggedIn, true),
            profile("alt", LoginState::LoggedIn, false),
        ];

        assert!(!should_prompt_for_profile(Some("alt"), &profiles, true, true));
        assert!(!should_prompt_for_profile(None, &profiles, false, true));
        assert!(!should_prompt_for_profile(None, &profiles, true, false));
        assert!(!should_prompt_for_profile(None, &profiles, false, false));
        assert!(!should_prompt_for_profile(None, &profiles[..1], true, true));
    }

    #[test]
    fn picker_lists_every_detected_profile_with_state_plus_a_new_login_entry() {
        let profiles = vec![
            profile("main", LoginState::LoggedIn, true),
            profile("alt", LoginState::LoggedOut, false),
            profile("ghost", LoginState::Unknown, false),
        ];

        let choices = profile_choices(&profiles);
        let rendered: Vec<String> = choices.iter().map(ToString::to_string).collect();

        assert_eq!(rendered.len(), 4, "3 accounts plus the new-login entry");
        assert_eq!(rendered[0], "main — logged in, active");
        assert_eq!(rendered[1], "alt — not logged in");
        assert_eq!(rendered[2], "ghost — login state unknown");
        assert_eq!(rendered[3], "+ Log in a new account…");
        assert_eq!(choices[3], ProfileChoice::NewLogin);
        assert_eq!(active_choice_index(&profiles), 0);
    }

    #[test]
    fn picker_page_shows_the_whole_list_so_new_login_is_never_below_the_fold() {
        // 7 accounts + the new-login row is exactly the case that hid the entry
        // behind a scroll at inquire's default page size of 7.
        assert_eq!(picker_page_size(8), 8);
        assert_eq!(picker_page_size(2), 7);
        assert_eq!(picker_page_size(40), 20);
    }

    #[test]
    fn picker_cursor_starts_on_the_environment_active_account() {
        let profiles = vec![
            profile("main", LoginState::LoggedIn, false),
            profile("alt", LoginState::LoggedIn, true),
        ];

        assert_eq!(active_choice_index(&profiles), 1);
    }

    fn main_profile_fixture(home: &Path) -> PathBuf {
        let main = home.join(".claude");
        std::fs::create_dir_all(main.join("agents")).unwrap();
        std::fs::create_dir_all(main.join("skills")).unwrap();
        std::fs::create_dir_all(main.join("sessions")).unwrap();
        std::fs::write(main.join("settings.json"), "{}").unwrap();
        std::fs::write(main.join("settings.local.json"), "{\"local\":true}").unwrap();
        std::fs::write(main.join(".credentials.json"), "SECRET").unwrap();
        main
    }

    #[test]
    fn seeding_links_shared_config_and_never_credentials_or_identity() {
        let home = TempDir::new().unwrap();
        main_profile_fixture(home.path());
        let profile_dir = home.path().join(".claude-new@example.com");
        std::fs::create_dir_all(&profile_dir).unwrap();

        let report = seed_profile_from_main(home.path(), &profile_dir).unwrap();

        assert!(report.linked.contains(&"agents".to_string()));
        assert!(report.linked.contains(&"skills".to_string()));
        assert!(report.linked.contains(&"settings.json".to_string()));
        assert!(profile_dir.join("skills").is_symlink());

        for private in PRIVATE_PROFILE_ENTRIES {
            assert!(
                !profile_dir.join(private).exists(),
                "{private} must stay private to the profile"
            );
        }
        assert!(!report.linked.contains(&".credentials.json".to_string()));
        // Entries absent from ~/.claude are reported, not fabricated.
        assert!(report.missing_in_main.contains(&"hooks".to_string()));
    }

    #[test]
    fn seeding_is_idempotent_and_leaves_operator_divergence_alone() {
        let home = TempDir::new().unwrap();
        main_profile_fixture(home.path());
        let profile_dir = home.path().join(".claude-new@example.com");
        std::fs::create_dir_all(&profile_dir).unwrap();

        let first = seed_profile_from_main(home.path(), &profile_dir).unwrap();
        assert!(!first.linked.is_empty());

        // Re-running recognises its own links instead of relinking or failing.
        let second = seed_profile_from_main(home.path(), &profile_dir).unwrap();
        assert!(second.linked.is_empty());
        assert_eq!(second.already_linked, first.linked);

        // An entry the operator later replaced with their own file is kept.
        std::fs::remove_file(profile_dir.join("settings.json")).unwrap();
        std::fs::write(profile_dir.join("settings.json"), "{\"mine\":true}").unwrap();
        let third = seed_profile_from_main(home.path(), &profile_dir).unwrap();

        assert!(third.skipped_existing.contains(&"settings.json".to_string()));
        assert!(!third.linked.contains(&"settings.json".to_string()));
        assert_eq!(
            std::fs::read_to_string(profile_dir.join("settings.json")).unwrap(),
            "{\"mine\":true}"
        );
    }

    #[test]
    fn seeding_never_targets_the_main_profile_itself() {
        let home = TempDir::new().unwrap();
        let main = main_profile_fixture(home.path());

        let report = seed_profile_from_main(home.path(), &main).unwrap();

        assert_eq!(report, SeedingReport::default());
        assert!(!main.join("agents").is_symlink());
    }

    #[test]
    fn new_profile_names_reject_reserved_and_unsafe_values() {
        assert!(validate_new_profile_name("someone@example.com").is_ok());
        assert!(validate_new_profile_name("  spaced@example.com  ").is_ok());
        assert!(validate_new_profile_name("").is_err());
        assert!(validate_new_profile_name("   ").is_err());
        assert!(validate_new_profile_name("main").is_err());
        assert!(validate_new_profile_name("has space").is_err());
        assert!(validate_new_profile_name("a/b").is_err());
        assert!(validate_new_profile_name(".hidden").is_err());
        assert!(validate_new_profile_name("thing.lock").is_err());
    }

    #[test]
    fn trailing_args_parse_as_factory_flags() {
        let parsed = parse_factory_args(&[
            OsString::from("--workers"),
            OsString::from("3"),
            OsString::from("--new"),
        ]);

        assert_eq!(parsed.workers, 3);
        assert!(parsed.start_new);
    }

    #[test]
    fn no_trailing_args_yields_factory_defaults() {
        let parsed = parse_factory_args(&[]);

        assert_eq!(parsed.workers, 0);
        assert!(!parsed.start_new);
        assert!(!parsed.set_default);
    }
}
