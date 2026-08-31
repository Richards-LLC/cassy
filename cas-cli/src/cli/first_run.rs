//! What a brand-new machine sees when someone types `cas`.
//!
//! Bare `cas` launches factory mode, which on a machine that has never been
//! configured fails a preflight and prints a list of everything that is wrong.
//! That is the correct output for a developer debugging a broken setup and the
//! wrong output for a person who just ran the install one-liner. This module
//! owns the one friendly line they get instead.
//!
//! # Why the command name is a constant
//!
//! The front door is `cas setup`, and the name lives in exactly one place. The
//! test below refuses to let a first-run hint ship if its named command is not
//! a real subcommand.

use std::path::Path;

/// The single command a brand-new machine is told to run.
///
/// The test `front_door_command_is_a_real_subcommand` fails the build if the
/// name here is not an actual clap subcommand, so the pointer can never become
/// a dead end.
pub const FRONT_DOOR_COMMAND: &str = "setup";

/// A machine nobody has set up yet: no host-level `~/.cas/` and no project to
/// stand in for it.
///
/// Deliberately conservative. If either exists, this is a configured machine
/// having a bad day — a broken project, a missing dependency — and the detailed
/// preflight output is what that person needs.
pub fn machine_is_unconfigured(host_cas_dir: &Path, project_cas_root: Option<&Path>) -> bool {
    project_cas_root.is_none() && !host_cas_dir.exists()
}

/// The one line. Not a banner, not a checklist: someone who just installed a
/// binary needs the next command and nothing else.
pub fn front_door_hint() -> String {
    format!("Welcome to Cassy. Run `cas {FRONT_DOOR_COMMAND}` in a project directory to get started.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// The whole point of the constant. A first-run pointer that names a
    /// command the binary does not have strands exactly the person it is meant
    /// to help, and nothing else in the codebase would catch it.
    #[test]
    fn front_door_command_is_a_real_subcommand() {
        let command = crate::cli::Cli::command();
        let names: Vec<_> = command.get_subcommands().map(|c| c.get_name()).collect();
        assert!(
            names.contains(&FRONT_DOOR_COMMAND),
            "first-run hint points at `cas {FRONT_DOOR_COMMAND}`, which is not a subcommand. \
             Available: {names:?}"
        );
    }

    #[test]
    fn hint_names_the_front_door_command_once_and_fits_one_line() {
        let hint = front_door_hint();
        assert!(hint.contains(&format!("cas {FRONT_DOOR_COMMAND}")));
        assert!(!hint.contains('\n'), "first-run hint must be one line: {hint}");
    }

    #[test]
    fn a_host_cas_dir_means_the_machine_is_configured() {
        let temp = tempfile::tempdir().unwrap();
        let host_cas = temp.path().join(".cas");
        std::fs::create_dir_all(&host_cas).unwrap();
        assert!(!machine_is_unconfigured(&host_cas, None));
    }

    #[test]
    fn a_project_root_means_the_machine_is_configured() {
        let temp = tempfile::tempdir().unwrap();
        let absent_host_cas = temp.path().join("nope/.cas");
        let project = temp.path().join("project/.cas");
        assert!(!machine_is_unconfigured(&absent_host_cas, Some(&project)));
    }

    #[test]
    fn no_host_dir_and_no_project_is_a_fresh_machine() {
        let temp = tempfile::tempdir().unwrap();
        let absent_host_cas = temp.path().join("nope/.cas");
        assert!(machine_is_unconfigured(&absent_host_cas, None));
    }
}
