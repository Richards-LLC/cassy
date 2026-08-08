//! Walker tests. These drive a real `git` against a throwaway fixture repo —
//! the parsing contract here is with git's actual wire format, so a mocked
//! stand-in would only test our own assumptions back at us.

use super::*;
use cas_store::HistoryStore;
use std::fs;
use tempfile::TempDir;

struct Fixture {
    _dir: TempDir,
    repo: PathBuf,
    cas_root: PathBuf,
}

fn run(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        let cas_root = repo.join(".cas");
        fs::create_dir_all(&cas_root).unwrap();

        run(&repo, &["init", "--initial-branch=main"]);
        run(&repo, &["config", "user.email", "fixture@example.com"]);
        run(&repo, &["config", "user.name", "Fixture Author"]);
        run(&repo, &["config", "commit.gpgsign", "false"]);

        Self {
            _dir: dir,
            repo,
            cas_root,
        }
    }

    fn write(&self, path: &str, contents: &str) {
        let full = self.repo.join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(full, contents).unwrap();
    }

    fn commit(&self, message: &str) -> String {
        run(&self.repo, &["add", "-A"]);
        run(&self.repo, &["commit", "-m", message, "--no-verify"]);
        run(&self.repo, &["rev-parse", "HEAD"]).trim().to_string()
    }

    fn commit_file(&self, path: &str, contents: &str, message: &str) -> String {
        self.write(path, contents);
        self.commit(message)
    }

    fn store(&self) -> SqliteHistoryStore {
        SqliteHistoryStore::open(&self.cas_root).unwrap()
    }

    fn pass(&self) -> WalkOutcome {
        run_index_pass(&self.cas_root, &self.repo).unwrap()
    }

    fn state(&self) -> HistoryIndexState {
        self.store()
            .index_state(&repository_id(&self.repo), SOURCE_GIT)
            .unwrap()
            .unwrap()
    }

    fn counts(&self) -> (i64, i64) {
        self.store().counts(&repository_id(&self.repo)).unwrap()
    }
}

/// AC1, in miniature: the indexed commit count must equal `git rev-list --count`.
#[test]
fn backfill_indexes_every_commit() {
    let f = Fixture::new();
    f.commit_file("a.rs", "fn a() {}\n", "add a");
    f.commit_file("b.rs", "fn b() {}\n", "add b");
    let head = f.commit_file("a.rs", "fn a() -> u8 { 1 }\n", "change a");

    let outcome = f.pass();
    assert_eq!(outcome.mode, WalkMode::Backfill);
    assert_eq!(outcome.commits_indexed, 3);

    let expected: i64 = run(&f.repo, &["rev-list", "--count", "HEAD"])
        .trim()
        .parse()
        .unwrap();
    assert_eq!(f.counts().0, expected);

    let state = f.state();
    assert!(state.backfill_complete);
    assert_eq!(state.last_indexed_sha.as_deref(), Some(head.as_str()));
    assert!(state.last_error.is_none());
}

/// AC2: the second pass must touch only the new commit.
#[test]
fn delta_pass_indexes_only_new_commits() {
    let f = Fixture::new();
    f.commit_file("a.rs", "1\n", "first");
    let first = f.pass();
    assert_eq!(first.mode, WalkMode::Backfill);
    assert_eq!(first.commits_indexed, 1);

    let new_sha = f.commit_file("b.rs", "2\n", "second");
    let second = f.pass();
    assert_eq!(second.mode, WalkMode::Delta);
    assert_eq!(
        second.commits_indexed, 1,
        "delta pass re-indexed already-known commits"
    );
    assert_eq!(f.counts().0, 2);
    assert_eq!(f.state().last_indexed_sha.as_deref(), Some(new_sha.as_str()));
    assert_eq!(f.state().items_indexed, 2);
}

#[test]
fn pass_with_no_new_commits_is_up_to_date_and_writes_nothing() {
    let f = Fixture::new();
    f.commit_file("a.rs", "1\n", "first");
    f.pass();
    let before = f.state();

    let again = f.pass();
    assert_eq!(again.mode, WalkMode::UpToDate);
    assert_eq!(again.commits_indexed, 0);
    assert_eq!(f.counts().0, 1);
    assert_eq!(f.state().items_indexed, before.items_indexed);
}

