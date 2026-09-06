#[cfg(test)]
mod tests {
    use super::super::{
        check_worktree_staleness, ready_blocked_sort_options, resolve_staleness_sync_ref,
        epic_branch_name, slugify_for_branch, sort_order_label, truncate_str, truncated_list_footer,
        truncated_list_header,
    };
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn epic_branch_name_keeps_id_after_long_title_slug() {
        let title = "A deliberately long epic title whose slug must be truncated before the id suffix";

        assert_eq!(
            epic_branch_name(title, "cas-3228"),
            "epic/a-deliberately-long-epic-title-whose-slug-must-be-truncated-before-cas-3228"
        );
    }

    // ========================================================================
    // cas-06f9 (GH #104): honest truncation + priority-first default
    // ========================================================================

    /// Unspecified sort means priority order on the ready/blocked surface.
    /// `TaskSortOptions`' own default is Created, which is incidental ordering
    /// for "what should I pick up next".
    #[test]
    fn unspecified_sort_defaults_to_priority_p0_first() {
        let opts = ready_blocked_sort_options(None, None);
        assert_eq!(opts.field, cas_types::TaskSortField::Priority);
        assert_eq!(opts.effective_order(), cas_types::SortOrder::Asc);
        assert_eq!(sort_order_label(&opts), "P0 first");
    }

    /// An explicit sort from the caller still wins — this only redefines
    /// "unspecified".
    #[test]
    fn explicit_sort_is_not_overridden_by_the_default() {
        let opts = ready_blocked_sort_options(Some("created"), Some("desc"));
        assert_eq!(opts.field, cas_types::TaskSortField::Created);
        assert_eq!(sort_order_label(&opts), "newest first");

        // An order without a field keeps priority but honours the direction.
        let reversed = ready_blocked_sort_options(None, Some("desc"));
        assert_eq!(reversed.field, cas_types::TaskSortField::Priority);
        assert_eq!(sort_order_label(&reversed), "lowest priority first");
    }

    /// The header must never imply an order the rows are not in.
    #[test]
    fn every_sort_field_has_a_distinct_honest_label() {
        let cases = [
            (Some("priority"), Some("asc"), "P0 first"),
            (Some("priority"), Some("desc"), "lowest priority first"),
            (Some("created"), Some("asc"), "oldest first"),
            (Some("created"), Some("desc"), "newest first"),
            (Some("updated"), Some("asc"), "least recently updated first"),
            (Some("updated"), Some("desc"), "most recently updated first"),
            (Some("title"), Some("asc"), "title A-Z"),
            (Some("title"), Some("desc"), "title Z-A"),
        ];
        for (sort, order, expected) in cases {
            let opts = ready_blocked_sort_options(sort, order);
            assert_eq!(sort_order_label(&opts), expected, "{sort:?}/{order:?}");
        }
    }

    /// A capped list states the true total; an uncapped one does not pretend
    /// to be capped.
    #[test]
    fn header_states_the_true_total_only_when_something_is_withheld() {
        let opts = ready_blocked_sort_options(None, None);

        let truncated = truncated_list_header("Ready tasks", 30, 10, &opts);
        assert!(
            truncated.starts_with("Ready tasks (showing 10 of 30, P0 first):"),
            "{truncated}"
        );

        let complete = truncated_list_header("Ready tasks", 3, 3, &opts);
        assert!(
            complete.starts_with("Ready tasks (3, P0 first):"),
            "{complete}"
        );
        assert!(!complete.contains("showing"), "{complete}");
    }

    /// The footer names the withheld count and the exact way to see it; an
    /// uncapped list gets no footer at all.
    #[test]
    fn footer_names_what_was_withheld_and_how_to_see_it() {
        let footer = truncated_list_footer(30, 10);
        assert!(footer.contains("and 20 more not shown"), "{footer}");
        assert!(footer.contains("limit=30"), "{footer}");

        assert!(truncated_list_footer(3, 3).is_empty());
        assert!(truncated_list_footer(0, 0).is_empty());
    }

    // ========================================================================
    // Epic Branch Slugification Tests
    // ========================================================================

    #[test]
    fn test_slugify_for_branch_simple() {
        assert_eq!(slugify_for_branch("Add User Auth"), "add-user-auth");
        assert_eq!(slugify_for_branch("Simple Title"), "simple-title");
    }

