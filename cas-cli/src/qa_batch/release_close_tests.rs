use super::*;
use cas_types::{FactoryBranchStart, TaskStatus, WorkTarget};

#[test]
fn deliberate_review_exemptions_require_green_lane_ci_cas_5a22() {
    use crate::mcp::tools::core::task::lifecycle::merge_close::CassyMergeClose;
    use cas_types::{MergeCiAdmission, TaskDepth};
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let repo = env.home().join("release");
    std::fs::create_dir_all(&repo).unwrap();
    let root = crate::store::init_cas_dir(&repo).unwrap();
    env.set("CAS_ROOT", &root);
    env.set("CAS_CLONE_PATH", &repo);
    std::fs::write(root.join("config.toml"), "[project]\ncanonical_id='exempt'\n").unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "Fixture"]);
    git(&["config", "user.email", "fixture@example.test"]);
    git(&["remote", "add", "origin", "https://github.com/example/exempt.git"]);
    std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
    git(&["add", ".gitignore"]);
    git(&["commit", "-qm", "base"]);
    let base = git(&["rev-parse", "HEAD"]);
    std::fs::write(repo.join("app.rs"), "fn shipped() {}\n").unwrap();
    git(&["add", "app.rs"]);
    git(&["commit", "-qm", "code"]);
    let code = git(&["rev-parse", "HEAD"]);
    std::fs::create_dir(repo.join("docs")).unwrap();
    std::fs::write(repo.join("docs/guide.md"), "Guide\n").unwrap();
    git(&["add", "docs/guide.md"]);
    git(&["commit", "-qm", "docs"]);
    let published = git(&["rev-parse", "HEAD"]);
    let tasks = crate::store::open_task_store(&root).unwrap();
    let mut train = Task::new("cas-exempt-train".into(), "train".into());
    train.task_type = cas_types::TaskType::Epic;
    tasks.add(&train).unwrap();
    let now = chrono::Utc::now();
    let window = crate::rapid_response::enable(&root, tasks.as_ref(), &train.id,
        chrono::Duration::hours(1), "incident", "revert", now).unwrap();
    let mut expected = BTreeMap::new();
    for (kind, admission, gate) in [
        ("mechanical", Some(MergeCiAdmission::Green), None),
        ("light", Some(MergeCiAdmission::Green), None),
        ("rapid", Some(MergeCiAdmission::Green), None),
        ("mechanical", None, Some("LANE_CI_MISSING")),
        ("light", Some(MergeCiAdmission::Advisory), Some("LANE_CI_MISSING")),
        ("rapid", Some(MergeCiAdmission::Override), Some("LANE_CI_MISSING")),
        ("rapid-reopened", Some(MergeCiAdmission::Green), Some("REVIEW_MISSING")),
        ("required", Some(MergeCiAdmission::Green), Some("REVIEW_MISSING")),
        ("forced", Some(MergeCiAdmission::Green), Some("REVIEW_MISSING")),
        ("light-pending", Some(MergeCiAdmission::Green), Some("REVIEW_MISSING")),
    ] {
        let id = (0..).map(|n| format!("cas-{kind}-{n}"))
            .find(|id| !expected.contains_key(id) && (!kind.starts_with("rapid") || !crate::rapid_response::verification_sampled(id))).unwrap();
        let mut task = Task::new(id.clone(), id.clone());
        task.origin_project = Some("exempt".into());
        task.status = TaskStatus::AwaitingMerge;
        if kind.starts_with("light") { task.depth = TaskDepth::Light; }
        if kind == "forced" { task.labels.push("force-verifier".into()); }
        task.deliverables.factory_branch_anchor = Some(if kind == "mechanical" || kind == "forced" { published.clone() } else { code.clone() });
        task.deliverables.factory_branch_start = Some(FactoryBranchStart {
            work_target: WorkTarget { repo_selector: "project:exempt".into(), target_branch: "main".into() },
            branch: format!("factory/{id}"), assignee: "worker".into(),
            base_sha: if kind == "mechanical" || kind == "forced" { code.clone() } else { base.clone() },
        });
        tasks.add(&task).unwrap();
        tasks.add_dependency(&cas_types::Dependency::new(id.clone(), train.id.clone(), cas_types::DependencyType::ParentChild)).unwrap();
        if let Some(admission) = admission { tasks.record_merge_admission(&id, &published, admission).unwrap(); }
        if kind.starts_with("rapid") { assert!(crate::rapid_response::record_skip(&root, &window, &id, "verification", "not sampled")); }
        if kind == "rapid-reopened" {
            for status in [TaskStatus::Closed, TaskStatus::Open, TaskStatus::AwaitingMerge] {
                let mut current = tasks.get(&id).unwrap();
                current.status = status;
                current.deliverables.factory_branch_anchor = task.deliverables.factory_branch_anchor.clone();
                tasks.update(&current).unwrap();
            }
            tasks.record_merge_admission(&id, &published, MergeCiAdmission::Green).unwrap();
        }
        if kind == "light-pending" {
            cas_store::create_verification_dispatch(&root, &id, "worker", "supervisor", now + chrono::Duration::hours(1)).unwrap();
        }
        expected.insert(id, gate);
    }
    crate::rapid_response::disable(&root, tasks.as_ref(), &train.id, now + chrono::Duration::minutes(1)).unwrap();
    let scope = ReleaseScope {
        project_id: "exempt".into(), repository: "example/exempt".into(), release_id: "v9.99.0".into(),
        reviewed_sha: code, final_release_sha: published, promotion_pr: None, release_tag: None,
        constituent_epic_ids: vec![], included_task_ids: expected.keys().cloned().collect(),
        no_code_members: BTreeMap::new(), proof_refs: vec![],
    };
    let inventory = resolve_release_inventory(&root, &repo, &scope).unwrap();
    let outcomes = crate::release_reconcile::preview_release(&root, &repo, &inventory).unwrap();
    for (id, gate) in expected {
        match gate {
            None => assert!(matches!(outcomes.get(&id), Some(CassyMergeClose::Closed { .. })), "{id}: {:?}", outcomes.get(&id)),
            Some(code) => assert!(matches!(outcomes.get(&id), Some(CassyMergeClose::Held(reason)) if reason == code), "{id}: {:?}", outcomes.get(&id)),
        }
    }
}

