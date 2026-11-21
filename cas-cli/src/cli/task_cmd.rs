//! `cas task`: operator-only task decisions taken at the operator's own terminal.

use std::path::Path;

use clap::Subcommand;

#[derive(Subcommand)]
pub enum TaskCommands {
    /// Operator-only: authorize closing an epic's historical deliveries already contained in their target
    RecoveryDecision {
        /// Epic whose contained historical deliveries this decision covers
        #[arg(long)]
        scope: String,
        /// What you decided and why; recorded on the epic
        #[arg(long)]
        note: String,
    },
}

pub fn execute(command: &TaskCommands, cas_root: Option<&Path>) -> anyhow::Result<()> {
    use crate::config::operator_policy::{InvocationContext, operator_action_refusal};
    match command {
        TaskCommands::RecoveryDecision { scope, note } => {
            if let Some(refusal) = operator_action_refusal(
                "Authorizing recovery of contained historical deliveries",
                "run `cas task recovery-decision` yourself from your own terminal",
                &InvocationContext::from_process(),
            ) {
                anyhow::bail!(refusal);
            }
            let root =
                cas_root.ok_or_else(|| anyhow::anyhow!("No Cassy project; run cas init first"))?;
            let store = crate::store::open_task_store(root)?;
            let decision =
                crate::recovery_decision::record(root, store.as_ref(), scope, note, chrono::Utc::now())?;
            println!(
                "Recovery decision {} recorded for {}. Each close cites operator_decision={} and commit_receipt=<containment commit>.",
                decision.id, decision.scope, decision.id
            );
            Ok(())
        }
    }
}
