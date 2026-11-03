use super::*;
use serde_json::json;

fn pulse_card() -> FastLaneConfig {
    FastLaneConfig {
        enabled: true,
        paths: vec!["apps/frontend/app/src/**".into(), "apps/frontend/locales/**".into()],
        deny: vec!["apps/frontend/app/src/middleware/**".into(), "**/*auth*".into()],
        max_files: 3,
        max_lines: 300,
        merge_checks: vec!["PR gate".into()],
        receipt_checks: vec!["Hermetic e2e".into()],
        receipt_statuses: vec!["Vercel".into()],
        receipt_window_mins: 30,
    }
}

fn task(kind: TaskType, labels: &[&str]) -> Task {
    let mut task = Task::new("cas-f1x0".into(), "Fix the grid".into());
    task.task_type = kind;
    task.labels = labels.iter().map(|label| label.to_string()).collect();
    task
}

fn changes(paths: &[&str]) -> Vec<Change> {
    paths.iter().map(|path| Change { path: path.to_string(), lines: 40 }).collect()
}

#[test]
fn eligibility_admits_a_small_frontend_hotfix_and_names_each_refusal() {
    let lane = pulse_card();
    let page = "apps/frontend/app/src/pages/admin/creator.vue";
    let refused = |task: &Task, paths: &[&str]| match evaluate(&lane, task, &changes(paths)) {
        Eligibility::Refused(reason) => reason,
        other => panic!("expected a refusal for {paths:?}, got {other:?}"),
    };
    let bug = task(TaskType::Bug, &[]);
    assert_eq!(
        evaluate(&lane, &bug, &changes(&[page, "apps/frontend/locales/en.json"])),
        Eligibility::Eligible { files: 2, lines: 80 }
    );
    assert_eq!(
        evaluate(&lane, &task(TaskType::Feature, &[HOTFIX_LABEL]), &changes(&[page])),
        Eligibility::Eligible { files: 1, lines: 40 }
    );
    assert_eq!(
        refused(&bug, &[page, "apps/frontend/server/api/creators.ts"]),
        "apps/frontend/server/api/creators.ts is outside fast_lane.paths"
    );
    assert_eq!(
        refused(&bug, &["apps/frontend/app/src/composables/oauth.ts"]),
        "apps/frontend/app/src/composables/oauth.ts matches fast_lane.deny"
    );
    assert_eq!(refused(&bug, &[page, page, page, page]), "4 files exceed the 3-file cap");
    assert!(refused(&task(TaskType::Feature, &[]), &[page]).contains("plan or phase work"));
    assert!(refused(&task(TaskType::Epic, &[HOTFIX_LABEL]), &[page]).contains("plan or phase work"));
}

fn check(name: &str, state: State) -> Check {
    Check { name: name.into(), state, url: Some(format!("https://ci/{name}")), run_id: Some("41".into()) }
}

fn observed(e2e: Option<State>, vercel: Option<State>) -> Observed {
    Observed {
        checks: e2e.map(|state| check("Hermetic e2e", state)).into_iter().collect(),
        statuses: vercel.map(|state| check("Vercel", state)).into_iter().collect(),
    }
}

#[test]
fn receipt_waits_reruns_once_and_reverts_only_an_attributable_failure() {
    use State::*;
    let lane = pulse_card();
    let never = |_: &str| false;
    let once = |_: &str| true;
    let red = observed(Some(Failure), Some(Success));
    let receipt_for = |merge: &Observed, parent: Option<&Observed>, reran: &dyn Fn(&str) -> bool| {
        receipt(&lane, merge, parent, reran)
    };
    assert_eq!(
        receipt_for(&observed(Some(Pending), None), None, &never),
        Receipt::Pending("Hermetic e2e is running; Vercel has not reported".into())
    );
    assert_eq!(receipt_for(&red, None, &never), Receipt::Rerun(check("Hermetic e2e", Failure)));
    assert_eq!(
        receipt_for(&red, Some(&observed(Some(Success), None)), &once),
        Receipt::Revert(check("Hermetic e2e", Failure))
    );
    assert_eq!(
        receipt_for(&red, Some(&observed(Some(Failure), None)), &once),
        Receipt::Passed { excused: vec![check("Hermetic e2e", Failure)] }
    );
    assert_eq!(
        receipt_for(&observed(Some(Success), Some(Failure)), None, &never),
        Receipt::Return(check("Vercel", Failure))
    );
    assert_eq!(
        receipt_for(&observed(Some(Success), Some(Success)), None, &never),
        Receipt::Passed { excused: vec![] }
    );

    let merge = "d".repeat(40);
    let at = DateTime::parse_from_rfc3339("2026-11-03T21:05:32Z").unwrap().with_timezone(&Utc);
    let notes = format!(
        "[2026-11-03 21:05] {}\n\n[2026-11-03 21:20] {}",
        merge_note("abc123", &merge, at),
        rerun_note(&merge, &check("Hermetic e2e", Failure))
    );
    assert_eq!(merged_record(&notes, &merge), Some(MergeRecord { head: "abc123".into(), at }));
    assert_eq!(merged_record(&notes, &"e".repeat(40)), None);
    assert!(reran(&notes, &merge, "Hermetic e2e"));
    assert!(!reran(&notes, &merge, "Frontend fresh build"));
}

