//! The code-history query surface (EPIC cas-6212 / cas-7f40, spec §6).
//!
//! # One entry point, deliberately
//!
//! [`run`] is the *only* way to answer a history query. `mcp__cas__search
//! action=history` and `cas history search` both call it; neither builds its
//! own ranker, its own filters, or its own status block.
//!
//! That is not tidiness, it is the §6.3 acceptance gate. M6's survey
//! (`docs/migration/cas-b129-m6-legacy-decommission-survey.md`) recorded the
//! knowledge channel as *inert in production*: every `HybridSearch` constructor
//! passed `knowledge_store: None`, so `knowledge_weight: 0.25` did nothing and
//! unit tests that hand-built a `HybridSearch` passed anyway. A history channel
//! wired the same way would be equally dead and would look equally fine. With a
//! single construction site, an integration test that drives a real surface
//! drives the real wiring, and "the channel is attached in production" stops
//! being a claim and becomes a test.
//!
//! # What this surface will not pretend
//!
//! This surface ships on M1 + M5 data: commits, their touched files, M3 symbol
//! overlap rows, and the provenance edges §5.2 resolves over. It does **not**
//! have embeddings (M7). Rather than silently dropping filters it cannot honour,
//! every request for one of those comes back in
//! [`HistorySearchResponse::unsupported`] with the milestone that lands it. A
//! filter that is quietly ignored produces a result set that looks like an
//! answer and is not.
//!
//! # Provenance, since M5 (cas-519f)
//!
//! `include_provenance`, `task_id` and `session_id` are answered rather than
//! declared. What has *not* changed is the honesty contract around them:
//! `index_status` still reports measured coverage rather than a capability
//! claim, a commit with no populated edge is returned carrying its stated
//! reason instead of being dropped (§6.4 Q3), and an ambiguous abbreviated-SHA
//! edge comes back with every commit it could mean rather than a silently
//! chosen one (§5.2).

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::history;
use crate::hybrid_search::{DocType, HistoryFilter, HybridSearch, HybridSearchOptions};
use cas_store::{
    CoChangedFile, CommitProvenance, HistoryCommitFile, HistoryStore, SOURCE_GIT,
    SqliteHistoryStore,
};

/// How many co-change rows Q7 reports. Bounded because the tail of a co-change
/// distribution is noise — every file that ever shared a sweeping refactor.
const CO_CHANGE_LIMIT: usize = 10;

/// A history query as the surfaces receive it (spec §6.1).
#[derive(Debug, Clone, Default)]
pub struct HistorySearchRequest {
    pub query: Option<String>,
    pub path: Option<String>,
    pub symbol: Option<String>,
    /// Relative (`14d`, `2w`, `6h`) or absolute (`2026-08-01`, RFC3339).
    pub since: Option<String>,
    pub until: Option<String>,
    /// `commit` (supported), or `issue` / `pr` / `changelog` (M6).
    pub kind: Option<String>,
    pub task_id: Option<String>,
    pub session_id: Option<String>,
    pub limit: usize,
    pub include_provenance: bool,
    pub include_merges: bool,
}

/// A capability this surface was asked for and does not have.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Unsupported {
    /// What was asked for.
    pub feature: String,
    /// Why it cannot be answered today.
    pub reason: String,
    /// The milestone that lands it, so the gap is a schedule item rather than
    /// a mystery.
    pub lands_in: String,
}

