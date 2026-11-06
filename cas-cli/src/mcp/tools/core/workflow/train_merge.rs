//! Ordered, durable supervisor merge intents for a release train.

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn train_merge_requires_registered_supervisor() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        let core = crate::mcp::CasCore::with_daemon(root, None, None);
        core.set_agent_id_for_testing("worker".into());
        let service = crate::mcp::CasService::new(core, None);
        let request = serde_json::from_value(serde_json::json!({
            "action": "train_merge", "id": "cas-train"
        })).unwrap();
        let error = service.factory(rmcp::handler::server::wrapper::Parameters(request))
            .await.expect_err("worker cannot reserve a train");
        assert!(error.message.contains("registered supervisor"), "{}", error.message);
    }
}
