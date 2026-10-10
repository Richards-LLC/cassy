use super::*;

use std::process::Command;

const BRANCH: &str = "epic/release-latency-cas-baa3";

fn red_at(tip: &str, merge: Option<&str>) -> RedTip {
    RedTip {
        tip: tip.to_string(),
        first_red_merge: merge.map(str::to_string),
        first_red_subject: merge.map(|_| "Merge factory/w-cas-1111".to_string()),
        failing: vec!["cas::lib store::tests::breaks".to_string()],
        detail: "1 test failed".to_string(),
        since: "2026-10-10T00:00:00Z".to_string(),
        bisect: Some(BisectCost { runs: 3, secs: 42 }),
        overrides: Vec::new(),
    }
}

#[test]
fn cas_6f48_red_tip_refuses_then_green_clears() {
    let dir = tempfile::tempdir().unwrap();
    let cas = dir.path();
    assert_eq!(
        merge_refusal(cas, BRANCH),
        None,
        "unknown epic never blocks"
    );

    record_green(cas, "cas-baa3", BRANCH, "aaaaaaaaaaaa").unwrap();
    assert_eq!(merge_refusal(cas, BRANCH), None);
    assert!(epic_status_line(cas, BRANCH).contains("green at aaaaaaaaa"));
    assert_eq!(status_section(cas), "");

    record_red(
        cas,
        "cas-baa3",
        BRANCH,
        red_at("cccccccccccc", Some("bbbbbbbbbbbb")),
    )
    .unwrap();
    let refusal = merge_refusal(cas, BRANCH).expect("a red epic refuses merges");
    assert!(refusal.contains("EPIC GATE RED"), "{refusal}");
    assert!(refusal.contains("first red merge bbbbbbbbb"), "{refusal}");
    assert!(refusal.contains("Merge factory/w-cas-1111"), "{refusal}");
    assert!(refusal.contains("store::tests::breaks"), "{refusal}");
    assert!(refusal.contains("Last green tip: aaaaaaaaa"), "{refusal}");
    assert!(refusal.contains("supervisor_override"), "{refusal}");
    assert_eq!(merge_refusal(cas, "epic/other-cas-0000"), None);

    let section = status_section(cas);
    assert!(section.contains("Integration gate RED"), "{section}");
    assert!(section.contains(BRANCH), "{section}");
    assert!(section.contains("bbbbbbbbb"), "{section}");
    let line = epic_status_line(cas, BRANCH);
    assert!(line.contains("RED since"), "{line}");
    assert!(line.contains("bbbbbbbbb"), "{line}");

    let gate = load(cas);
    let entry = &gate.epics[BRANCH];
    assert_eq!(entry.last_green_tip.as_deref(), Some("aaaaaaaaaaaa"));
    assert_eq!(entry.last_checked_tip.as_deref(), Some("cccccccccccc"));
    assert_eq!(
        entry.red.as_ref().unwrap().bisect,
        Some(BisectCost { runs: 3, secs: 42 })
    );

    record_green(cas, "cas-baa3", BRANCH, "dddddddddddd").unwrap();
    assert_eq!(
        merge_refusal(cas, BRANCH),
        None,
        "a green run clears the gate"
    );
    assert_eq!(status_section(cas), "");
    assert!(epic_status_line(cas, BRANCH).contains("green at ddddddddd"));
}

#[test]
fn cas_6f48_later_red_run_keeps_first_culprit_and_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let cas = dir.path();
    record_red(
        cas,
        "cas-baa3",
        BRANCH,
        red_at("c1c1c1c1c1c1", Some("b1b1b1b1b1b1")),
    )
    .unwrap();
    record_override(cas, BRANCH, "bright-lark-8: merging the fix").unwrap();
    record_red(
        cas,
        "cas-baa3",
        BRANCH,
        red_at("c2c2c2c2c2c2", Some("b2b2b2b2b2b2")),
    )
    .unwrap();

    let gate = load(cas);
    let red = gate.epics[BRANCH].red.clone().unwrap();
    assert_eq!(red.first_red_merge.as_deref(), Some("b1b1b1b1b1b1"));
    assert_eq!(red.tip, "c2c2c2c2c2c2");
    assert_eq!(
        red.overrides,
        vec!["bright-lark-8: merging the fix".to_string()]
    );

    record_override(cas, "epic/not-red-cas-0000", "ignored").unwrap();
    assert!(!load(cas).epics.contains_key("epic/not-red-cas-0000"));
}

