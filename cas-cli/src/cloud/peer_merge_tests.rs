//! cas-a131: the "about to merge" note and the same-file overlap warning.
use super::*;
use crate::cloud::coordinator::AgentInfo;
use crate::cloud::peers::{
    META_BRANCH, META_CANONICAL_ID, META_FACTORY_SESSION, META_HOSTNAME, META_OWNER, META_ROLE,
    apply_same_repo_scope, mark_own, repo_peers,
};
use crate::config::SameRepoScope;
use std::collections::HashMap;
use std::process::Command;

const REPO: &str = "github.com/acme/widgets";
const TARGET: &str = "epic/burn-down";

fn sup(id: &str, user: &str, branch: &str, session: Option<&str>, age_secs: i64) -> AgentInfo {
    let mut metadata: HashMap<String, String> = [
        (META_ROLE, "supervisor"),
        (META_CANONICAL_ID, REPO),
        (META_HOSTNAME, "box"),
        (META_BRANCH, branch),
        (META_OWNER, "someone@example.com"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    if let Some(session) = session {
        metadata.insert(META_FACTORY_SESSION.to_string(), session.to_string());
    }
    AgentInfo {
        id: id.to_string(),
        name: id.to_string(),
        agent_type: "primary".to_string(),
        status: "active".to_string(),
        pid: None,
        session_id: None,
        machine_id: None,
        last_heartbeat: (Utc::now() - chrono::Duration::seconds(age_secs)).to_rfc3339(),
        active_tasks: 0,
        metadata,
        user_id: Some(user.to_string()),
        team_id: None,
        project_id: None,
    }
}

/// `me` merges; `same` delivers into the target; `other` is on another
/// branch; `stale` and `no-daemon` cannot take the note; `teammate` is
/// another account's supervisor on the target.
fn peers() -> Vec<Peer> {
    let agents = vec![
        sup("me", "u-me", TARGET, Some("s-me"), 5),
        sup("same", "u-me", TARGET, Some("s-same"), 5),
        sup("other", "u-me", "main", Some("s-other"), 5),
        sup("stale", "u-me", TARGET, Some("s-stale"), 3_600),
        sup("no-daemon", "u-me", TARGET, None, 5),
        sup("teammate", "u-bob", TARGET, Some("s-bob"), 5),
    ];
    let mut peers = repo_peers(&agents, REPO, &[], Some("me"), Utc::now());
    mark_own(&mut peers, Some("u-me"));
    peers
}

fn intent(tip: &str) -> MergeIntent<'_> {
    MergeIntent {
        target: TARGET,
        source: "factory/lively-puma-3-cas-a131",
        task_id: Some("cas-a131"),
        tip,
        merger: "me@box",
    }
}

#[test]
fn cas_a131_note_goes_to_live_supervisors_delivering_into_the_target() {
    let peers = peers();
    let mut names: Vec<&str> = merge_intent_recipients(&peers, TARGET)
        .into_iter()
        .map(|peer| peer.name.as_str())
        .collect();
    names.sort();
    assert_eq!(names, ["same", "teammate"]);
    // peers.same_repo_scope = own: the teammate is never a recipient.
    let (own, _hidden) = apply_same_repo_scope(peers, SameRepoScope::Own);
    let names: Vec<&str> = merge_intent_recipients(&own, TARGET)
        .into_iter()
        .map(|peer| peer.name.as_str())
        .collect();
    assert_eq!(names, ["same"]);
}

#[test]
fn cas_a131_note_is_sent_once_per_tip_and_not_on_retry() {
    let dir = tempfile::tempdir().unwrap();
    let (own, _) = apply_same_repo_scope(peers(), SameRepoScope::Own);
    let mut sent: Vec<String> = Vec::new();
    let mut send = |peer: &Peer, summary: &str, body: &str| {
        // An informational note naming target, source, task, merger and tip.
        for expected in [TARGET, "factory/lively-puma-3-cas-a131", "cas-a131", "me@box", "tip-"] {
            assert!(body.contains(expected), "{expected:?} missing: {body}");
        }
        assert!(summary.contains(TARGET), "{summary}");
        assert!(body.contains("nothing waits"), "informational only: {body}");
        sent.push(peer.name.clone());
        Ok(())
    };
    assert_eq!(
        deliver_merge_intent(dir.path(), &intent("tip-1"), &own, &mut send),
        MergeIntentOutcome::Sent(vec!["same@box".into()])
    );
    // A retry of the same merge sends nothing.
    assert_eq!(
        deliver_merge_intent(dir.path(), &intent("tip-1"), &own, &mut send),
        MergeIntentOutcome::AlreadySent
    );
    // A new tip of the same branch is a new merge.
    assert_eq!(
        deliver_merge_intent(dir.path(), &intent("tip-2"), &own, &mut send),
        MergeIntentOutcome::Sent(vec!["same@box".into()])
    );
    assert_eq!(sent, ["same", "same"]);
    assert!(dir.path().join(MERGE_INTENT_LEDGER).is_file());
}

#[test]
fn cas_a131_no_note_without_another_supervisor_on_the_target() {
    let dir = tempfile::tempdir().unwrap();
    let only_elsewhere: Vec<Peer> = peers()
        .into_iter()
        .filter(|peer| peer.name == "other" || peer.name == "stale")
        .collect();
    let mut calls = 0;
    let outcome = deliver_merge_intent(dir.path(), &intent("tip-1"), &only_elsewhere, |_, _, _| {
        calls += 1;
        Ok(())
    });
    assert_eq!(outcome, MergeIntentOutcome::NoPeers);
    assert_eq!(calls, 0);
    // A failed send is reported, never raised.
    let (own, _) = apply_same_repo_scope(peers(), SameRepoScope::Own);
    let outcome = deliver_merge_intent(dir.path(), &intent("tip-9"), &own, |_, _, _| {
        Err("cloud down".into())
    });
    assert!(matches!(outcome, MergeIntentOutcome::Failed(ref errors) if errors[0].contains("cloud down")), "{outcome:?}");
}

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
