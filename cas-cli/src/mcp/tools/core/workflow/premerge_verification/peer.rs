//! Legacy peer approval messages are readable but carry no decision authority.
use std::path::Path;
use cas_types::Agent;
use crate::cloud::peer_mailbox::ClaimedPeerMessage;

pub(crate) const TAG: &str = "CAS_PEER_PREMERGE_V1 ";

#[derive(Debug)]
pub(crate) enum PeerReceiptIngested {
    NotAReceipt,
    Rejected(String),
}

pub(crate) fn ingest_peer_receipt(
    _root: &Path,
    _recipient: &Agent,
    message: &ClaimedPeerMessage,
) -> Result<PeerReceiptIngested, String> {
    Ok(if message.body.starts_with(TAG) {
        PeerReceiptIngested::Rejected("Peer approvals no longer create verification authority; the train owner must verify this delivery.".into())
    } else {
        PeerReceiptIngested::NotAReceipt
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::peer_mailbox::PeerScope;
    use cas_types::{AgentRole, PeerPremergeReceipt, Task, VerificationStatus};
    use super::super::{git, portable_plan_digest};

    #[tokio::test(flavor = "multi_thread")]
    async fn peer_approval_cannot_create_an_owner_dispatch_cas_b674() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        const PROJECT: &str = "peer-repair-fixture";
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/api/agents"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"agents":[{
                "id":"source-supervisor", "name":"source", "agent_type":"primary", "status":"active",
                "machine_id":"source-machine", "last_heartbeat":chrono::Utc::now().to_rfc3339(), "active_tasks":0,
                "metadata":{"role":"supervisor", "canonical_id":PROJECT, "factory_session":"source-session"}
            }, {
                "id":"other-supervisor", "name":"other", "agent_type":"primary", "status":"active",
                "machine_id":"other-machine", "last_heartbeat":chrono::Utc::now().to_rfc3339(), "active_tasks":0,
                "metadata":{"role":"supervisor", "canonical_id":PROJECT, "factory_session":"other-session"}
            }]}))).mount(&server).await;
        let endpoint = server.uri();
        tokio::task::spawn_blocking(move || {
            let _env = crate::test_support::TestEnvGuard::temp_home();
            let home = _env.home();
            let origin = home.join("origin.git");
            let source = home.join("source");
            let receiver = home.join("receiver");
            git(home, &["init", "--bare", "-b", "main", origin.to_str().unwrap()]).unwrap();
            git(home, &["init", "-b", "main", source.to_str().unwrap()]).unwrap();
            git(&source, &["config", "user.name", "Review fixture"]).unwrap();
            git(&source, &["config", "user.email", "review@example.test"]).unwrap();
            git(&source, &["remote", "add", "origin", origin.to_str().unwrap()]).unwrap();
            std::fs::write(source.join(".gitignore"), ".cas/\n").unwrap();
            git(&source, &["add", "."]).unwrap();
            git(&source, &["commit", "-qm", "base"]).unwrap();
            let base = git(&source, &["rev-parse", "HEAD"]).unwrap();
            git(&source, &["switch", "-c", "factory/peer"]).unwrap();
            std::fs::write(source.join("delivery"), "old\n").unwrap();
            git(&source, &["add", "."]).unwrap();
            git(&source, &["commit", "-qm", "delivery"]).unwrap();
            let old_sha = git(&source, &["rev-parse", "HEAD"]).unwrap();
            git(&source, &["push", "origin", "HEAD:main"]).unwrap();
            git(home, &["clone", "--branch", "main", origin.to_str().unwrap(), receiver.to_str().unwrap()]).unwrap();
            let root = crate::store::init_cas_dir(&receiver).unwrap();
            crate::cloud::set_canonical_id_in_config_toml(&root, PROJECT).unwrap();
            crate::cloud::CloudConfig { endpoint, token: Some("fixture-token".into()), ..Default::default() }
                .save_to_cas_dir(&root).unwrap();
            let recipient = Agent::new_with_role("receiver-supervisor".into(), "receiver".into(), AgentRole::Supervisor);
            crate::store::open_agent_store(&root).unwrap().register(&recipient).unwrap();
            let tasks = crate::store::open_task_store_local(&root).unwrap();
            let mut task = Task::new("cas-d7f9-repair".into(), "Reviewed repair".into());
            task.status = cas_types::TaskStatus::AwaitingMerge;
            task.deliverables.parked_branch = Some("factory/peer".into());
            task.deliverables.factory_branch_anchor = Some(old_sha.clone());
            task.deliverables.work_target = Some(cas_types::WorkTarget {
                repo_selector: format!("project:{PROJECT}"), target_branch: "main".into(),
            });
            tasks.add(&task).unwrap();
            let receipt = |task: &Task, sha: &str, target: &str, cycle: &str| PeerPremergeReceipt {
                registry_version: Some(serde_json::json!(["G-104"])),
                status: VerificationStatus::Approved, project_id: PROJECT.into(),
                supervisor_id: "source-supervisor".into(), task_id: task.id.clone(),
                source_branch: "factory/peer".into(), source_sha: sha.into(), target_branch: "main".into(),
                target_sha: target.into(), candidate_tree: git(&source, &["rev-parse", &format!("{sha}^{{tree}}")]).unwrap(),
                landed_sha: sha.into(), dispatch_id: format!("dispatch-{cycle}"), verdict_id: format!("ver-{cycle}"),
                plan_digest: portable_plan_digest(&root, task).unwrap(),
            };
            let message = |receipt: &PeerPremergeReceipt, age: i64| ClaimedPeerMessage {
                id: format!("message-{}", receipt.dispatch_id), project_id: PROJECT.into(),
                recipient_agent_id: recipient.id.clone(), sender_agent_id: "source-supervisor".into(),
                sender_name: "source".into(), sender_machine_id: Some("source-machine".into()),
                sender_session: Some("source-session".into()), sender_label: None,
                body: format!("{TAG}{}", serde_json::to_string(receipt).unwrap()), summary: None, in_reply_to: None,
                created_at: (chrono::Utc::now() - chrono::Duration::minutes(age)).to_rfc3339(),
                attempts: 1, scope: PeerScope::Project,
            };
            let old = receipt(&task, &old_sha, &base, "old");
            let old_message = message(&old, 2);
            // A peer may supply worker evidence, but cannot mint another
            // supervisor's authoritative verification cycle.
            assert!(matches!(ingest_peer_receipt(&root, &recipient, &old_message).unwrap(), PeerReceiptIngested::Rejected(_)));
            assert!(cas_store::get_latest_verification_dispatch(&root, &task.id).unwrap().is_none());
            assert!(crate::store::open_verification_store(&root).unwrap().get_latest_for_task(&task.id).unwrap().is_none());
            let local = cas_store::create_verification_dispatch(&root, &task.id, &recipient.id, &recipient.id,
                chrono::Utc::now() + chrono::Duration::minutes(10)).unwrap();
            assert_eq!(local.owner_agent_id, recipient.id);
        }).await.unwrap();
    }
}
