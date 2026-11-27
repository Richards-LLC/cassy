use super::*;
use cas_types::{FactoryBranchStart, TaskStatus, WorkTarget};

#[test]
fn reverted_delivery_is_held_at_release_closeout_cas_aeb3() {
    use crate::mcp::tools::core::task::lifecycle::merge_close::CassyMergeClose;
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let repo = env.home().join("release");
    std::fs::create_dir_all(&repo).unwrap();
    let root = crate::store::init_cas_dir(&repo).unwrap();
    env.set("CAS_ROOT", &root);
    env.set("CAS_CLONE_PATH", &repo);
    std::fs::write(root.join("config.toml"), "[project]\ncanonical_id='reverted'\n").unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "Fixture"]);
    git(&["config", "user.email", "fixture@example.test"]);
    git(&["remote", "add", "origin", "https://github.com/example/reverted.git"]);
    std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
    std::fs::create_dir_all(repo.join("docs/qa")).unwrap();
    std::fs::create_dir_all(repo.join(".claude")).unwrap();
    std::fs::write(repo.join(".claude/CODEMAP.md"), "Baseline codemap.\n").unwrap();
    std::fs::write(repo.join("docs/PRODUCT_OVERVIEW.md"), "# Product\n\n## Core Concepts\n\nBaseline.\n").unwrap();
    std::fs::write(repo.join("docs/qa/journeys.yaml"), "version: 1\nbug_families: {}\njourneys: []\ninvariants: {}\n").unwrap();
    git(&["add", ".gitignore", "docs", ".claude/CODEMAP.md"]);
    git(&["commit", "-qm", "base"]);
    let tasks = crate::store::open_task_store(&root).unwrap();
    let mut anchors = BTreeMap::new();
    let mut foreign = None;
    let mut removals = BTreeMap::new();
    for id in ["cas-reverted", "cas-partial", "cas-ledger", "cas-compact", "cas-unfolded", "cas-retained", "cas-redrafted", "cas-resynced", "cas-foreign-cut", "cas-evolved", "cas-undone", "cas-doc-evolved", "cas-doc-reverted", "cas-doc-only", "cas-superseded", "cas-wrong-review"] {
        if id == "cas-undone" {
            std::fs::write(repo.join("shared.rs"), "fn shared() { baseline() }\n").unwrap();
            git(&["add", "shared.rs"]);
            git(&["commit", "-qm", "shared baseline"]);
        }
        let base = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-qb", id]);
        let source = format!("{id}.rs");
        if id != "cas-doc-only" {
            std::fs::write(repo.join(&source), "fn delivered() {}\n").unwrap();
            git(&["add", &source]);
            git(&["commit", "-qm", "delivery source"]);
        }
        let first = git(&["rev-parse", "HEAD"]);
        if id == "cas-undone" {
            std::fs::write(repo.join("shared.rs"), "fn shared() { delivered() }\n").unwrap();
            git(&["commit", "-qam", "deliver shared change"]);
        }
        if matches!(id, "cas-doc-evolved" | "cas-doc-reverted" | "cas-doc-only") {
            std::fs::write(repo.join("docs/PRODUCT_OVERVIEW.md"),
                "# Product\n\n## Core Concepts\n\nShipped marmoset.\n").unwrap();
            std::fs::write(repo.join(".claude/CODEMAP.md"), "Shipped marmoset codemap.\n").unwrap();
            std::fs::write(repo.join("docs/qa/journeys.yaml"),
                "version: 1\nbug_families: {}\njourneys: []\ninvariants: {}\n# Shipped marmoset\n").unwrap();
            git(&["add", "docs/PRODUCT_OVERVIEW.md", ".claude/CODEMAP.md", "docs/qa/journeys.yaml"]);
            git(&["commit", "-qm", "delivery generated docs"]);
        }
        if id == "cas-redrafted" {
            // cas-f4a2 / v4.28: 03bc's red control reverted inside its own
            // delivery, aeb3's fragment draft rewritten before release prep
            // folds it, and 343d's bundle that release prep rebuilds.
            std::fs::write(repo.join(&source), "fn delivered() { red_control() }\n").unwrap();
            git(&["commit", "-qam", "test: red control"]);
            git(&["revert", "--no-edit", "HEAD"]);
            std::fs::create_dir_all(repo.join("docs/product-overview.d")).unwrap();
            std::fs::create_dir_all(repo.join("hub-web/dist")).unwrap();
            let overview = format!("docs/product-overview.d/{id}.md");
            std::fs::write(repo.join(&overview), "## Core Concepts\n\nFirst draft.\n").unwrap();
            std::fs::write(repo.join("hub-web/dist/app.js"), "bundle(delivered)\n").unwrap();
            git(&["add", &overview, "hub-web/dist/app.js"]);
            git(&["commit", "-qm", "draft overview and bundle"]);
            std::fs::write(repo.join(&overview), format!("## Core Concepts\n\nOverview for {id}.\n")).unwrap();
            git(&["commit", "-qam", "final overview"]);
        }
        if id == "cas-foreign-cut" {
            // cas-2430 review: a commit naming another task removes this
            // delivery's source on the lane. It is not the task's own work,
            // so the loss still reads as a revert.
            git(&["rm", "-q", &source]);
            git(&["commit", "-qm", "fix(cas-9f1e): drop the helper"]);
        }
        if id == "cas-resynced" {
            // cas-2430 (the cas-aeb3 shape): the delivery lands, main reverts
            // it, and the lane's sync merge of main removes the source again.
            // The sync is not the task's own restoration: still a revert.
            git(&["checkout", "-q", "main"]);
            git(&["merge", "--no-ff", "-qm", "first landing", id]);
            git(&["revert", "-m", "1", "--no-edit", "HEAD"]);
            git(&["checkout", "-q", id]);
            git(&["merge", "--no-ff", "-qm", "sync train into delivery", "main"]);
        }
        if id == "cas-retained" {
            git(&["checkout", "-qb", "train-input", "main"]);
            std::fs::write(repo.join("foreign.rs"), "fn other_delivery() {}\n").unwrap();
            git(&["add", "foreign.rs"]);
            git(&["commit", "-qm", "another delivery"]);
            foreign = Some(git(&["rev-parse", "HEAD"]));
            git(&["checkout", "-q", "main"]);
            git(&["merge", "--ff-only", "train-input"]);
            git(&["checkout", "-q", id]);
            git(&["merge", "--no-ff", "-qm", "sync train into delivery", "main"]);
        }
        if id == "cas-retained" || id == "cas-unfolded" {
            std::fs::create_dir_all(repo.join("docs/product-overview.d")).unwrap();
            std::fs::create_dir_all(repo.join("docs/qa/bug-families")).unwrap();
            std::fs::write(repo.join(format!("docs/product-overview.d/{id}.md")),
                format!("## Core Concepts\n\nOverview for {id}.\n")).unwrap();
            std::fs::write(repo.join(format!("docs/qa/bug-families/{id}.yaml")),
                format!("title: Family for {id}\ntasks: [{id}]\n")).unwrap();
            git(&["add", "docs/product-overview.d", "docs/qa/bug-families"]);
        }
        // The delivery's final commit alone cannot describe its net effect.
        let doc = format!("{id}.md");
        if id != "cas-doc-only" {
            std::fs::write(repo.join(&doc), "Delivery guide\n").unwrap();
            git(&["add", &doc]);
            git(&["commit", "-qm", "delivery guide"]);
        }
        let anchor = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "main"]);
        if id == "cas-redrafted" {
            git(&["checkout", "-qb", "cas-assembly", &base]);
            git(&["merge", "--no-ff", "-qm", "assemble reviewed source", id]);
            let assembly = git(&["rev-parse", "HEAD"]);
            let target = WorkTarget { repo_selector: "project:reverted".into(), target_branch: "main".into() };
            let mut task = Task::new("cas-assembly".into(), "Reviewed assembly".into());
            task.origin_project = Some("reverted".into());
            task.status = TaskStatus::AwaitingMerge;
            task.labels.push(crate::mcp::tools::core::task::lifecycle::review_tier::FORCE_VERIFIER_LABEL.into());
            task.deliverables.work_target = Some(target.clone());
            task.deliverables.factory_branch_anchor = Some(assembly.clone());
            task.deliverables.factory_branch_start = Some(FactoryBranchStart {
                work_target: target.clone(), branch: "cas-assembly".into(),
                assignee: "worker".into(), base_sha: base.clone(),
            });
            task.deliverables.authorized_merge_sources.push(cas_types::AuthorizedMergeSource {
                commit: anchor.clone(), supervisor_id: "supervisor".into(), work_target: target,
            });
            task.deliverables.premerge_approval = Some(cas_types::PortablePremergeApproval {
                verdict_id: "ver-assembly".into(), dispatch_id: "vdispatch-assembly".into(),
                approved_anchor: assembly.clone(), delivery_cycle: task.deliverables.delivery_cycle,
            });
            tasks.add(&task).unwrap();
            anchors.insert(task.id, assembly);
            git(&["checkout", "-q", "main"]);
            git(&["merge", "--no-ff", "-qm", "land assembly", "cas-assembly"]);
        } else {
            git(&["merge", "--no-ff", "-qm", "merge delivery", id]);
        }
        let merge = git(&["rev-parse", "HEAD"]);
        if matches!(id, "cas-doc-evolved" | "cas-doc-reverted" | "cas-doc-only") {
            // v4.28 cas-d725: an unrelated revert touched the generated
            // overview before release prep rewrote the delivered prose.
            let overview = repo.join("docs/PRODUCT_OVERVIEW.md");
            std::fs::write(&overview,
                "# Product\n\n## Core Concepts\n\nShipped marmoset.\n\nSibling experiment.\n").unwrap();
            git(&["commit", "-qam", "unrelated sibling experiment"]);
            git(&["revert", "--no-edit", "HEAD"]);
            let sibling = format!("{id}-sibling");
            git(&["checkout", "-qb", &sibling, &base]);
            std::fs::write(&overview,
                "# Product\n\n## Core Concepts\n\nSibling ostrich.\n").unwrap();
            git(&["commit", "-qam", "sibling overview"]);
            let conflict = Command::new("git").arg("-C").arg(&repo)
                .args(["merge", "-q", "main"]).output().unwrap();
            assert!(!conflict.status.success());
            std::fs::write(&overview,
                "# Product\n\n## Core Concepts\n\nRegenerated zeppelin.\n").unwrap();
            git(&["add", "docs/PRODUCT_OVERVIEW.md"]);
            git(&["commit", "-qm", "sync train into docs sibling"]);
            git(&["checkout", "-q", "main"]);
            git(&["merge", "--no-ff", "-qm", "merge docs sibling", &sibling]);
            std::fs::write(repo.join(".claude/CODEMAP.md"), "Regenerated zeppelin codemap.\n").unwrap();
            std::fs::write(repo.join("docs/qa/journeys.yaml"),
                "version: 1\nbug_families: {}\njourneys: []\ninvariants: {}\n# Regenerated zeppelin\n").unwrap();
            git(&["commit", "-qam", "regenerate release docs"]);
            if id == "cas-doc-reverted" {
                // cas-895d3: generated docs cannot hide a source revert.
                git(&["revert", "--no-edit", &first]);
            }
        } else if id == "cas-reverted" {
            git(&["revert", "-m", "1", "--no-edit", &merge]);
        } else if id == "cas-partial" {
            git(&["revert", "--no-edit", &first]);
        } else if id == "cas-evolved" {
            // GH #2026: a train sibling's merge resolution rewrites the
            // delivered line after it landed. That is evolution, not a revert.
            git(&["checkout", "-qb", "sibling", &base]);
            std::fs::write(repo.join(&source), "fn delivered() { sibling() }\n").unwrap();
            git(&["add", &source]);
            git(&["commit", "-qm", "sibling delivery"]);
            let _conflict = Command::new("git").arg("-C").arg(&repo).args(["merge", "-q", "main"]).output().unwrap();
            std::fs::write(repo.join(&source), "fn delivered() { evolved() }\n").unwrap();
            git(&["add", &source]);
            git(&["commit", "-qm", "sync train into sibling"]);
            git(&["checkout", "-q", "main"]);
            git(&["merge", "--no-ff", "-qm", "merge sibling", "sibling"]);
        } else if id == "cas-undone" {
            // An unlabeled edit restoring the pre-delivery line is a revert.
            std::fs::write(repo.join("shared.rs"), "fn shared() { baseline() }\n").unwrap();
            git(&["commit", "-qam", "tidy shared"]);
        } else if id == "cas-superseded" || id == "cas-wrong-review" {
            // cas-0489 (v4.28 cas-43ab): a later task's own commit deletes the
            // delivered file outright. That is a drop; only a reviewed-drop
            // naming a commit that really replaced it can accept it.
            git(&["rm", "-q", &source]);
            git(&["commit", "-qm", "fix(cas-9f9a): replace the helper"]);
            removals.insert(id.to_string(), git(&["rev-parse", "HEAD"]));
        }
        let mut task = Task::new(id.into(), id.into());
        task.origin_project = Some("reverted".into());
        task.status = TaskStatus::AwaitingMerge;
        task.depth = cas_types::TaskDepth::Light;
        task.deliverables.factory_branch_anchor = Some(anchor.clone());
        task.deliverables.factory_branch_start = Some(FactoryBranchStart {
            work_target: WorkTarget { repo_selector: "project:reverted".into(), target_branch: "main".into() },
            branch: id.into(), assignee: "worker".into(), base_sha: base,
        });
        if id == "cas-redrafted" {
            task.status = TaskStatus::Closed;
            task.deliverables.premerge_approval = Some(cas_types::PortablePremergeApproval {
                verdict_id: "ver-source".into(), dispatch_id: "vdispatch-source".into(),
                approved_anchor: anchor.clone(), delivery_cycle: task.deliverables.delivery_cycle,
            });
        }
        tasks.add(&task).unwrap();
        tasks.record_merge_admission(id, &merge, cas_types::MergeCiAdmission::Green).unwrap();
        anchors.insert(id.to_string(), anchor);
    }
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    git(&["revert", "--no-edit", foreign.as_deref().unwrap()]);
    // One real release compile consumes the retained member's fragments.
    // A deletion in the same release without folded bytes stays a loss.
    git(&["rm", "docs/product-overview.d/cas-unfolded.md", "docs/qa/bug-families/cas-unfolded.yaml"]);
    std::fs::write(repo.join("hub-web/dist/app.js"), "bundle(release rebuild)\n").unwrap();
    let compile = Command::new("python3")
        .args(["-c", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/doc-fragments.py"))])
        .args(["compile", "--root"]).arg(&repo).output().unwrap();
    assert!(compile.status.success(), "{compile:?}");
    git(&["add", "-A"]);
    git(&["commit", "-qm", &format!("release: {tag}")]);
    let published = git(&["rev-parse", "HEAD"]);
    git(&["tag", &tag]);
    let ledger = repo.join("release-review.md");
    let body = format!("{}{}{}", concat!(
        "| task | verdict |\n| --- | --- |\n",
        "| cas-compact | NOT A ROUTE: reverted, not shipped |\n",
        "| cas-retained-extra | NOT A ROUTE: reverted |\n",
        "| cas-retained | NOT A ROUTE: documentation-only |\n",
        "```example\n| cas-retained | NOT A ROUTE: reverted |\n```\n\n",
        "| id | cell | expected | observed | verdict | label | evidence path | defect task |\n",
        "| --- | --- | --- | --- | --- | --- | --- | --- |\n",
        "| R09 | cas-ledger live working indicator | Reverted on this train and absent from the release | terminal: Reverted, not shipped | NOT A ROUTE | real-build | review/revert.txt | - |\n",
    ),
        // cas-0489: a supervisor-reviewed drop names the commit that replaced the
        // delivery; the same words naming a commit that never touched the file
        // for another member do not release it.
        format!("| R10 | cas-superseded replaced by a later delivery | reviewed-drop: {} -- the helper moved into the shared module | terminal: superseded, shipped elsewhere | NOT A ROUTE | real-build | review/drop.txt | - |\n", removals["cas-superseded"]),
        format!("| R11 | cas-wrong-review replaced by a later delivery | reviewed-drop: {} -- the helper moved into the shared module | terminal: superseded, shipped elsewhere | NOT A ROUTE | real-build | review/drop.txt | - |\n", removals["cas-superseded"]),
    );
    let member_ids: Vec<_> = anchors.keys().cloned().collect();
    let body = format!("{body}\nMembers: {}\n", member_ids.join(", "));
    std::fs::write(&ledger, &body).unwrap();
    // Drive publication and receipt admission instead of injecting review state.
    let artifacts = cas_store::SqliteArtifactStore::open(&root).unwrap();
    let context = crate::artifacts::PublishContext {
        store: &artifacts,
        roots: crate::artifacts::paths::PublishRoots::new(&root, &repo.join("artifacts"), "cas-ledger"),
        task_id: "cas-ledger".into(), cloud: None,
    };
    let artifact = crate::artifacts::publish(&context, &ledger).unwrap().artifact;
    let recorder = cas_types::Agent::new_with_role(
        "supervisor".into(), "supervisor".into(), cas_types::AgentRole::Supervisor,
    );
    let review = prepare_receipt(&root, &recorder, "independent-reviewer", &member_ids,
        &published, &artifact.id, &ledger, cas_store::RELEASE_VERDICT_SCHEMA).unwrap();
    cas_store::record_qa_batch_receipt(&root, &review).unwrap();
    let scope = ReleaseScope {
        project_id: "reverted".into(), repository: "example/reverted".into(), release_id: tag.clone(),
        reviewed_sha: published.clone(), final_release_sha: published.clone(), promotion_pr: None,
        release_tag: Some(tag.clone()), constituent_epic_ids: vec![], included_task_ids: anchors.keys().cloned().collect(),
        no_code_members: BTreeMap::new(), proof_refs: vec![ReleaseProofRef {
            kind: ReleaseProofKind::Review, artifact_id: review.ledger_artifact_id.clone(),
            sha256: review.ledger_sha256.clone(), size_bytes: review.ledger_size_bytes,
            reviewed_sha: published, receipt_id: Some(review.id.clone()),
        }],
    };
    let mut unbound = scope.clone();
    unbound.proof_refs.clear();
    let unbound_inventory = resolve_release_inventory(&root, &repo, &unbound).unwrap();
    assert!(unbound_inventory.deliveries.iter().any(|d| d.task_id == "cas-ledger"));
    let inventory = resolve_release_inventory(&root, &repo, &scope).unwrap();
    assert!(inventory.deliveries.iter().any(|delivery| delivery.task_id == "cas-assembly"), "{:?}", inventory.exceptions);
    let preview = crate::release_reconcile::preview_release(&root, &repo, &inventory).unwrap();
    let assembly = tasks.get("cas-assembly").unwrap();
    let source = tasks.get("cas-redrafted").unwrap();
    let expect_content_hold = |scope: &ReleaseScope, code| {
        let inventory = resolve_release_inventory(&root, &repo, scope).unwrap();
        assert!(inventory.exceptions.iter().any(|exception| exception.task_id == assembly.id && exception.code == code));
    };
    for foreign_target in [false, true] {
        let mut changed = assembly.clone();
        if foreign_target {
            changed.deliverables.authorized_merge_sources[0].work_target.target_branch = "other".into();
        } else {
            changed.deliverables.authorized_merge_sources.clear();
        }
        tasks.update(&changed).unwrap();
        expect_content_hold(&scope, ReleaseInventoryCode::UnprovenContent);
    }
    let mut unreviewed = assembly.clone();
    unreviewed.deliverables.premerge_approval = None;
    tasks.update(&unreviewed).unwrap();
    let current = crate::release_reconcile::preview_release(&root, &repo, &inventory).unwrap();
    assert!(matches!(current.get(&assembly.id), Some(CassyMergeClose::Held(code)) if code == "REVIEW_MISSING"));
    tasks.update(&assembly).unwrap();
    let mut stale_source = source.clone();
    stale_source.deliverables.premerge_approval.as_mut().unwrap().delivery_cycle += 1;
    tasks.update(&stale_source).unwrap();
    expect_content_hold(&scope, ReleaseInventoryCode::UnprovenContent);
    tasks.update(&source).unwrap();
    let mut unrelated = assembly.clone();
    let base = &assembly.deliverables.factory_branch_start.as_ref().unwrap().base_sha;
    let tree = git(&["rev-parse", &format!("{}^{{tree}}", scope.final_release_sha)]);
    let unrelated_head = git(&["commit-tree", &tree, "-p", base, "-p", &anchors["cas-redrafted"], "-m", "different-tree merge"]);
    unrelated.deliverables.factory_branch_anchor = Some(unrelated_head.clone());
    tasks.update(&unrelated).unwrap();
    let mut unrelated_scope = scope.clone();
    unrelated_scope.release_tag = None;
    unrelated_scope.final_release_sha = unrelated_head;
    expect_content_hold(&unrelated_scope, ReleaseInventoryCode::UnprovenContent);
    tasks.update(&assembly).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let result = runtime.block_on(crate::release_reconcile::reconcile_release(
        &root, &scope, &inventory, crate::release_reconcile::ReconcileTrigger::Publish { tag: tag.clone() },
    )).unwrap();
    for id in ["cas-reverted", "cas-partial", "cas-ledger", "cas-compact", "cas-unfolded", "cas-resynced", "cas-foreign-cut", "cas-undone", "cas-doc-reverted", "cas-wrong-review"] {
        assert_eq!(tasks.get(id).unwrap().status, TaskStatus::AwaitingMerge, "{id}");
        assert!(matches!(result.members.get(id), Some(CassyMergeClose::Held(reason)) if reason == "RELEASE_MEMBER_REVERTED"), "{id}: {:?}", result.members.get(id));
        assert!(inventory.exceptions.iter().any(|e| e.task_id == id && serde_json::to_value(&e.code).unwrap() == "reverted"));
        assert!(!inventory.deliveries.iter().any(|d| d.task_id == id));
    }
    assert_eq!(tasks.get("cas-doc-only").unwrap().status, TaskStatus::AwaitingMerge);
    assert!(matches!(result.members.get("cas-doc-only"), Some(CassyMergeClose::Held(reason))
        if reason == "RELEASE_CONTENT_UNPROVEN"), "{:?}", result.members.get("cas-doc-only"));
    for id in ["cas-retained", "cas-redrafted", "cas-assembly", "cas-evolved", "cas-doc-evolved", "cas-superseded"] {
        assert_eq!(tasks.get(id).unwrap().status, TaskStatus::Closed, "{id}: {:?}", result.members.get(id));
    }
    for id in ["cas-reverted", "cas-partial", "cas-unfolded", "cas-retained", "cas-redrafted", "cas-resynced", "cas-foreign-cut", "cas-evolved", "cas-undone", "cas-doc-evolved", "cas-doc-reverted", "cas-doc-only"] {
        let task = tasks.get(id).unwrap();
        assert_eq!(release_delivery_effect(&repo,
            task.deliverables.factory_branch_start.as_ref().map(|start| start.base_sha.as_str()),
            task.deliverables.factory_branch_anchor.as_deref().unwrap(), &scope.final_release_sha, id).is_ok(),
            ["cas-retained", "cas-redrafted", "cas-evolved", "cas-doc-evolved"].contains(&id), "{id}");
    }
    // cas-8f6f: the published tag consumes the same reviewed drop as closeout.
    for id in ["cas-retained", "cas-superseded", "cas-assembly"] {
        let verdict = prepare_release_tag_receipt(&root, &[id.into()], &tag).unwrap();
        assert_eq!(verdict.task_heads.get(id), anchors.get(id));
        assert_eq!(verdict.release_tag.as_deref(), Some(tag.as_str()));
    }
    for id in ["cas-reverted", "cas-partial", "cas-unfolded", "cas-foreign-cut", "cas-wrong-review"] {
        assert!(prepare_release_tag_receipt(&root, &[id.into()], &tag).is_err());
    }
    std::fs::write(&ledger, format!("{body}\nUnrecorded amendment.\n")).unwrap();
    assert!(prepare_release_tag_receipt(&root, &["cas-superseded".into()], &tag).is_err());
    assert_eq!(serde_json::to_value(&preview).unwrap(), serde_json::to_value(&result.members).unwrap());
    git(&["rm", "cas-redrafted.rs"]);
    git(&["commit", "-qm", "drop the imported source"]);
    let mut dropped = scope;
    dropped.release_tag = None;
    dropped.final_release_sha = git(&["rev-parse", "HEAD"]);
    expect_content_hold(&dropped, ReleaseInventoryCode::Reverted);
}

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
    git(&["commit", "--allow-empty", "-qm", "release"]);
    let release = git(&["rev-parse", "HEAD"]);
    // Only the forge boundary is fake: target CI is green even when the
    // delivery's exact anchor has no checks, or has pending/failed checks.
    let checks = |conclusion: Option<&str>| serde_json::json!({"check_runs": [
        {"name": "Classify Changes", "status": "completed", "conclusion": "success"},
        {"name": "Docs Lint", "status": if conclusion.is_some() { "completed" } else { "in_progress" }, "conclusion": conclusion},
        {"name": "Scoped Validation (factory/PR)", "status": "completed", "conclusion": "skipped"}
    ]});
    let response = root.join("late-ci.json");
    std::fs::write(&response, r#"{"check_runs":[]}"#).unwrap();
    let gh = root.join("gh-fixture");
    crate::test_paths::warm_stub(&gh, &format!(r#"#!/bin/sh
if [ "$1 $2" = 'repo view' ]; then
    printf '%s\n' '{{"nameWithOwner":"example/exempt"}}'
    exit 0
fi
printf '%s\n' "$4" >> .cas/ci-requests.log
case "$4" in
repos/example/exempt/commits/{published}/check-runs) cat .cas/late-ci.json ;;
repos/example/exempt/commits/{release}/check-runs) cat <<'JSON'
{}
JSON
;;
repos/example/exempt/commits/{code}/check-runs) printf '%s\n' '{{"check_runs":[]}}' ;;
*) exit 97 ;;
esac
"#, checks(Some("success"))));
    env.set(crate::github_issue_attach::GH_BIN_ENV, &gh);
    let tasks = crate::store::open_task_store(&root).unwrap();
    let mut train = Task::new("cas-exempt-train".into(), "train".into());
    train.task_type = cas_types::TaskType::Epic;
    tasks.add(&train).unwrap();
    let now = chrono::Utc::now();
    let window = crate::rapid_response::enable(&root, tasks.as_ref(), &train.id,
        chrono::Duration::hours(1), "incident", "revert", now).unwrap();
    let mut expected = BTreeMap::new();
    let mut late_id = String::new();
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
        if kind == "mechanical" && admission == Some(MergeCiAdmission::Green) {
            late_id = id.clone();
        }
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
        reviewed_sha: code, final_release_sha: release, promotion_pr: None, release_tag: None,
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

    // Replay the pending-at-merge incident through release preview and apply,
    // without reopening the shipped docs task or producing a no-op merge.
    tasks.record_merge_admission(&late_id, &published, MergeCiAdmission::Advisory).unwrap();
    let before = tasks.get(&late_id).unwrap();
    let scope = ReleaseScope { included_task_ids: vec![late_id.clone()], ..scope };
    let inventory = resolve_release_inventory(&root, &repo, &scope).unwrap();
    for (case, response_body, eligible) in [
        ("pending", checks(None), false),
        ("failed", checks(Some("failure")), false),
        ("wrong-anchor", serde_json::json!({"check_runs": []}), false),
        ("late-green", checks(Some("success")), true),
    ] {
        std::fs::write(&response, response_body.to_string()).unwrap();
        let outcomes = crate::release_reconcile::preview_release(&root, &repo, &inventory).unwrap();
        let outcome = outcomes.get(&late_id).unwrap();
        assert_eq!(matches!(outcome, CassyMergeClose::Closed { .. }), eligible, "{case}: {outcome:?}");
        if !eligible { assert!(matches!(outcome, CassyMergeClose::Held(code) if code == "LANE_CI_MISSING"), "{case}: {outcome:?}"); }
        assert_eq!(serde_json::to_value(tasks.get(&late_id).unwrap()).unwrap(), serde_json::to_value(&before).unwrap());
    }
    let requests = std::fs::read_to_string(root.join("ci-requests.log")).unwrap_or_default();
    assert!(requests.lines().any(|endpoint| endpoint == format!("repos/example/exempt/commits/{published}/check-runs")));
    assert!(!requests.lines().any(|endpoint| endpoint == format!("repos/example/exempt/commits/{}/check-runs", scope.final_release_sha)));
    let outcomes = reconcile_members(&root, &repo, &inventory, true, &mut |_, _| Ok(())).unwrap();
    assert!(matches!(outcomes.get(&late_id), Some(CassyMergeClose::Closed { .. })));
    let closed = tasks.get(&late_id).unwrap();
    assert_eq!(closed.status, TaskStatus::Closed);
    assert_eq!(closed.deliverables.factory_branch_anchor, before.deliverables.factory_branch_anchor);
    assert_eq!(closed.deliverables.green_merge_commit, before.deliverables.green_merge_commit);
    let replay = crate::release_reconcile::preview_release(&root, &repo, &inventory).unwrap();
    assert!(matches!(replay.get(&late_id), Some(CassyMergeClose::AlreadyClosed)));
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
    git(&["commit", "--allow-empty", "-m", "release base"]);
    let base = git(&["rev-parse", "HEAD"]);
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
    child.assignee = Some("fixture-worker".into());
    child.status = TaskStatus::Closed;
    child.deliverables.factory_branch_anchor = Some(published.clone());
    child.deliverables.green_merge_commit = Some(published.clone());
    child.deliverables.factory_branch_start = Some(FactoryBranchStart {
        work_target: WorkTarget { repo_selector: "project:parent-fixture".into(), target_branch: "main".into() },
        branch: "factory/worker".into(), assignee: "fixture-worker".into(), base_sha: base,
    });
    tasks.add(&child).unwrap();
    tasks.add_dependency(&cas_types::Dependency::new(
        child.id.clone(), parent.id.clone(), cas_types::DependencyType::ParentChild,
    )).unwrap();
    let mut qa = Task::new("cas-release-qa".into(), "Artifact-only release QA".into());
    qa.origin_project = parent.origin_project.clone();
    qa.execution_note = Some("no-code".into());
    qa.assignee = Some("fixture-qa-worker".into());
    let artifacts = cas_store::SqliteArtifactStore::open(&root).unwrap();
    let context = crate::artifacts::PublishContext {
        store: &artifacts,
        roots: crate::artifacts::paths::PublishRoots::new(&root, &repo.join("artifacts"), &qa.id),
        task_id: qa.id.clone(),
        cloud: None,
    };
    let report_path = repo.join("report.md");
    std::fs::write(&report_path, format!("# Release QA\nReviewed source: {published}\n")).unwrap();
    let report = crate::artifacts::publish(&context, &report_path).unwrap().artifact;
    artifacts.mark_uploaded(&report.id, "fixture-qa-report").unwrap();
    artifacts.mark_committed(&report.id, None).unwrap();
    let replacement_path = repo.join("replacement.md");
    std::fs::write(&replacement_path, format!("# Corrected Release QA\nReviewed source: {published}\n")).unwrap();
    let replacement = crate::artifacts::publish(&context, &replacement_path).unwrap().artifact;
    artifacts.mark_uploaded(&replacement.id, "fixture-qa-replacement").unwrap();
    artifacts.mark_committed(&replacement.id, None).unwrap();
    qa.external_ref = Some(format!("artifact:{}/{}", qa.id, report.id));
    tasks.add(&qa).unwrap();
    tasks.add_dependency(&cas_types::Dependency::new(
        qa.id.clone(), parent.id.clone(), cas_types::DependencyType::ParentChild,
    )).unwrap();
    let ledger_path = repo.join("LEDGER.md");
    let covered = [child.id.clone()];
    std::fs::write(&ledger_path, crate::qa_ledger::scaffold_with_coverage(&qa, &[], &published, &covered)).unwrap();
    let ledger = crate::artifacts::publish(&context, &ledger_path).unwrap().artifact;
    artifacts.mark_uploaded(&ledger.id, "fixture-qa-ledger").unwrap();
    artifacts.mark_committed(&ledger.id, None).unwrap();
    let receipt = prepare_receipt(
        &root, &reviewer, "fixture-qa-worker", &covered, &published,
        &ledger.id, &ledger_path, cas_store::QA_BATCH_RECEIPT_SCHEMA,
    ).unwrap();
    cas_store::record_qa_batch_receipt(&root, &receipt).unwrap();
    let verification_boundary = cas_types::VerificationProofBoundary::task_at(
        crate::mcp::tools::core::task::lifecycle::repository_proof::capture_repository_proof_at_target(
            &repo, "main", Vec::new(),
        ).unwrap(),
    );
    let mut connection = rusqlite::Connection::open(root.join("cas.db")).unwrap();
    let mut report_task = qa.clone();
    report_task.demo_statement = "Review the release QA report".into();
    tasks.update(&report_task).unwrap();
    let snapshot = crate::mcp::tools::core::task::lifecycle::report_proof::read_report_verdict(
        &connection, true, &report_task, &report_task,
    ).unwrap().proof_to_record().unwrap();
    let dispatch = cas_store::create_verification_dispatch_reviewing_with_conn(
        &connection, &qa.id, "fixture-qa-worker", &reviewer.id, &verification_boundary,
        Some(&snapshot), chrono::Utc::now() + chrono::Duration::hours(1), false,
    ).unwrap();
    let mut approve = |dispatch: &cas_types::VerificationDispatch, task: &mut Task| {
        let mut approval = cas_types::Verification::new(format!("ver-{}", dispatch.id), task.id.clone());
        approval.agent_id = Some(reviewer.id.clone());
        approval.issuer_agent_id = Some(reviewer.id.clone());
        approval.dispatch_id = Some(dispatch.id.clone());
        approval.provenance = cas_types::VerificationProvenance::SupervisorDirect;
        approval.status = cas_types::VerificationStatus::Approved;
        let transaction = connection.transaction().unwrap();
        cas_store::add_verification_with_conn(&transaction, &approval).unwrap();
        cas_store::resolve_verification_dispatch_with_conn(&transaction, &dispatch.id, &reviewer.id, None, true)
            .unwrap().unwrap();
        transaction.commit().unwrap();
        task.status = TaskStatus::Closed;
        task.closed_at = Some(chrono::Utc::now());
        tasks.update(task).unwrap();
    };
    approve(&dispatch, &mut report_task);
    let scope = ReleaseScope {
        project_id: "parent-fixture".into(), repository: "example/parent-fixture".into(),
        release_id: format!("v{}", env!("CARGO_PKG_VERSION")),
        reviewed_sha: published.clone(), final_release_sha: published,
        promotion_pr: None, release_tag: None, constituent_epic_ids: vec![parent.id.clone()],
        included_task_ids: vec![child.id.clone()], no_code_members: BTreeMap::new(),
        proof_refs: vec![ReleaseProofRef {
            kind: ReleaseProofKind::Qa, artifact_id: ledger.id, sha256: ledger.sha256,
            size_bytes: ledger.size_bytes, reviewed_sha: receipt.staging_sha, receipt_id: Some(receipt.id),
        }],
    };
    let core = crate::mcp::CasCore::for_cli(root.clone());
    assert!(core.get_agent_id().is_err());
    let inventory = resolve_release_inventory(&root, &repo, &scope).unwrap();
    assert!(inventory.exceptions.is_empty(), "{:?}", inventory.exceptions);
    assert_eq!(inventory.deliveries.iter().map(|delivery| &delivery.task_id).collect::<Vec<_>>(), vec![&child.id]);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let assert_held = |input: &ReleaseInventory, expected: &str| {
        let outcome = runtime.block_on(core.close_release_parent(input, &parent.id));
        assert!(matches!(outcome, crate::release_reconcile::ParentOutcome::Held { ref gate, .. }
            if gate.split(':').next() == Some(expected)), "{outcome:?}");
    };
    let mut missing = inventory.clone();
    missing.scope.proof_refs.clear();
    assert_held(&missing, "RELEASE_PARENT_MEMBER_HELD");
    let mut stale = inventory.clone();
    stale.scope.reviewed_sha = git(&["rev-parse", "HEAD^1"]);
    assert_held(&stale, "RELEASE_PARENT_MEMBER_HELD");
    let mut all_reports = inventory.clone();
    all_reports.deliveries.clear();
    assert_held(&all_reports, "RELEASE_EMPTY_PARENT");
    let mut replaced = report_task.clone();
    replaced.external_ref = Some(format!("artifact:{}/{}", replaced.id, replacement.id));
    tasks.update(&replaced).unwrap();
    assert_held(&inventory, "RELEASE_PARENT_MEMBER_HELD");
    let mut foreign = report_task.clone();
    foreign.external_ref = Some(format!("artifact:{}/{}", child.id, report.id));
    tasks.update(&foreign).unwrap();
    assert_held(&inventory, "RELEASE_PARENT_MEMBER_HELD");
    let mut incomplete = report_task.clone();
    incomplete.status = TaskStatus::Open;
    tasks.update(&incomplete).unwrap();
    assert_held(&inventory, "RELEASE_PARENT_CHILDREN_INCOMPLETE");
    tasks.update(&report_task).unwrap();
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
    let unbound_dispatch = cas_store::create_verification_dispatch_bound(
        &root, &qa.id, "fixture-qa-worker", &reviewer.id, &verification_boundary,
        chrono::Utc::now() + chrono::Duration::hours(1), false,
    ).unwrap();
    assert_held(&inventory, "RELEASE_PARENT_MEMBER_HELD");
    approve(&unbound_dispatch, &mut qa);
    assert_held(&inventory, "RELEASE_PARENT_MEMBER_HELD");
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
    // cas-f0e7a: a closed no-code member, unlinked from the train, binds no
    // report proof (bind_reports skips terminal members), as cas-928bf on the frozen train.
    let mut closed_report = Task::new("cas-closed-report".into(), "Shipped notes".into());
    closed_report.origin_project = Some("close-fixture".into());
    closed_report.execution_note = Some("no-code".into());
    closed_report.external_ref = Some("artifact:art-unbound".into());
    closed_report.status = TaskStatus::Closed;
    tasks.add(&closed_report).unwrap();
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
        included_task_ids: ["cas-approved", "cas-candidate", "cas-unshipped", "cas-other-source", "cas-unreviewed", "cas-report", "cas-closed", "cas-equal", "cas-closed-report"].map(str::to_string).to_vec(),
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
    // cas-f0e7a: the frozen train's cut shape, three members already closed beside the
    // closed no-code member with no inventory delivery, passes the cut gate.
    let mut cut = scope.clone();
    cut.included_task_ids = ["cas-approved", "cas-candidate", "cas-equal", "cas-closed-report"].map(str::to_string).to_vec();
    cut.no_code_members.clear();
    let cut_inventory = resolve_release_inventory(&root, &repo, &cut).unwrap();
    assert!(!cut_inventory.deliveries.iter().any(|d| d.task_id == "cas-closed-report"));
    let cut_preview = crate::cli::release_automation::ReleasePreview::from_members(cut.clone(),
        crate::release_reconcile::preview_release(&root, &repo, &cut_inventory).unwrap());
    let shown = serde_json::to_value(&cut_preview).unwrap();
    assert_eq!(shown["would_close"], 0);
    assert!(cut.included_task_ids.iter().all(|id| shown["members_by_id"][id] == "already_closed"), "{shown}");
    cut_preview.allow_cut().unwrap();
    // A batch ledger cannot replace the missing code delivery approvals.
    let mut later = receipt;
    later.id = "qab-later".into();
    later.task_heads = ["cas-unreviewed", "cas-unshipped", "cas-other-source", "cas-closed"].map(|id| (id.into(), anchor.clone())).into();
    cas_store::import_qa_batch_receipt(&root, &later).unwrap();
    let inventory = resolve_release_inventory(&root, &repo, &scope).unwrap();
    let held = crate::release_reconcile::preview_release(&root, &repo, &inventory).unwrap();
    for id in later.task_heads.keys() {
        assert!(matches!(held.get(id), Some(CassyMergeClose::Held(reason)) if reason == "REVIEW_MISSING"));
        use crate::mcp::tools::core::workflow::premerge_verification::cycle;
        let current = cycle::current(&conn, &tasks.get(id).unwrap(), &anchor).unwrap();
        let mut reviewed = boundary.clone();
        reviewed.receipt_id = current.clone();
        let dispatch = cas_store::create_verification_dispatch_bound(&root, id, "worker", "supervisor",
            &reviewed, chrono::Utc::now() + chrono::Duration::hours(1), false).unwrap();
        conn.execute("UPDATE verification_dispatches SET state='resolved' WHERE id=?", [&dispatch.id]).unwrap();
        let verdict = format!("ver-repaired-{id}");
        conn.execute("INSERT INTO verifications (id,task_id,agent_id,verification_type,status,summary,files_reviewed,created_at,dispatch_id) VALUES (?1,?2,'reviewer','task','approved','pass','[]',?3,?4)",
            rusqlite::params![verdict, id, chrono::Utc::now().to_rfc3339(), dispatch.id]).unwrap();
        if let Some(cycle_id) = current {
            let event = cas_types::Event::new(cas_types::EventType::VerificationAdded,
                cas_types::EventEntityType::Verification, &verdict, "review")
                .with_metadata(serde_json::to_value(cycle::Stamp {
                    cycle_id, approved_anchor: anchor.clone(), dispatch_id: dispatch.id,
                    verdict_id: verdict, registry_version: Some(crate::gates::registry_version_in_force()),
                }).unwrap());
            cas_store::record_event_with_conn(&conn, &event).unwrap();
        }
    }
    // Existing merge approvals alone complete the operation; no ledger author
    // or recorder participates in code acceptance.
    let complete = runtime.block_on(crate::release_reconcile::reconcile_release(&root, &scope, &inventory,
        crate::release_reconcile::ReconcileTrigger::Backfill)).unwrap();
    assert!(complete.complete, "{complete:?}");
    assert!(complete.residuals.is_empty());
    assert!(!complete.exceptions.is_empty());
    use crate::mcp::tools::core::task::lifecycle::{close_ops::lifecycle_race_hook, report_proof::require_report_verdict};
    let agents = crate::store::open_agent_store(&root).unwrap();
    agents.init().unwrap();
    agents.register(&cas_types::Agent::new_with_role(
        "supervisor".into(), "supervisor".into(), cas_types::AgentRole::Supervisor,
    )).unwrap();
    let artifacts = cas_store::SqliteArtifactStore::open(&root).unwrap();
    let mut report_proofs = BTreeMap::new();
    let mut reviews = BTreeMap::new();
    for id in ["cas-bound-report", "cas-revised-report", "cas-revoked-report"] {
        let mut report = Task::new(id.into(), id.into());
        report.origin_project = Some("close-fixture".into());
        report.task_type = cas_types::TaskType::Spike;
        report.execution_note = Some("no-code".into());
        report.external_ref = Some("https://example.test/reports/1".into());
        report.status = TaskStatus::AwaitingMerge;
        tasks.add(&report).unwrap();
        let artifact = id.replacen("cas", "art", 1);
        artifacts.record_local(&cas_store::NewArtifact {
            id: artifact.clone(), task_id: id.into(), name: "report.md".into(),
            mime: "text/markdown".into(), size_bytes: 10, sha256: "c".repeat(64),
        }).unwrap();
        artifacts.mark_uploaded(&artifact, &id.replacen("cas", "cloud", 1)).unwrap();
        artifacts.mark_committed(&artifact, None).unwrap();
        let report = tasks.get(id).unwrap();
        let reviewed = require_report_verdict(&root, true, &report, &report).unwrap().proof_to_record().unwrap();
        let dispatch = cas_store::create_verification_dispatch_reviewing_with_conn(
            &conn, id, "supervisor", "supervisor", &cas_types::VerificationProofBoundary::task(),
            Some(&reviewed), chrono::Utc::now() + chrono::Duration::hours(1), false,
        ).unwrap();
        let mut verdict = cas_types::Verification::new(format!("ver-{id}"), id.into());
        verdict.status = cas_types::VerificationStatus::Approved;
        verdict.summary = "Report reviewed".into();
        verdict.provenance = cas_types::VerificationProvenance::SupervisorDirect;
        verdict.agent_id = Some("supervisor".into());
        verdict.issuer_agent_id = verdict.agent_id.clone();
        verdict.dispatch_id = Some(dispatch.id.clone());
        cas_store::VerificationStore::add(&cas_store::SqliteVerificationStore::open(&root).unwrap(), &verdict).unwrap();
        cas_store::resolve_verification_dispatch_with_conn(&conn, &dispatch.id, "supervisor", None, true).unwrap();
        report_proofs.insert(id.to_string(), vec![ReleaseProofRef {
            kind: ReleaseProofKind::Report, artifact_id: artifact, sha256: "c".repeat(64), size_bytes: 10,
            reviewed_sha: anchor.clone(), receipt_id: None,
        }]);
        reviews.insert(id, dispatch.id);
    }
    cas_store::import_qa_batch_receipt(&root, &QaBatchReceipt {
        id: "qab-reports".into(),
        task_heads: report_proofs.keys().map(|id| (id.clone(), anchor.clone())).collect(),
        ..later
    }).unwrap();
    let mut revised = tasks.get("cas-revised-report").unwrap();
    revised.external_ref = Some("https://example.test/reports/2".into());
    tasks.update(&revised).unwrap();
    let mut reports = scope.clone();
    reports.included_task_ids = report_proofs.keys().cloned().collect();
    reports.no_code_members = report_proofs;
    let report_inventory = resolve_release_inventory(&root, &repo, &reports).unwrap();
    let before = snapshot(&root);
    let shown = crate::release_reconcile::preview_release(&root, &repo, &report_inventory).unwrap();
    assert_eq!(snapshot(&root), before);
    for id in ["cas-bound-report", "cas-revoked-report"] {
        assert!(matches!(shown.get(id), Some(CassyMergeClose::Closed { .. })), "{id}: {:?}", shown.get(id));
    }
    assert!(matches!(shown.get("cas-revised-report"), Some(CassyMergeClose::Held(reason)) if reason == "REVIEW_MISSING"));
    assert_eq!(
        cas_store::get_verification_dispatch(&root, &reviews["cas-revised-report"]).unwrap().state,
        cas_types::VerificationDispatchState::Resolved,
    );
    let withdrawn = root.clone();
    lifecycle_race_hook::before_terminal_write("cas-revoked-report", move || {
        cas_store::SqliteArtifactStore::open(&withdrawn).unwrap()
            .revoke("art-revoked-report", "supervisor", "Report withdrawn").unwrap();
    });
    let applied = reconcile_members(&root, &repo, &report_inventory, true, &mut |_, _| Ok(())).unwrap();
    assert!(matches!(applied.get("cas-bound-report"), Some(CassyMergeClose::Closed { .. })), "{:?}", applied.get("cas-bound-report"));
    assert_eq!(tasks.get("cas-bound-report").unwrap().status, TaskStatus::Closed);
    assert!(matches!(applied.get("cas-revoked-report"), Some(CassyMergeClose::Held(_))), "{:?}", applied.get("cas-revoked-report"));
    assert_eq!(tasks.get("cas-revoked-report").unwrap().status, TaskStatus::AwaitingMerge);
    assert!(matches!(applied.get("cas-revised-report"), Some(CassyMergeClose::Held(reason)) if reason == "REVIEW_MISSING"));
    assert_eq!(
        cas_store::get_verification_dispatch(&root, &reviews["cas-revised-report"]).unwrap().state,
        cas_types::VerificationDispatchState::Invalidated,
    );
}

