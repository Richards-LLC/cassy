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
/// The head is `--head`/`-H`, else the checked-out branch.
pub(crate) fn epic_pr_draft_refusal(
    command: &str,
    current_branch: impl Fn() -> Option<String>,
) -> Option<String> {
    for statement in command.split(['\n', ';', '|', '&']) {
        let tokens: Vec<&str> = statement
            .split_whitespace()
            .map(|token| token.trim_matches(|ch: char| matches!(ch, '\'' | '"' | '`')))
            .filter(|token| !token.is_empty())
            .collect();
        let Some(gh) = tokens.iter().position(|token| *token == "gh" || token.ends_with("/gh")) else {
            continue;
        };
        if tokens.get(gh + 1) != Some(&"pr") || tokens.get(gh + 2) != Some(&"create") {
            continue;
        }
        let args = &tokens[gh + 3..];
        if args.iter().any(|arg| matches!(*arg, "--draft" | "-d")) {
            continue;
        }
        let head = args
            .iter()
            .enumerate()
            .find_map(|(index, arg)| {
                arg.strip_prefix("--head=")
                    .map(str::to_string)
                    .or_else(|| matches!(*arg, "--head" | "-H").then(|| args.get(index + 1).map(|v| v.to_string())).flatten())
            })
            .or_else(&current_branch);
        let Some(head) = head.filter(|head| head.rsplit(':').next().is_some_and(|branch| branch.starts_with("epic/")))
        else {
            continue;
        };
        return Some(format!(
            "🚫 EPIC PR NOT DRAFT: open the {head} PR with `--draft`. Every merge into a ready epic PR reruns its full CI \
             and cancels the last run (gabber: 43% of all CI). The release PR is the gate: mark it ready with \
             `gh pr ready` only then. Note GitHub still runs `pull_request` workflows on drafts unless the \
             workflow skips them (`if: github.event.pull_request.draft == false`) and lists \
             `ready_for_review` in its `pull_request` types, so the checks run when the PR is marked ready \
             instead of the skipped draft run counting as a pass."
        ));
    }
    None
}

/// A batching notice when `target` has received [`BATCH_MERGE_THRESHOLD`]
/// or more merges in the last [`BATCH_WINDOW_MINUTES`] minutes.
pub(crate) fn merge_batching_notice(repo: &Path, target: &str) -> Option<String> {
    let since = format!("--since={BATCH_WINDOW_MINUTES} minutes ago");
    let output = std::process::Command::new("git")
        .args(["log", "--first-parent", "--merges", &since, "--format=%H", target, "--"])
        .current_dir(repo)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let merges = String::from_utf8_lossy(&output.stdout).lines().filter(|line| !line.is_empty()).count();
    (merges >= BATCH_MERGE_THRESHOLD).then(|| {
        format!(
            "⏱️ Batch: {merges} merges into {target} in the last {BATCH_WINDOW_MINUTES} min. Each push to a branch \
             with an open PR reruns its CI and cancels the previous run; merge ready deliveries together."
        )
    })
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
