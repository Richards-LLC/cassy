//! `cas codex` — launch the CAS factory with a Codex supervisor on an
//! explicitly selected ChatGPT account profile.
//!
//! The Claude sibling of this launcher is `cas claude` (cas-cli/src/cli/claude.rs);
//! this module keeps the same shape so the two providers behave identically from
//! the operator's side. What differs is the underlying convention, which was
//! verified against codex-cli 0.147.0 rather than assumed (cas-9cc3):
//!
//! * `CODEX_HOME` scopes *everything*: `codex doctor` under a fresh
//!   `CODEX_HOME` reports its own `config.toml`, its own `auth.json`, and its own
//!   sqlite/session root. Logging in under one `CODEX_HOME` leaves every other
//!   home logged out.
//! * Login state has no JSON surface. `codex login status` prints
//!   `Not logged in` / `Logged in using …` and exits 1 / 0, so the probe reads
//!   both and degrades to `Unknown` rather than guessing.
//! * Shared configuration can be symlinked into a profile home: a symlinked
//!   `config.toml` parses normally (verified: `configured servers 7` resolved
//!   through the link) and survives a codex run untouched. `auth.json` is never
//!   linked — it stays a real per-profile file.
//!
//! Profile convention: `main` → `~/.codex`, any other name → `~/.codex-<name>`,
//! matching `~/.claude-<name>`. Named selection exports `CODEX_HOME` before the
//! factory starts; panes inherit it (`cas-pty` never calls `env_clear`), so the
//! supervisor and its codex workers land on the chosen account.

use std::ffi::OsString;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use clap::{Args, FromArgMatches, Subcommand};

use super::Cli;
use super::factory::FactoryArgs;

/// Configuration entries a new codex profile inherits from `~/.codex` by
/// symlink, so a freshly logged-in account is immediately fully equipped.
///
/// Everything absent from this list stays private to the profile. In particular
/// `auth.json` is never linked or copied: credentials are the one thing that
/// must not be shared between accounts. Session history, per-home sqlite
/// databases (`*_N.sqlite`), caches, locks, `installation_id` and `log/` are
/// also deliberately private — they are per-account runtime state, not
/// configuration.
pub(crate) const SHARED_PROFILE_ENTRIES: &[&str] = &[
    "config.toml",
    "AGENTS.md",
    "agents",
    "skills",
    "plugins",
    "hooks.json",
];

/// Entries that must never be shared between profiles, whatever a caller asks
/// for. Credentials are the whole point of having separate accounts.
pub(crate) const NEVER_SHARED_ENTRIES: &[&str] = &["auth.json"];

/// What one seeding pass did, so the caller can report it honestly.
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct SeedReport {
    pub(crate) linked: Vec<String>,
    pub(crate) already_present: Vec<String>,
    pub(crate) absent_in_source: Vec<String>,
    pub(crate) refused: Vec<String>,
}

