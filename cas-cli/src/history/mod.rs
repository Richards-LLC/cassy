//! Structural git-history walker (EPIC cas-6212 / cas-7a21, spec §4.2).
//!
//! Turns `git log` into rows in `history_commits` / `history_commit_files`,
//! tracking how far it got in `history_index_state.last_indexed_sha` — the
//! watermark.
//!
//! # The four rules this module exists to enforce (spec §4.2)
//!
//! 1. **Commit SHA, not mtime**, as the incrementality key.
//! 2. **The watermark advances only with the batch it describes**, inside one
//!    transaction ([`cas_store::HistoryStore::commit_batch`]). A partial batch
//!    re-runs; it is never both "done" and "not done".
//! 3. **A watermark that is not an ancestor of HEAD** (force-push, rebase,
//!    branch switch) triggers a full re-backfill rather than a delta that would
//!    silently skip commits.
//! 4. **Backfill is chunked and resumable** — [`CHUNK_SIZE`] commits per
//!    transaction, so an interrupted run resumes instead of restarting.
//!
//! # Ordering
//!
//! Commits are walked in `--topo-order --reverse`, i.e. ancestors first. That
//! is load-bearing, not cosmetic: the watermark is written at a chunk boundary
//! and later read as `<watermark>..HEAD`, which *excludes everything reachable
//! from the watermark*. Under git's default (commit-date) ordering a chunk
//! boundary can precede its own ancestors, and those ancestors would then be
//! excluded from the resumed range and never indexed. Topological order makes
//! that impossible.
//!
//! # Divergence from spec §4.2's sketched command line
//!
//! The spec's sketch passes `--no-renames`, but its own data model (§4.1) has
//! `change_type = R` and an `old_path` column, which only exist *with* rename
//! detection. The two cannot both hold. Rename detection wins here, so the
//! documented schema is actually populatable; codemap keeps `--no-renames`
//! because its own A+D semantics are deliberate and unchanged.
//!
//! Also measured rather than assumed: **`--name-status` and `--numstat` cannot
//! be combined in one `git log`** — the later flag silently wins. The walker
//! therefore runs two passes per chunk and joins them on the post-change path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use cas_store::{
    HistoryCommit, HistoryCommitFile, HistoryIndexState, HistoryStore, SOURCE_CHANGELOG, SOURCE_GIT,
    SOURCE_GITHUB, SqliteHistoryStore,
};

use crate::git_log::{parse_name_status_z, parse_numstat_z};

pub mod changelog;
pub mod epochs;
pub mod github;
pub mod provenance;
pub mod refs;

/// Commits per transaction during backfill (spec §4.2 rule 4).
pub const CHUNK_SIZE: usize = 500;

/// Width of the stored abbreviated SHA. Fixed by construction — never read back
/// from `git --short`, whose width is dynamic and produced the mixed 7/8-char
/// corpus that cas-ea51 had to correct. Variable-width prefix joins against the
/// full SHA are M5's job.
const SHORT_SHA_LEN: usize = 8;

/// Record separator injected into `--format` so commit headers can be told
/// apart from the NUL-separated file entries that follow them.
const REC: char = '\u{1}';
/// Field separator within a commit header.
const FS: char = '\u{1f}';

/// What a pass actually did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkMode {
    /// No watermark (or a broken one): the whole history was walked.
    Backfill,
    /// Watermark present and valid: only `watermark..HEAD`.
    Delta,
    /// Watermark already at HEAD; git was asked, nothing was written.
    UpToDate,
}

impl WalkMode {
    pub fn as_str(self) -> &'static str {
        match self {
            WalkMode::Backfill => "backfill",
            WalkMode::Delta => "delta",
            WalkMode::UpToDate => "up-to-date",
        }
    }
}

/// Result of one indexing pass.
#[derive(Debug, Clone)]
pub struct WalkOutcome {
    pub mode: WalkMode,
    pub commits_indexed: usize,
    pub files_indexed: usize,
    pub chunks: usize,
    /// Set when a stale watermark forced a re-backfill (spec §4.2 rule 3).
    pub watermark_reset: bool,
    pub head_sha: String,
}

