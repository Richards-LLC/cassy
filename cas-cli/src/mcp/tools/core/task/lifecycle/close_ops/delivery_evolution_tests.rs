use super::*;

#[test]
fn release_preview_reuses_parallel_proofs_with_identical_outcomes_cas_b7be() {
    use crate::qa_batch::{ReleaseScope, resolve_release_inventory};
    use cas_types::{FactoryBranchStart, Task, TaskStatus, WorkTarget};
    use std::sync::atomic::Ordering;

    let mut env = crate::test_support::TestEnvGuard::temp_home();
    for members in [4, 32] {
        let repo = env.home().join(format!("preview-{members}"));
        std::fs::create_dir(&repo).unwrap();
        let root = crate::store::init_cas_dir(&repo).unwrap();
        env.set("CAS_ROOT", &root);
        env.set("CAS_CLONE_PATH", &repo);
        std::fs::write(root.join("config.toml"), "[project]\ncanonical_id='b7be-fixture'\n").unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap();
            assert!(output.status.success(), "{args:?}: {output:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.name", "Preview fixture"]);
        git(&["config", "user.email", "fixture@example.test"]);
        git(&["remote", "add", "origin", "https://github.com/Example/b7be-fixture.git"]);
        let content = |left: usize, right: usize| {
            let middle = (0..64).map(|i| format!("v{i}: {i}")).collect::<Vec<_>>().join(", ");
            format!("const STATE = {{ left: {left}, {middle}, right: {right} }};\n")
        };
        std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
        std::fs::write(repo.join("shared.rs"), content(0, 0)).unwrap();
        git(&["add", ".gitignore", "shared.rs"]);
        git(&["commit", "-qm", "base"]);
        let tasks = crate::store::open_task_store(&root).unwrap();
        let mut ids = Vec::new();
        for i in 1..=members {
            let id = format!("cas-b7{i:06x}");
            let branch = format!("factory/{id}");
            let base = git(&["rev-parse", "HEAD"]);
            git(&["checkout", "-qb", &branch]);
            std::fs::write(repo.join("shared.rs"), content(i, i - 1)).unwrap();
            git(&["commit", "-qam", &format!("fix({id}): deliver left update")]);
            let anchor = git(&["rev-parse", "HEAD"]);
            git(&["checkout", "-q", "main"]);
            std::fs::write(repo.join("shared.rs"), content(i - 1, i)).unwrap();
            git(&["commit", "-qam", "independent right-side progress"]);
            let merge = Command::new("git").arg("-C").arg(&repo)
                .args(["merge", "--no-ff", "--no-commit", &branch]).output().unwrap();
            assert!(merge.status.success() || merge.status.code() == Some(1), "{merge:?}");
            std::fs::write(repo.join("shared.rs"), content(i, i)).unwrap();
            git(&["add", "shared.rs"]);
            git(&["commit", "-qm", "integrate member"]);
            let mut task = Task::new(id.clone(), id.clone());
            task.origin_project = Some("b7be-fixture".into());
            task.status = TaskStatus::AwaitingMerge;
            task.deliverables.factory_branch_anchor = Some(anchor);
            task.deliverables.factory_branch_start = Some(FactoryBranchStart {
                work_target: WorkTarget { repo_selector: "project:b7be-fixture".into(), target_branch: "main".into() },
                branch, assignee: "fixture".into(), base_sha: base,
            });
            tasks.add(&task).unwrap();
            ids.push(id);
        }
        let final_sha = git(&["rev-parse", "HEAD"]);
        let scope = ReleaseScope {
            project_id: "b7be-fixture".into(), repository: "Example/b7be-fixture".into(),
            release_id: "v9.99.0".into(), reviewed_sha: final_sha.clone(), final_release_sha: final_sha,
            promotion_pr: None, release_tag: None, constituent_epic_ids: vec![], included_task_ids: ids,
            no_code_members: Default::default(), proof_refs: vec![],
        };
        // cas-0e29e: an edge is the unit of a walk's Git and hunk work. The
        // release must read each (path, edge) once, not once per member walk.
        let edges = git(&["rev-list", "--parents", "HEAD"]).lines()
            .map(|record| record.split_whitespace().count() - 1).sum::<usize>();
        let mut results = Vec::new();
        let mut blame_counts = Vec::new();
        // Per-member walks are the reference outcome for the shared walk.
        for (shared, enabled) in [(false, true), (true, false), (true, true)] {
            // A whole-walk hit must not conceal the edge cache under test.
            forget_walks();
            USE_PARALLEL_PROOF_CACHE.store(enabled, Ordering::Relaxed);
            USE_SHARED_WALK.store(shared, Ordering::Relaxed);
            EDGE_READS.store(0, Ordering::Relaxed);
            let trace = repo.join(format!("git-{shared}-{enabled}.jsonl"));
            env.set("GIT_TRACE2_EVENT", &trace);
            let inventory = resolve_release_inventory(&root, &repo, &scope).unwrap();
            assert!(inventory.exceptions.is_empty(), "{:?}", inventory.exceptions);
            let outcomes = crate::release_reconcile::preview_release(&root, &repo, &inventory).unwrap();
            assert_eq!(outcomes.len(), members);
            assert!(outcomes.values().all(|outcome| matches!(outcome,
                crate::mcp::tools::core::task::lifecycle::merge_close::CassyMergeClose::Held(code)
                    if code == "REVIEW_MISSING")));
            results.push(serde_json::to_vec(&outcomes).unwrap());
            env.remove("GIT_TRACE2_EVENT");
            let starts = std::fs::read_to_string(trace).unwrap().lines().filter(|line| {
                let event: serde_json::Value = serde_json::from_str(line).unwrap();
                event["event"] == "start" && event["argv"][1] == "blame"
            }).count();
            blame_counts.push(starts);
            let reads = EDGE_READS.load(Ordering::Relaxed);
            eprintln!("PREVIEW_SCALING members={members} shared={shared} edges={edges} edge_reads={reads}");
            assert!(!shared || reads <= edges, "{reads} edge reads for {edges} edges");
        }
        assert_eq!(results[0], results[1]);
        assert_eq!(results[0], results[2]);
        eprintln!("PREVIEW_SCALING members={members} uncached_blame={} cached_blame={}", blame_counts[1], blame_counts[2]);
        assert!(blame_counts[2] > 0, "edge proof must actually execute");
        assert!(blame_counts[2] <= 2 * members, "{blame_counts:?}");
    }
}