    #[test]
    fn test_slugify_for_branch_special_chars() {
        assert_eq!(slugify_for_branch("Fix Bug #123"), "fix-bug-123");
        assert_eq!(slugify_for_branch("Add @feature!"), "add-feature");
        assert_eq!(
            slugify_for_branch("Special!@#$%^&*()Chars"),
            "special-chars"
        );
    }

    #[test]
    fn test_slugify_for_branch_multiple_spaces() {
        assert_eq!(slugify_for_branch("Multiple   Spaces"), "multiple-spaces");
        assert_eq!(
            slugify_for_branch("  Leading Trailing  "),
            "leading-trailing"
        );
    }

    #[test]
    fn test_slugify_for_branch_truncation() {
        // Test that long titles are truncated to 50 chars
        let long_title = "A".repeat(100);
        let result = slugify_for_branch(&long_title);
        assert_eq!(result.len(), 50);
        assert!(result.chars().all(|c| c == 'a'));
    }

    #[test]
    fn test_slugify_for_branch_preserves_numbers() {
        assert_eq!(
            slugify_for_branch("Version 2.0 Release"),
            "version-2-0-release"
        );
        assert_eq!(slugify_for_branch("Cassy v1"), "cassy-v1");
    }

    // ========================================================================
    // truncate_str Tests
    // ========================================================================

    #[test]
    fn truncate_str_handles_unicode_boundary() {
        let value = format!("{}✅ trailing", "a".repeat(99));
        assert_eq!(truncate_str(&value, 100), format!("{}...", "a".repeat(99)));
    }

    #[test]
    fn truncate_str_keeps_short_values() {
        assert_eq!(truncate_str("short", 10), "short");
    }

    // ========================================================================
    // Assignment freshness sync-ref resolution (cas-44e9)
    // ========================================================================

    #[test]
    fn resolve_staleness_preferred_wins_over_upstream_and_default() {
        let got = resolve_staleness_sync_ref(
            Some("epic/alpha"),
            "factory/worker",
            Some("origin/factory/worker"),
            "main",
        );
        assert_eq!(got, "epic/alpha");
    }

    #[test]
    fn resolve_staleness_preferred_wins_even_when_two_epics_would_exist() {
        // Regression: old code listed epic/* and took .last() — wrong under multi-epic.
        // Preferred (task parent epic A) must always win; never invent from listing.
        let got = resolve_staleness_sync_ref(
            Some("epic/a-first"),
            "factory/hv-scope",
            None,
            "main",
        );
        assert_eq!(got, "epic/a-first");
        assert_ne!(got, "epic/z-last");
    }

    #[test]
    fn resolve_staleness_factory_without_preferred_uses_default_not_epic_list() {
        // No parent epic / focus pin → base/main. Must not return an epic/* name.
        let got = resolve_staleness_sync_ref(None, "factory/worker", None, "main");
        assert_eq!(got, "main");
        assert!(!got.starts_with("epic/"));
    }

    #[test]
    fn resolve_staleness_empty_preferred_falls_through_to_upstream() {
        let got = resolve_staleness_sync_ref(
            Some("   "),
            "factory/worker",
            Some("origin/main"),
            "main",
        );
        assert_eq!(got, "origin/main");
    }

    #[test]
    fn resolve_staleness_epic_branch_without_preferred_uses_self() {
        let got = resolve_staleness_sync_ref(None, "epic/own", None, "main");
        assert_eq!(got, "epic/own");
    }

    fn git(path: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .expect("git spawn");
        assert!(status.success(), "git {args:?} failed");
    }

