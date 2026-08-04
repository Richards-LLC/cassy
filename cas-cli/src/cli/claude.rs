//! `cas claude` — launch the CAS factory with a Claude supervisor on an
//! explicitly selected account profile.
//!
//! `cas claude alt` is the operator-facing spelling for "run CAS, supervised by
//! Claude, signed in as my alt subscription". It is the Claude sibling of
//! `cas codex` / `cas grok`, with one extra positional: the account profile.
//!
//! Account selection works by exporting `CLAUDE_CONFIG_DIR` into this process
//! before the factory starts. Panes inherit it (`cas-pty` never calls
//! `env_clear`), so the supervisor runs on the chosen account, and
//! `spawn_workers` captures the same value as `requester_config_dir` so workers
//! land on that account too.
//!
//! `--bare` keeps the plain "just open Claude Code on this profile" launcher.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use clap::{Args, FromArgMatches};

use super::factory::FactoryArgs;
use super::Cli;

/// Arguments for `cas claude [profile] [factory-args...]`.
#[derive(Args, Clone, Debug)]
pub struct ClaudeArgs {
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

#[derive(Debug, Eq, PartialEq)]
struct Profile {
    name: String,
    directory: PathBuf,
    logged_in: bool,
    active: bool,
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
    let mut profiles = vec![profile_for("main", home.join(".claude"), active_dir)];

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
            if profile_name.is_empty() {
                continue;
            }
            profiles.push(profile_for(profile_name, path, active_dir));
        }
    }

    profiles[1..].sort_by(|left, right| left.name.cmp(&right.name));
    Ok(profiles)
}

fn profile_for(name: &str, directory: PathBuf, active_dir: Option<&Path>) -> Profile {
    Profile {
        name: name.to_string(),
        logged_in: directory.join(".credentials.json").is_file(),
        active: active_dir.is_some_and(|active| active == directory),
        directory,
    }
}

fn profile_listing(home: &Path, active_dir: Option<&Path>) -> Result<String> {
    let profiles = scan_profiles(home, active_dir)
        .with_context(|| format!("could not inspect Claude profiles under {}", home.display()))?;
    let mut output = String::from(
        "Usage: cas claude <profile> [factory-args...]\n\nDetected Claude profiles:\n",
    );

    for profile in profiles {
        let login = if profile.logged_in {
            "logged in"
        } else {
            "not logged in"
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

/// Build the command used for a `--bare` profile launch without executing it.
pub(crate) fn build_claude_command(profile_dir: &Path, args: &[OsString]) -> Command {
    let mut command = Command::new("claude");
    command
        .env("CLAUDE_CONFIG_DIR", profile_dir)
        // An inherited key takes priority over Claude Code OAuth, defeating the
        // caller's explicit subscription/profile choice.
        .env_remove("ANTHROPIC_API_KEY")
        .args(args);
    command
}

/// Warn about a profile directory that Claude will have to bootstrap or log in.
fn warn_about_profile_state(profile_dir: &Path) {
    if !profile_dir.is_dir() {
        eprintln!(
            "Note: {} does not exist yet; Claude will create it and prompt you to /login.",
            profile_dir.display()
        );
    } else if !profile_dir.join(".credentials.json").is_file() {
        eprintln!(
            "Note: {} is not logged in yet; Claude will prompt you to /login.",
            profile_dir.display()
        );
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
    if args.list_profiles || args.bare {
        return Ok(());
    }
    let Some(profile) = args.profile.as_deref() else {
        return Ok(());
    };

    let home = dirs::home_dir().context("cannot determine home directory for Claude profiles")?;
    let profile_dir = resolve_profile_dir(&home, profile);
    warn_about_profile_state(&profile_dir);
    eprintln!("Using Claude account config: {}", profile_dir.display());

    // SAFETY: called from `cli::run` before `initialize_telemetry` spawns any
    // thread, so the process is still single-threaded here.
    unsafe {
        std::env::set_var("CLAUDE_CONFIG_DIR", &profile_dir);
        // An inherited API key overrides subscription OAuth entirely, which
        // would silently defeat the profile the operator just asked for.
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    Ok(())
}

/// Run `cas claude`: factory with a Claude supervisor by default, plain Claude
/// Code under `--bare`, profile listing under `--list-profiles`.
pub fn execute(args: &ClaudeArgs, cli: &Cli, cas_root: Option<&Path>) -> Result<()> {
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
    let profile = args.profile.as_deref().unwrap_or("main");
    let profile_dir = resolve_profile_dir(&home, profile);

    warn_about_profile_state(&profile_dir);
    eprintln!("Using Claude account config: {}", profile_dir.display());

    let mut command = build_claude_command(&profile_dir, &args.args);
    exec_claude(&mut command)
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
        std::fs::write(alt.join(".credentials.json"), "{}").unwrap();

        let profiles = scan_profiles(home.path(), Some(alt.as_path())).unwrap();

        assert_eq!(
            profiles,
            vec![
                Profile {
                    name: "main".to_string(),
                    directory: main,
                    logged_in: false,
                    active: false,
                },
                Profile {
                    name: "alt".to_string(),
                    directory: alt,
                    logged_in: true,
                    active: true,
                },
                Profile {
                    name: "work".to_string(),
                    directory: work,
                    logged_in: false,
                    active: false,
                },
            ]
        );
    }

    #[test]
    fn bare_launch_command_sets_profile_scrubs_api_key_and_forwards_args() {
        let args = vec![OsString::from("--continue"), OsString::from("--verbose")];
        let command = build_claude_command(Path::new("/tmp/.claude-alt"), &args);

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
        assert!(envs.contains(&(OsStr::new("ANTHROPIC_API_KEY"), None)));
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