#[test]
fn cas_6f48_corrupt_gate_file_never_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(GATE_FILE);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"{not json").unwrap();
    assert_eq!(load(dir.path()), GateFile::default());
    assert_eq!(merge_refusal(dir.path(), BRANCH), None);
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// An epic with six worker merges; merge `broken_at` (1-based) adds `BROKEN`.
fn epic_fixture(repo: &Path, merges: usize, broken_at: usize) -> (String, Vec<String>) {
    git(repo, &["init", "-q"]);
    std::fs::write(repo.join("README"), "base\n").unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-qm", "base"]);
    let green = git(repo, &["rev-parse", "HEAD"]);
    let mut merge_commits = Vec::new();
    for index in 1..=merges {
        let worker = format!("factory/w-cas-{index}");
        git(repo, &["switch", "-qc", &worker]);
        let file = if index == broken_at {
            "BROKEN".to_string()
        } else {
            format!("f{index}")
        };
        std::fs::write(repo.join(&file), format!("{index}\n")).unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", &format!("work {index}")]);
        git(repo, &["switch", "-q", "main"]);
        git(
            repo,
            &[
                "merge",
                "-q",
                "--no-ff",
                "-m",
                &format!("Merge {worker}"),
                &worker,
            ],
        );
        merge_commits.push(git(repo, &["rev-parse", "HEAD"]));
    }
    (green, merge_commits)
}

fn passes(repo: &Path, commit: &str) -> bool {
    !Command::new("git")
        .args(["cat-file", "-e", &format!("{commit}:BROKEN")])
        .current_dir(repo)
        .status()
        .unwrap()
        .success()
}

#[test]
fn cas_6f48_bisect_names_the_first_red_merge_with_log_runs() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    let (green, merges) = epic_fixture(repo, 6, 3);
    let tip = merges.last().unwrap().clone();

    let candidates = bisect_candidates(repo, Some(&green), &tip).unwrap();
    assert_eq!(
        candidates, merges,
        "first-parent merges since green, oldest first"
    );

    let mut probed = Vec::new();
    let culprit = first_red_merge(repo, Some(&green), &tip, &mut |commit: &str| {
        probed.push(commit.to_string());
        Ok(passes(repo, commit))
    })
    .unwrap()
    .expect("the epic alone fails");
    assert_eq!(culprit.commit, merges[2], "merge 3 introduced the failure");
    assert!(culprit.isolated);
    assert_eq!(culprit.runs, probed.len());
    assert!(
        culprit.runs <= 4,
        "1 tip check + ceil(log2 6) probes, got {}",
        culprit.runs
    );
    assert_eq!(
        commit_subject(repo, &culprit.commit).as_deref(),
        Some("Merge factory/w-cas-3")
    );
}

#[test]
fn cas_6f48_bisect_without_green_and_union_only_failure() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    let (_green, merges) = epic_fixture(repo, 4, 1);
    let tip = merges.last().unwrap().clone();

    // No green tip: the window is the tip's own first-parent history.
    let culprit = first_red_merge(repo, None, &tip, &mut |commit: &str| {
        Ok(passes(repo, commit))
    })
    .unwrap()
    .unwrap();
    assert_eq!(culprit.commit, merges[0]);

    // The tests pass on the epic alone: the failure exists only in the union.
    let mut runs = 0;
    let none = first_red_merge(repo, None, &tip, &mut |_commit: &str| {
        runs += 1;
        Ok(true)
    })
    .unwrap();
    assert_eq!(none, None);
    assert_eq!(runs, 1, "only the tip is probed");
}