    /// Multi-epic repo: epic/a and epic/z both exist; worker on factory/* is behind
    /// only epic/a. Preferred=epic/a must report that branch — never epic/z.
    #[test]
    fn check_worktree_staleness_uses_preferred_when_two_epic_branches_exist() {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(p, &["add", "seed.txt"]);
        git(p, &["commit", "-q", "-m", "seed"]);

        // epic/a: one extra commit (worker will be behind this)
        git(p, &["checkout", "-q", "-b", "epic/a"]);
        std::fs::write(p.join("a.txt"), "a\n").unwrap();
        git(p, &["add", "a.txt"]);
        git(p, &["commit", "-q", "-m", "epic a"]);

        // epic/z: two extra commits off main (would be wrong multi-epic pick if .last())
        git(p, &["checkout", "-q", "main"]);
        git(p, &["checkout", "-q", "-b", "epic/z"]);
        std::fs::write(p.join("z1.txt"), "z1\n").unwrap();
        git(p, &["add", "z1.txt"]);
        git(p, &["commit", "-q", "-m", "epic z1"]);
        std::fs::write(p.join("z2.txt"), "z2\n").unwrap();
        git(p, &["add", "z2.txt"]);
        git(p, &["commit", "-q", "-m", "epic z2"]);

        // factory worker branched from main (behind both epics)
        git(p, &["checkout", "-q", "main"]);
        git(p, &["checkout", "-q", "-b", "factory/worker"]);

        let path = p.to_str().unwrap();
        let (behind, branch) = check_worktree_staleness(path, Some("epic/a"))
            .expect("staleness check should succeed");
        assert_eq!(branch, "epic/a", "must name preferred epic A, not concurrent epic Z");
        assert!(!branch.contains("epic/z"), "wrong epic must not appear: {branch}");
        assert_eq!(behind, 1, "worker is 1 commit behind epic/a");

        // Without preferred: base/main — still must not invent epic/z via list.last()
        let (behind_main, branch_main) =
            check_worktree_staleness(path, None).expect("staleness without preferred");
        assert_eq!(branch_main, "main");
        assert_eq!(behind_main, 0);
        assert!(!branch_main.starts_with("epic/"));
    }

    #[test]
    fn check_worktree_staleness_missing_path_returns_none() {
        assert!(check_worktree_staleness("/nonexistent/worktree/path", Some("epic/a")).is_none());
    }

    // -----------------------------------------------------------------------
    // cas-f8bc (GH #106): behindness must count only commits whose CONTENT the
    // worktree lacks. Counting the merge of the worker's own landed lane made
    // assignment refuse a worker for being "behind" its own work, and that
    // refused assignment was the prerequisite for the merge that would clear
    // it — a closed loop.
    // -----------------------------------------------------------------------

    fn commit_file(p: &std::path::Path, name: &str, body: &str) {
        std::fs::write(p.join(name), body).unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-q", "-m", body]);
    }

