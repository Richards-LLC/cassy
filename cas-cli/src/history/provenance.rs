//! Repairing the commit → session spine (EPIC cas-6212 / cas-519f, spec §5.3).
//!
//! # What was broken
//!
//! `commit_links` is the table `blame_impl` joins through to answer "which
//! session, and which prompt, produced this line". On the live database it held
//! **0 rows**, so every blamed line came back `is_ai_generated: false` — a
//! confident wrong answer rather than a gap.
//!
//! It was empty for a structural reason, not a bug in the writer:
//! `detect_and_link_git_commit` fires only from the PostToolUse **Bash** hook,
//! on a recognised `git commit` command, and returns early for harnesses whose
//! `tool_response` is a bare string. In a factory running mixed harnesses most
//! commits never reach it, and no other path ever wrote the table.
//!
//! # The repair, and why it is shaped like this
//!
//! Spec §5.3 moves the link off the harness-specific hook and onto the daemon
//! indexer. Two properties of that decision are deliberate:
//!
//! **It is driven by missing rows, not by the watermark.** Writing links inside
//! `commit_batch`'s transaction would tie a derived row to the watermark that
//! advances past it: a crash between "commit indexed" and "link written" would
//! leave a commit the delta pass never revisits. Instead the pass asks the index
//! which commits have no link and repairs those. The same code therefore fixes
//! the whole pre-existing backlog and every future commit, and a crash costs one
//! retry.
//!
//! **A reconstructed link never overwrites an observed one.** The hook watched
//! the commit happen; this pass infers it from a `worker_git_commit` event
//! emitted at session stop. Both are useful, they are not equally good, and
//! `link_method` plus [`CommitLinkStore::add_reconstructed`]'s
//! `ON CONFLICT DO NOTHING` keep the difference legible for good.
//!
//! # What it will not invent
//!
//! Only edges that actually **name a session** produce a row — `commit_links`
//! is keyed on one, and a synthesised session id would make an unanswerable
//! question look answered. An ambiguous prefix (a 7-char abbreviation matching
//! more than one indexed commit) is skipped for the same reason: §5.2 forbids
//! silently picking a winner, and a spine row *is* picking a winner.
//!
//! `prompt_ids` is left empty. The evidence reconstructs a session, not a
//! prompt; filling that field from a session's prompts would manufacture
//! precisely the "which prompt caused this line" claim spec §5.3 says must wait.

use std::path::Path;

use anyhow::{Context, Result};
use cas_store::{
    CommitLinkStore, HistoryStore, LINK_METHOD_INDEXER_WORKER_EVENT, LinkConfidence,
    SqliteCommitLinkStore, SqliteHistoryStore,
};
use cas_types::CommitLink;

/// Commits examined per repair pass.
///
/// Bounded because the daemon runs this on a tick beside the index pass and a
/// 2,500-commit backlog must not turn one tick into a long transaction storm.
/// The pass is resumable by construction — it asks for unlinked commits every
/// time — so a bound costs nothing but wall-clock.
pub const REPAIR_BATCH: usize = 500;

/// What one repair pass did. Every number here is reported rather than
/// summarised into "ok": a pass that examined 500 commits and wrote 3 links is
/// a very different state from one that examined 3 and wrote 3.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepairOutcome {
    /// Commits with no `commit_links` row that this pass looked at.
    pub examined: usize,
    /// Links written.
    pub written: usize,
    /// Commits whose only edges name no session — the anchor edge names a task,
    /// not a session, so this is the common and expected case.
    pub no_session_edge: usize,
    /// Commits skipped because their only session-bearing edge was an ambiguous
    /// prefix. Counted separately because it is the one skip that means "the
    /// evidence exists but is not decisive".
    pub skipped_ambiguous: usize,
    /// Rows that lost a race with a concurrent writer (the hook, most likely).
    /// Never an error: losing to an observation is the outcome we want.
    pub already_present: usize,
}