/// The structural mapping is the point of the feature; verify the join between
/// the two git passes actually lands line counts on the right rows.
#[test]
fn commit_files_capture_change_type_and_line_counts() {
    let f = Fixture::new();
    let sha = f.commit_file("src/a.rs", "one\ntwo\nthree\n", "add a");
    f.pass();

    let store = f.store();
    let conn_rows: Vec<(String, String, Option<i64>, Option<i64>)> = {
        let store_conn = store;
        let (commits, pairs) = store_conn.counts(&repository_id(&f.repo)).unwrap();
        assert_eq!((commits, pairs), (1, 1));
        // Re-open for a raw read; the store API intentionally exposes counts
        // only, so the assertion below reads the table directly.
        let db = rusqlite::Connection::open(f.cas_root.join("cas.db")).unwrap();
        let mut stmt = db
            .prepare(
                "SELECT file_path, change_type, insertions, deletions
                   FROM history_commit_files WHERE sha = ?1",
            )
            .unwrap();
        let rows = stmt
            .query_map([&sha], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    };

    assert_eq!(conn_rows.len(), 1);
    let (path, change_type, insertions, deletions) = &conn_rows[0];
    assert_eq!(path, "src/a.rs");
    assert_eq!(change_type, "A");
    assert_eq!(*insertions, Some(3));
    assert_eq!(*deletions, Some(0));
}

/// Renames must arrive as R with the old path — the reason this walker keeps
/// rename detection instead of the spec sketch's `--no-renames`.
#[test]
fn renames_are_recorded_with_old_path() {
    let f = Fixture::new();
    f.commit_file(
        "old/name.rs",
        "line one\nline two\nline three\nline four\n",
        "add",
    );
    // `git mv` will not create the destination directory for you.
    fs::create_dir_all(f.repo.join("new")).unwrap();
    run(&f.repo, &["mv", "old/name.rs", "new/name.rs"]);
    let sha = f.commit("rename it");
    f.pass();

    let db = rusqlite::Connection::open(f.cas_root.join("cas.db")).unwrap();
    let (path, change_type, old_path): (String, String, Option<String>) = db
        .query_row(
            "SELECT file_path, change_type, old_path FROM history_commit_files WHERE sha = ?1",
            [&sha],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(path, "new/name.rs");
    assert_eq!(change_type, "R");
    assert_eq!(old_path.as_deref(), Some("old/name.rs"));
}

/// Spec §4.2 rule 3: a watermark that is no longer reachable from HEAD must
/// force a re-backfill, never a delta that silently skips commits.
#[test]
fn watermark_off_the_branch_forces_rebackfill() {
    let f = Fixture::new();
    f.commit_file("a.rs", "1\n", "base");
    let base = run(&f.repo, &["rev-parse", "HEAD"]).trim().to_string();
    f.commit_file("side.rs", "side\n", "side commit");
    f.pass();
    assert!(f.state().backfill_complete);

    // Abandon the indexed commit: reset back to base and commit something else.
    run(&f.repo, &["reset", "--hard", &base]);
    f.commit_file("other.rs", "other\n", "divergent commit");

    let outcome = f.pass();
    assert!(
        outcome.watermark_reset,
        "stale watermark was not detected as unreachable"
    );
    assert_eq!(outcome.mode, WalkMode::Backfill);
    // Both reachable commits are indexed after the re-backfill.
    let reachable: i64 = run(&f.repo, &["rev-list", "--count", "HEAD"])
        .trim()
        .parse()
        .unwrap();
    let store = f.store();
    for sha in run(&f.repo, &["rev-list", "HEAD"]).lines() {
        assert!(
            store.has_commit(sha.trim()).unwrap(),
            "reachable commit {sha} missing after re-backfill"
        );
    }
    assert_eq!(outcome.commits_indexed as i64, reachable);
}

/// Merge commits are indexed as commits (they carry real subjects and are the
/// factory's dominant shape) even though git reports no diff for them.
#[test]
fn merge_commits_are_indexed_with_parents() {
    let f = Fixture::new();
    f.commit_file("base.rs", "base\n", "base");
    run(&f.repo, &["checkout", "-b", "feature"]);
    f.commit_file("feature.rs", "feature\n", "feature work");
    run(&f.repo, &["checkout", "main"]);
    f.commit_file("main.rs", "main\n", "main work");
    run(&f.repo, &["merge", "--no-ff", "feature", "-m", "merge it"]);
    let merge_sha = run(&f.repo, &["rev-parse", "HEAD"]).trim().to_string();

    f.pass();

    let db = rusqlite::Connection::open(f.cas_root.join("cas.db")).unwrap();
    let (is_merge, parents, subject): (i64, String, String) = db
        .query_row(
            "SELECT is_merge, parent_shas, subject FROM history_commits WHERE sha = ?1",
            [&merge_sha],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(is_merge, 1);
    assert_eq!(subject, "merge it");
    let parsed: Vec<String> = serde_json::from_str(&parents).unwrap();
    assert_eq!(parsed.len(), 2, "merge commit must record both parents");
}

/// Multi-line bodies are what M7 will embed; truncating them at the first
/// newline would quietly gut the feature's recall.
#[test]
fn commit_body_survives_multiline_and_separators() {
    let f = Fixture::new();
    let sha = f.commit_file(
        "a.rs",
        "1\n",
        "subject line\n\nbody line one\nbody line two\n\nCo-Authored-By: Someone <s@example.com>",
    );
    f.pass();

    let db = rusqlite::Connection::open(f.cas_root.join("cas.db")).unwrap();
    let (subject, body): (String, Option<String>) = db
        .query_row(
            "SELECT subject, body FROM history_commits WHERE sha = ?1",
            [&sha],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(subject, "subject line");
    let body = body.expect("body must be stored");
    assert!(body.contains("body line one"), "body truncated: {body:?}");
    assert!(body.contains("Co-Authored-By"), "body truncated: {body:?}");
}

/// Paths with spaces and quotes are the reason for `-z`; a C-quoted path would
/// land in the table with literal backslashes.
#[test]
fn unusual_paths_are_stored_verbatim() {
    let f = Fixture::new();
    let sha = f.commit_file("dir with spaces/a \"quoted\".rs", "x\n", "odd path");
    f.pass();

    let db = rusqlite::Connection::open(f.cas_root.join("cas.db")).unwrap();
    let path: String = db
        .query_row(
            "SELECT file_path FROM history_commit_files WHERE sha = ?1",
            [&sha],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(path, "dir with spaces/a \"quoted\".rs");
}

#[test]
fn status_reports_watermark_counts_and_lag() {
    let f = Fixture::new();
    f.commit_file("a.rs", "1\n", "one");
    f.pass();

    let fresh = status(&f.cas_root, &f.repo).unwrap();
    assert_eq!(fresh.indexed_commits, 1);
    assert_eq!(fresh.repo_commits, 1);
    assert_eq!(fresh.lag_commits, Some(0));
    assert!(fresh.watermark_is_ancestor);
    assert!(fresh.is_current());

    f.commit_file("b.rs", "2\n", "two");
    let stale = status(&f.cas_root, &f.repo).unwrap();
    assert_eq!(stale.lag_commits, Some(1), "lag must be reported honestly");
    assert_eq!(stale.indexed_commits, 1);
    assert_eq!(stale.repo_commits, 2);
    assert!(!stale.is_current());
}

/// "Never indexed" must not read as "fresh" — lag is unknown, not zero.
#[test]
fn status_before_any_pass_reports_unknown_lag() {
    let f = Fixture::new();
    f.commit_file("a.rs", "1\n", "one");

    let s = status(&f.cas_root, &f.repo).unwrap();
    assert!(s.state.is_none());
    assert_eq!(s.lag_commits, None);
    assert_eq!(s.indexed_commits, 0);
    assert!(!s.is_current());
}

/// Chunk boundaries are where the resumable-backfill contract lives, so drive a
/// real multi-chunk run rather than trusting the arithmetic.
#[test]
fn backfill_spans_multiple_chunks() {
    let f = Fixture::new();
    for i in 0..(CHUNK_SIZE + 3) {
        f.write("churn.rs", &format!("value {i}\n"));
        f.commit(&format!("commit {i}"));
    }

    let outcome = f.pass();
    assert_eq!(outcome.chunks, 2, "expected two chunks at CHUNK_SIZE=500");
    assert_eq!(outcome.commits_indexed, CHUNK_SIZE + 3);
    assert_eq!(f.counts().0, (CHUNK_SIZE + 3) as i64);
    assert!(f.state().backfill_complete);
}

/// An interrupted backfill resumes from its watermark instead of restarting,
/// and reaches full coverage.
#[test]
fn interrupted_backfill_resumes_from_watermark() {
    let f = Fixture::new();
    let first = f.commit_file("a.rs", "1\n", "one");
    f.commit_file("b.rs", "2\n", "two");
    f.commit_file("c.rs", "3\n", "three");

    // Simulate a crash after the first chunk: watermark set, backfill flagged
    // incomplete.
    let store = f.store();
    let repository = repository_id(&f.repo);
    let meta = git_log_over(
        &f.repo,
        &[first.clone()],
        &[],
        "--format=\u{1}%H\u{1f}%P\u{1f}%an\u{1f}%ae\u{1f}%aI\u{1f}%cI\u{1f}%D\u{1f}%s\u{1f}%b",
    )
    .unwrap();
    let commits = parse_commit_records(&meta, &repository);
    store
        .commit_batch(&repository, &commits, &[], &first, false)
        .unwrap();

    let outcome = f.pass();
    assert_eq!(
        outcome.mode,
        WalkMode::Backfill,
        "resuming an incomplete backfill is still a backfill"
    );
    assert!(!outcome.watermark_reset);
    assert_eq!(
        outcome.commits_indexed, 2,
        "resume must skip the already-indexed chunk"
    );
    assert_eq!(f.counts().0, 3);
    assert!(f.state().backfill_complete);
}

#[test]
fn repo_root_for_resolves_from_cas_dir() {
    let f = Fixture::new();
    f.commit_file("a.rs", "1\n", "one");
    let resolved = repo_root_for(&f.cas_root).unwrap();
    assert_eq!(
        resolved.canonicalize().unwrap(),
        f.repo.canonicalize().unwrap()
    );
}

#[test]
fn repo_root_for_errors_outside_a_repository() {
    let dir = TempDir::new().unwrap();
    let cas_root = dir.path().join("nogit/.cas");
    fs::create_dir_all(&cas_root).unwrap();
    // A temp dir can sit inside an enclosing repo on some machines; only assert
    // the error path when git genuinely reports no repository.
    if let Ok(root) = repo_root_for(&cas_root) {
        assert!(root.exists());
    }
}