#[test]
fn unregistered_reconcile_queues_parent_review_cas_d47d() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    for key in ["CAS_SESSION_ID", "CAS_AGENT_NAME", "CAS_AGENT_ID"] {
        env.remove(key);
    }
    let repo = env.home().join("release");
    std::fs::create_dir_all(&repo).unwrap();
    let root = crate::store::init_cas_dir(&repo).unwrap();
    env.set("CAS_ROOT", &root);
    env.set("CAS_CLONE_PATH", &repo);
    std::fs::write(root.join("config.toml"), "[project]\ncanonical_id='parent-fixture'\n[verification]\nenabled=true\n").unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Fixture"]);
    git(&["config", "user.email", "fixture@example.test"]);
    git(&["remote", "add", "origin", "https://github.com/example/parent-fixture.git"]);
    std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
    git(&["add", ".gitignore"]);
    git(&["commit", "-m", "published release"]);
    let published = git(&["rev-parse", "HEAD"]);
    let tasks = crate::store::open_task_store(&root).unwrap();
    let agents = crate::store::open_agent_store(&root).unwrap();
    let reviewer = cas_types::Agent::new_with_role(
        "parent-reviewer".into(), "parent-reviewer".into(), cas_types::AgentRole::Supervisor,
    );
    agents.register(&reviewer).unwrap();
    let mut parent = Task::new("cas-parent".into(), "Published parent".into());
    parent.task_type = cas_types::TaskType::Epic;
    parent.origin_project = Some("parent-fixture".into());
    parent.epic_verification_owner = Some(reviewer.id.clone());
    tasks.add(&parent).unwrap();
    let mut child = Task::new("cas-shipped".into(), "Closed shipped member".into());
    child.origin_project = parent.origin_project.clone();
    child.status = TaskStatus::Closed;
    child.deliverables.factory_branch_anchor = Some(published.clone());
    child.deliverables.green_merge_commit = Some(published.clone());
    child.deliverables.factory_branch_start = Some(FactoryBranchStart {
        work_target: WorkTarget { repo_selector: "project:parent-fixture".into(), target_branch: "main".into() },
        branch: "factory/worker".into(), assignee: "worker".into(), base_sha: published.clone(),
    });
    tasks.add(&child).unwrap();
    tasks.add_dependency(&cas_types::Dependency::new(
        child.id.clone(), parent.id.clone(), cas_types::DependencyType::ParentChild,
    )).unwrap();
    let scope = ReleaseScope {
        project_id: "parent-fixture".into(), repository: "example/parent-fixture".into(),
        release_id: format!("v{}", env!("CARGO_PKG_VERSION")),
        reviewed_sha: published.clone(), final_release_sha: published,
        promotion_pr: None, release_tag: None, constituent_epic_ids: vec![parent.id.clone()],
        included_task_ids: vec![child.id.clone()], no_code_members: BTreeMap::new(), proof_refs: vec![],
    };
    let core = crate::mcp::CasCore::for_cli(root.clone());
    assert!(core.get_agent_id().is_err());
    let inventory = resolve_release_inventory(&root, &repo, &scope).unwrap();
    assert!(inventory.exceptions.is_empty(), "{:?}", inventory.exceptions);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let result = runtime.block_on(crate::release_reconcile::reconcile_release(
        &root, &scope, &inventory, crate::release_reconcile::ReconcileTrigger::Backfill,
    )).unwrap();
    let Some(crate::release_reconcile::ParentOutcome::Pending { dispatch_id, owner }) = result.parents.get(&parent.id) else {
        panic!("{:?}", result.parents);
    };
    assert_eq!(owner, &reviewer.id);
    let dispatch = cas_store::get_latest_verification_dispatch(&root, &parent.id).unwrap().unwrap();
    assert_eq!(&dispatch.id, dispatch_id);
    let key = cas_store::release_reconcile_store::ReleaseOperationKey {
        project_id: scope.project_id.clone(), release_id: scope.release_id.clone(),
        final_release_sha: scope.final_release_sha.clone(),
    };
    assert_eq!(dispatch.requester_agent_id, format!("release:{}", key.id().unwrap()));
    assert_eq!(dispatch.state, cas_types::VerificationDispatchState::Pending);
    assert!(cas_store::get_verification_for_dispatch(&root, dispatch_id).unwrap().is_none());
    assert_eq!(tasks.get(&parent.id).unwrap().status, TaskStatus::Open);
    assert_eq!(tasks.get(&child.id).unwrap().status, TaskStatus::Closed);
    let replay = runtime.block_on(crate::release_reconcile::reconcile_release(
        &root, &scope, &inventory, crate::release_reconcile::ReconcileTrigger::Backfill,
    )).unwrap();
    assert_eq!(result.parents, replay.parents);
}

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
    for id in ["cas-approved", "cas-candidate", "cas-unshipped", "cas-other-source", "cas-unreviewed", "cas-report", "cas-closed", "cas-equal"] {
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
        if id == "cas-equal" { task.deliverables.green_merge_commit = Some(published.clone()); }
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
        head_commit: published.clone(), state_digest: String::new(), target_branch: None, anchor_commits: vec![anchor.clone()],
    });
    let dispatch = cas_store::create_verification_dispatch_bound(&root, "cas-approved", "worker", "supervisor", &boundary,
        chrono::Utc::now() + chrono::Duration::hours(1), false).unwrap();
    let mut bound = boundary.repository.clone().unwrap();
    // The peer recorder binds the source anchor but records the landed head.
    bound.peer_premerge = Some(cas_types::PeerPremergeAttestation {
        message_id: "peer-message".into(), sender_machine_id: "peer".into(), sender_session: "peer-session".into(),
        receipt: cas_types::PeerPremergeReceipt {
            registry_version: None,
            status: cas_types::VerificationStatus::Approved, project_id: "close-fixture".into(),
            supervisor_id: "supervisor".into(), task_id: "cas-approved".into(),
            source_branch: "factory/cas-approved".into(), source_sha: anchor.clone(), target_branch: "main".into(),
            target_sha: base.clone(), candidate_tree: git(&["rev-parse", "HEAD^{tree}"]), landed_sha: published.clone(),
            dispatch_id: dispatch.id.clone(), verdict_id: "ver-approved".into(), plan_digest: "fixture-plan".into(),
        },
    });
    // Seed the durable verdict exactly as the verified recorder leaves it.
    let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
    conn.execute("UPDATE verification_dispatches SET state='resolved', repository_proof=?1 WHERE id=?2",
        rusqlite::params![serde_json::to_string(&bound).unwrap(), dispatch.id]).unwrap();
    conn.execute("INSERT INTO verifications (id,task_id,agent_id,verification_type,status,summary,files_reviewed,created_at,dispatch_id,provenance,issuer_agent_id) VALUES ('ver-approved','cas-approved','reviewer','task','approved','pass','[]',?1,?2,'peer_attested','supervisor')", rusqlite::params![chrono::Utc::now().to_rfc3339(), dispatch.id]).unwrap();
    let cycle = cas_store::delivery_cycles(&root, "cas-approved").unwrap().pop().unwrap();
    conn.execute("UPDATE verification_dispatches SET receipt_id=?1 WHERE id=?2",
        rusqlite::params![cycle.cycle_id, dispatch.id]).unwrap();
    let stamp = serde_json::json!({"cycle_id": cycle.cycle_id, "approved_anchor": anchor,
        "registry_version": crate::gates::registry_version_in_force(),
        "dispatch_id": dispatch.id, "verdict_id": "ver-approved"});
    conn.execute("INSERT INTO events(event_type,entity_type,entity_id,summary,metadata) VALUES ('verification_added','verification','ver-approved','approval',?1)",
        [stamp.to_string()]).unwrap();
    // A newer task dispatch must not hide an approval of this delivery cycle.
    cas_store::create_verification_dispatch_bound(&root, "cas-approved", "worker", "supervisor",
        &cas_types::VerificationProofBoundary::task(), chrono::Utc::now() + chrono::Duration::hours(1), false).unwrap();
    // cas-0d3f: a local premerge verdict binds a synthetic commit, with the
    // delivery's source and reviewed tree in its immutable candidate manifest.
    let tree = git(&["rev-parse", &format!("{anchor}^{{tree}}")]);
    std::fs::write(repo.join("app.rs"), "fn never_shipped() {}\n").unwrap();
    git(&["add", "app.rs"]);
    let unshipped_tree = git(&["write-tree"]);
    git(&["reset", "--hard", &published]);
    for (id, source, tree) in [
        ("cas-candidate", &anchor, &tree),
        ("cas-unshipped", &anchor, &unshipped_tree),
        ("cas-other-source", &base, &tree),
    ] {
        let key = format!("{:x}", Sha256::digest(serde_json::to_vec(&(
            "premerge-v1", id, &repo, "factory/cas-candidate", source,
            "main", &base, "fixture-context",
        )).unwrap()));
        let synthetic = git(&["commit-tree", &tree, "-p", &base, "-m", &format!("CAS_PREMERGE_CANDIDATE_V1 {key}")]);
        let candidate_proof = cas_types::RepositoryProofBoundary {
            peer_premerge: None, repository_root: repo.display().to_string(), worktree_root: repo.display().to_string(),
            head_commit: synthetic, state_digest: "fixture-digest".into(), target_branch: None, anchor_commits: vec![],
        };
        let candidate = crate::mcp::tools::core::workflow::premerge_verification::Candidate {
            cas_root: root.clone(), task_id: id.into(), repository: repo.clone(),
            source_branch: "factory/cas-candidate".into(), source_sha: source.clone(),
            target_branch: "main".into(), target_sha: base.clone(), tree: tree.clone(),
            context_digest: "fixture-context".into(), proof: candidate_proof.clone(),
            preparation_ms: 0, inherited_supervisor_call: None,
        };
        let manifest = root.join("verification-candidates").join(key).join("candidate.json");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        std::fs::write(&manifest, serde_json::to_vec(&candidate).unwrap()).unwrap();
        let candidate_dispatch = cas_store::create_verification_dispatch_bound(&root, id, "worker", "supervisor",
            &cas_types::VerificationProofBoundary::task_at(candidate_proof), chrono::Utc::now() + chrono::Duration::hours(1), false).unwrap();
        conn.execute("UPDATE verification_dispatches SET state='resolved' WHERE id=?", [&candidate_dispatch.id]).unwrap();
        conn.execute("INSERT INTO verifications (id,task_id,agent_id,verification_type,status,summary,files_reviewed,created_at,dispatch_id) VALUES (?1,?2,'reviewer','task','approved','pass','[]',?3,?4)", rusqlite::params![format!("ver-{id}"), id, chrono::Utc::now().to_rfc3339(), candidate_dispatch.id]).unwrap();
    }
    // A different head with no source binding must never approve this member.
    let mut unrelated = bound.clone();
    unrelated.head_commit = base.clone();
    unrelated.anchor_commits.clear();
    unrelated.peer_premerge = None;
    let unbound_dispatch = cas_store::create_verification_dispatch_bound(&root, "cas-unreviewed", "worker", "supervisor",
        &cas_types::VerificationProofBoundary::task_at(unrelated), chrono::Utc::now() + chrono::Duration::hours(1), false).unwrap();
    conn.execute("UPDATE verification_dispatches SET state='resolved' WHERE id=?", [&unbound_dispatch.id]).unwrap();
    conn.execute("INSERT INTO verifications (id,task_id,agent_id,verification_type,status,summary,files_reviewed,created_at,dispatch_id) VALUES ('ver-unbound','cas-unreviewed','reviewer','task','approved','pass','[]',?1,?2)", rusqlite::params![chrono::Utc::now().to_rfc3339(), unbound_dispatch.id]).unwrap();
    let scope = ReleaseScope { project_id: "close-fixture".into(), repository: "example/close-fixture".into(),
        release_id: tag.clone(), reviewed_sha: base, final_release_sha: published,
        promotion_pr: None, release_tag: Some(tag.clone()), constituent_epic_ids: vec![train.id.clone()],
        included_task_ids: ["cas-approved", "cas-candidate", "cas-unshipped", "cas-other-source", "cas-unreviewed", "cas-report", "cas-closed", "cas-equal"].map(str::to_string).to_vec(),
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
    // cas-afdf: replay a consolidated store copy outside Git from the
    // original repository. Only the candidate's store location is relocated;
    // repository, proof, key, source and tree bindings remain unchanged.
    let replay = tempfile::tempdir().unwrap();
    let copied_root = replay.path().join("store");
    std::fs::create_dir_all(&copied_root).unwrap();
    for (path, bytes) in &before {
        let relative = path.strip_prefix(&root).unwrap();
        if relative == Path::new("cas.db") || path.to_string_lossy().ends_with("-wal") { continue; }
        let copy = copied_root.join(relative);
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        if relative.starts_with("verification-candidates") && path.file_name().unwrap() == "candidate.json" {
            let mut candidate: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            candidate["cas_root"] = serde_json::json!(copied_root);
            std::fs::write(copy, serde_json::to_vec(&candidate).unwrap()).unwrap();
        } else {
            std::fs::write(copy, bytes).unwrap();
        }
    }
    conn.backup(rusqlite::MAIN_DB, copied_root.join("cas.db"), None).unwrap();
    assert!(crate::worktree::GitOperations::new(replay.path().to_path_buf()).canonical_repo_key().is_err());
    env.set("CAS_ROOT", &copied_root);
    env.remove("CAS_CLONE_PATH");
    {
        let _cwd = env.enter_dir(&repo);
        let replay_inventory = resolve_release_inventory(&copied_root, &repo, &scope).unwrap();
        let replay_preview = crate::release_reconcile::preview_release(&copied_root, &repo, &replay_inventory).unwrap();
        assert_eq!(serde_json::to_value(&replay_preview).unwrap(), serde_json::to_value(&preview).unwrap());
        // Sharing an origin URL cannot substitute a different Git common dir.
        let foreign = replay.path().join("other-repository");
        git(&["clone", "--shared", "--quiet", repo.to_str().unwrap(), foreign.to_str().unwrap()]);
        let output = Command::new("git").arg("-C").arg(&foreign)
            .args(["remote", "set-url", "origin", "https://github.com/example/close-fixture.git"]).output().unwrap();
        assert!(output.status.success());
        assert!(resolve_release_inventory(&copied_root, &foreign, &scope).is_err());
    }
    env.set("CAS_ROOT", &root);
    env.set("CAS_CLONE_PATH", &repo);
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
    for id in ["cas-approved", "cas-candidate", "cas-report", "cas-equal"] {
        assert!(matches!(result.members.get(id), Some(CassyMergeClose::Closed { .. })), "{id}: {:?}", result.members.get(id));
        assert_eq!(tasks.get(id).unwrap().status, TaskStatus::Closed);
    }
    assert!(matches!(result.members.get("cas-closed"), Some(CassyMergeClose::Held(reason)) if reason == "REVIEW_MISSING"));
    assert!(matches!(result.members.get("cas-unreviewed"), Some(CassyMergeClose::Held(reason)) if reason == "REVIEW_MISSING"));
    for id in ["cas-unshipped", "cas-other-source"] {
        assert!(matches!(result.members.get(id), Some(CassyMergeClose::Held(reason)) if reason == "REVIEW_MISSING"));
    }
    assert_eq!(result.residuals.len(), 3);
    for id in &scope.included_task_ids {
        let closed_now = matches!(result.members.get(id), Some(CassyMergeClose::Closed { .. }));
        assert_eq!(revision(id) - revisions[id], i64::from(closed_now));
    }
    // A batch ledger cannot replace the missing code delivery approvals.
    let mut later = receipt;
    later.id = "qab-later".into();
    later.task_heads = ["cas-unreviewed", "cas-unshipped", "cas-other-source", "cas-closed"].map(|id| (id.into(), anchor.clone())).into();
    cas_store::import_qa_batch_receipt(&root, &later).unwrap();
    let inventory = resolve_release_inventory(&root, &repo, &scope).unwrap();
    let held = crate::release_reconcile::preview_release(&root, &repo, &inventory).unwrap();
    for id in later.task_heads.keys() {
        assert!(matches!(held.get(id), Some(CassyMergeClose::Held(reason)) if reason == "REVIEW_MISSING"));
        let dispatch = cas_store::create_verification_dispatch_bound(&root, id, "worker", "supervisor",
            &boundary, chrono::Utc::now() + chrono::Duration::hours(1), false).unwrap();
        conn.execute("UPDATE verification_dispatches SET state='resolved' WHERE id=?", [&dispatch.id]).unwrap();
        conn.execute("INSERT INTO verifications (id,task_id,agent_id,verification_type,status,summary,files_reviewed,created_at,dispatch_id) VALUES (?1,?2,'reviewer','task','approved','pass','[]',?3,?4)",
            rusqlite::params![format!("ver-repaired-{id}"), id, chrono::Utc::now().to_rfc3339(), dispatch.id]).unwrap();
    }
    // Existing merge approvals alone complete the operation; no ledger author
    // or recorder participates in code acceptance.
    let complete = runtime.block_on(crate::release_reconcile::reconcile_release(&root, &scope, &inventory,
        crate::release_reconcile::ReconcileTrigger::Backfill)).unwrap();
    assert!(complete.complete);
    assert!(complete.residuals.is_empty());
    assert!(!complete.exceptions.is_empty());
}
