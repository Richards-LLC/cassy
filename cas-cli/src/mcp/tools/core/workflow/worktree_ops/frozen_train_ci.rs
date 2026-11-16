//! A frozen train's full set owns Rust proof; retain its lane admission rows.
use super::*;

pub(super) fn lookup(root: &Path, branch: &str, target: &str, cwd: &Path) -> BranchCiLookup {
    let _ = root;
    lookup_branch_ci(branch, target, cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn frozen_train_admission_retains_no_build_gates_cas_db43() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let (repo, _, tip) = super::super::tests::delivery_ci_fixture_with(true, "", "");
        env.set(crate::github_issue_attach::GH_BIN_ENV, repo.path().join("gh-fixture"));
        let root = crate::store::init_cas_dir(repo.path()).unwrap();
        let store = crate::store::open_task_store(&root).unwrap();
        let target = "epic/release-train-fixture";
        let mut train = cas_types::Task::new("cas-tr".into(), "Release train".into());
        train.task_type = cas_types::TaskType::Epic;
        train.branch = Some(target.into());
        train.labels = vec!["release-train".into()];
        store.add(&train).unwrap();
        let git = crate::worktree::GitOperations::new(repo.path().to_path_buf());
        git.create_branch_from(target, "main").unwrap();
        let run = |name: &str, status: &str, conclusion: Option<&str>| serde_json::json!({
            "id": 1, "name": name, "head_sha": tip, "status": status, "conclusion": conclusion,
            "html_url": "https://github.com/acme/cas/actions/runs/100/job/1"
        });
        let base = vec![
            run("Classify CI diff", "completed", Some("success")),
            run("Scoped Validation (fast)", "completed", Some("success")),
            run("Scoped Validation (factory/PR)", "completed", Some("success")),
            run("Scoped Validation — impact own tests", "in_progress", None),
            run("Scoped Validation — impact ring 1/3", "queued", None),
        ];
        for (case, frozen, changed, expected) in [
            ("unfrozen", false, None, "pending"),
            ("frozen", true, None, "green"),
            ("fast red", true, Some((1, "failure")), "red"),
            ("factory pending", true, Some((2, "pending")), "pending"),
            ("factory skipped", true, Some((2, "skipped")), "pending"),
            ("factory missing", true, Some((2, "missing")), "pending"),
            ("lifted", false, None, "pending"),
        ] {
            train.labels = vec!["release-train".into()];
            if frozen { train.labels.push(format!("release-train-frozen:1:{tip}")); }
            store.update(&train).unwrap();
            let mut rows = base.clone();
            match changed {
                Some((index, "missing")) => { rows.remove(index); }
                Some((index, "pending")) => {
                    rows[index]["status"] = "in_progress".into();
                    rows[index]["conclusion"] = serde_json::Value::Null;
                }
                Some((index, conclusion)) => { rows[index]["conclusion"] = conclusion.into(); }
                None => {}
            }
            let response = serde_json::json!({"check_runs": rows}).to_string();
            crate::test_paths::warm_stub(&repo.path().join("gh-fixture"), &format!(
                "#!/bin/sh\nif [ \"$1 $2\" = 'repo view' ]; then echo '{{\"nameWithOwner\":\"acme/cas\"}}'; exit 0; fi\ncat <<'JSON'\n{response}\nJSON\n"
            ));
            let state = lookup(&root, &tip, target, repo.path()).state;
            match expected {
                "green" => {
                    assert!(matches!(state, BranchCiState::Green { .. }), "{case}: {state:?}");
                    assert_eq!(admit_branch_ci(&state, false, None), Ok(false));
                }
                "red" => {
                    assert!(matches!(state, BranchCiState::Red { .. }), "{case}: {state:?}");
                    assert!(admit_branch_ci(&state, false, None).is_err());
                }
                _ => {
                    assert!(matches!(state, BranchCiState::CodePending { .. }), "{case}: {state:?}");
                    assert!(admit_branch_ci(&state, false, None).is_err());
                }
            }
        }
    }
}