impl RepairOutcome {
    pub fn is_noop(&self) -> bool {
        self.examined == 0
    }
}

/// Repair up to `limit` missing `commit_links` rows for `repository`.
///
/// Safe to call on every tick and safe to interrupt.
pub fn repair_commit_links(
    cas_root: &Path,
    repo_root: &Path,
    limit: usize,
) -> Result<RepairOutcome> {
    repair_commit_links_from(cas_root, repo_root, limit, 0)
}

/// [`repair_commit_links`], resuming past `offset` commits of the work list.
///
/// The offset is what makes a drain-everything loop terminate. Most commits on
/// this corpus have no session-bearing edge and never will — measured: 89.7% of
/// `worker_git_commit` rows carry no SHA at all, and the anchor edge names a
/// task rather than a session. Since those commits stay on the "no link" list
/// forever, a loop that always reads from the top re-examines the same
/// unresolvable head and advances only by however many links it wrote. On the
/// live corpus that stalls after three passes with 977 commits never looked at.
pub fn repair_commit_links_from(
    cas_root: &Path,
    repo_root: &Path,
    limit: usize,
    offset: usize,
) -> Result<RepairOutcome> {
    let store = SqliteHistoryStore::open(cas_root).context("opening the history store")?;
    let links = SqliteCommitLinkStore::open(cas_root).context("opening the commit link store")?;
    let repository = super::repository_id(repo_root);

    let pending = store.commits_without_links(&repository, limit.max(1), offset)?;
    let mut outcome = RepairOutcome {
        examined: pending.len(),
        ..Default::default()
    };
    if pending.is_empty() {
        return Ok(outcome);
    }

    let resolved = store.resolve_provenance(&repository, &pending)?;

    for sha in &pending {
        let Some(provenance) = resolved.get(sha) else {
            continue;
        };

        // Strongest session-bearing edge. `links` is already sorted
        // strongest-first by the resolver, so the first match is the best one.
        let session_edge = provenance
            .links
            .iter()
            .find(|l| l.names_a_session() && !l.ambiguous);

        let Some(edge) = session_edge else {
            if provenance
                .links
                .iter()
                .any(|l| l.names_a_session() && l.ambiguous)
            {
                outcome.skipped_ambiguous += 1;
            } else {
                outcome.no_session_edge += 1;
            }
            continue;
        };

        // A low-confidence edge is evidence worth *showing* in a query answer
        // and not evidence worth writing into the spine, which downstream
        // readers (blame) treat as fact.
        if edge.confidence < LinkConfidence::High {
            outcome.no_session_edge += 1;
            continue;
        }

        let Some(hit) = store.commit_hit_by_sha(sha)? else {
            continue;
        };
        let commit = hit.commit;
        let committed_at = chrono::DateTime::parse_from_rfc3339(&commit.committed_at)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now());

        let link = CommitLink::reconstructed(
            commit.sha.clone(),
            edge.session_id.clone().unwrap_or_default(),
            edge.agent_id.clone().unwrap_or_default(),
            // The branch the commit was seen on, when git recorded one. Not
            // guessed from the current checkout, which is a different branch by
            // the time this runs.
            commit.branch_hint.clone().unwrap_or_default(),
            commit.subject.clone(),
            hit.files.iter().map(|f| f.file_path.clone()).collect(),
            committed_at,
            commit.author_name.clone().unwrap_or_default(),
            LINK_METHOD_INDEXER_WORKER_EVENT,
        );

        if links.add_reconstructed(&link)? {
            outcome.written += 1;
        } else {
            outcome.already_present += 1;
        }
    }

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::{HistoryCommit, LINK_METHOD_HOOK_OBSERVED};
    use rusqlite::params;

    /// A repository + `.cas` with the history tables and the two provenance
    /// source tables (`events`, `commit_links`) present.
    struct Fixture {
        _temp: tempfile::TempDir,
        cas_root: std::path::PathBuf,
        repo_root: std::path::PathBuf,
    }

    /// A full 40-char object name with the given prefix. Built rather than
    /// typed: a hand-counted literal that is 39 chars long silently stops being
    /// a SHA, and the matcher correctly declines it — which looks like a
    /// resolver bug and is a typo.
    fn sha_of(prefix: &str, fill: char) -> String {
        format!(
            "{prefix}{}",
            std::iter::repeat(fill)
                .take(cas_store::FULL_SHA_LEN - prefix.len())
                .collect::<String>()
        )
    }

    fn git(repo: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    fn fixture() -> Fixture {
        let temp = tempfile::TempDir::new().unwrap();
        let repo_root = temp.path().to_path_buf();
        let cas_root = repo_root.join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();

        git(&repo_root, &["init", "-q"]);
        git(&repo_root, &["config", "user.email", "t@example.com"]);
        git(&repo_root, &["config", "user.name", "T"]);

        // The two tables the resolver reads from other subsystems.
        let conn = cas_store::shared_db::shared_connection(&cas_root.join("cas.db")).unwrap();
        {
            let conn = conn.lock().unwrap();
            conn.execute_batch(cas_store::EVENT_SCHEMA).unwrap();
            conn.execute_batch(cas_store::COMMIT_LINK_SCHEMA).unwrap();
        }

        Fixture {
            _temp: temp,
            cas_root,
            repo_root,
        }
    }

    fn index_commit(fx: &Fixture, sha: &str, subject: &str) {
        let store = SqliteHistoryStore::open(&fx.cas_root).unwrap();
        let repository = super::super::repository_id(&fx.repo_root);
        let commit = HistoryCommit {
            sha: sha.to_string(),
            short_sha: sha[..8].to_string(),
            committed_at: "2026-08-01T00:00:00+00:00".to_string(),
            subject: subject.to_string(),
            branch_hint: Some("factory/worker".to_string()),
            author_name: Some("T".to_string()),
            repository,
            symbol_mapping: "pending".to_string(),
            ..Default::default()
        };
        let repository = super::super::repository_id(&fx.repo_root);
        store
            .commit_batch(&repository, &[commit], &[], sha, true)
            .unwrap();
    }

    fn emit_worker_event(fx: &Fixture, head_sha: &str, session: &str) {
        let conn = cas_store::shared_db::shared_connection(&fx.cas_root.join("cas.db")).unwrap();
        let conn = conn.lock().unwrap();
        conn.execute(
            "INSERT INTO events (event_type, entity_type, entity_id, summary, metadata, created_at, session_id)
             VALUES ('worker_git_commit', 'worker', 'worker-1', 'final git state', ?1, '2026-08-01T01:00:00+00:00', ?2)",
            params![
                format!(r#"{{"branch":"factory/worker","head_sha":"{head_sha}"}}"#),
                session
            ],
        )
        .unwrap();
    }

    /// AC3: the indexer populates `commit_links` for a commit the hook never
    /// saw, and stamps it with a method that says it was reconstructed.
    #[test]
    fn a_commit_with_a_worker_event_gets_a_reconstructed_link() {
        let fx = fixture();
        let sha = &sha_of("1111111", 'a');
        index_commit(&fx, sha, "feat: something");
        // A 7-char abbreviation — the width a fixed sha[0..8] slice would miss.
        emit_worker_event(&fx, &sha[..7], "session-abc");

        let outcome = repair_commit_links(&fx.cas_root, &fx.repo_root, 100).unwrap();
        assert_eq!(outcome.examined, 1);
        assert_eq!(outcome.written, 1, "{outcome:?}");

        let links = SqliteCommitLinkStore::open(&fx.cas_root).unwrap();
        let link = links.get(sha).unwrap().expect("link written");
        assert_eq!(link.session_id, "session-abc");
        assert_eq!(link.agent_id, "worker-1");
        assert_eq!(
            link.link_method.as_deref(),
            Some(LINK_METHOD_INDEXER_WORKER_EVENT)
        );
        assert!(
            !link.is_observed(),
            "a reconstructed link must not claim to be an observation"
        );
        assert!(
            link.prompt_ids.is_empty(),
            "the event names a session, never a prompt — inventing one would fake M5's own gap"
        );
    }

    /// The rule spec §5.3 exists for: reconstruction must not be able to
    /// demote an observed link.
    #[test]
    fn an_observed_link_is_never_overwritten() {
        let fx = fixture();
        let sha = &sha_of("2222222", 'b');
        index_commit(&fx, sha, "fix: something");
        emit_worker_event(&fx, &sha[..8], "session-from-event");

        let links = SqliteCommitLinkStore::open(&fx.cas_root).unwrap();
        links
            .add(&CommitLink::new(
                sha.to_string(),
                "session-observed".to_string(),
                "agent-observed".to_string(),
                "main".to_string(),
                "fix: something".to_string(),
                vec!["a.rs".to_string()],
                vec!["prompt-1".to_string()],
                "T".to_string(),
            ))
            .unwrap();

        let outcome = repair_commit_links(&fx.cas_root, &fx.repo_root, 100).unwrap();
        // The commit already has a link, so it is not even in the work list.
        assert_eq!(outcome.examined, 0);

        let link = links.get(sha).unwrap().unwrap();
        assert_eq!(link.session_id, "session-observed");
        assert_eq!(link.link_method.as_deref(), Some(LINK_METHOD_HOOK_OBSERVED));
        assert!(link.is_observed());
    }

    /// An ambiguous 7-char prefix is evidence, but not evidence of *which*
    /// commit. Writing a spine row from it would be picking a winner.
    #[test]
    fn an_ambiguous_prefix_writes_no_link_and_says_so() {
        let fx = fixture();
        // Two indexed commits sharing a 7-char prefix.
        let a = &sha_of("3333333", 'c');
        let b = &sha_of("3333333", 'd');
        index_commit(&fx, a, "first");
        index_commit(&fx, b, "second");
        emit_worker_event(&fx, "3333333", "session-ambiguous");

        let outcome = repair_commit_links(&fx.cas_root, &fx.repo_root, 100).unwrap();
        assert_eq!(outcome.examined, 2);
        assert_eq!(outcome.written, 0, "{outcome:?}");
        assert_eq!(outcome.skipped_ambiguous, 2, "{outcome:?}");

        let links = SqliteCommitLinkStore::open(&fx.cas_root).unwrap();
        assert!(links.get(a).unwrap().is_none());
        assert!(links.get(b).unwrap().is_none());
    }

    /// Running twice must not double-write or error — the daemon calls this on
    /// every tick.
    #[test]
    fn the_pass_is_idempotent() {
        let fx = fixture();
        let sha = &sha_of("4444444", 'e');
        index_commit(&fx, sha, "chore: x");
        emit_worker_event(&fx, &sha[..8], "session-1");

        let first = repair_commit_links(&fx.cas_root, &fx.repo_root, 100).unwrap();
        let second = repair_commit_links(&fx.cas_root, &fx.repo_root, 100).unwrap();
        assert_eq!(first.written, 1);
        assert_eq!(second.examined, 0, "nothing left to repair: {second:?}");
    }

    /// A commit no edge names must produce no row at all — an empty spine entry
    /// with a blank session would make blame report a session that never was.
    #[test]
    fn a_commit_with_no_edge_gets_no_row() {
        let fx = fixture();
        let sha = &sha_of("5555555", 'f');
        index_commit(&fx, sha, "docs: x");

        let outcome = repair_commit_links(&fx.cas_root, &fx.repo_root, 100).unwrap();
        assert_eq!(outcome.examined, 1);
        assert_eq!(outcome.written, 0);
        assert_eq!(outcome.no_session_edge, 1);
        assert!(
            SqliteCommitLinkStore::open(&fx.cas_root)
                .unwrap()
                .get(sha)
                .unwrap()
                .is_none()
        );
    }
}
