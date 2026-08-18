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
use super::account_picker::{
    LoginState, Profile, ProfileLayout, Selection, report_seeding, scan_profiles_with,
    seed_profile_dir, should_prompt_for_profile,
};
use super::factory::FactoryArgs;

/// `main` → `~/.codex`, any other name → `~/.codex-<name>`, matching the
/// `~/.claude-<name>` convention.
const LAYOUT: ProfileLayout = ProfileLayout {
    main_dir: ".codex",
    named_prefix: ".codex-",
    main_name: "main",
};

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

/// Entries that must stay private to a codex profile, asserted against the
/// shared list so credentials can never be seeded into another account.
pub(crate) const PRIVATE_PROFILE_ENTRIES: &[&str] = &[
    "auth.json",
    "history.jsonl",
    "sessions",
    "log",
    "installation_id",
    "cache",
    "tmp",
    "shell_snapshots",
    "models_cache.json",
];

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

/// Resolve a convention-based profile name under `home`.
/// Detect codex accounts, probing each with the codex CLI.
fn scan_profiles(home: &Path, active_dir: Option<&Path>) -> io::Result<Vec<Profile>> {
    scan_profiles_with(LAYOUT, home, active_dir, probe_login_state)
}

pub(crate) fn resolve_profile_dir(home: &Path, profile: &str) -> PathBuf {
    LAYOUT.profile_dir(home, profile)
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
        let login = super::account_picker::login_state_label(profile.login_state);
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

/// Resolve the picker to a profile name, running a new login when asked.
///
/// The prompt itself (rows, labels, cursor, page size, the new-login row and
/// its validated email entry) is shared with `cas claude`; only what happens
/// after "new login" is codex-specific.
fn prompt_for_profile(home: &Path, profiles: &[Profile]) -> Result<String> {
    match super::account_picker::prompt_for_selection("Codex", LAYOUT, profiles)? {
        Selection::Existing(name) => Ok(name),
        Selection::NewLogin(name) => new_login(home, &name),
    }
}

/// Create, seed and sign in a brand new codex account, then return its profile
/// name so the caller can launch on it immediately.
fn new_login(home: &Path, profile: &str) -> Result<String> {
    let profile_dir = resolve_profile_dir(home, profile);
    std::fs::create_dir_all(&profile_dir).with_context(|| {
        format!(
            "could not create Codex profile directory {}",
            profile_dir.display()
        )
    })?;

    let report = seed_profile_dir(
        &home.join(".codex"),
        &profile_dir,
        SHARED_PROFILE_ENTRIES,
        PRIVATE_PROFILE_ENTRIES,
    )?;
    report_seeding(&report, "~/.codex");
    eprintln!("Signing in Codex account home: {}", profile_dir.display());

    // Deliberately `status()` rather than `exec()`: this runs inside the launch
    // path, so the process must survive the login and go on to start the factory
    // on the account that was just created.
    let status = build_codex_command(profile, &profile_dir, &[OsString::from("login")])
        .status()
        .context("could not run `codex login`; install the Codex CLI or add it to PATH")?;
    if !status.success() {
        anyhow::bail!(
            "`codex login` did not complete for {}; the account was not added",
            profile_dir.display()
        );
    }
    if probe_login_state(profile, &profile_dir) == LoginState::LoggedOut {
        anyhow::bail!(
            "{} still reports `Not logged in` after the login flow",
            profile_dir.display()
        );
    }
    Ok(profile.to_string())
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
    if args.command.is_some() || args.list_profiles {
        return Ok(());
    }

    let home = dirs::home_dir().context("cannot determine home directory for Codex profiles")?;

    let profile = match args.profile() {
        // An explicitly named account is the no-prompt fast path.
        Some(profile) => profile.to_string(),
        None => {
            let active_dir = std::env::var_os("CODEX_HOME").map(PathBuf::from);
            let profiles = scan_profiles(&home, active_dir.as_deref()).with_context(|| {
                format!("could not inspect Codex profiles under {}", home.display())
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
    eprintln!("Using Codex account home: {}", profile_dir.display());

    set_profile_env(&profile, &profile_dir);
    // `execute`/`execute_bare` run later in the same process; record the choice
    // so the operator is asked once, not once per launch path.
    let _ = SELECTED_PROFILE.set(profile);

    Ok(())
}

/// The account chosen during `apply_profile_env`, for later launch paths.
static SELECTED_PROFILE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

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

    // Account selection already happened in `apply_profile_env`, deliberately:
    // it runs before `initialize_telemetry` spawns a thread, and `set_var` in a
    // multi-threaded process is UB. Prompting here would also ask twice
    // (cas-898d lesson).
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
        // `apply_profile_env` already ran the picker in the single-threaded
        // window; reuse its answer instead of asking a second time.
        None => match SELECTED_PROFILE.get() {
            Some(selected) => selected.clone(),
            None => "main".to_string(),
        },
    };
    let profile_dir = resolve_profile_dir(&home, &profile);

    // `apply_profile_env` announces whatever it selected. Announcing again here
    // printed the account line twice for every explicit-profile launch
    // (cas-898d lesson 3), so only speak up for the fallback it left alone.
    if SELECTED_PROFILE.get().is_none() && args.profile().is_none() {
        warn_about_profile_state(&profile, &profile_dir);
        eprintln!("Using Codex account home: {}", profile_dir.display());
    }

    let mut command = build_codex_command(&profile, &profile_dir, args.passthrough_args());
    exec_codex(&mut command)
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
                let report = seed_profile_dir(
                    &home.join(".codex"),
                    &profile_dir,
                    SHARED_PROFILE_ENTRIES,
                    PRIVATE_PROFILE_ENTRIES,
                )
                .with_context(|| {
                    format!("could not seed Codex profile {}", profile_dir.display())
                })?;
                report_seeding(&report, "~/.codex");
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
    fn resolves_main_and_named_profiles_through_the_shared_layout() {
        let home = Path::new("/tmp/test-home");

        assert_eq!(resolve_profile_dir(home, "main"), home.join(".codex"));
        assert_eq!(resolve_profile_dir(home, "alt"), home.join(".codex-alt"));
        assert_eq!(
            resolve_profile_dir(home, "daniel@petrastella.io"),
            home.join(".codex-daniel@petrastella.io")
        );
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
    fn shared_seed_entries_never_include_credentials_or_runtime_state() {
        // The shared helper asserts this too; keeping it here pins the codex
        // lists themselves, which are the provider-specific half of the split.
        for entry in SHARED_PROFILE_ENTRIES {
            assert!(
                !PRIVATE_PROFILE_ENTRIES.contains(entry),
                "{entry} appears in both the shared and private codex lists"
            );
        }
        for forbidden in ["auth.json", "history.jsonl", "sessions", "log", "installation_id"] {
            assert!(
                PRIVATE_PROFILE_ENTRIES.contains(&forbidden),
                "{forbidden} must be declared private for codex profiles"
            );
        }
        assert!(SHARED_PROFILE_ENTRIES.contains(&"config.toml"));
        assert!(SHARED_PROFILE_ENTRIES.contains(&"skills"));
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
