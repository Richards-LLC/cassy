//! A frozen train's full set owns Rust proof; retain its lane admission rows.
use super::*;

const NO_BUILD_CHECKS: [&str; 2] = ["Scoped Validation (fast)", "Scoped Validation (factory/PR)"];

fn active_target(root: &Path, target: &str, cwd: &Path) -> bool {
    let Ok(store) = crate::store::open_task_store(root) else {
        return false;
    };
    let Ok(tasks) = store.list(None) else {
        return false;
    };
    let Some((_, freeze)) = super::super::release_freeze::frozen_target(root, &tasks, target)
    else {
        return false;
    };
    crate::worktree::GitOperations::new(cwd.to_path_buf()).is_ancestor(&freeze.tip, target)
}

fn impact(run: &serde_json::Value) -> bool {
    run["name"].as_str().is_some_and(|name| {
        name == "Scoped Validation — impact own tests"
            || name.starts_with("Scoped Validation — impact ring ")
    })
}

fn rows(output: &GhApiOutput, sha: &str) -> Option<Vec<serde_json::Value>> {
    if !output.success {
        return None;
    }
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let rows = response["check_runs"].as_array()?;
    if response["total_count"]
        .as_u64()
        .is_some_and(|total| total > rows.len() as u64)
        || rows.iter().any(|row| row["head_sha"].as_str() != Some(sha))
    {
        return None;
    }
    let (latest, superseded) = select_latest_check_runs(rows);
    if superseded {
        return None;
    }
    Some(latest.into_iter().cloned().collect())
}

// cas-rule: rule-176 scope: "(2) Each check runs once per change, at the cheapest point that catches its defect. Running the same check in more places is a defect."
pub(super) fn lookup(root: &Path, branch: &str, target: &str, cwd: &Path) -> BranchCiLookup {
    let frozen = || {
        if !active_target(root, target, cwd) {
            return None;
        }
        let (tip, _) = delivery_ci_commit(branch, target, cwd).ok()?;
        let binary = crate::github_repo::gh_binary();
        let repo = crate::github_repo::resolve_origin(cwd, &binary, BRANCH_CI_LOOKUP_TIMEOUT)
            .ok()?
            .canonical;
        let mut output = fetch_branch_ci(&tip, cwd, &repo, &binary);
        let rows = rows(&output, &tip)?;
        // Both real no-build jobs must have run. A skipped/missing row cannot
        // authorize dropping impact work; retain ordinary admission instead.
        if !NO_BUILD_CHECKS.iter().all(|name| {
            rows.iter().any(|row| {
                row["name"] == *name
                    && row["status"] == "completed"
                    && row["conclusion"] == "success"
            })
        }) {
            return None;
        }
        let retained: Vec<_> = rows.into_iter().filter(|row| !impact(row)).collect();
        output.stdout = serde_json::to_vec(&serde_json::json!({"check_runs": retained})).ok()?;
        let state = require_code_validation(classify_code_ci_response(branch, &tip, output));
        Some(BranchCiLookup {
            state,
            repo: Some(repo),
            repository_lookup_attempted: true,
            gh_failed: false,
        })
    };
    frozen().unwrap_or_else(|| lookup_branch_ci(branch, target, cwd))
}

