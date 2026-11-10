// One-off proof harness authorized in supervisor message #4235331.
// Remove this module in a separate commit before final delivery tests.
#[tokio::test]
async fn optin_real_cas6354_release_replay() {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use crate::mcp::tools::core::task::lifecycle::close_ops::delivery_content_presence_on_target_for_task;
    let fixture_path = Path::new("/mnt/rewind/cas/scratch/cas-9c84f-replay/fixture.json");
    let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(fixture_path).unwrap()).unwrap();
    let repo = PathBuf::from(fixture["repo"].as_str().unwrap()).canonicalize().unwrap();
    assert!(repo.starts_with("/mnt/rewind/cas/scratch/cas-9c84f-replay"));
    let root = repo.join(".cas");
    assert!(root.join("cas.db").is_file());
    let anchor = "0e3f3d91e23980372afa3873f924e7f473c23fa4";
    let reviewed = "1cdfc26680cbb835190c367691c1b85623590fd7";
    let release = "da583fe07387d68ea64ad0ee2705d98405703a35";
    assert_eq!(commit(&repo, "HEAD").as_deref(), Some(release));
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_ROOT", &root);
    env.set("CAS_CLONE_PATH", &repo);
    // All task/agent writes below target only the supervisor's readonly backup.
    let tasks = crate::store::open_task_store(&root).unwrap();
    let mut task = tasks.get("cas-6354").unwrap();
    let historical_task = serde_json::to_value(&task).unwrap();
    assert_eq!(task.deliverables.factory_branch_anchor.as_deref(), Some(anchor));
    task.status = cas_types::TaskStatus::AwaitingMerge;
    task.closed_at = None; task.close_reason = None; task.terminal_outcome = None;
    task.pending_verification = false; task.pending_worktree_merge = false;
    // Replay before the hand-reviewed override. Preserve original park intent,
    // source/start identity and existing independent/test receipt evidence.
    task.notes = task.notes.lines().filter(|line|
        !line.contains("reviewed-drop:") && !line.contains("reviewed delivery content supersession accepted")
    ).collect::<Vec<_>>().join("\n");
    task.notes.push_str("\nBUILD PROOF deferred to fixture assembly; exact original worker tests retained in copied store.\n");
    let original_deliverables = task.deliverables.clone();
    tasks.update(&task).unwrap();
    // Reopening a terminal row correctly retires its park. This fixture rolls
    // back the historic close, so restore its original pre-close park snapshot
    // in a second fixture-only update; production retirement rules are retained.
    let mut task = tasks.get("cas-6354").unwrap();
    task.deliverables = original_deliverables;
    tasks.update(&task).unwrap();
    let task = tasks.get("cas-6354").unwrap();
    let agents = crate::store::open_agent_store(&root).unwrap();
    let recorder = agents.list(None).unwrap().into_iter().find(|a| a.name == "cosmic-pelican-12").unwrap();
    let original = cas_store::list_qa_batch_receipts(&root, "cas-src").unwrap().into_iter()
        .find(|r| r.id == "qab-99da5a451b02491d9e0c063e1846ed8f").unwrap();
    let original_author = original.ledger_author_id.clone().unwrap();
    let blob = |rev: &str, path: &str| {
        let out = Command::new("git").args(["rev-parse", &format!("{rev}:{path}")]).current_dir(&repo).output().unwrap();
        assert!(out.status.success()); String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let manifest = release_close::ReleaseTransformations {
        project_id: "cas-src".into(), release_id: "v4.10.0".into(), reviewed_sha: reviewed.into(), final_release_sha: release.into(),
        paths: [(".claude/CODEMAP.md", "e85bbd1bf993005efba330b304ac0b6b30b8117c", "codemap"),
                ("docs/qa/journeys.yaml", "30c6e026dca73353da0bc55866f886e1e3564a45", "cas-qa-craft")]
            .into_iter().map(|(path, producer, skill)| release_close::ReleasePathTransformation {
                path: path.into(), input_blobs: vec![blob(anchor, path)], output_blob: blob(release, path),
                producing_commit: producer.into(), producer: skill.into(),
                producer_blob: blob(release, &format!("cas-cli/src/builtins/skills/{skill}/SKILL.md")),
            }).collect(),
    };
    let evidence = repo.join("replay-inputs");
    let report = evidence.join("release-transformations.md");
    let archived_review = std::fs::read_to_string(evidence.join("historical-review.md")).unwrap();
    std::fs::write(&report, format!("{archived_review}\n\n# Historical fixture transformation supplement\n\n\
        This isolated replay mechanically encodes the recorded Nov 9 release supersession. It does not create a live QA walk or a new independent human verdict.\n\n\
        ```cassy-release-transformations\n{}\n```\n", serde_json::to_string_pretty(&manifest).unwrap())).unwrap();
    let artifacts = cas_store::SqliteArtifactStore::open(&root).unwrap();
    let publish = crate::artifacts::PublishContext {
        store: &artifacts, roots: crate::artifacts::paths::PublishRoots::new(&root, &evidence, &task.id),
        task_id: task.id.clone(), cloud: None,
    };
    let artifact = crate::artifacts::publish(&publish, &report).unwrap().artifact;
    // Fixture-only projection of archived independent authority onto the new
    // typed supplement; keep the provenance and original receipt in outcome.
    let mut receipt = original.clone();
    receipt.id = "qab-replay-cas6354-transformations".into();
    receipt.ledger_artifact_id = artifact.id.clone(); receipt.ledger_path = report.to_string_lossy().into_owned();
    receipt.ledger_sha256 = artifact.sha256.clone(); receipt.ledger_size_bytes = artifact.size_bytes;
    receipt.task_heads.retain(|id, _| id == "cas-6354"); receipt.imported = false;
    cas_store::record_qa_batch_receipt(&root, &receipt).unwrap();
    let scope = ReleaseScope {
        project_id: "cas-src".into(), repository: "Richards-LLC/cassy".into(), release_id: "v4.10.0".into(),
        reviewed_sha: reviewed.into(), final_release_sha: release.into(), promotion_pr: None, release_tag: Some("v4.10.0".into()),
        constituent_epic_ids: Vec::new(), included_task_ids: vec![task.id.clone()], no_code_members: BTreeMap::new(),
        proof_refs: vec![ReleaseProofRef { kind: ReleaseProofKind::Review, artifact_id: artifact.id,
            sha256: artifact.sha256, size_bytes: artifact.size_bytes, reviewed_sha: reviewed.into(), receipt_id: Some(receipt.id.clone()) }],
    };
    let mut inventory = resolve_release_inventory(&root, &repo, &scope).unwrap();
    // Limit lifecycle mutation to the one explicitly named incident delivery;
    // all other copied rows remain read-only fixture context.
    inventory.deliveries.retain(|d| d.task_id == "cas-6354");
    inventory.exceptions.retain(|e| e.task_id == "cas-6354");
    let core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
    core.set_agent_id_for_testing(recorder.id.clone());
    let before_content = delivery_content_presence_on_target_for_task(&repo, anchor, release, "cas-6354");
    let outcomes = core.close_release_inventory_outcomes(&inventory).await;
    let rows: BTreeMap<_,_> = outcomes.iter().map(|(id, outcome)| (id.clone(), outcome.receipt(id))).collect();
    let after = tasks.get("cas-6354").unwrap();
    let harness_sha = fixture["harness_sha"].as_str().unwrap();
    let result = serde_json::json!({
        "harness_sha": harness_sha, "fixture_path": fixture_path, "repo": repo,
        "source": {"anchor": anchor, "reviewed": reviewed, "release": release,
            "original_receipt": original, "original_author": original_author, "historical_task": historical_task},
        "fixture_only_supplement": manifest, "inventory": inventory,
        "unbound_content": format!("{before_content:?}"), "outcomes": rows,
        "status": after.status, "closed_without_override": after.status == cas_types::TaskStatus::Closed,
        "safety": "Only isolated cloned repository and readonly store backup; no live writes, no network publication, no new actual QA/review claimed."
    });
    std::fs::write(repo.parent().unwrap().join("outcome.json"), serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    assert_eq!(after.status, cas_types::TaskStatus::Closed, "{}", result["outcomes"]);
}
