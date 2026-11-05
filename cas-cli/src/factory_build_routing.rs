//! Build admission overflow uses the existing peer directory and mailbox.
use std::path::Path;

pub(crate) fn route(
    _root: &Path,
    _sender: &cas_types::Agent,
    _task_id: &str,
    _brief: Option<&str>,
    _config: &crate::config::FactoryConfig,
    _guard: &crate::factory_build_guard::BuildGuardSnapshot,
) -> Result<Option<String>, String> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use wiremock::matchers::{method, path};

    const REPO: &str = "github.com/acme/widgets";

    fn peer(id: &str, slots: usize, load: f64, age: i64) -> serde_json::Value {
        serde_json::json!({
            "id": id, "name": id, "agent_type": "primary", "status": "active",
            "last_heartbeat": (Utc::now() - Duration::seconds(age)).to_rfc3339(),
            "active_tasks": 0, "machine_id": format!("machine-{id}"),
            "metadata": {
                "role": "supervisor", "canonical_id": REPO,
                "factory_session": "peer-factory", "hostname": id,
                "build_capacity_v1": serde_json::json!({
                    "observed_at": (Utc::now() - Duration::seconds(age)).to_rfc3339(),
                    "free_slots": slots, "load_1m": load, "cpu_count": 8,
                    "cpu_psi_avg10": null, "memory_psi_avg10": null
                }).to_string()
            }
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cas_d1fe_routes_admission_overflow_once_and_preserves_local_fallbacks() {
        let server = MockServer::start().await;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_USER_CLOUD_JSON", env.home().join("no-cloud.json"));
        let root = crate::store::init_cas_dir(env.home()).unwrap();
        crate::cloud::set_canonical_id_in_config_toml(&root, REPO).unwrap();
        let cloud = crate::cloud::CloudConfig {
            endpoint: server.uri(), token: Some("synthetic-test-token".into()),
            ..Default::default()
        };
        cloud.save_to_cas_dir(&root).unwrap();
        let mut sender = cas_types::Agent::new("local-sup".into(), "local-sup".into());
        sender.role = cas_types::AgentRole::Supervisor;
        sender.factory_session = Some("local-factory".into());
        sender.metadata.insert("hostname".into(), "local".into());
        crate::store::open_agent_store(&root).unwrap().register(&sender).unwrap();
        let tasks = crate::store::open_task_store(&root).unwrap();
        for (case, peers, off, note, expect_route) in [
            ("fresh", vec![peer("few", 1, 1.0, 0), peer("busy", 3, 8.0, 0), peer("best", 3, 4.0, 0)], false, "", true),
            ("absent", vec![], false, "", false),
            ("stale", vec![peer("best", 3, 4.0, 91)], false, "", false),
            ("future", vec![peer("best", 3, 4.0, -30)], false, "", false),
            ("saturated", vec![peer("best", 0, 4.0, 0)], false, "", false),
            ("off", vec![peer("best", 3, 4.0, 0)], true, "", false),
            ("pingpong", vec![peer("best", 3, 4.0, 0)], false, "build-route:github.com/acme/widgets:cas-pingpong from another-machine", false),
        ] {
            server.reset().await;
            Mock::given(method("GET")).and(path("/api/agents"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"agents": peers})))
                .mount(&server).await;
            Mock::given(method("POST")).and(path("/api/peer-messages"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": format!("message-{case}"), "status": "queued"})))
                .mount(&server).await;
            let mut task = cas_types::Task::new(format!("cas-{case}"), "Compile change".into());
            task.description = "Full task description".into();
            task.notes = note.into();
            tasks.add(&task).unwrap();
            let config: crate::config::Config = toml::from_str(&format!("[factory]\nauto_route_builds = {}\n", !off)).unwrap();
            let config = config.factory();
            let guard = crate::factory_build_guard::evaluate(8, Some(24.0), 5, 1, 5);
            assert!(!guard.violations().is_empty());
            let (root_copy, sender_copy, id) = (root.clone(), sender.clone(), task.id.clone());
            let outcome = tokio::task::spawn_blocking(move || route(&root_copy, &sender_copy, &id, Some("Supervisor full brief"), &config, &guard))
                .await.unwrap().unwrap();
            assert_eq!(outcome.is_some(), expect_route, "{case}: {outcome:?}");
            if let Some(receipt) = outcome {
                assert!(receipt.contains("best") && receipt.contains("3 free slots") && receipt.contains("0.50"), "{receipt}");
                let updated = tasks.get(&task.id).unwrap();
                assert!(updated.assignee.is_none());
                assert!(updated.notes.contains("build-route:") && updated.notes.contains("best"));
                let (root_copy, sender_copy, id) = (root.clone(), sender.clone(), task.id.clone());
                tokio::task::spawn_blocking(move || route(&root_copy, &sender_copy, &id, None, &Default::default(), &crate::factory_build_guard::evaluate(8, Some(24.0), 5, 1, 5)))
                    .await.unwrap().unwrap();
            }
            let sends = server.received_requests().await.unwrap().into_iter()
                .filter(|r| r.method == "POST" && r.url.path() == "/api/peer-messages").collect::<Vec<_>>();
            assert_eq!(sends.len(), usize::from(expect_route), "{case}: no duplicate route");
            if expect_route {
                let body: serde_json::Value = serde_json::from_slice(&sends[0].body).unwrap();
                assert_eq!(body["recipient_agent_id"], "best");
                assert!(body["body"].as_str().unwrap().contains("Full task description"));
                assert!(body["body"].as_str().unwrap().contains("Supervisor full brief"));
                assert!(body["body"].as_str().unwrap().contains("ask"));
                assert_eq!(body["dedupe_key"], format!("build-route:{REPO}:{}", task.id));
            } else {
                assert_eq!(tasks.get(&task.id).unwrap().notes, note);
            }
        }
    }
}
