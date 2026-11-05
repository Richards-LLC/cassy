//! Factory CI waste guards (cas-fee3, gabber CI audit cassy#1513).
//!
//! Gabber spent 43% of 1,000 CI runs re-running a ready epic→staging PR on
//! every supervisor merge, and 15% on runs the next merge cancelled. Two small
//! guards follow: an epic PR opens as a draft, so the release PR is the one
//! CI gate, and a supervisor merging into the same branch many times in a
//! short window is told to batch.

use std::path::Path;

/// Merges into one branch inside [`BATCH_WINDOW_MINUTES`] that earn a
/// batching notice.
pub(crate) const BATCH_MERGE_THRESHOLD: usize = 3;
pub(crate) const BATCH_WINDOW_MINUTES: u32 = 30;

/// The refusal for `gh pr create` from an `epic/*` head without `--draft`.
pub(crate) fn epic_pr_draft_refusal(
    _command: &str,
    _current_branch: impl Fn() -> Option<String>,
) -> Option<String> {
    None
}

/// A batching notice when `target` has received [`BATCH_MERGE_THRESHOLD`]
/// or more merges in the last [`BATCH_WINDOW_MINUTES`] minutes.
pub(crate) fn merge_batching_notice(_repo: &Path, _target: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// gabber: the long-lived epic→staging PR reran the full battery on
    /// each of 53 merges. A ready epic PR is refused; a draft is not.
    #[test]
    fn an_epic_pr_opens_as_a_draft_cas_fee3() {
        let none = || None;
        let refused = epic_pr_draft_refusal(
            "gh pr create --base staging --head epic/rebuild-cas-0ab0 --title Rebuild",
            none,
        )
        .expect("a ready epic PR is refused");
        assert!(refused.contains("--draft"), "{refused}");
        assert!(
            epic_pr_draft_refusal("gh pr create --base staging --head epic/rebuild-cas-0ab0 --draft", none).is_none()
        );
        assert!(
            epic_pr_draft_refusal("gh pr create --base staging --title Rebuild", || Some("epic/rebuild-cas-0ab0".into()))
                .is_some(),
            "a PR from a checked-out epic branch is an epic PR"
        );
        assert!(
            epic_pr_draft_refusal("gh pr create --base main --head factory/w-cas-1", none).is_none(),
            "a delivery PR is not an epic PR"
        );
    }

    /// gabber: 571 run-min were cancelled by the next merge. The third merge
    /// into one branch within 30 minutes earns a batching notice.
    #[test]
    fn repeated_merges_into_one_branch_earn_a_batching_notice_cas_fee3() {
        let temp = tempfile::TempDir::new().unwrap();
        let repo = temp.path();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .current_dir(repo)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q", "-b", "epic/x"]);
        git(&["commit", "-q", "--allow-empty", "-m", "base"]);
        let merge = |n: usize| {
            git(&["checkout", "-q", "-b", &format!("w{n}")]);
            git(&["commit", "-q", "--allow-empty", "-m", &format!("work {n}")]);
            git(&["checkout", "-q", "epic/x"]);
            git(&["merge", "-q", "--no-ff", "-m", &format!("merge {n}"), &format!("w{n}")]);
        };
        merge(1);
        merge(2);
        assert!(merge_batching_notice(repo, "epic/x").is_none(), "two merges are routine");
        merge(3);
        let notice = merge_batching_notice(repo, "epic/x").expect("third merge earns a notice");
        assert!(notice.contains("3 merges") && notice.contains("epic/x"), "{notice}");
    }
}
