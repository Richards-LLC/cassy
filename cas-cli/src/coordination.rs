//! Durable, fail-closed authority operations for external Slack threads.

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
                cas_mcp::actions::COORDINATION_ACTIONS.contains(&action),
                "coordination action {action} is missing"
            );
        }
    }
}
