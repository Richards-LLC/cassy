//! Project hold mutations are deliberately absent from MCP and Commander ops.
use std::{io::Write, path::Path};

use anyhow::{Context, Result, bail};
use cas_store::project_hold::{Decision, Hold, STATE_FILE, State};
use chrono::Utc;
use clap::Subcommand;

use crate::config::operator_policy::{InvocationContext, operator_action_refusal};

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum HoldCommand {
    /// Stop project dispatch and merges (operator only)
    Set {
        #[arg(long)]
        reason: String,
    },
    /// Resume project dispatch and merges (operator only)
    Lift {
        #[arg(long)]
        reason: String,
    },
    /// Inspect the active hold and its audit history
    Status,
}

pub(super) fn execute(
    command: &HoldCommand,
    cli: &crate::cli::Cli,
    root: Option<&Path>,
) -> Result<()> {
    let root = root.context("Project hold requires a Cassy project")?;
    let state = match command {
        HoldCommand::Status => cas_store::project_hold::load(root)?,
        _ => change(root, command, &InvocationContext::from_process())?,
    };
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&state)?);
    } else if let Some(notice) = cas_store::project_hold::notice(root) {
        println!("{notice}");
    } else {
        println!("Project hold: off\nDispatch and merges follow normal admission.");
    }
    Ok(())
}

fn change(root: &Path, command: &HoldCommand, caller: &InvocationContext) -> Result<State> {
    if let Some(refusal) = operator_action_refusal(
        "Setting or lifting a project hold",
        "run `cas factory hold` yourself from your own terminal",
        caller,
    ) {
        bail!(refusal);
    }
    let (action, reason) = match command {
        HoldCommand::Set { reason } => ("set", reason.trim()),
        HoldCommand::Lift { reason } => ("lift", reason.trim()),
        HoldCommand::Status => bail!("Status does not change a hold"),
    };
    if reason.is_empty() {
        bail!("Project hold requires a nonempty --reason");
    }
    let path = root.join(STATE_FILE);
    let parent = path
        .parent()
        .context("Project hold has no state directory")?;
    std::fs::create_dir_all(parent)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(parent.join("project-hold.lock"))?;
    fs2::FileExt::lock_exclusive(&lock)?;
    let mut state = cas_store::project_hold::load(root)?;
    if action == "set" && state.active.is_some() {
        bail!("Project already has a hold; lift it before replacing it");
    }
    if action == "lift" && state.active.is_none() {
        bail!("Project has no active hold");
    }
    let operator = std::process::Command::new("id")
        .arg("-un")
        .output()
        .context("Cannot identify the operator")?;
    if !operator.status.success() {
        bail!("Cannot identify the operator");
    }
    let operator = String::from_utf8(operator.stdout)?.trim().to_string();
    if operator.is_empty() {
        bail!("Cannot identify the operator");
    }
    let now = Utc::now();
    state.active = (action == "set").then(|| Hold {
        reason: reason.into(),
        operator: operator.clone(),
        set_at: now,
    });
    state.history.push(Decision {
        action: action.into(),
        reason: reason.into(),
        operator,
        at: now,
    });
    let mut staging = tempfile::NamedTempFile::new_in(parent)?;
    staging.write_all(&serde_json::to_vec_pretty(&state)?)?;
    staging.as_file().sync_all()?;
    staging.persist(path)?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_hold_authority_and_audit_cover_set_and_lift() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let operator = InvocationContext {
            env_names: Default::default(),
            ancestors: vec!["bash".into()],
            stdin_is_terminal: true,
            stdout_is_terminal: true,
            cgroup: String::new(),
        };
        for command in [
            HoldCommand::Set {
                reason: "review plan".into(),
            },
            HoldCommand::Lift {
                reason: "reviewed".into(),
            },
        ] {
            // Each identity kind must be refused for either state transition.
            for (env, ancestor, cgroup, terminal) in [
                ("CAS_AGENT_ROLE", "bash", "", true),
                ("CLAUDECODE", "bash", "", true),
                ("", "codex", "", true),
                ("", "claude", "", true),
                ("", "grok", "", true),
                ("", "bash", "0::/user.slice/cas-worker-example", true),
                ("", "bash", "", false),
            ] {
                let caller = InvocationContext {
                    env_names: if env.is_empty() {
                        Default::default()
                    } else {
                        [env.into()].into()
                    },
                    ancestors: vec![ancestor.into()],
                    cgroup: cgroup.into(),
                    stdin_is_terminal: terminal,
                    stdout_is_terminal: terminal,
                };
                assert!(
                    change(dir.path(), &command, &caller)
                        .unwrap_err()
                        .to_string()
                        .contains("operator-only")
                );
            }
            let state = change(dir.path(), &command, &operator).unwrap();
            assert_eq!(
                state.active.is_some(),
                matches!(command, HoldCommand::Set { .. })
            );
        }
        let state = cas_store::project_hold::load(dir.path()).unwrap();
        assert_eq!(
            state
                .history
                .iter()
                .map(|d| d.action.as_str())
                .collect::<Vec<_>>(),
            ["set", "lift"]
        );
        assert_eq!(state.history[0].reason, "review plan");
        assert_eq!(state.history[1].reason, "reviewed");
        assert!(!state.history[0].operator.is_empty());
        assert!(state.history[1].at >= state.history[0].at);
        let before = std::fs::read(dir.path().join(STATE_FILE)).unwrap();
        assert!(
            change(
                dir.path(),
                &HoldCommand::Set {
                    reason: "  ".into()
                },
                &operator
            )
            .is_err()
        );
        assert_eq!(std::fs::read(dir.path().join(STATE_FILE)).unwrap(), before);
    }
}