#[test]
fn merge_gate_judges_only_the_merge_checks_newest_run_first() {
    let lane = pulse_card();
    let run = |id: u64, name: &str, status: &str, conclusion: Option<&str>| json!({
        "id": id, "name": name, "status": status, "conclusion": conclusion,
        "html_url": format!("https://github.com/o/r/actions/runs/9{id}/job/1"),
    });
    let gate = |runs: Vec<serde_json::Value>| merge_gate(&lane, &check_runs(&json!({ "check_runs": runs })));
    assert_eq!(
        gate(vec![
            run(1, "PR gate", "completed", Some("failure")),
            run(2, "PR gate", "completed", Some("success")),
            run(3, "PostgreSQL contracts", "completed", Some("failure")),
            run(4, "Hermetic e2e", "in_progress", None),
        ]),
        Gate::Green(vec!["PR gate".into()])
    );
    assert_eq!(
        gate(vec![run(5, "PR gate", "completed", Some("failure"))]),
        Gate::Red { name: "PR gate".into(), url: Some("https://github.com/o/r/actions/runs/95/job/1".into()) }
    );
    assert_eq!(gate(vec![]), Gate::Pending("PR gate has not reported".into()));
}

#[test]
fn an_attributable_failure_opens_a_revert_pr_for_the_merge() {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    let repo = dir.path().join("work");
    let run = |cwd: &Path, args: &[&str]| {
        let out = Command::new("git")
            .current_dir(cwd)
            .args(["-c", "user.name=QA", "-c", "user.email=qa@example.test"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    run(dir.path(), &["init", "-q", "--bare", "-b", "main", "origin.git"]);
    run(dir.path(), &["clone", "-q", origin.to_str().unwrap(), "work"]);
    run(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    run(&repo, &["config", "user.name", "QA"]);
    run(&repo, &["config", "user.email", "qa@example.test"]);
    let page = repo.join("apps/frontend/app/src/pages/creator.vue");
    std::fs::create_dir_all(page.parent().unwrap()).unwrap();
    std::fs::write(&page, "<div class=\"grid\"/>\n").unwrap();
    run(&repo, &["add", "."]);
    run(&repo, &["commit", "-qm", "base"]);
    run(&repo, &["push", "-q", "origin", "HEAD:main"]);
    run(&repo, &["checkout", "-qb", "factory/fix"]);
    std::fs::write(&page, "<div class=\"grid grid-cols-2\"/>\n").unwrap();
    run(&repo, &["commit", "-qam", "fix grid"]);
    let head = run(&repo, &["rev-parse", "HEAD"]);
    assert_eq!(
        evaluate_delivery(&pulse_card(), &task(TaskType::Bug, &[]), &repo, "main", &head),
        Eligibility::Eligible { files: 1, lines: 2 }
    );
    run(&repo, &["checkout", "-q", "main"]);
    run(&repo, &["merge", "-q", "--no-ff", "-m", "merge fix", "factory/fix"]);
    run(&repo, &["push", "-q", "origin", "main"]);
    let merge = run(&repo, &["rev-parse", "HEAD"]);

    let args = dir.path().join("gh-args");
    let gh = dir.path().join("gh");
    std::fs::write(
        &gh,
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\necho https://github.com/o/r/pull/614\n", args.display()),
    )
    .unwrap();
    std::os::unix::fs::PermissionsExt::set_mode(&mut std::fs::metadata(&gh).unwrap().permissions(), 0o755);
    std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    let url = open_revert_pr(&repo, "main", &merge, "cas-f1x0", &check("Hermetic e2e", State::Failure), &gh)
        .unwrap();
    assert_eq!(url, "https://github.com/o/r/pull/614");
    let reverted = run(&origin, &["rev-parse", "refs/heads/fast-lane-revert/cas-f1x0^{tree}"]);
    assert_eq!(reverted, run(&origin, &[ "rev-parse", &format!("{merge}^1^{{tree}}")]));
    let sent = std::fs::read_to_string(&args).unwrap();
    assert!(
        sent.starts_with("pr\ncreate\n--base\nmain\n--head\nfast-lane-revert/cas-f1x0\n"),
        "{sent}"
    );
    assert!(sent.contains("https://ci/Hermetic e2e"), "{sent}");
    assert!(run(&repo, &["worktree", "list"]).lines().count() == 1, "scratch worktree removed");
}
