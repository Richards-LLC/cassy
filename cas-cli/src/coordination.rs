//! Durable, fail-closed authority operations for external Slack threads.

pub mod binding_reconciliation;
pub mod binding_intake;
pub mod binding_repair;
mod binding_runtime;
pub mod external_work;
#[cfg(feature = "mcp-proxy")]
pub mod stakeholder_contacts;

#[cfg(test)]
mod tests {
    #[test]
    fn coordination_actions_are_registered() {
        for action in [
            "claim",
            "claim_status",
            "release_claim",
            "transfer_claim",
            "accept_claim",
        ] {
            assert!(
                cas_mcp::actions::FACTORY_ACTIONS.contains(&action),
                "factory action {action} is missing"
            );
            assert!(
                !cas_mcp::actions::COORDINATION_ACTIONS.contains(&action),
                "coordination must not publish supervisor-only action {action}"
            );
        }
    }
}