/// cas-692a7: cas-23a6 shipped in a release holding an approved, receipt-less
/// verdict at its exact anchor (2026-11-10), but backfill minted its cycles on
/// 2026-11-12 and release close then read it as REVIEW_MISSING.
#[test]
fn pre_backfill_exact_head_approval_closes_at_release_cas_692a7() {
    use crate::mcp::tools::core::task::lifecycle::merge_close::CassyMergeClose;
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let repo = env.home().join("release");
    std::fs::create_dir_all(&repo).unwrap();
    let root = crate::store::init_cas_dir(&repo).unwrap();
    env.set("CAS_ROOT", &root);
    env.set("CAS_CLONE_PATH", &repo);
    std::fs::write(root.join("config.toml"), "[project]\ncanonical_id='backfill'\n").unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "Fixture"]);
    git(&["config", "user.email", "fixture@example.test"]);
    git(&["remote", "add", "origin", "https://github.com/example/backfill.git"]);
    std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
    git(&["add", ".gitignore"]);
    git(&["commit", "-qm", "base"]);
    let base = git(&["rev-parse", "HEAD"]);
    std::fs::write(repo.join("app.rs"), "fn first() {}\n").unwrap();
    git(&["add", "app.rs"]);
    git(&["commit", "-qm", "first delivery"]);
    let retired = git(&["rev-parse", "HEAD"]);
    std::fs::write(repo.join("app.rs"), "fn reviewed() {}\n").unwrap();
    git(&["commit", "-qam", "reviewed delivery"]);
    let anchor = git(&["rev-parse", "HEAD"]);
    git(&["commit", "-q", "--allow-empty", "-m", "landed merge"]);
    let published = git(&["rev-parse", "HEAD"]);
    let tasks = crate::store::open_task_store(&root).unwrap();
    let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
    let backfilled = "2026-11-12T22:52:08.620538+00:00";
    let mut expected = BTreeMap::new();
    // (member, verdict requested at, anchor also retired by an ended cycle)
    for (id, requested, reretired, closes) in [
        ("cas-pre-backfill", "2026-11-10T21:14:46.323+00:00", false, true),
        ("cas-after-cycle", "2026-11-13T00:00:00+00:00", false, false),
        ("cas-retired-head", "2026-11-10T21:14:46.323+00:00", true, false),
    ] {
        let mut task = Task::new(id.into(), id.into());
        task.origin_project = Some("backfill".into());
        task.status = TaskStatus::AwaitingMerge;
        task.deliverables.factory_branch_anchor = Some(anchor.clone());
        task.deliverables.factory_branch_start = Some(FactoryBranchStart {
            work_target: WorkTarget { repo_selector: "project:backfill".into(), target_branch: "main".into() },
            branch: format!("factory/{id}"), assignee: "worker".into(), base_sha: base.clone(),
        });
        tasks.add(&task).unwrap();
        // cas-23a6's backfilled shape: an ended cycle 1, an open cycle 2 holding the anchor.
        conn.execute("DELETE FROM delivery_cycles WHERE task_id=?1", [id]).unwrap();
        let first = if reretired { vec![retired.clone(), anchor.clone()] } else { vec![retired.clone()] };
        for (n, anchors, ended) in [(1, first, Some(backfilled)), (2, vec![retired.clone(), anchor.clone()], None)] {
            conn.execute("INSERT INTO delivery_cycles VALUES (?1,'backfill',?2,?3,?4,?5,?6,?7,?7)", rusqlite::params![
                cas_store::delivery_cycle_id("backfill", id, n), id, n, serde_json::to_string(&anchors).unwrap(),
                base, ended, backfilled]).unwrap();
        }
        let boundary = cas_types::VerificationProofBoundary::task_at(cas_types::RepositoryProofBoundary {
            peer_premerge: None, repository_root: repo.display().to_string(), worktree_root: repo.display().to_string(),
            head_commit: published.clone(), state_digest: String::new(), target_branch: None, anchor_commits: vec![anchor.clone()],
        });
        let dispatch = cas_store::create_verification_dispatch_bound(&root, id, "worker", "supervisor", &boundary,
            chrono::Utc::now() + chrono::Duration::hours(1), false).unwrap();
        conn.execute("UPDATE verification_dispatches SET state='resolved', requested_at=?1 WHERE id=?2",
            rusqlite::params![requested, dispatch.id]).unwrap();
        conn.execute("INSERT INTO verifications (id,task_id,agent_id,verification_type,status,summary,files_reviewed,created_at,dispatch_id) VALUES (?1,?2,'reviewer','task','approved','pass','[]',?3,?4)",
            rusqlite::params![format!("ver-{id}"), id, requested, dispatch.id]).unwrap();
        expected.insert(id.to_string(), closes);
    }
    let scope = ReleaseScope {
        project_id: "backfill".into(), repository: "example/backfill".into(), release_id: "v9.99.0".into(),
        reviewed_sha: anchor, final_release_sha: published, promotion_pr: None, release_tag: None,
        constituent_epic_ids: vec![], included_task_ids: expected.keys().cloned().collect(),
        no_code_members: BTreeMap::new(), proof_refs: vec![],
    };
    let inventory = resolve_release_inventory(&root, &repo, &scope).unwrap();
    let outcomes = crate::release_reconcile::preview_release(&root, &repo, &inventory).unwrap();
    for (id, closes) in expected {
        let outcome = outcomes.get(&id);
        if closes {
            assert!(matches!(outcome, Some(CassyMergeClose::Closed { .. })), "{id}: {outcome:?}");
        } else {
            assert!(matches!(outcome, Some(CassyMergeClose::Held(reason)) if reason == "REVIEW_MISSING"), "{id}: {outcome:?}");
        }
    }
}