    fn seeded_epic_repo() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let p = dir.path().to_path_buf();
        git(&p, &["init", "-q", "-b", "main"]);
        commit_file(&p, "seed.txt", "seed");
        git(&p, &["checkout", "-q", "-b", "epic/a"]);
        (dir, p)
    }

    /// The live repro: the supervisor merges the worker's own completed lane
    /// into the epic, and the worker is then told it is 1 commit behind.
    #[test]
    fn own_merged_lane_does_not_count_as_behind_cas_f8bc() {
        let (_dir, p) = seeded_epic_repo();
        git(&p, &["checkout", "-q", "-b", "factory/worker"]);
        commit_file(&p, "w1.txt", "worker work 1");
        commit_file(&p, "w2.txt", "worker work 2");
        git(&p, &["checkout", "-q", "epic/a"]);
        git(
            &p,
            &["merge", "-q", "--no-ff", "factory/worker", "-m", "Merge factory/worker"],
        );
        git(&p, &["checkout", "-q", "factory/worker"]);

        // Precondition: the naive measure is what produced the deadlock.
        let naive = String::from_utf8_lossy(
            &Command::new("git")
                .args(["rev-list", "--count", "HEAD..epic/a"])
                .current_dir(&p)
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .parse::<u32>()
        .unwrap();
        assert_eq!(naive, 1, "precondition: the merge node reads as behindness");

        let (behind, branch) =
            check_worktree_staleness(p.to_str().unwrap(), Some("epic/a")).expect("staleness");
        assert_eq!(branch, "epic/a");
        assert_eq!(
            behind, 0,
            "a worker whose own lane was just merged has nothing to sync"
        );
    }

    /// The exemption must not blind the gate to real staleness.
    #[test]
    fn another_workers_merged_commits_still_count_as_behind_cas_f8bc() {
        let (_dir, p) = seeded_epic_repo();
        git(&p, &["checkout", "-q", "-b", "factory/me"]);
        git(&p, &["checkout", "-q", "epic/a"]);
        git(&p, &["checkout", "-q", "-b", "factory/other"]);
        commit_file(&p, "o1.txt", "other work 1");
        commit_file(&p, "o2.txt", "other work 2");
        git(&p, &["checkout", "-q", "epic/a"]);
        git(
            &p,
            &["merge", "-q", "--no-ff", "factory/other", "-m", "Merge factory/other"],
        );
        git(&p, &["checkout", "-q", "factory/me"]);

        let (behind, _) =
            check_worktree_staleness(p.to_str().unwrap(), Some("epic/a")).expect("staleness");
        assert_eq!(
            behind, 2,
            "the other worker's two commits are genuinely missing here"
        );
    }

    /// When the supervisor replays a lane onto the epic (rebase/cherry-pick)
    /// the worker's commits reappear under new SHAs. Reachability alone still
    /// calls those "behind"; patch-id equality is what clears them.
    #[test]
    fn own_lane_replayed_under_new_shas_is_exempt_but_real_work_is_not_cas_f8bc() {
        let (_dir, p) = seeded_epic_repo();
        git(&p, &["checkout", "-q", "-b", "factory/me"]);
        commit_file(&p, "m1.txt", "my work 1");
        // Epic advances on its own first, so the replay lands on a different
        // parent and therefore a different SHA.
        git(&p, &["checkout", "-q", "epic/a"]);
        commit_file(&p, "e1.txt", "epic side work");
        let mine = String::from_utf8_lossy(
            &Command::new("git")
                .args(["rev-parse", "factory/me"])
                .current_dir(&p)
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();
        git(&p, &["cherry-pick", &mine]);
        git(&p, &["checkout", "-q", "factory/me"]);

        let (behind, _) =
            check_worktree_staleness(p.to_str().unwrap(), Some("epic/a")).expect("staleness");
        assert_eq!(
            behind, 1,
            "only the epic-side commit is genuinely missing; the replayed lane is not"
        );
    }

    /// A squash-merge collapses the worker's commits into one new commit whose
    /// patch-id matches none of the originals, so commit-level rules alone
    /// still count it. Tree equality is what closes this.
    #[test]
    fn own_squash_merged_lane_does_not_count_as_behind_cas_f8bc() {
        let (_dir, p) = seeded_epic_repo();
        git(&p, &["checkout", "-q", "-b", "factory/worker"]);
        commit_file(&p, "w1.txt", "worker work 1");
        commit_file(&p, "w2.txt", "worker work 2");
        git(&p, &["checkout", "-q", "epic/a"]);
        git(&p, &["merge", "-q", "--squash", "factory/worker"]);
        git(&p, &["commit", "-q", "-m", "Squashed worker lane"]);
        git(&p, &["checkout", "-q", "factory/worker"]);

        // Precondition: the commit-level measure alone would still deadlock.
        let by_commits = String::from_utf8_lossy(
            &Command::new("git")
                .args([
                    "rev-list",
                    "--count",
                    "--no-merges",
                    "--cherry-pick",
                    "--right-only",
                    "HEAD...epic/a",
                ])
                .current_dir(&p)
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .parse::<u32>()
        .unwrap();
        assert_eq!(
            by_commits, 1,
            "precondition: a squashed lane is invisible to patch-id matching"
        );

        let (behind, _) =
            check_worktree_staleness(p.to_str().unwrap(), Some("epic/a")).expect("staleness");
        assert_eq!(
            behind, 0,
            "the worker's own squash-merged lane must not read as staleness"
        );
    }

    /// A worker holding unmerged work of its own, with nothing new on the epic,
    /// is not behind — being ahead is not being stale.
    #[test]
    fn worker_ahead_of_epic_is_not_behind_cas_f8bc() {
        let (_dir, p) = seeded_epic_repo();
        git(&p, &["checkout", "-q", "-b", "factory/worker"]);
        commit_file(&p, "wip.txt", "unmerged work");

        let (behind, _) =
            check_worktree_staleness(p.to_str().unwrap(), Some("epic/a")).expect("staleness");
        assert_eq!(behind, 0, "ahead is not behind");
    }

    /// A worker that has simply not synced is still reported as stale.
    #[test]
    fn plain_unsynced_worker_is_still_behind_cas_f8bc() {
        let (_dir, p) = seeded_epic_repo();
        commit_file(&p, "e1.txt", "epic work 1");
        commit_file(&p, "e2.txt", "epic work 2");
        git(&p, &["checkout", "-q", "-b", "factory/worker", "HEAD~2"]);

        let (behind, _) =
            check_worktree_staleness(p.to_str().unwrap(), Some("epic/a")).expect("staleness");
        assert_eq!(behind, 2, "genuine staleness must still block assignment");
    }
}