/// GitHub cancels a workflow, not one job. Only cancel a published repair's
/// exact lane run when its remaining unfinished jobs are all Rust impact jobs.
// cas-rule: rule-176 scope: "(2) Each check runs once per change, at the cheapest point that catches its defect. Running the same check in more places is a defect."
pub(super) fn cancel_after_merge(
    root: &Path,
    target: &str,
    cwd: &Path,
    repo: Option<&str>,
    sha: &str,
    published: bool,
) {
    if !published || !active_target(root, target, cwd) {
        return;
    }
    let Some(repo) = repo else { return };
    let binary = crate::github_repo::gh_binary();
    let Some(rows) = rows(&fetch_branch_ci(sha, cwd, repo, &binary), sha) else {
        return;
    };
    let candidates: HashSet<_> = rows
        .iter()
        .filter(|row| impact(row) && check_run_unfinished(row))
        .filter_map(check_run_workflow_id)
        .collect();
    for id in candidates {
        let output = gh_api_get(&format!("repos/{repo}/actions/runs/{id}"), cwd, &binary);
        let Ok(run) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
            continue;
        };
        if !output.success
            || run["head_sha"] != sha
            || run["event"] != "push"
            || run["path"] != ".github/workflows/lane.yml"
            || !matches!(run["status"].as_str(), Some("queued" | "in_progress"))
            || !run["head_branch"]
                .as_str()
                .is_some_and(|branch| branch.starts_with("factory/"))
            || rows.iter().any(|row| {
                check_run_workflow_id(row) == Some(id) && !impact(row) && check_run_unfinished(row)
            })
        {
            continue;
        }
        let mut command = Command::new(&binary);
        command.current_dir(cwd).env("GH_HOST", "github.com").args([
            "api",
            "--method",
            "POST",
            &format!("repos/{repo}/actions/runs/{id}/cancel"),
        ]);
        let result = crate::bounded_process::run_command(
            &mut command,
            crate::bounded_process::Deadline::after(BRANCH_CI_LOOKUP_TIMEOUT),
            BRANCH_CI_LOOKUP_TIMEOUT,
        );
        if !result.is_ok_and(|output| output.status.success()) {
            tracing::warn!(%sha, run_id = id, "frozen repair lane cancellation failed; full-set proof still required");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn frozen_train_admission_retains_no_build_gates_cas_db43() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let (repo, _, tip) = super::super::tests::delivery_ci_fixture_with(true, "", "");
        env.set(
            crate::github_issue_attach::GH_BIN_ENV,
            repo.path().join("gh-fixture"),
        );
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
        let frozen_tip = git.resolve_commit("main").unwrap();
        let run = |name: &str, status: &str, conclusion: Option<&str>| {
            serde_json::json!({
                "id": 1, "name": name, "head_sha": tip, "status": status, "conclusion": conclusion,
                "html_url": "https://github.com/acme/cas/actions/runs/100/job/1"
            })
        };
        let base = vec![
            run("Classify CI diff", "completed", Some("success")),
            run("Scoped Validation (fast)", "completed", Some("success")),
            run(
                "Scoped Validation (factory/PR)",
                "completed",
                Some("success"),
            ),
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
            ("journey pending", true, Some((5, "pending")), "pending"),
            ("journey red", true, Some((5, "failure")), "red"),
            ("foreign sha", true, Some((0, "foreign")), "pending"),
            ("truncated", true, Some((0, "truncated")), "pending"),
            ("lifted", false, None, "pending"),
        ] {
            train.labels = vec!["release-train".into()];
            if frozen {
                train
                    .labels
                    .push(format!("release-train-frozen:1:{frozen_tip}"));
            }
            store.update(&train).unwrap();
            let mut rows = base.clone();
            if case.starts_with("journey") {
                rows.push(run(
                    "Scoped Validation — Commander journeys 1/1",
                    "completed",
                    Some("success"),
                ));
            }
            let mut total = rows.len();
            match changed {
                Some((index, "foreign")) => {
                    rows[index]["head_sha"] = "unrelated".into();
                }
                Some((_, "truncated")) => {
                    total = 100;
                }
                Some((index, "missing")) => {
                    rows.remove(index);
                }
                Some((index, "pending")) => {
                    rows[index]["status"] = "in_progress".into();
                    rows[index]["conclusion"] = serde_json::Value::Null;
                }
                Some((index, conclusion)) => {
                    rows[index]["conclusion"] = conclusion.into();
                }
                None => {}
            }
            let response = serde_json::json!({"check_runs": rows, "total_count": total});
            crate::test_paths::warm_stub(
                &repo.path().join("gh-fixture"),
                &format!(
                    "#!/bin/sh\nif [ \"$1 $2\" = 'repo view' ]; then printf '%s\\n' '{{\"nameWithOwner\":\"acme/cas\"}}'; exit 0; fi\ncat <<'JSON'\n{response}\nJSON\n"
                ),
            );
            let state = lookup(&root, &tip, target, repo.path()).state;
            match expected {
                "green" => {
                    assert!(
                        matches!(state, BranchCiState::Green { .. }),
                        "{case}: {state:?}"
                    );
                    assert_eq!(admit_branch_ci(&state, false, None), Ok(false));
                }
                "red" => {
                    assert!(
                        matches!(state, BranchCiState::Red { .. }),
                        "{case}: {state:?}"
                    );
                    assert!(admit_branch_ci(&state, false, None).is_err());
                }
                _ => {
                    assert!(
                        matches!(state, BranchCiState::CodePending { .. }),
                        "{case}: {state:?}"
                    );
                    assert!(admit_branch_ci(&state, false, None).is_err());
                }
            }
        }
        train
            .labels
            .push(format!("release-train-frozen:1:{frozen_tip}"));
        store.update(&train).unwrap();
        for (case, published, mutation, cancel) in [
            ("unfrozen", true, None, false),
            ("unpublished", false, None, false),
            ("published", true, None, true),
            ("other sha", true, Some(("head_sha", "other")), false),
            (
                "other workflow",
                true,
                Some(("path", ".github/workflows/full-validation.yml")),
                false,
            ),
            (
                "other event",
                true,
                Some(("event", "workflow_dispatch")),
                false,
            ),
            ("completed", true, Some(("status", "completed")), false),
            ("journey running", true, None, false),
            ("cancel refused", true, None, true),
        ] {
            train.labels = vec!["release-train".into()];
            if case != "unfrozen" {
                train
                    .labels
                    .push(format!("release-train-frozen:1:{frozen_tip}"));
            }
            store.update(&train).unwrap();
            let mut run_info = serde_json::json!({"head_sha": tip, "event": "push",
                "path": ".github/workflows/lane.yml", "status": "in_progress", "head_branch": "factory/ci-fixture"});
            if let Some((key, value)) = mutation {
                run_info[key] = value.into();
            }
            let mut rows = base.clone();
            if case == "journey running" {
                rows.push(run(
                    "Scoped Validation — Commander journeys 1/1",
                    "in_progress",
                    None,
                ));
            }
            let response = serde_json::json!({"check_runs": rows});
            let calls = repo.path().join("cancel-calls");
            let _ = std::fs::remove_file(&calls);
            let cancel_status = if case == "cancel refused" { 1 } else { 0 };
            crate::test_paths::warm_stub(
                &repo.path().join("gh-fixture"),
                &format!(
                    "#!/bin/sh\ncase \"$4\" in\n*/check-runs) cat <<'JSON'\n{response}\nJSON\n;;\n*/actions/runs/100) cat <<'JSON'\n{run_info}\nJSON\n;;\n*/cancel) printf '%s\\n' \"$2 $3 $4\" >> cancel-calls; exit {cancel_status} ;;\n*) exit 97 ;;\nesac\n"
                ),
            );
            cancel_after_merge(
                &root,
                target,
                repo.path(),
                Some("acme/cas"),
                &tip,
                published,
            );
            assert_eq!(calls.exists(), cancel, "{case}");
            if cancel {
                assert_eq!(
                    std::fs::read_to_string(&calls).unwrap(),
                    "--method POST repos/acme/cas/actions/runs/100/cancel\n"
                );
            }
        }
    }
}