/// Honest snapshot for `cas history status` (spec §10.1).
#[derive(Debug, Clone)]
pub struct HistoryStatus {
    pub repository: String,
    pub head_sha: String,
    pub indexed_commits: i64,
    pub indexed_pairs: i64,
    /// `git rev-list --count HEAD`, i.e. the number of commits that *should* be
    /// indexed. Reported so the caller can see coverage rather than trust it.
    pub repo_commits: i64,
    /// Commits between the watermark and HEAD. `None` when the watermark is
    /// unusable (never run, or no longer an ancestor of HEAD) — reported as
    /// unknown rather than as 0, which would read as "fresh".
    pub lag_commits: Option<i64>,
    pub watermark_is_ancestor: bool,
    pub state: Option<HistoryIndexState>,
    /// `(doc_kind, count)` for `history_docs` (M6). Empty when the doc index
    /// has never run, which is a different fact from "the repository has no
    /// issues" and is reported as such.
    pub doc_counts: Vec<(String, i64)>,
    /// Docs still awaiting an embedding — M7's queue depth, surfaced now so the
    /// backlog is visible before the drain that consumes it exists.
    pub docs_pending_embedding: i64,
    /// The `github` ledger row: cursor, last attempt, and the declared boundary
    /// (`last_error`) when GitHub data is absent (spec §10.2).
    pub github_state: Option<HistoryIndexState>,
    /// The `changelog` ledger row.
    pub changelog_state: Option<HistoryIndexState>,
    /// `symbol_mapping` value → commit count (M3, spec §4.1). Reported rather
    /// than summarised: a large `absent` bucket means the symbol index has not
    /// caught up, and that must be visible instead of inferred from a thin
    /// `history_commit_symbols` table.
    pub symbol_mapping: Vec<(String, i64)>,
}

impl HistoryStatus {
    /// True when every commit reachable from HEAD is indexed and the backfill
    /// flag agrees.
    pub fn is_current(&self) -> bool {
        self.state.as_ref().is_some_and(|s| s.backfill_complete)
            && self.lag_commits == Some(0)
            && self.indexed_commits >= self.repo_commits
    }

    /// Age of a non-zero lag since the last successful observation, in
    /// seconds. Commit timestamps cannot answer this question: a watermark and
    /// HEAD committed one second apart stay one second apart even when the
    /// daemon has been stalled for days.
    ///
    /// A caught-up watermark is exactly zero. Missing, diverged, or malformed
    /// timestamps stay unknown rather than being rendered as fresh.
    pub(crate) fn lag_age_seconds_at(&self, now: chrono::DateTime<chrono::Utc>) -> Option<i64> {
        match self.lag_commits {
            Some(0) => Some(0),
            Some(lag) if lag > 0 => {
                let state = self.state.as_ref()?;
                // A failed attempt is not a successful observation. The
                // previous successful batch remains the honest lower bound.
                let observed_at = if state.last_error.is_none() {
                    state.last_attempt_at.as_ref().or(state.last_indexed_at.as_ref())
                } else {
                    state.last_indexed_at.as_ref()
                }?;
                let observed_at = chrono::DateTime::parse_from_rfc3339(observed_at)
                    .ok()?
                    .with_timezone(&chrono::Utc);
                Some((now - observed_at).num_seconds().max(0))
            }
            _ => None,
        }
    }
}