/// cas-0e29e: the v4.29 cut preview spent its time in `advance`, which
/// re-tokenized every (owned line, added line) pair of a changed hunk for each
/// of the hunk's owners: O(owners x added) tokenizations per edge. A later
/// commit replacing all of a delivery's lines must tokenize each hunk line once.
#[test]
fn ownership_walk_tokenizes_each_hunk_line_once_cas_0e29e() {
    const LINES: usize = 200;
    let env = crate::test_support::TestEnvGuard::temp_home();
    let repo = env.home().join("tokenize");
    std::fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    };
    let write = |body: &dyn Fn(usize) -> String| {
        let text: String = (0..LINES).map(|i| body(i) + "\n").collect();
        std::fs::write(repo.join("owned.rs"), text).unwrap();
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "Fixture"]);
    git(&["config", "user.email", "fixture@example.test"]);
    write(&|i| format!("let value_{i} = base({i});"));
    git(&["add", "owned.rs"]);
    git(&["commit", "-qm", "base"]);
    let parent = git(&["rev-parse", "HEAD"]);
    write(&|i| format!("let value_{i} = delivered_{i}(input);"));
    git(&["commit", "-qam", "fix(cas-0e29e): deliver"]);
    let delivery = git(&["rev-parse", "HEAD"]);
    write(&|i| format!("let value_{i} = replaced_{i}(input, extra);"));
    git(&["commit", "-qam", "later rewrite of every delivered line"]);
    TOKENIZED_LINES.with(|count| count.set(0));
    let presence = line_content_presence(&repo, &parent, &delivery, "HEAD", "owned.rs").unwrap();
    assert!(presence.is_some());
    let tokenized = TOKENIZED_LINES.with(std::cell::Cell::get);
    eprintln!("TOKENIZATION lines={LINES} operations={tokenized}");
    assert!(tokenized > 0, "the ownership walk must run");
    assert!(tokenized <= 2 * LINES, "tokenized {tokenized} lines for one {LINES}-line hunk");
}