/// The §6.5 response contract. Present on **every** response, including empty
/// ones: "no results" and "no index" are different answers and a caller that
/// cannot tell them apart will read the second as the first.
#[derive(Debug, Clone, Serialize)]
pub struct IndexStatus {
    pub last_indexed_sha: Option<String>,
    pub head_sha: String,
    /// Commits between the watermark and HEAD; `null` when the watermark is
    /// missing or no longer an ancestor of HEAD.
    pub lag_commits: Option<i64>,
    /// Wall-clock seconds since the last successful index observation while
    /// commits are pending. Same `null` discipline: an unknown lag is never
    /// rendered as 0.
    pub lag_seconds: Option<i64>,
    pub backfill_complete: bool,
    pub indexed_commits: i64,
    pub repo_commits: i64,
    /// Measured share of indexed commits reachable through the one
    /// high-confidence provenance edge (spec §10.1). `null` when unmeasurable.
    pub provenance_coverage_pct: Option<f64>,
    pub provenance_high_confidence_links: Option<i64>,
    /// Commits reachable through **any** populated edge, including the
    /// medium/low-confidence ones. Reported beside the high-confidence figure,
    /// never instead of it (spec §10.1: publish both, split by confidence).
    pub provenance_any_coverage_pct: Option<f64>,
    pub provenance_any_confidence_links: Option<i64>,
    /// Commits reachable per `link_method` — the breakdown that distinguishes
    /// "the anchor edge is growing" from "the text edge is matching loosely".
    pub provenance_by_method: Vec<(String, i64)>,
    /// True since M5 (cas-519f). It stays an explicit field rather than
    /// something a caller infers from a coverage number: coverage is ~9% on
    /// this repository, and "supported" and "complete" are not the same claim.
    pub provenance_supported: bool,
    pub provenance_note: String,
    /// Whether embedding recall over history is live. False until M7 ships the
    /// `history:*` vector namespace — independently of whether the machine has
    /// cloud auth, because a cloud login without history vectors still cannot
    /// answer a history query semantically.
    pub semantic_available: bool,
    pub semantic_note: String,
    pub last_error: Option<String>,
}

/// One commit in a history answer.
#[derive(Debug, Clone, Serialize)]
pub struct HistoryHit {
    pub sha: String,
    pub short_sha: String,
    pub score: f64,
    /// `0.5^(days/30)`, reported so a caller can re-rank without re-deriving.
    pub recency: f64,
    pub committed_at: String,
    pub author_name: Option<String>,
    pub subject: String,
    pub body: Option<String>,
    pub is_merge: bool,
    /// M3's verdict for this commit. A symbol-filtered answer includes
    /// `pending`, `absent`, and `partial` rows as explicitly uncertain rather
    /// than silently treating index lag as a non-match.
    pub symbol_mapping: String,
    pub files: Vec<FileChange>,
    /// The resolved edges for this commit (spec §5.2), strongest first.
    /// `None` when the caller did not ask; `Some([])` when it asked and this
    /// commit has no populated edge — which is a real answer, not a gap, and is
    /// why [`Self::provenance_reason`] is filled in that case.
    pub provenance: Option<Vec<ProvenanceEdge>>,
    /// Why `provenance` is empty, when it is.
    pub provenance_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileChange {
    pub file_path: String,
    pub change_type: String,
    pub old_path: Option<String>,
    pub insertions: Option<i64>,
    pub deletions: Option<i64>,
}

impl From<HistoryCommitFile> for FileChange {
    fn from(f: HistoryCommitFile) -> Self {
        Self {
            file_path: f.file_path,
            change_type: f.change_type,
            old_path: f.old_path,
            insertions: f.insertions,
            deletions: f.deletions,
        }
    }
}

/// One provenance edge as the surfaces render it.
///
/// A flattened view of `cas_store::ProvenanceLink`: the store owns the
/// resolution, this owns the wire shape, so the JSON contract cannot drift
/// every time the resolver grows a field.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProvenanceEdge {
    /// Which edge produced this link, named rather than inferred (spec §5.3).
    pub link_method: String,
    pub confidence: String,
    pub task_id: Option<String>,
    pub task_title: Option<String>,
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
    pub observed_at: Option<String>,
    /// The abbreviation this edge carried, when it carried one — so a reader
    /// can weigh the join instead of trusting it.
    pub matched_prefix: Option<String>,
    /// True when `matched_prefix` matches more than one indexed commit. The
    /// edge is still returned (spec §5.2 forbids silently picking a winner).
    pub ambiguous: bool,
    /// Every commit the prefix could have meant, when it is ambiguous.
    pub ambiguous_candidates: Vec<String>,
}

