//! Same-file overlap reporting survives removal of peer merge fan-out.
use super::*;
use std::process::Command;

const TARGET: &str = "epic/burn-down";

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
}

fn branch_with(repo: &Path, branch: &str, files: &[&str]) {
    git(repo, &["checkout", "-q", "-b", branch, TARGET]);
    for file in files {
        std::fs::write(repo.join(file), format!("{branch} {file}\n")).unwrap();
        git(repo, &["add", file]);
    }
    git(repo, &["commit", "-q", "-m", &format!("work on {branch}")]);
    git(repo, &["checkout", "-q", TARGET]);
}

#[test]
fn cas_a131_overlap_warns_on_shared_files_only_and_merged_branches_drop_out() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", TARGET]);
    std::fs::write(repo.join("seed.rs"), "seed\n").unwrap();
    git(repo, &["add", "seed.rs"]);
    git(repo, &["commit", "-q", "-m", "seed"]);
    branch_with(repo, "factory/mine", &["a.rs", "shared.vue"]);
    branch_with(repo, "factory/theirs", &["shared.vue", "c.rs"]);
    branch_with(repo, "factory/disjoint", &["d.rs"]);
    branch_with(repo, "factory/merged", &["a.rs"]);
    git(repo, &["merge", "-q", "--no-ff", "factory/merged", "-m", "merged"]);

    assert_eq!(
        changed_files(repo, TARGET, "factory/mine"),
        Some(vec!["a.rs".to_string(), "shared.vue".to_string()])
    );
    assert_eq!(changed_files(repo, TARGET, "factory/merged"), None, "merged drops out");
    assert_eq!(changed_files(repo, TARGET, "factory/missing"), None);

    let own = [InFlight { branch: "factory/mine".into(), target: TARGET.into() }];
    let flight = |branch: &str, target: &str| InFlight { branch: branch.into(), target: target.into() };
    let peers = vec![
        ("happy-owl@prowl (teammate bob@example.com)".to_string(), flight("factory/theirs", TARGET)),
        ("happy-owl@prowl (teammate bob@example.com)".to_string(), flight("factory/disjoint", TARGET)),
        ("happy-owl@prowl (teammate bob@example.com)".to_string(), flight("factory/merged", TARGET)),
        // Same files, different target: not this merge's concern.
        ("calm-fox@box (you)".to_string(), flight("factory/theirs", "main")),
    ];
    let overlaps = find_overlaps(repo, &own, &peers, Instant::now() + OVERLAP_BUDGET);
    assert_eq!(
        overlaps,
        [BranchOverlap {
            target: TARGET.into(),
            own_branch: "factory/mine".into(),
            peer: "happy-owl@prowl (teammate bob@example.com)".into(),
            peer_branch: "factory/theirs".into(),
            files: vec!["shared.vue".into()],
        }]
    );
    let lines = render_overlaps(&overlaps);
    let text = lines.join("\n");
    assert!(text.contains("shared.vue"), "{text}");
    assert!(text.contains("factory/mine") && text.contains("factory/theirs"), "{text}");
    assert!(text.contains("happy-owl@prowl"), "{text}");
    for line in &lines {
        assert!(line.chars().count() <= 80, "wider than 80: {line}");
    }
    // An exhausted budget reports nothing rather than blocking.
    assert!(find_overlaps(repo, &own, &peers, Instant::now()).is_empty());
}
