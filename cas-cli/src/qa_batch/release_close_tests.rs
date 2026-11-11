use super::*;
use cas_types::{FactoryBranchStart, TaskStatus, WorkTarget};

/// Publication of a later tree cannot turn one missing review into a train-wide hold.
#[test]
fn published_train_closes_members_independently_cas_734b() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let repo = env.home().join("release");
    std::fs::create_dir_all(&repo).unwrap();
    let root = crate::store::init_cas_dir(&repo).unwrap();
    env.set("CAS_ROOT", &root);
    env.set("CAS_CLONE_PATH", &repo);
    std::fs::write(root.join("config.toml"), "[project]\ncanonical_id='close-fixture'\n[verification]\nenabled=true\n").unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Fixture"]);
    git(&["config", "user.email", "fixture@example.test"]);
    git(&["remote", "add", "origin", "https://github.com/example/close-fixture.git"]);
    git(&["commit", "--allow-empty", "-m", "base"]);
    let base = git(&["rev-parse", "HEAD"]);
    std::fs::write(repo.join("app.rs"), "fn shipped() {}\n").unwrap();
    git(&["add", "app.rs"]);
    git(&["commit", "-m", "delivery"]);
    let anchor = git(&["rev-parse", "HEAD"]);
    std::fs::write(repo.join("app.rs"), "fn shipped() {}\nfn later() {}\n").unwrap();
    git(&["commit", "-am", "release changes after review"]);
    let published = git(&["rev-parse", "HEAD"]);
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    git(&["tag", &tag]);
    let tasks = crate::store::open_task_store(&root).unwrap();
    assert!(crate::config::Config::load(&root).unwrap().verification_enabled());
    let mut train = Task::new("cas-train".into(), "Published train".into());
    train.origin_project = Some("close-fixture".into());
    train.task_type = cas_types::TaskType::Epic;
    train.status = TaskStatus::Closed;
    tasks.add(&train).unwrap();
    for id in ["cas-approved", "cas-unreviewed", "cas-report", "cas-closed", "cas-equal"] {
        let mut task = Task::new(id.into(), id.into());
        task.origin_project = Some("close-fixture".into());
        task.status = if id == "cas-closed" { TaskStatus::Closed } else { TaskStatus::AwaitingMerge };
        if id == "cas-report" {
            task.execution_note = Some("no-code".into());
        } else {
            task.deliverables.factory_branch_anchor = Some(anchor.clone());
            task.deliverables.factory_branch_start = Some(FactoryBranchStart {
                work_target: WorkTarget { repo_selector: "project:close-fixture".into(), target_branch: "main".into() },
                branch: format!("factory/{id}"), assignee: "worker".into(),
                base_sha: if id == "cas-equal" { anchor.clone() } else { base.clone() },
            });
        }
        if id == "cas-closed" { task.deliverables.factory_branch_start = None; }
        tasks.add(&task).unwrap();
        tasks.add_dependency(&cas_types::Dependency::new(id.into(), train.id.clone(), cas_types::DependencyType::ParentChild)).unwrap();
    }
    let artifact = cas_store::PublishedArtifact {
        id: "art-report".into(), task_id: "cas-report".into(), name: "report.md".into(),
        mime: "text/markdown".into(), size_bytes: 10, sha256: "a".repeat(64),
        cloud_artifact_id: Some("cloud-report".into()), status: "committed".into(),
        cloud_url: None, slack_permalink: None, created_at: "2026-11-11T00:00:00Z".into(), revocation: None,
    };
    tasks.append_trusted_note("cas-report", &format!("ARTIFACT_PROOF: {}", serde_json::to_string(&artifact).unwrap())).unwrap();
    let proof = ReleaseProofRef { kind: ReleaseProofKind::Report, artifact_id: artifact.id,
        sha256: artifact.sha256, size_bytes: 10, reviewed_sha: anchor.clone(), receipt_id: None };
    let receipt = QaBatchReceipt {
        schema_version: cas_store::RELEASE_VERDICT_SCHEMA, id: "qab-review".into(),
        project_id: "close-fixture".into(), ledger_artifact_id: "art-recorded".into(),
        ledger_path: "/peer/recorded-review.md".into(), ledger_sha256: "b".repeat(64), ledger_size_bytes: 20,
        staging_sha: anchor.clone(), task_heads: ["cas-report"].into_iter().map(|id| (id.into(), anchor.clone())).collect(),
        recorder_agent_id: "supervisor".into(), recorded_at: "2026-11-11T00:00:00Z".into(),
        release_tag: None, ledger_author_id: Some("reviewer".into()), ledger_author_role: Some("sub_agent".into()),
        author_overrides: BTreeMap::new(), imported: true,
    };
    cas_store::import_qa_batch_receipt(&root, &receipt).unwrap();
    let boundary = cas_types::VerificationProofBoundary::task_at(cas_types::RepositoryProofBoundary {
        peer_premerge: None, repository_root: repo.display().to_string(), worktree_root: repo.display().to_string(),
        head_commit: anchor.clone(), state_digest: String::new(), target_branch: None, anchor_commits: vec![anchor.clone()],
    });
    let dispatch = cas_store::create_verification_dispatch_bound(&root, "cas-approved", "worker", "supervisor", &boundary,
        chrono::Utc::now() + chrono::Duration::hours(1), false).unwrap();
    // Seed the durable verdict exactly as the verified recorder leaves it.
    let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
    conn.execute("UPDATE verification_dispatches SET state='resolved' WHERE id=?", [&dispatch.id]).unwrap();
    conn.execute("INSERT INTO verifications (id,task_id,agent_id,verification_type,status,summary,files_reviewed,created_at,dispatch_id) VALUES ('ver-approved','cas-approved','reviewer','task','approved','pass','[]',?1,?2)", rusqlite::params![chrono::Utc::now().to_rfc3339(), dispatch.id]).unwrap();
    let scope = ReleaseScope { project_id: "close-fixture".into(), repository: "example/close-fixture".into(),
        release_id: tag.clone(), reviewed_sha: base, final_release_sha: published,
        promotion_pr: None, release_tag: Some(tag.clone()), constituent_epic_ids: vec![train.id.clone()],
        included_task_ids: ["cas-approved", "cas-unreviewed", "cas-report", "cas-closed", "cas-equal"].map(str::to_string).to_vec(),
        no_code_members: BTreeMap::from([("cas-report".into(), vec![proof])]), proof_refs: vec![], };
    // A preview must leave the store, artifact cache and operation directory
    // byte-for-byte unchanged. SQLite's transient reader locks live in -shm.
    fn snapshot(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        if !path.exists() { return files; }
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { files.extend(snapshot(&path)); }
            else if !path.to_string_lossy().ends_with("-shm") { files.insert(path.clone(), std::fs::read(path).unwrap()); }
        }
        files
    }
    let revision = |id: &str| conn.query_row("SELECT revision FROM task_mutation_revisions WHERE entity_id=?", [id], |row| row.get::<_, i64>(0)).unwrap();
    let revisions: BTreeMap<_, _> = scope.included_task_ids.iter().map(|id| (id.clone(), revision(id))).collect();
    let before = snapshot(&root);
    let cache = env.home().join(".cas/artifacts");
    let cache_before = snapshot(&cache);
    let inventory = resolve_release_inventory(&root, &repo, &scope).unwrap();
    let preview = crate::release_reconcile::preview_release(&root, &repo, &inventory).unwrap();
    assert_eq!(snapshot(&root), before);
    assert_eq!(snapshot(&cache), cache_before);
    use crate::mcp::tools::core::task::lifecycle::merge_close::CassyMergeClose;
    // Stale generations and unbound report approval hold only their member.
    let mut stale = inventory.clone();
    stale.deliveries.iter_mut().find(|d| d.task_id == "cas-approved").unwrap().generation = "stale".into();
    let outcomes = crate::release_reconcile::preview_release(&root, &repo, &stale).unwrap();
    assert!(matches!(outcomes.get("cas-approved"), Some(CassyMergeClose::Held(reason)) if reason == "RELEASE_GENERATION_CHANGED"));
    assert!(matches!(outcomes.get("cas-equal"), Some(CassyMergeClose::Closed { .. })));
    let mut unbound = inventory.clone();
    unbound.deliveries.iter_mut().find(|d| d.task_id == "cas-report").unwrap().proof_refs[0].size_bytes += 1;
    let outcomes = crate::release_reconcile::preview_release(&root, &repo, &unbound).unwrap();
    assert!(matches!(outcomes.get("cas-report"), Some(CassyMergeClose::Held(reason)) if reason.split(':').next() == Some("RELEASE_NO_CODE_PROOF_BINDING")));
    assert_eq!(snapshot(&root), before);
    assert_eq!(snapshot(&cache), cache_before);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let result = runtime.block_on(crate::release_reconcile::reconcile_release(&root, &scope, &inventory,
        crate::release_reconcile::ReconcileTrigger::Publish { tag })).unwrap();
    assert_eq!(serde_json::to_value(&preview).unwrap(), serde_json::to_value(&result.members).unwrap());
    for id in ["cas-approved", "cas-report", "cas-equal"] {
        assert!(matches!(result.members.get(id), Some(CassyMergeClose::Closed { .. })), "{id}: {:?}", result.members.get(id));
        assert_eq!(tasks.get(id).unwrap().status, TaskStatus::Closed);
    }
    assert!(matches!(result.members.get("cas-closed"), Some(CassyMergeClose::AlreadyClosed)));
    assert!(matches!(result.members.get("cas-unreviewed"), Some(CassyMergeClose::Held(reason)) if reason == "REVIEW_MISSING"));
    assert_eq!(result.residuals.len(), 1);
    assert_eq!(result.residuals[0].task_id, "cas-unreviewed");
    for id in &scope.included_task_ids {
        let closed_now = matches!(result.members.get(id), Some(CassyMergeClose::Closed { .. }));
        assert_eq!(revision(id) - revisions[id], i64::from(closed_now));
    }
    // Adding only the missing exact-head approval completes the same operation;
    // an already-closed member's historical exception cannot hold the train.
    let mut later = receipt;
    later.id = "qab-later".into();
    later.task_heads = BTreeMap::from([("cas-unreviewed".into(), anchor)]);
    cas_store::import_qa_batch_receipt(&root, &later).unwrap();
    let inventory = resolve_release_inventory(&root, &repo, &scope).unwrap();
    let complete = runtime.block_on(crate::release_reconcile::reconcile_release(&root, &scope, &inventory,
        crate::release_reconcile::ReconcileTrigger::Backfill)).unwrap();
    assert!(complete.complete);
    assert!(complete.residuals.is_empty());
    assert!(!complete.exceptions.is_empty());
}