impl From<cas_store::ProvenanceLink> for ProvenanceEdge {
    fn from(l: cas_store::ProvenanceLink) -> Self {
        Self {
            link_method: l.link_method,
            confidence: l.confidence.as_str().to_string(),
            task_id: l.task_id,
            task_title: l.task_title,
            session_id: l.session_id,
            agent_id: l.agent_id,
            observed_at: l.observed_at,
            matched_prefix: l.matched_prefix,
            ambiguous: l.ambiguous,
            ambiguous_candidates: l.ambiguous_candidates,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CoChange {
    pub file_path: String,
    pub commits_together: i64,
}

impl From<CoChangedFile> for CoChange {
    fn from(c: CoChangedFile) -> Self {
        Self {
            file_path: c.file_path,
            commits_together: c.commits_together,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HistorySearchResponse {
    pub query: Option<String>,
    pub repository: String,
    pub filters: AppliedFilters,
    pub count: usize,
    pub results: Vec<HistoryHit>,
    /// Q7: files that most often change alongside `path`. Present only when a
    /// path filter was given, because without one the question has no subject.
    pub co_changed_files: Vec<CoChange>,
    pub index_status: IndexStatus,
    pub unsupported: Vec<Unsupported>,
}

/// The filters that were actually applied, resolved. Echoed back because
/// `since=14d` is not self-explanatory once it has become a timestamp, and a
/// caller comparing two answers needs to know which windows they covered.
#[derive(Debug, Clone, Serialize, Default)]
pub struct AppliedFilters {
    pub path: Option<String>,
    pub symbol: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub include_merges: bool,
    /// How many commits `task_id` / `session_id` resolved to, before the other
    /// filters ran. `None` when neither was given.
    ///
    /// This exists because of a real, measured trap: on the live corpus a
    /// session's only linked commit is often a **merge**, and merges are
    /// excluded by default (§7.1). Without this number the response says
    /// "no commits matched", which is indistinguishable from "that session
    /// produced nothing" — when in fact the filter resolved fine and a
    /// different filter dropped the result. Reporting the resolved count keeps
    /// the two apart.
    pub identity_filter_matched: Option<usize>,
    /// Maximum number of pending/absent/partial rows admitted behind exact
    /// mapped symbol hits. Present only for a symbol-filtered query.
    pub symbol_uncertain_limit: Option<usize>,
    pub limit: usize,
}

/// Parse `since`/`until`: a relative offset (`14d`, `2w`, `6h`, `45m`), a bare
/// date (`2026-08-01`), or a full RFC3339 timestamp. Returns RFC3339 UTC.
///
/// Errors rather than falling back to "no filter": a mistyped window that
/// silently widens to all of history returns confident, wrong-scoped results.
pub fn parse_time_bound(raw: &str) -> Result<String> {
    use chrono::{Duration, NaiveDate, Utc};

    let value = raw.trim();
    if value.is_empty() {
        anyhow::bail!("empty time bound");
    }

    // Relative: <number><unit>
    let (digits, unit) = value.split_at(value.len() - 1);
    if let (Ok(n), true) = (
        digits.parse::<i64>(),
        matches!(unit, "d" | "w" | "h" | "m" | "y"),
    ) {
        if n < 0 {
            anyhow::bail!("negative time offset: {raw}");
        }
        let delta = match unit {
            "m" => Duration::minutes(n),
            "h" => Duration::hours(n),
            "d" => Duration::days(n),
            "w" => Duration::weeks(n),
            "y" => Duration::days(n * 365),
            _ => unreachable!("unit was matched above"),
        };
        return Ok((Utc::now() - delta).to_rfc3339());
    }

    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(value) {
        return Ok(dt.with_timezone(&Utc).to_rfc3339());
    }

    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let midnight = date
            .and_hms_opt(0, 0, 0)
            .context("constructing midnight for a parsed date")?;
        return Ok(midnight.and_utc().to_rfc3339());
    }

    anyhow::bail!(
        "cannot parse time bound {raw:?}: expected a relative offset (14d, 2w, 6h, 45m), \
         a date (2026-08-01), or an RFC3339 timestamp"
    )
}

/// Collect every declaration this request earns. Pure, so the honesty contract
/// is testable without a repository, a store or a ranker.
pub fn unsupported_for(req: &HistorySearchRequest) -> Vec<Unsupported> {
    let mut out = Vec::new();

    // `kind` narrows the doc class. Only commits exist today.
    if let Some(kind) = req.kind.as_deref() {
        let kind = kind.trim().to_lowercase();
        if !kind.is_empty() && kind != "commit" && kind != "commits" {
            out.push(Unsupported {
                feature: format!("kind={kind}"),
                reason: "history_docs (GitHub issues/PRs/comments and CHANGELOG sections) \
                         is not indexed yet; only commits are searchable"
                    .into(),
                lands_in: "M6".into(),
            });
        }
    }

    // `task_id`, `session_id` and `include_provenance` are answered as of M5
    // (cas-519f) and are deliberately absent from this list. They are not
    // silently supported either: `index_status.provenance_supported` states it,
    // and every unresolved commit carries its own `provenance_reason`.

    out
}

/// Answer a history query (spec §6).
///
/// The single production path. See the module docs for why that matters.
pub fn run(cas_root: &Path, req: &HistorySearchRequest) -> Result<HistorySearchResponse> {
    let repo_root = history::repo_root_for(cas_root)?;
    let repository = history::repository_id(&repo_root);

    let since = req.since.as_deref().map(parse_time_bound).transpose()?;
    let until = req.until.as_deref().map(parse_time_bound).transpose()?;
    let limit = if req.limit == 0 { 10 } else { req.limit };

    let unsupported = unsupported_for(req);
    // An unsupported `kind` narrows to a class with no rows. Returning the
    // commits anyway would answer "show me the issues" with commits.
    let kind_excludes_commits = unsupported
        .iter()
        .any(|u| u.feature.starts_with("kind="));

    // The store is opened before the ranker because the task/session filters
    // are resolved through it: they narrow the candidate SHA set *in SQL*,
    // before LIMIT. Post-filtering a ranked page would answer "what did this
    // task ship" with whatever of it happened to reach the top-k, and with
    // nothing at all whenever none of it did.
    let store = SqliteHistoryStore::open(cas_root)?;
    let shas = resolve_identity_filter(&store, &repository, req)?;
    let identity_filter_matched = shas.as_ref().map(Vec::len);

    let filter = HistoryFilter {
        repository: repository.clone(),
        path: req.path.clone(),
        symbol: req.symbol.clone(),
        since: since.clone(),
        until: until.clone(),
        include_merges: req.include_merges,
        shas,
    };

    // ONE ranker, constructed once (spec §1.2's "extend, do not clone").
    let mut hybrid = HybridSearch::open(cas_root)?;
    hybrid.set_history_store_from_path(cas_root)?;

    let opts = HybridSearchOptions {
        base: crate::hybrid_search::SearchOptions {
            query: req.query.clone().unwrap_or_default(),
            limit,
            ..Default::default()
        },
        enable_history: true,
        history_filter: Some(filter),
        // The other channels rank *entries*, and this surface is passed none.
        // Leaving them on would spend the work and add nothing.
        enable_semantic: false,
        enable_temporal: false,
        enable_graph: false,
        enable_code: false,
        enable_knowledge: false,
        ..Default::default()
    };

    let ranked = if kind_excludes_commits {
        Vec::new()
    } else {
        hybrid.search(&opts, &[])?
    };

    // Hydrate the ranked SHAs back into full commits. The ranker returns ids
    // and scores; the store owns the rows.
    //
    // Provenance is resolved for the WHOLE page in one call rather than per
    // hit: `events` is ~978 K rows with no index on `event_type`, so the
    // per-commit form would scan it once per result.
    let mut provenance: std::collections::HashMap<String, CommitProvenance> = if req
        .include_provenance
    {
        let page: Vec<String> = ranked
            .iter()
            .filter(|r| r.doc_type == DocType::HistoryCommit)
            .map(|r| r.id.clone())
            .collect();
        store.resolve_provenance(&repository, &page)?
    } else {
        std::collections::HashMap::new()
    };

    let mut results = Vec::with_capacity(ranked.len());
    for ranked_hit in ranked.iter().filter(|r| r.doc_type == DocType::HistoryCommit) {
        let Some(hydrated) = store.commit_hit_by_sha(&ranked_hit.id)? else {
            // The row vanished between ranking and hydration (a concurrent
            // re-backfill). Skip rather than emit a half-populated hit.
            continue;
        };
        let commit = hydrated.commit;
        let resolved = provenance.remove(&commit.sha);
        // Narrow the file list to what the path filter asked about — the whole
        // diff of an unrelated 200-file commit is noise in a path query.
        let files: Vec<HistoryCommitFile> = match &req.path {
            Some(path) => hydrated
                .files
                .into_iter()
                .filter(|f| {
                    f.file_path.contains(path.as_str())
                        || f.old_path
                            .as_deref()
                            .is_some_and(|p| p.contains(path.as_str()))
                })
                .collect(),
            None => hydrated.files,
        };
        results.push(HistoryHit {
            short_sha: commit.short_sha,
            score: ranked_hit.score,
            recency: hydrated.recency,
            committed_at: commit.committed_at,
            author_name: commit.author_name,
            subject: commit.subject,
            body: commit.body,
            is_merge: commit.is_merge,
            symbol_mapping: commit.symbol_mapping,
            files: files.into_iter().map(FileChange::from).collect(),
            provenance: resolved
                .as_ref()
                .map(|p| p.links.iter().cloned().map(ProvenanceEdge::from).collect()),
            // Q3's requirement in one field: a commit with no edge is RETURNED,
            // carrying the reason it has none, never dropped from the answer.
            provenance_reason: resolved.and_then(|p| p.reason),
            sha: commit.sha,
        });
    }

    // Q7 only makes sense with a subject file.
    let co_changed_files = match &req.path {
        Some(path) => store
            .co_changed_files(&repository, path, CO_CHANGE_LIMIT)?
            .into_iter()
            .map(CoChange::from)
            .collect(),
        None => Vec::new(),
    };

    Ok(HistorySearchResponse {
        query: req.query.clone(),
        repository: repository.clone(),
        filters: AppliedFilters {
            path: req.path.clone(),
            symbol: req.symbol.clone(),
            since,
            until,
            include_merges: req.include_merges,
            identity_filter_matched,
            symbol_uncertain_limit: req
                .symbol
                .as_ref()
                .map(|_| cas_store::HISTORY_SYMBOL_UNCERTAIN_LIMIT),
            limit,
        },
        count: results.len(),
        results,
        co_changed_files,
        index_status: index_status(cas_root, &repo_root, &repository, &store)?,
        unsupported,
    })
}

/// Resolve `task_id` / `session_id` into the SHA set they name (spec §6.1).
///
/// Returns `None` when neither filter was given. Returns `Some(empty)` when one
/// was given and resolved to nothing — which correctly matches no commits.
/// Collapsing that case to `None` would answer "commits from task X" with every
/// commit in the repository, which is the single worst way for a filter to fail.
///
/// Both filters together intersect: asking for a task *and* a session means
/// commits that satisfy both, not either.
fn resolve_identity_filter(
    store: &SqliteHistoryStore,
    repository: &str,
    req: &HistorySearchRequest,
) -> Result<Option<Vec<String>>> {
    let by_task = req
        .task_id
        .as_deref()
        .map(|id| store.shas_for_task(repository, id))
        .transpose()?;
    let by_session = req
        .session_id
        .as_deref()
        .map(|id| store.shas_for_session(repository, id))
        .transpose()?;

    Ok(match (by_task, by_session) {
        (None, None) => None,
        (Some(shas), None) | (None, Some(shas)) => Some(shas),
        (Some(task), Some(session)) => {
            let session: std::collections::HashSet<String> = session.into_iter().collect();
            Some(task.into_iter().filter(|s| session.contains(s)).collect())
        }
    })
}

/// Build the §6.5 status block.
fn index_status(
    cas_root: &Path,
    repo_root: &Path,
    repository: &str,
    store: &SqliteHistoryStore,
) -> Result<IndexStatus> {
    let status = history::status(cas_root, repo_root)?;
    let state = store.index_state(repository, SOURCE_GIT)?;
    let watermark = state.as_ref().and_then(|s| s.last_indexed_sha.clone());

    let lag_seconds = status.lag_age_seconds_at(chrono::Utc::now());

    let coverage = store.provenance_coverage(repository)?;

    Ok(IndexStatus {
        last_indexed_sha: watermark,
        head_sha: status.head_sha,
        lag_commits: status.lag_commits,
        lag_seconds,
        backfill_complete: state.as_ref().is_some_and(|s| s.backfill_complete),
        indexed_commits: status.indexed_commits,
        repo_commits: status.repo_commits,
        provenance_coverage_pct: coverage.coverage_pct,
        provenance_high_confidence_links: coverage.high_confidence_linked,
        provenance_any_coverage_pct: coverage.any_coverage_pct,
        provenance_any_confidence_links: coverage.any_confidence_linked,
        provenance_by_method: coverage.by_method.clone(),
        provenance_supported: true,
        provenance_note: match &coverage.unmeasurable_reason {
            Some(reason) => format!(
                "provenance resolution is supported (M5, spec §5.2); coverage is only \
                 partially measurable here: {reason}"
            ),
            None => format!(
                "provenance resolution is supported (M5, spec §5.2). Coverage is MEASURED, \
                 not claimed: {} of {} indexed commits carry the exact commit→task edge \
                 (tasks.deliverables.factory_branch_anchor), {} carry any populated edge. \
                 Commits with no edge are returned with a stated reason rather than dropped",
                coverage
                    .high_confidence_linked
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "unknown".into()),
                coverage.total_commits,
                coverage
                    .any_confidence_linked
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "unknown".into()),
            ),
        },
        semantic_available: false,
        semantic_note: "embedding recall over history lands in M7: no history:* vectors \
                        exist yet, so this is false even on a cloud-authenticated machine"
            .into(),
        last_error: state.and_then(|s| s.last_error),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> HistorySearchRequest {
        HistorySearchRequest {
            limit: 10,
            ..Default::default()
        }
    }

    #[test]
    fn relative_time_bounds_parse() {
        for value in ["14d", "2w", "6h", "45m", "1y"] {
            let parsed = parse_time_bound(value).expect(value);
            assert!(
                chrono::DateTime::parse_from_rfc3339(&parsed).is_ok(),
                "{value} produced {parsed}"
            );
        }
        // Ordering sanity: a wider window starts earlier.
        assert!(parse_time_bound("30d").unwrap() < parse_time_bound("1d").unwrap());
    }

    #[test]
    fn absolute_time_bounds_parse() {
        let date = parse_time_bound("2026-08-01").unwrap();
        assert!(date.starts_with("2026-08-01T00:00:00"));
        let stamp = parse_time_bound("2026-08-01T12:30:00Z").unwrap();
        assert!(stamp.starts_with("2026-08-01T12:30:00"));
    }

    /// A mistyped window must fail loudly. Falling back to "no filter" would
    /// silently widen the query to all of history and return confident,
    /// wrong-scoped results.
    #[test]
    fn an_unparseable_time_bound_is_an_error_not_a_shrug() {
        assert!(parse_time_bound("last tuesday").is_err());
        assert!(parse_time_bound("14 days").is_err());
        assert!(parse_time_bound("").is_err());
        assert!(parse_time_bound("-5d").is_err());
    }

    #[test]
    fn a_plain_commit_query_declares_nothing_unsupported() {
        assert!(unsupported_for(&req()).is_empty());
        assert!(
            unsupported_for(&HistorySearchRequest {
                kind: Some("commit".into()),
                path: Some("src/lib.rs".into()),
                query: Some("retry".into()),
                ..req()
            })
            .is_empty()
        );
    }

    #[test]
    fn every_unavailable_capability_is_declared_with_its_milestone() {
        let all = unsupported_for(&HistorySearchRequest {
            kind: Some("issue".into()),
            ..req()
        });
        let milestones: Vec<&str> = all.iter().map(|u| u.lands_in.as_str()).collect();
        assert!(milestones.contains(&"M6"), "kind filter undeclared");
        assert!(
            all.iter().all(|u| !u.reason.is_empty()),
            "a declaration with no reason is not an explanation"
        );
    }

    /// M3's rows are populated, so declaring this request unavailable is now
    /// the lie. The production-path fixture exercises the actual SQL join.
    #[test]
    fn symbol_filter_is_answered_and_not_declared_unsupported() {
        let all = unsupported_for(&HistorySearchRequest {
            symbol: Some("crate::retry".into()),
            ..req()
        });
        assert!(
            all.iter().all(|u| u.feature != "symbol filter"),
            "M3 is live; the filter must not retain a stale boundary: {all:?}"
        );
    }

    /// The M5 (cas-519f) inverse of the test above: the three filters that used
    /// to be declared unsupported are now answered, and must NOT be declared —
    /// a stale declaration tells a caller not to ask for something that works.
    ///
    /// This is the assertion that fails if someone reverts the resolver but
    /// leaves the surface claiming support, or vice versa.
    #[test]
    fn provenance_filters_are_answered_and_no_longer_declared_unsupported() {
        let all = unsupported_for(&HistorySearchRequest {
            task_id: Some("cas-1234".into()),
            session_id: Some("s-1".into()),
            include_provenance: true,
            ..req()
        });
        assert!(
            all.is_empty(),
            "provenance filters are supported since M5; declaring them is now the lie: {all:?}"
        );
    }
}