/// cas-c8ae (GH #1993): verdict rows stay on the machine that reviewed a
/// delivery. A cut on another machine admits the member through the approval
/// carried on its task row, bound to that exact anchor and delivery cycle.
#[test]
fn approval_carried_on_task_admits_cut_on_another_machine_cas_c8ae() {
    use crate::mcp::tools::core::task::lifecycle::merge_close::CassyMergeClose;
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let repo = env.home().join("release");
    std::fs::create_dir_all(&repo).unwrap();
    let root = crate::store::init_cas_dir(&repo).unwrap();
    env.set("CAS_ROOT", &root);
    env.set("CAS_CLONE_PATH", &repo);
    std::fs::write(root.join("config.toml"), "[project]\ncanonical_id='crossmachine'\n").unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "Fixture"]);
    git(&["config", "user.email", "fixture@example.test"]);
    git(&["remote", "add", "origin", "https://github.com/example/crossmachine.git"]);
    std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
    git(&["add", ".gitignore"]);
    git(&["commit", "-qm", "base"]);
    let base = git(&["rev-parse", "HEAD"]);
    std::fs::write(repo.join("app.rs"), "fn reviewed() {}\n").unwrap();
    git(&["add", "app.rs"]);
    git(&["commit", "-qm", "reviewed delivery"]);
    let anchor = git(&["rev-parse", "HEAD"]);
    git(&["commit", "-q", "--allow-empty", "-m", "landed merge"]);
    let published = git(&["rev-parse", "HEAD"]);
    let tasks = crate::store::open_task_store(&root).unwrap();
    let mut expected = BTreeMap::new();
    // (member, anchor its carried approval reviewed, closes)
    for (id, approved_anchor, closes) in [
        ("cas-reviewed-elsewhere", anchor.clone(), true),
        ("cas-other-anchor", base.clone(), false),
    ] {
        let mut task = Task::new(id.into(), id.into());
        task.origin_project = Some("crossmachine".into());
        task.status = TaskStatus::AwaitingMerge;
        task.deliverables.factory_branch_anchor = Some(anchor.clone());
        task.deliverables.factory_branch_start = Some(FactoryBranchStart {
            work_target: WorkTarget { repo_selector: "project:crossmachine".into(), target_branch: "main".into() },
            branch: format!("factory/{id}"), assignee: "worker".into(), base_sha: base.clone(),
        });
        tasks.add(&task).unwrap();
        // The task row as pulled from the reviewing machine; no local verdict rows.
        let mut synced = tasks.get(id).unwrap();
        synced.deliverables.premerge_approval = Some(cas_types::PortablePremergeApproval {
            verdict_id: format!("ver-{id}"), dispatch_id: format!("vdispatch-{id}"),
            approved_anchor, delivery_cycle: synced.deliverables.delivery_cycle,
        });
        tasks.update(&synced).unwrap();
        expected.insert(id.to_string(), closes);
    }
    let scope = ReleaseScope {
        project_id: "crossmachine".into(), repository: "example/crossmachine".into(), release_id: "v9.99.0".into(),
        reviewed_sha: anchor, final_release_sha: published, promotion_pr: None, release_tag: None,
        constituent_epic_ids: vec![], included_task_ids: expected.keys().cloned().collect(),
        no_code_members: BTreeMap::new(), proof_refs: vec![],
    };
    let inventory = resolve_release_inventory(&root, &repo, &scope).unwrap();
    let outcomes = crate::release_reconcile::preview_release(&root, &repo, &inventory).unwrap();
    for (id, closes) in expected {
        let outcome = outcomes.get(&id);
        if closes {
            assert!(matches!(outcome, Some(CassyMergeClose::Closed { .. })), "{id}: {outcome:?}");
        } else {
            assert!(matches!(outcome, Some(CassyMergeClose::Held(reason)) if reason == "REVIEW_MISSING"), "{id}: {outcome:?}");
        }
    }
}