/// Resolve the repository root for `cas_root` (`.cas/` lives inside it).
pub fn repo_root_for(cas_root: &Path) -> Result<PathBuf> {
    let start = cas_root.parent().unwrap_or(cas_root);
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(start)
        .output()
        .with_context(|| format!("running git in {}", start.display()))?;
    if !out.status.success() {
        return Err(anyhow!(
            "not a git repository: {} ({})",
            start.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(PathBuf::from(root))
}

/// Stable identity for a repository in the index tables.
pub fn repository_id(repo_root: &Path) -> String {
    repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn git(repo_root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
        .with_context(|| format!("git {}", args.join(" ")))?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn head_sha(repo_root: &Path) -> Result<String> {
    Ok(git(repo_root, &["rev-parse", "HEAD"])?.trim().to_string())
}

/// Is `sha` reachable from HEAD? A `false` here is what distinguishes "nothing
/// new" from "the branch moved out from under us" (spec §4.2 rule 3).
fn is_ancestor_of_head(repo_root: &Path, sha: &str) -> bool {
    Command::new("git")
        .args(["merge-base", "--is-ancestor", sha, "HEAD"])
        .current_dir(repo_root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn count_revs(repo_root: &Path, range: &str) -> Result<i64> {
    let out = git(repo_root, &["rev-list", "--count", range])?;
    Ok(out.trim().parse().unwrap_or(0))
}

/// The commit list for a pass, ancestors first.
fn revs_to_index(repo_root: &Path, since: Option<&str>) -> Result<Vec<String>> {
    let range = match since {
        Some(sha) => format!("{sha}..HEAD"),
        None => "HEAD".to_string(),
    };
    let out = git(
        repo_root,
        &["rev-list", "--topo-order", "--reverse", &range],
    )?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// Run a `git log` over an explicit commit set, feeding the SHAs on stdin so
/// the command line cannot overflow on a large chunk.
fn git_log_over(repo_root: &Path, shas: &[String], extra: &[&str], format: &str) -> Result<String> {
    use std::io::Write;
    use std::process::Stdio;

    let mut args: Vec<&str> = vec!["log", "--no-walk", "--stdin", "-z", format];
    args.extend_from_slice(extra);

    let mut child = Command::new("git")
        .args(&args)
        .current_dir(repo_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning git log")?;

    {
        let mut stdin = child.stdin.take().context("git log stdin unavailable")?;
        let payload = shas.join("\n");
        stdin.write_all(payload.as_bytes())?;
        stdin.write_all(b"\n")?;
    }

    let out = child.wait_with_output().context("waiting on git log")?;
    if !out.status.success() {
        return Err(anyhow!(
            "git log failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Split `\x01`-prefixed records, returning `(header_fields_and_tail)` slices.
fn split_records(raw: &str) -> impl Iterator<Item = &str> {
    raw.split(REC).skip(1)
}

/// Parse the metadata half of a chunk.
fn parse_commit_records(raw: &str, repository: &str) -> Vec<HistoryCommit> {
    let mut commits = Vec::new();
    for record in split_records(raw) {
        // `%b` is last so a body containing a field separator degrades into
        // "body keeps the rest" rather than shifting every later field.
        let mut fields = record.splitn(8, FS);
        let (
            Some(sha),
            Some(parents),
            Some(author_name),
            Some(author_email),
            Some(authored_at),
            Some(committed_at),
            Some(decoration),
            Some(subject_and_rest),
        ) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        )
        else {
            continue;
        };

        let sha = sha.trim();
        if sha.len() != 40 {
            continue;
        }

        // Subject and body share the tail; the body is everything after the
        // next separator, and ends at the NUL that closes the format output.
        let (subject, body) = match subject_and_rest.split_once(FS) {
            Some((subject, rest)) => {
                let body = rest.split('\0').next().unwrap_or("");
                (subject, body.trim_end_matches('\n').to_string())
            }
            None => (
                subject_and_rest.split('\0').next().unwrap_or(""),
                String::new(),
            ),
        };

        let parent_shas: Vec<String> = parents
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();

        commits.push(HistoryCommit {
            short_sha: sha.chars().take(SHORT_SHA_LEN).collect(),
            sha: sha.to_string(),
            is_merge: parent_shas.len() > 1,
            parent_shas,
            author_name: non_empty(author_name),
            author_email: non_empty(author_email),
            authored_at: non_empty(authored_at),
            committed_at: committed_at.trim().to_string(),
            subject: subject.to_string(),
            body: non_empty(&body),
            branch_hint: non_empty(decoration),
            repository: repository.to_string(),
            // M3 has not mapped a freshly indexed commit yet. Keeping the
            // explicit pending verdict lets a symbol query return it as
            // unknown instead of claiming it did not touch the symbol.
            symbol_mapping: "pending".to_string(),
        });
    }
    commits
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Everything after the header's terminating NUL is the file section.
fn file_section(record: &str) -> &str {
    match record.find('\0') {
        Some(idx) => &record[idx + 1..],
        None => "",
    }
}

/// Parse both git passes for a chunk and join them on the post-change path.
fn parse_file_records(name_status_raw: &str, numstat_raw: &str) -> Vec<HistoryCommitFile> {
    // sha -> path -> (insertions, deletions)
    let mut counts: HashMap<String, HashMap<String, (Option<i64>, Option<i64>)>> = HashMap::new();
    for record in split_records(numstat_raw) {
        let Some(sha) = record.split(FS).next().map(str::trim) else {
            continue;
        };
        let entry = counts.entry(sha.to_string()).or_default();
        for n in parse_numstat_z(file_section(record)) {
            entry.insert(n.path, (n.insertions, n.deletions));
        }
    }

    let mut files = Vec::new();
    for record in split_records(name_status_raw) {
        let Some(sha) = record.split(FS).next().map(str::trim) else {
            continue;
        };
        let per_commit = counts.get(sha);
        for e in parse_name_status_z(file_section(record)) {
            let (insertions, deletions) = per_commit
                .and_then(|m| m.get(&e.path))
                .copied()
                .unwrap_or((None, None));
            files.push(HistoryCommitFile {
                sha: sha.to_string(),
                file_path: e.path,
                change_type: e.status,
                old_path: e.old_path,
                insertions,
                deletions,
            });
        }
    }
    files
}

/// Index one chunk of commits: two git passes, one transaction.
fn index_chunk(
    store: &SqliteHistoryStore,
    repo_root: &Path,
    repository: &str,
    shas: &[String],
    backfill_complete: bool,
) -> Result<(usize, usize)> {
    const META_FORMAT: &str = "--format=\u{1}%H\u{1f}%P\u{1f}%an\u{1f}%ae\u{1f}%aI\u{1f}%cI\u{1f}%D\u{1f}%s\u{1f}%b";
    const FILE_FORMAT: &str = "--format=\u{1}%H\u{1f}";

    let meta_raw = git_log_over(repo_root, shas, &[], META_FORMAT)?;
    let commits = parse_commit_records(&meta_raw, repository);

    let name_status_raw = git_log_over(repo_root, shas, &["--name-status"], FILE_FORMAT)?;
    let numstat_raw = git_log_over(repo_root, shas, &["--numstat"], FILE_FORMAT)?;
    let files = parse_file_records(&name_status_raw, &numstat_raw);

    let watermark = shas
        .last()
        .ok_or_else(|| anyhow!("empty chunk has no watermark"))?;

    store.commit_batch(repository, &commits, &files, watermark, backfill_complete)?;
    Ok((commits.len(), files.len()))
}

/// Run one indexing pass: backfill if there is no usable watermark, otherwise a
/// delta from it.
///
/// Errors are recorded on the state row before propagating, so a failure is
/// visible in `cas history status` rather than only in the caller's log.
pub fn run_index_pass(cas_root: &Path, repo_root: &Path) -> Result<WalkOutcome> {
    let store = SqliteHistoryStore::open(cas_root)?;
    let repository = repository_id(repo_root);

    match run_index_pass_inner(&store, repo_root, &repository) {
        Ok(outcome) => Ok(outcome),
        Err(e) => {
            let _ = store.record_attempt(&repository, SOURCE_GIT, Some(&e.to_string()));
            Err(e)
        }
    }
}

fn run_index_pass_inner(
    store: &SqliteHistoryStore,
    repo_root: &Path,
    repository: &str,
) -> Result<WalkOutcome> {
    let head = head_sha(repo_root)?;
    let state = store.index_state(repository, SOURCE_GIT)?;

    let mut watermark_reset = false;
    let since = match state.as_ref().and_then(|s| {
        s.backfill_complete
            .then_some(s.last_indexed_sha.as_deref())
            .flatten()
    }) {
        Some(sha) if is_ancestor_of_head(repo_root, sha) => Some(sha.to_string()),
        Some(_) => {
            // Rule 3: the branch moved somewhere our watermark cannot describe.
            store.reset_watermark(repository, SOURCE_GIT)?;
            watermark_reset = true;
            None
        }
        None => {
            // An incomplete backfill still has a usable resume point: its
            // watermark's ancestors are all indexed (topo order), so resume
            // from it rather than re-walking from the root.
            match state.as_ref().and_then(|s| s.last_indexed_sha.as_deref()) {
                Some(sha) if is_ancestor_of_head(repo_root, sha) => Some(sha.to_string()),
                Some(_) => {
                    store.reset_watermark(repository, SOURCE_GIT)?;
                    watermark_reset = true;
                    None
                }
                None => None,
            }
        }
    };

    let is_delta = state
        .as_ref()
        .is_some_and(|s| s.backfill_complete && !watermark_reset)
        && since.is_some();

    let revs = revs_to_index(repo_root, since.as_deref())?;
    if revs.is_empty() {
        // Nothing new. Still record the attempt so "checked recently" is a fact
        // in the ledger and not an inference.
        store.record_attempt(repository, SOURCE_GIT, None)?;
        return Ok(WalkOutcome {
            mode: WalkMode::UpToDate,
            commits_indexed: 0,
            files_indexed: 0,
            chunks: 0,
            watermark_reset,
            head_sha: head,
        });
    }

    let total_chunks = revs.len().div_ceil(CHUNK_SIZE);
    let mut commits_indexed = 0;
    let mut files_indexed = 0;

    for (idx, chunk) in revs.chunks(CHUNK_SIZE).enumerate() {
        // `backfill_complete` is only true on the final chunk: a crash halfway
        // must leave the ledger saying "incomplete", not "done".
        let last_chunk = idx + 1 == total_chunks;
        let (c, f) = index_chunk(store, repo_root, repository, chunk, last_chunk)?;
        commits_indexed += c;
        files_indexed += f;
    }

    Ok(WalkOutcome {
        mode: if is_delta {
            WalkMode::Delta
        } else {
            WalkMode::Backfill
        },
        commits_indexed,
        files_indexed,
        chunks: total_chunks,
        watermark_reset,
        head_sha: head,
    })
}

/// Read-only freshness report. Never indexes (spec §4.2 rule 5).
pub fn status(cas_root: &Path, repo_root: &Path) -> Result<HistoryStatus> {
    let store = SqliteHistoryStore::open(cas_root)?;
    let repository = repository_id(repo_root);
    let head = head_sha(repo_root)?;
    let state = store.index_state(&repository, SOURCE_GIT)?;
    let (indexed_commits, indexed_pairs) = store.counts(&repository)?;
    let repo_commits = count_revs(repo_root, "HEAD")?;

    let watermark = state.as_ref().and_then(|s| s.last_indexed_sha.clone());
    let watermark_is_ancestor = watermark
        .as_deref()
        .is_some_and(|sha| is_ancestor_of_head(repo_root, sha));
    let lag_commits = match (&watermark, watermark_is_ancestor) {
        (Some(sha), true) => count_revs(repo_root, &format!("{sha}..HEAD")).ok(),
        _ => None,
    };

    let symbol_mapping = store.symbol_mapping_counts(&repository)?;

    Ok(HistoryStatus {
        doc_counts: store.doc_counts(&repository)?,
        docs_pending_embedding: store.docs_pending_embedding(&repository)?,
        github_state: store.index_state(&repository, SOURCE_GITHUB)?,
        changelog_state: store.index_state(&repository, SOURCE_CHANGELOG)?,
        repository,
        head_sha: head,
        indexed_commits,
        indexed_pairs,
        repo_commits,
        lag_commits,
        watermark_is_ancestor,
        state,
        symbol_mapping,
    })
}

pub mod search;

/// What one docs pass did, per source. Both halves are reported independently
/// because either can be a declared boundary while the other succeeds — a repo
/// with no CHANGELOG and a working `gh` is an ordinary, fully-honest state.
#[derive(Debug, Clone, Default)]
pub struct DocsOutcome {
    /// `Ok` with the fetch counts, `Err` with the declared boundary text.
    pub github: Option<Result<github::FetchOutcome, String>>,
    /// Number of CHANGELOG sections indexed, or `None` when the repository has
    /// no CHANGELOG (a boundary, not a failure).
    pub changelog_sections: Option<usize>,
    pub changelog_error: Option<String>,
}

/// Index the CHANGELOG's release sections.
///
/// Returns `Ok(None)` when there is no CHANGELOG — spec §10.2 treats absent
/// source data as a declared boundary, and a repository without a CHANGELOG is
/// the overwhelmingly common case.
pub fn run_changelog_pass(cas_root: &Path, repo_root: &Path) -> Result<Option<usize>> {
    let store = SqliteHistoryStore::open(cas_root)?;
    let repository = repository_id(repo_root);

    match changelog::collect(repo_root, &repository) {
        Ok(None) => {
            store.record_attempt(
                &repository,
                SOURCE_CHANGELOG,
                Some("no CHANGELOG.md in the repository root"),
            )?;
            Ok(None)
        }
        Ok(Some(docs)) => {
            // The cursor is the newest release date the file carries. It is not
            // used to skip work — the file is re-parsed in full every pass —
            // but it makes "how current is the CHANGELOG index" answerable.
            let cursor = docs.iter().filter_map(|d| d.updated_at.clone()).max();
            let count = store.upsert_docs(
                &repository,
                SOURCE_CHANGELOG,
                &docs,
                cursor.as_deref(),
                true,
            )?;
            Ok(Some(count))
        }
        Err(e) => {
            store.record_attempt(&repository, SOURCE_CHANGELOG, Some(&e.to_string()))?;
            Err(e)
        }
    }
}

/// Run both doc sources. Neither half can stop the other: spec §8 requires the
/// git index to keep working when GitHub is absent, and the same logic applies
/// between the two doc sources.
pub fn run_docs_pass(
    cas_root: &Path,
    repo_root: &Path,
    repo: Option<&str>,
    force: bool,
    want_github: bool,
    want_changelog: bool,
) -> DocsOutcome {
    let mut outcome = DocsOutcome::default();

    if want_github {
        outcome.github = Some(match repo {
            Some(repo) => github::run_pass(cas_root, repo_root, repo, force)
                .map_err(|e| e.to_string()),
            // No `issues.repo`: report the boundary and propose nothing, the
            // precedent set by the SessionStart detector (spec §8).
            None => {
                let repository = repository_id(repo_root);
                if let Ok(store) = SqliteHistoryStore::open(cas_root) {
                    let _ = store.record_attempt(
                        &repository,
                        SOURCE_GITHUB,
                        Some(&crate::gh_graphql::GhError::RepoNotConfigured.to_string()),
                    );
                }
                Err(crate::gh_graphql::GhError::RepoNotConfigured.to_string())
            }
        });
    }

    if want_changelog {
        match run_changelog_pass(cas_root, repo_root) {
            Ok(sections) => outcome.changelog_sections = sections,
            Err(e) => outcome.changelog_error = Some(e.to_string()),
        }
    }

    outcome
}
pub mod symbols;

#[cfg(test)]
mod tests;