/// Seed a new profile home by symlinking the shared configuration surface from
/// the main home, never the credential material.
///
/// Idempotent by construction: an entry that already exists in the target — a
/// real file the operator diverged, or a link from an earlier pass — is left
/// exactly as it is and reported as `already_present`. A source entry that does
/// not exist is reported rather than silently skipped, and anything on
/// `NEVER_SHARED_ENTRIES` is refused even if a caller passes it.
///
/// Provider-agnostic on purpose: the entry list is the only provider-specific
/// part, so `cas claude`'s profile seeding (cas-898d) can call this with its own
/// list instead of growing a second implementation.
pub(crate) fn seed_profile_dir(
    source_dir: &Path,
    target_dir: &Path,
    entries: &[&str],
) -> io::Result<SeedReport> {
    let mut report = SeedReport::default();
    if source_dir == target_dir {
        return Ok(report);
    }
    std::fs::create_dir_all(target_dir)?;

    for entry in entries {
        if NEVER_SHARED_ENTRIES.contains(entry) {
            report.refused.push((*entry).to_string());
            continue;
        }
        let source = source_dir.join(entry);
        let target = target_dir.join(entry);
        if !source.exists() {
            report.absent_in_source.push((*entry).to_string());
            continue;
        }
        // `symlink_metadata` so an existing dangling link still counts as
        // present: re-seeding must not silently replace what is already there.
        if target.symlink_metadata().is_ok() {
            report.already_present.push((*entry).to_string());
            continue;
        }
        symlink_entry(&source, &target)?;
        report.linked.push((*entry).to_string());
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

/// Arguments for `cas codex [profile] [factory-args...]`.
#[derive(Args, Clone, Debug)]
#[command(subcommand_precedence_over_arg = true)]
pub struct CodexArgs {
    #[command(subcommand)]
    pub command: Option<CodexCommand>,

    /// List detected account profiles with login state and exit.
    #[arg(long = "list-profiles")]
    pub list_profiles: bool,

    /// Launch plain Codex on this profile instead of the CAS factory.
    #[arg(long = "bare")]
    pub bare: bool,

    /// `[PROFILE]` followed by `cas factory` flags (or Codex flags with `--bare`).
    ///
    /// `main` maps to ~/.codex; any other name maps to ~/.codex-<name>. Omit the
    /// profile to be asked (interactive, >1 account) or to keep whichever account
    /// the environment already selects.
    ///
    /// Deliberately one list rather than a `profile` positional plus a trailing
    /// list: a dedicated positional makes clap reject a leading factory flag, and
    /// `cas codex --workers 3` has always worked. Splitting here keeps both
    /// spellings valid.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<OsString>,
}

impl CodexArgs {
    /// The account profile, if the first argument names one.
    ///
    /// A leading token that starts with `-` is a factory/Codex flag, not a
    /// profile.
    pub(crate) fn profile(&self) -> Option<&str> {
        let first = self.args.first()?.to_str()?;
        (!first.starts_with('-')).then_some(first)
    }

    /// Everything after the optional profile: `cas factory` or Codex flags.
    pub(crate) fn passthrough_args(&self) -> &[OsString] {
        if self.profile().is_some() {
            &self.args[1..]
        } else {
            &self.args
        }
    }
}

#[derive(Subcommand, Clone, Debug)]
pub enum CodexCommand {
    /// Sign in to exactly one Codex account profile.
    Login {
        /// Account profile: `main` maps to ~/.codex; any other name maps to ~/.codex-<name>.
        profile: String,

        /// Remaining arguments passed to `codex login`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LoginState {
    LoggedIn,
    LoggedOut,
    Unknown,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Profile {
    pub(crate) name: String,
    pub(crate) directory: PathBuf,
    pub(crate) login_state: LoginState,
    pub(crate) active: bool,
}

/// Resolve a convention-based profile name under `home`.
pub(crate) fn resolve_profile_dir(home: &Path, profile: &str) -> PathBuf {
    if profile == "main" {
        home.join(".codex")
    } else {
        home.join(format!(".codex-{profile}"))
    }
}

/// Directories that carry the `.codex-` prefix but are not accounts.
///
/// The Claude side found `.claude-support@petrastella.io.lock` being offered as
/// a selectable account (cas-898d); the same lock-directory convention applies
/// here, so the scan excludes it before an operator can pick it.
fn is_account_directory(profile_name: &str) -> bool {
    !profile_name.is_empty() && !profile_name.ends_with(".lock") && !profile_name.starts_with('.')
}

pub(crate) fn scan_profiles(home: &Path, active_dir: Option<&Path>) -> io::Result<Vec<Profile>> {
    scan_profiles_with(home, active_dir, probe_login_state)
}

fn scan_profiles_with(
    home: &Path,
    active_dir: Option<&Path>,
    mut login_state_for: impl FnMut(&str, &Path) -> LoginState,
) -> io::Result<Vec<Profile>> {
    let mut profiles = vec![profile_for(
        "main",
        home.join(".codex"),
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
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(profile_name) = file_name.strip_prefix(".codex-") else {
                continue;
            };
            if !is_account_directory(profile_name) {
                continue;
            }
            profiles.push(profile_for(
                profile_name,
                path.clone(),
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

/// Classify `codex login status` output for one profile home.
///
/// Codex has no `--json` login surface, so this reads what it does publish:
/// exit 0 with a `Logged in …` line, exit 1 with `Not logged in`. Anything else
/// — a missing binary, a timeout, an output shape change — is `Unknown`, which
/// stays visible in the picker instead of silently dropping the account.
fn classify_login_output(success: bool, stdout: &str, stderr: &str) -> LoginState {
    let combined = format!("{stdout}\n{stderr}").to_lowercase();
    if combined.contains("not logged in") {
        return LoginState::LoggedOut;
    }
    if combined.contains("logged in") {
        return LoginState::LoggedIn;
    }
    if success { LoginState::LoggedIn } else { LoginState::Unknown }
}

/// Ask the codex CLI itself whether this profile's credential is usable.
fn probe_login_state(profile: &str, profile_dir: &Path) -> LoginState {
    if !profile_dir.is_dir() {
        return LoginState::LoggedOut;
    }
    let mut command = Command::new("codex");
    configure_profile_command(&mut command, profile, profile_dir);
    command.args(["login", "status"]);

    let Ok(output) = command.output() else {
        return LoginState::Unknown;
    };
    classify_login_output(
        output.status.success(),
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )
}

fn profile_listing(home: &Path, active_dir: Option<&Path>) -> Result<String> {
    let profiles = scan_profiles(home, active_dir)
        .with_context(|| format!("could not inspect Codex profiles under {}", home.display()))?;
    let mut output =
        String::from("Usage: cas codex <profile> [factory-args...]\n\nDetected Codex profiles:\n");

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

/// Bind a codex subprocess to one profile's home.
///
/// The API-key scrub mirrors the `ANTHROPIC_API_KEY` scrub on the Claude side:
/// an explicitly chosen account must win over an inherited key, or the operator
/// picks account A and silently talks to whatever the ambient key belongs to.
/// `OPENAI_API_KEY` and `CODEX_ACCESS_TOKEN` are the two the CLI documents
/// reading (`codex login --with-api-key` / `--with-access-token`);
/// `CODEX_API_KEY` is scrubbed defensively alongside them.
pub(crate) fn configure_profile_command(command: &mut Command, profile: &str, profile_dir: &Path) {
    if profile == "main" {
        // Codex's own default is ~/.codex. Removing the selector also prevents
        // an ambient CODEX_HOME from leaking into an explicit `main` choice.
        command.env_remove("CODEX_HOME");
    } else {
        command.env("CODEX_HOME", profile_dir);
    }
    command
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .env_remove("CODEX_ACCESS_TOKEN");
}

/// Return whether a bare launch should ask the operator to select an account.
///
/// Gating on DETECTED rather than confirmed-logged-in profiles is deliberate
/// (cas-898d): a probe failure must not silently collapse the picker down to a
/// default launch. An explicit profile always wins, and pipe/script launches are
/// left alone — a prompt there would hang or eat input meant for codex.
fn should_prompt_for_profile(
    explicit_profile: Option<&str>,
    profiles: &[Profile],
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
) -> bool {
    explicit_profile.is_none()
        && stdin_is_terminal
        && stdout_is_terminal
        && profiles.len() > 1
}

/// Present the account picker for codex profiles.
///
/// Every detected profile is offered, annotated with its login state; `Unknown`
/// is selectable and marked rather than hidden.
fn prompt_for_profile(profiles: &[Profile]) -> Result<String> {
    let labels: Vec<String> = profiles.iter().map(profile_choice_label).collect();

    let choice = inquire::Select::new("Choose Codex account", labels.clone())
        .with_help_message("This selection applies only to this Codex session")
        .prompt()
        .context("Codex account selection cancelled")?;

    let index = labels
        .iter()
        .position(|label| label == &choice)
        .context("selected Codex account no longer resolves")?;
    Ok(profiles[index].name.clone())
}

fn profile_choice_label(profile: &Profile) -> String {
    let state = match profile.login_state {
        LoginState::LoggedIn => "logged in",
        LoginState::LoggedOut => "not logged in",
        LoginState::Unknown => "login state unknown",
    };
    let active = if profile.active { ", active" } else { "" };
    format!("{} ({state}{active})", profile.name)
}

/// Build the command used for a `--bare` profile launch without executing it.
pub(crate) fn build_codex_command(profile: &str, profile_dir: &Path, args: &[OsString]) -> Command {
    let mut command = Command::new("codex");
    configure_profile_command(&mut command, profile, profile_dir);
    command.args(args);
    command
}

/// Warn about a profile directory codex will have to bootstrap or log in.
fn warn_about_profile_state(profile: &str, profile_dir: &Path) {
    if !profile_dir.is_dir() {
        eprintln!(
            "Note: {} does not exist yet; codex will create it.",
            profile_dir.display()
        );
        return;
    }
    if probe_login_state(profile, profile_dir) == LoginState::LoggedOut {
        eprintln!(
            "Note: {} is not logged in yet; run `cas codex login {profile}`.",
            profile_dir.display()
        );
    }
}

fn set_profile_env(profile: &str, profile_dir: &Path) {
    // SAFETY: every caller runs before telemetry creates background threads.
    unsafe {
        if profile == "main" {
            std::env::remove_var("CODEX_HOME");
        } else {
            std::env::set_var("CODEX_HOME", profile_dir);
        }
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("CODEX_API_KEY");
        std::env::remove_var("CODEX_ACCESS_TOKEN");
    }
}

/// Export the selected account into this process before anything spawns a
/// thread or a pane.
///
/// This MUST run before `initialize_telemetry` — `std::env::set_var` in a
/// multi-threaded process is UB, and telemetry spawns a background thread. The
/// factory supervisor pane and every codex worker then inherit `CODEX_HOME`
/// through ordinary process environment inheritance.
pub fn apply_profile_env(args: &CodexArgs) -> Result<()> {
    if args.command.is_some() || args.list_profiles || args.bare {
        return Ok(());
    }
    let Some(profile) = args.profile() else {
        return Ok(());
    };

    let home = dirs::home_dir().context("cannot determine home directory for Codex profiles")?;
    let profile_dir = resolve_profile_dir(&home, profile);
    warn_about_profile_state(profile, &profile_dir);
    eprintln!("Using Codex account home: {}", profile_dir.display());

    set_profile_env(profile, &profile_dir);

    Ok(())
}

/// Run `cas codex`: factory with a Codex supervisor by default, plain codex
/// under `--bare`, profile listing under `--list-profiles`.
pub fn execute(args: &CodexArgs, cli: &Cli, cas_root: Option<&Path>) -> Result<()> {
    if let Some(command) = &args.command {
        return execute_command(command);
    }

    if args.list_profiles {
        let home = dirs::home_dir().context("cannot determine home directory for Codex profiles")?;
        let active_dir = std::env::var_os("CODEX_HOME").map(PathBuf::from);
        print!("{}", profile_listing(&home, active_dir.as_deref())?);
        return Ok(());
    }

    if args.bare {
        return execute_bare(args);
    }

    // An explicit profile was already exported by `apply_profile_env`. A bare
    // interactive launch with more than one detected account stops here and
    // asks, instead of silently loading whichever account the environment
    // happened to select.
    if args.profile().is_none() && io::stdin().is_terminal() && io::stdout().is_terminal() {
        let home = dirs::home_dir().context("cannot determine home directory for Codex profiles")?;
        let active_dir = std::env::var_os("CODEX_HOME").map(PathBuf::from);
        let profiles = scan_profiles(&home, active_dir.as_deref())?;
        if should_prompt_for_profile(None, &profiles, true, true) {
            let profile = prompt_for_profile(&profiles)?;
            let profile_dir = resolve_profile_dir(&home, &profile);
            eprintln!("Using Codex account home: {}", profile_dir.display());
            set_profile_env(&profile, &profile_dir);
        }
    }

    let mut factory_args = parse_factory_args(args.passthrough_args());
    factory_args.supervisor_cli = "codex".to_string();
    factory_args.supervisor_cli_explicit = true;
    super::factory::execute(&factory_args, cli, cas_root)
}

/// Parse the trailing arguments as `cas factory` flags.
///
/// Kept separate from `CodexArgs` because `FactoryArgs` carries a subcommand,
/// which clap cannot disambiguate from the leading `profile` positional.
fn parse_factory_args(args: &[OsString]) -> FactoryArgs {
    let command = FactoryArgs::augment_args(clap::Command::new("cas codex"));
    let matches = match command.try_get_matches_from(
        std::iter::once(OsString::from("cas codex")).chain(args.iter().cloned()),
    ) {
        Ok(matches) => matches,
        Err(err) => err.exit(),
    };
    match FactoryArgs::from_arg_matches(&matches) {
        Ok(parsed) => parsed,
        Err(err) => err.exit(),
    }
}

/// Plain codex launch. On Unix this replaces the CAS process with codex.
fn execute_bare(args: &CodexArgs) -> Result<()> {
    let home = dirs::home_dir().context("cannot determine home directory for Codex profiles")?;
    let profile = match args.profile() {
        Some(profile) => profile.to_string(),
        None if io::stdin().is_terminal() && io::stdout().is_terminal() => {
            let active_dir = std::env::var_os("CODEX_HOME").map(PathBuf::from);
            let profiles = scan_profiles(&home, active_dir.as_deref())?;
            if should_prompt_for_profile(None, &profiles, true, true) {
                prompt_for_profile(&profiles)?
            } else {
                "main".to_string()
            }
        }
        None => "main".to_string(),
    };
    let profile_dir = resolve_profile_dir(&home, &profile);

    warn_about_profile_state(&profile, &profile_dir);
    eprintln!("Using Codex account home: {}", profile_dir.display());

    let mut command = build_codex_command(&profile, &profile_dir, args.passthrough_args());
    exec_codex(&mut command)
}

/// Tell the operator what the profile inherited, and what it deliberately did not.
fn report_seeding(report: &SeedReport) {
    if !report.linked.is_empty() {
        eprintln!(
            "Seeded shared Codex configuration into this profile: {} (auth.json stays private).",
            report.linked.join(", ")
        );
    }
    if !report.absent_in_source.is_empty() {
        eprintln!(
            "Note: not present in ~/.codex, so not seeded: {}.",
            report.absent_in_source.join(", ")
        );
    }
}

fn execute_command(command: &CodexCommand) -> Result<()> {
    match command {
        CodexCommand::Login { profile, args } => {
            let home =
                dirs::home_dir().context("cannot determine home directory for Codex profiles")?;
            let profile_dir = resolve_profile_dir(&home, profile);
            std::fs::create_dir_all(&profile_dir).with_context(|| {
                format!(
                    "could not create Codex profile directory {}",
                    profile_dir.display()
                )
            })?;
            eprintln!("Logging in Codex account home: {}", profile_dir.display());

            if profile != "main" {
                let report =
                    seed_profile_dir(&home.join(".codex"), &profile_dir, SHARED_PROFILE_ENTRIES)
                        .with_context(|| {
                            format!("could not seed Codex profile {}", profile_dir.display())
                        })?;
                report_seeding(&report);
            }

            let login_args = std::iter::once(OsString::from("login"))
                .chain(args.iter().cloned())
                .collect::<Vec<_>>();
            let mut command = build_codex_command(profile, &profile_dir, &login_args);
            exec_codex(&mut command)
        }
    }
}

#[cfg(unix)]
fn exec_codex(command: &mut Command) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let error = command.exec();
    if error.kind() == io::ErrorKind::NotFound {
        anyhow::bail!("could not find `codex` on PATH; install the Codex CLI or add it to PATH")
    }
    Err(error).context("failed to launch `codex`")
}

#[cfg(not(unix))]
fn exec_codex(_command: &mut Command) -> Result<()> {
    anyhow::bail!("`cas codex --bare` is supported on Unix only")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use tempfile::TempDir;

    #[test]
    fn resolves_main_and_named_profiles_under_home() {
        let home = Path::new("/tmp/test-home");

        assert_eq!(resolve_profile_dir(home, "main"), home.join(".codex"));
        assert_eq!(resolve_profile_dir(home, "alt"), home.join(".codex-alt"));
        assert_eq!(
            resolve_profile_dir(home, "daniel@petrastella.io"),
            home.join(".codex-daniel@petrastella.io")
        );
    }

    #[test]
    fn scans_main_and_named_directories_with_login_and_active_state() {
        let home = TempDir::new().unwrap();
        let main = home.path().join(".codex");
        let alt = home.path().join(".codex-alt");
        let work = home.path().join(".codex-work");
        for dir in [&main, &alt, &work] {
            std::fs::create_dir_all(dir).unwrap();
        }

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
    fn scan_excludes_lock_directories_but_keeps_logged_out_accounts() {
        let home = TempDir::new().unwrap();
        for dir in [
            ".codex",
            ".codex-support@petrastella.io",
            ".codex-support@petrastella.io.lock",
        ] {
            std::fs::create_dir_all(home.path().join(dir)).unwrap();
        }

        let profiles =
            scan_profiles_with(home.path(), None, |_, _| LoginState::LoggedOut).unwrap();

        let names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["main", "support@petrastella.io"]);
    }

    #[test]
    fn login_state_is_read_from_the_cli_output_shape_and_falls_back_to_unknown() {
        // Verified against codex-cli 0.147.0 on 2026-08-18.
        assert_eq!(
            classify_login_output(false, "Not logged in\n", ""),
            LoginState::LoggedOut
        );
        assert_eq!(
            classify_login_output(true, "Logged in using an API key - sk-pro***0\n", ""),
            LoginState::LoggedIn
        );
        assert_eq!(
            classify_login_output(true, "Logged in using ChatGPT\n", ""),
            LoginState::LoggedIn
        );
        // An unrecognised failure must not be reported as either state.
        assert_eq!(
            classify_login_output(false, "", "error: something new\n"),
            LoginState::Unknown
        );
    }

    #[test]
    fn a_missing_profile_directory_is_logged_out_not_unknown() {
        let home = TempDir::new().unwrap();
        assert_eq!(
            probe_login_state("ghost", &home.path().join(".codex-ghost")),
            LoginState::LoggedOut
        );
    }

    #[test]
    fn named_profile_scopes_codex_home_and_scrubs_inherited_keys() {
        let args = vec![OsString::from("--search")];
        let command = build_codex_command("alt", Path::new("/tmp/.codex-alt"), &args);

        assert_eq!(command.get_program(), OsStr::new("codex"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            args.iter().collect::<Vec<_>>()
        );
        let envs = command.get_envs().collect::<Vec<_>>();
        assert!(envs.contains(&(OsStr::new("CODEX_HOME"), Some(OsStr::new("/tmp/.codex-alt")))));
        assert!(envs.contains(&(OsStr::new("OPENAI_API_KEY"), None)));
        assert!(envs.contains(&(OsStr::new("CODEX_API_KEY"), None)));
        assert!(envs.contains(&(OsStr::new("CODEX_ACCESS_TOKEN"), None)));
    }

    #[test]
    fn main_profile_clears_the_selector_so_codex_uses_its_own_default_home() {
        let command = build_codex_command("main", Path::new("/tmp/.codex"), &[]);
        let envs = command.get_envs().collect::<Vec<_>>();

        assert!(envs.contains(&(OsStr::new("CODEX_HOME"), None)));
        assert!(envs.contains(&(OsStr::new("OPENAI_API_KEY"), None)));
    }

    #[test]
    fn prompts_only_for_interactive_ambiguous_account_selection() {
        let profiles = vec![
            Profile {
                name: "main".to_string(),
                directory: PathBuf::from("/tmp/.codex"),
                login_state: LoginState::LoggedIn,
                active: false,
            },
            Profile {
                name: "alt".to_string(),
                directory: PathBuf::from("/tmp/.codex-alt"),
                login_state: LoginState::Unknown,
                active: false,
            },
        ];

        assert!(should_prompt_for_profile(None, &profiles, true, true));
        assert!(!should_prompt_for_profile(None, &profiles, false, true));
        assert!(!should_prompt_for_profile(None, &profiles, true, false));
        assert!(!should_prompt_for_profile(Some("alt"), &profiles, true, true));
        // A single detected account is not a choice.
        assert!(!should_prompt_for_profile(None, &profiles[..1], true, true));
    }

    #[test]
    fn unknown_login_state_stays_visible_in_the_choice_label() {
        let profile = Profile {
            name: "alt".to_string(),
            directory: PathBuf::from("/tmp/.codex-alt"),
            login_state: LoginState::Unknown,
            active: true,
        };

        assert_eq!(
            profile_choice_label(&profile),
            "alt (login state unknown, active)"
        );
    }

    #[test]
    fn shared_seed_entries_never_include_credentials_or_runtime_state() {
        for forbidden in [
            "auth.json",
            "history.jsonl",
            "sessions",
            "log",
            "installation_id",
            "cache",
        ] {
            assert!(
                !SHARED_PROFILE_ENTRIES.contains(&forbidden),
                "{forbidden} must stay private to a codex profile"
            );
        }
        assert!(SHARED_PROFILE_ENTRIES.contains(&"config.toml"));
        assert!(SHARED_PROFILE_ENTRIES.contains(&"skills"));
    }

    #[test]
    fn seeding_links_shared_config_is_idempotent_and_never_touches_credentials() {
        let home = TempDir::new().unwrap();
        let main = home.path().join(".codex");
        let profile = home.path().join(".codex-work@example.com");
        std::fs::create_dir_all(main.join("skills")).unwrap();
        std::fs::write(main.join("config.toml"), "model = \"gpt-5\"\n").unwrap();
        std::fs::write(main.join("auth.json"), "{\"secret\":true}").unwrap();

        let first = seed_profile_dir(&main, &profile, SHARED_PROFILE_ENTRIES).unwrap();
        assert_eq!(first.linked, vec!["config.toml", "skills"]);
        assert!(first.absent_in_source.contains(&"AGENTS.md".to_string()));
        // The shared surface resolves through the link...
        assert_eq!(
            std::fs::read_to_string(profile.join("config.toml")).unwrap(),
            "model = \"gpt-5\"\n"
        );
        // ...and credentials were not carried across, by omission or by refusal.
        assert!(!profile.join("auth.json").exists());
        let refused = seed_profile_dir(&main, &profile, &["auth.json"]).unwrap();
        assert_eq!(refused.refused, vec!["auth.json"]);
        assert!(!profile.join("auth.json").exists());

        // Re-seeding leaves an operator's diverged file alone.
        std::fs::remove_file(profile.join("config.toml")).unwrap();
        std::fs::write(profile.join("config.toml"), "model = \"mine\"\n").unwrap();
        let second = seed_profile_dir(&main, &profile, SHARED_PROFILE_ENTRIES).unwrap();
        assert!(second.linked.is_empty());
        assert!(second.already_present.contains(&"config.toml".to_string()));
        assert_eq!(
            std::fs::read_to_string(profile.join("config.toml")).unwrap(),
            "model = \"mine\"\n"
        );
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
