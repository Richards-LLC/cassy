use super::*;
use std::process::Command;

const YAML: &str = "version: 1\nupdated: '2026-10-21'\nsurfaces: ['src/pages/**']\ninvariants:\n  INV: {statement: Arrived, check: Read state}\njourneys:\n  - id: signup\n    status: live\n    seeded: true\n    entry: {url: /signup, trigger: Open}\n    owners: ['src/pages/signup.ts']\n";

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "upkeep fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
        .env("GIT_COMMITTER_NAME", "upkeep fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().into()
}

fn commit(repo: &Path, files: &[(&str, &str)]) -> String {
    for (path, body) in files {
        let path = repo.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    git(repo, &["add", "-A"]);
    git(repo, &["-c", "commit.gpgsign=false", "commit", "-qm", "fixture"]);
    git(repo, &["rev-parse", "HEAD"])
}

/// G-49 moved from park to the lane lint: the same diff-scoped G1/G2 findings,
/// computed from base and head snapshots with no task.
#[test]
fn lane_upkeep_refuses_unowned_surface_and_registry_breakage_cas_4b33() {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q", "-b", "main"]);
    let base = commit(repo.path(), &[
        ("src/pages/signup.ts", "signup"),
        (crate::qa_registry::DEFAULT_PATH, YAML),
    ]);
    let qa = QaConfig::default();

    let unowned = commit(repo.path(), &[("src/pages/email.ts", "new entry")]);
    let upkeep = for_diff(repo.path(), &qa, &base, &unowned).unwrap();
    assert!(upkeep.refuses(), "{upkeep:?}");
    assert_eq!(upkeep.findings.iter().map(|f| f.gate).collect::<Vec<_>>(), ["G2"]);

    let owned = commit(repo.path(), &[(
        crate::qa_registry::DEFAULT_PATH,
        &format!("{YAML}ignore: ['src/pages/email.ts']\n"),
    )]);
    assert!(!for_diff(repo.path(), &qa, &base, &owned).unwrap().refuses());

    let broken = commit(repo.path(), &[(
        crate::qa_registry::DEFAULT_PATH,
        &format!("{}ignore: ['src/pages/email.ts']\n", YAML.replace("invariants:", "bug_families:\n  ghost: {title: Ghost, journeys: [missing]}\ninvariants:")),
    )]);
    let upkeep = for_diff(repo.path(), &qa, &base, &broken).unwrap();
    assert!(upkeep.refuses(), "{upkeep:?}");
    assert_eq!(upkeep.findings.iter().map(|f| f.gate).collect::<Vec<_>>(), ["G1"]);
}
