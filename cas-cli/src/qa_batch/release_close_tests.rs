use super::*;
use cas_store::{NewArtifact, PublishedArtifact, SqliteArtifactStore};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path}};

#[derive(Clone, Copy, Debug)]
enum ProofCase {
    Portable,
    ForeignOwner,
    TamperedDownload,
    RevokedRemote,
    UnavailableRemote,
    ReceiptMismatch,
    TamperedLocal,
    RevokedLocal,
}

#[tokio::test]
async fn imported_release_review_resolves_verified_peer_ledger_cas_52c1() {
    for case in [
        ProofCase::Portable,
        ProofCase::ForeignOwner,
        ProofCase::TamperedDownload,
        ProofCase::RevokedRemote,
        ProofCase::UnavailableRemote,
        ProofCase::ReceiptMismatch,
        ProofCase::TamperedLocal,
        ProofCase::RevokedLocal,
    ] {
        let server = MockServer::start().await;
        let body = "# Independent release review\n\nReviewed the shipped delivery.\n";
        let view = match case {
            ProofCase::RevokedRemote => ResponseTemplate::new(409)
                .set_body_json(serde_json::json!({"error":"artifact_revoked"})),
            ProofCase::UnavailableRemote => ResponseTemplate::new(503),
            _ => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "artifact_id":"cloud-ledger", "url":format!("{}/ledger", server.uri()),
                "expires_at":"2099-01-01T00:00:00Z", "name":"REVIEW.md",
                "mime":"text/markdown", "size_bytes":body.len(),
            })),
        };
        Mock::given(method("GET")).and(path("/api/artifacts/cloud-ledger/url"))
            .respond_with(view).mount(&server).await;
        let downloaded = if matches!(case, ProofCase::TamperedDownload) { "altered bytes" } else { body };
        Mock::given(method("GET")).and(path("/ledger"))
            .respond_with(ResponseTemplate::new(200).set_body_string(downloaded))
            .mount(&server).await;
        let endpoint = server.uri();
        tokio::task::spawn_blocking(move || {
            let mut env = crate::test_support::TestEnvGuard::temp_home();
            let repo = env.home().join("consumer");
            std::fs::create_dir_all(&repo).unwrap();
            let root = crate::store::init_cas_dir(&repo).unwrap();
            env.set("CAS_ROOT", &root);
            env.set("CAS_CLONE_PATH", &repo);
            std::fs::write(root.join("config.toml"), format!(
                "[project]\ncanonical_id='portable-review'\n[factory]\nartifacts_root={}\n",
                serde_json::to_string(&env.home().join("artifacts")).unwrap(),
            )).unwrap();
            crate::cloud::CloudConfig {
                endpoint, token: Some("test-fixture-token".into()), ..Default::default()
            }.save_to_cas_dir(&root).unwrap();
            let git = |args: &[&str]| {
                let out = Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap();
                assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
                String::from_utf8(out.stdout).unwrap().trim().to_string()
            };
            git(&["init", "-b", "main"]);
            git(&["config", "user.name", "Fixture"]);
            git(&["config", "user.email", "fixture@example.test"]);
            git(&["remote", "add", "origin", "https://github.com/example/portable-review.git"]);
            git(&["commit", "--allow-empty", "-m", "fixture"]);
            let head = git(&["rev-parse", "HEAD"]);
            let tasks = crate::store::open_task_store(&root).unwrap();
            let mut owner = Task::new("cas-proof".into(), "Published release review".into());
            owner.origin_project = Some(if matches!(case, ProofCase::ForeignOwner) {
                "another-project"
            } else { "portable-review" }.into());
            tasks.add(&owner).unwrap();
            let mut member = Task::new("cas-member".into(), "Shipped evidence".into());
            member.origin_project = Some("portable-review".into());
            member.execution_note = Some("no-code".into());
            tasks.add(&member).unwrap();
            let artifact = PublishedArtifact {
                id: "art-peer-ledger".into(), task_id: owner.id.clone(), name: "REVIEW.md".into(),
                mime: "text/markdown".into(), size_bytes: body.len() as u64,
                sha256: format!("{:x}", Sha256::digest(body.as_bytes())),
                cloud_artifact_id: Some("cloud-ledger".into()), status: "committed".into(),
                cloud_url: None, slack_permalink: None, created_at: "2026-11-10T00:00:00Z".into(),
                revocation: None,
            };
            tasks.append_trusted_note(&owner.id, &format!("ARTIFACT_PROOF: {}", serde_json::to_string(&artifact).unwrap())).unwrap();
            let store = SqliteArtifactStore::open(&root).unwrap();
            assert!(store.get(&artifact.id).unwrap().is_none());
            let mut ledger_path = "/Users/producer/.cas/artifacts/release/REVIEW.md".to_string();
            if matches!(case, ProofCase::TamperedLocal | ProofCase::RevokedLocal) {
                store.record_local(&NewArtifact {
                    id: artifact.id.clone(), task_id: owner.id.clone(), name: artifact.name.clone(),
                    mime: artifact.mime.clone(), size_bytes: artifact.size_bytes, sha256: artifact.sha256.clone(),
                }).unwrap();
                store.mark_uploaded(&artifact.id, "cloud-ledger").unwrap();
                store.mark_committed(&artifact.id, None).unwrap();
                let file = repo.join("REVIEW.md");
                std::fs::write(&file, if matches!(case, ProofCase::TamperedLocal) { "modified" } else { body }).unwrap();
                ledger_path = file.to_string_lossy().into_owned();
                if matches!(case, ProofCase::RevokedLocal) {
                    store.revoke(&artifact.id, "reviewer", "fixture revocation").unwrap();
                }
            }
            let hash = if matches!(case, ProofCase::ReceiptMismatch) { "0".repeat(64) } else { artifact.sha256.clone() };
            let proof = ReleaseProofRef {
                kind: ReleaseProofKind::Review, artifact_id: artifact.id.clone(), sha256: hash.clone(),
                size_bytes: artifact.size_bytes, reviewed_sha: head.clone(), receipt_id: Some("qab-peer".into()),
            };
            let receipt = QaBatchReceipt {
                schema_version: cas_store::RELEASE_VERDICT_SCHEMA, id: "qab-peer".into(),
                project_id: "portable-review".into(), ledger_artifact_id: artifact.id.clone(),
                ledger_path, ledger_sha256: hash, ledger_size_bytes: artifact.size_bytes,
                staging_sha: head.clone(), task_heads: BTreeMap::from([(member.id.clone(), head.clone())]),
                recorder_agent_id: "peer-supervisor".into(), recorded_at: "2026-11-10T00:00:00Z".into(),
                release_tag: None, ledger_author_id: Some("independent-reviewer".into()),
                ledger_author_role: Some("sub_agent".into()), author_overrides: BTreeMap::new(), imported: true,
            };
            cas_store::import_qa_batch_receipt(&root, &receipt).unwrap();
            let scope = ReleaseScope {
                project_id: "portable-review".into(), repository: "example/portable-review".into(),
                release_id: "v9.99.0".into(), reviewed_sha: head.clone(), final_release_sha: head,
                promotion_pr: None, release_tag: None, constituent_epic_ids: vec![],
                included_task_ids: vec![member.id.clone()],
                no_code_members: BTreeMap::from([(member.id.clone(), vec![proof.clone()])]),
                proof_refs: vec![proof],
            };
            let inventory = resolve_release_inventory(&root, &repo, &scope).unwrap();
            assert_eq!(inventory.deliveries.len(), 1);
            assert!(inventory.exceptions.is_empty());
            let prepared = ReleaseClose::prepare(&root, &repo, &inventory);
            if matches!(case, ProofCase::Portable) {
                let prepared = prepared.unwrap_or_else(|error| panic!("{case:?}: {error}"));
                assert_eq!(prepared.reviews.len(), 1);
                assert_eq!(prepared.reviews[0].1, body);
                assert_eq!(prepared.reviews[0].0, receipt);
                assert!(prepared.member_error(&member.id).is_none());
                assert_eq!(store.get(&artifact.id).unwrap().unwrap().sha256, artifact.sha256);
            } else {
                assert!(prepared.is_err(), "{case:?} must hold the release");
            }
            assert_eq!(tasks.get(&member.id).unwrap().status, member.status);
            assert!(cas_store::get_qa_batch_receipt(&root, &receipt.id).unwrap().unwrap().imported);
        }).await.unwrap();
    }
}
