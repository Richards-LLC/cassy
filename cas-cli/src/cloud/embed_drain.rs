//! The automagic embedding drain (EPIC cas-6212 / cas-db6e, M7 — spec §4.4, §7).
//!
//! # Why this module exists at all
//!
//! Vectors used to be computed in exactly one place: inside `cas cloud sync`.
//! That made "is my corpus embedded?" depend on whether a human had recently
//! typed a sync command — and on 2026-08-08 a 107-page knowledge backlog sat
//! un-embedded until a supervisor ran sync by hand. The manual step *was* the
//! defect. This module is the drain the daemon calls on a tick, so a logged-in
//! installation converges to zero pending vectors on its own, for every corpus.
//!
//! # What it drains, and in which order
//!
//! One arm, three queues: knowledge pages, `history_commits`, `history_docs`.
//! Knowledge goes first because it is the corpus a user can see immediately in
//! retrieval; history is the larger backfill and is happy to take several
//! ticks. All three share the ONE chunk loop in
//! [`crate::cloud::embeddings::drain_units`] — no forked pagination, no second
//! copy of the 32-input cap.
//!
//! # Boundaries this drain declares rather than logs
//!
//! - **Logged out** → [`DrainReport::capability_absent`], no LMDB environment
//!   created, no HTTP request made. A provider-absent install must not
//!   materialise vector storage it will never fill.
//! - **Endpoint without `/api/embeddings`** (404/501) → the same flag, set from
//!   [`EmbedReport::capability_absent`]; a boundary of the installation, not an
//!   alarm.
//! - **Anything else that fails** → `request_errors` / `errors` on the report,
//!   plus a `history_index_state('embeddings')` ledger row that `cas doctor`
//!   reads. Never a `tracing::warn!` and nothing else — that shape is precisely
//!   how cas-a924's permanent `400` stayed invisible.

use std::path::Path;

use cas_store::{
    HistoryCommit, HistoryDoc, HistoryStore, KnowledgeStore, SOURCE_EMBEDDINGS, SqliteHistoryStore,
    SqliteKnowledgeStore,
};

use crate::cloud::CloudConfig;
use crate::cloud::embeddings::{
    DEFAULT_EMBED_BATCH, EmbedReport, EmbedUnit, KnowledgeEmbedder, KnowledgeVectorCache,
    MAX_EMBED_TEXT_CHARS, RateLimiter, cap_embedding_text, drain_units_with_quarantine,
    embed_pending_pages, history_commit_key, history_doc_key,
};
use crate::error::CasError;

/// What one drain tick did, per corpus.
#[derive(Debug, Clone, Default)]
pub struct DrainReport {
    /// Knowledge-page half. `None` when there is no knowledge store to drain.
    pub knowledge: Option<EmbedReport>,
    /// Code-history half (commits + docs). `None` when there is no history
    /// store.
    pub history: Option<EmbedReport>,
    /// Current source-code symbols. Stored and ranked separately from both
    /// knowledge and history.
    pub code: Option<EmbedReport>,
    /// True when this installation has no embedding capability: logged out, or
    /// an endpoint that does not implement `/api/embeddings`. A declared
    /// boundary — the drain did nothing and that is correct.
    pub capability_absent: bool,
}

impl DrainReport {
    pub fn embedded(&self) -> usize {
        self.knowledge.as_ref().map_or(0, |r| r.embedded)
            + self.history.as_ref().map_or(0, |r| r.embedded)
            + self.code.as_ref().map_or(0, |r| r.embedded)
    }

    pub fn requests(&self) -> usize {
        self.knowledge.as_ref().map_or(0, |r| r.requests)
            + self.history.as_ref().map_or(0, |r| r.requests)
            + self.code.as_ref().map_or(0, |r| r.requests)
    }

    pub fn skipped(&self) -> usize {
        self.knowledge.as_ref().map_or(0, |r| r.skipped)
            + self.history.as_ref().map_or(0, |r| r.skipped)
            + self.code.as_ref().map_or(0, |r| r.skipped)
    }

    /// Units still awaiting a vector across every corpus. The number that must
    /// reach zero; the honest measure of whether the drain is working.
    pub fn pending_after(&self) -> usize {
        self.knowledge.as_ref().map_or(0, |r| r.pending_after)
            + self.history.as_ref().map_or(0, |r| r.pending_after)
            + self.code.as_ref().map_or(0, |r| r.pending_after)
    }

    /// Units the provider refused this tick, retired from the queue with the
    /// refusal recorded on the row.
    ///
    /// Deliberately absent from [`Self::problems`]: a quarantined unit is a
    /// durable fact stored per row and read by `cas doctor` from the store, not
    /// a failure of this run. Folding it into the run's error ledger is exactly
    /// what froze the doctor line at one provider message for three days
    /// (GH #695).
    pub fn quarantined(&self) -> usize {
        self.knowledge.as_ref().map_or(0, |r| r.quarantined)
            + self.history.as_ref().map_or(0, |r| r.quarantined)
            + self.code.as_ref().map_or(0, |r| r.quarantined)
    }

    /// Every problem worth showing a human, verbatim.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (corpus, report) in [
            ("knowledge", self.knowledge.as_ref()),
            ("history", self.history.as_ref()),
            ("code", self.code.as_ref()),
        ] {
            let Some(report) = report else { continue };
            for message in &report.request_errors {
                out.push(format!("{corpus}: {message}"));
            }
            for (id, message) in &report.errors {
                out.push(format!("{corpus} {id}: {message}"));
            }
            if report.rejected_zero > 0 {
                out.push(format!(
                    "{corpus}: {} unit(s) got an unusable zero vector",
                    report.rejected_zero
                ));
            }
            if report.rejected_dims > 0 {
                out.push(format!(
                    "{corpus}: {} unit(s) got the wrong vector dimension",
                    report.rejected_dims
                ));
            }
        }
        out
    }

    pub fn did_work(&self) -> bool {
        self.embedded() > 0 || self.skipped() > 0 || self.quarantined() > 0
    }
}

/// Should this commit's message be embedded at all?
///
/// Spec §12 Q5 ruling: the exclusion is **heuristic, not structural**. A
/// squash-merge workflow puts real content in merge commits, so keying on
/// `is_merge` would throw away exactly the prose those repositories care about.
/// Only git's own generated subjects — `^Merge (branch|pull request)` — are
/// noise, and only those are skipped.
pub fn is_noise_merge_subject(subject: &str) -> bool {
    let subject = subject.trim_start();
    subject.starts_with("Merge branch") || subject.starts_with("Merge pull request")
}

/// Embedded text for a commit: `subject + "\n" + body` (spec §4.4), capped at
/// [`MAX_EMBED_TEXT_CHARS`].
pub fn commit_embedding_text(commit: &HistoryCommit) -> String {
    let text = match commit
        .body
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
    {
        Some(body) => format!("{}\n{}", commit.subject, body),
        None => commit.subject.clone(),
    };
    cap_embedding_text(text)
}

/// Embedded text for a doc: `title + body` (spec §4.4).
///
/// Comments carry no title and CHANGELOG sections carry no separate body, so
/// both halves are optional and the join drops the empty one rather than
/// embedding a leading blank line that shifts every vector slightly.
pub fn doc_embedding_text(doc: &HistoryDoc) -> String {
    let title = doc.title.as_deref().unwrap_or("").trim();
    let body = doc.body.as_deref().unwrap_or("").trim();
    let text = match (title.is_empty(), body.is_empty()) {
        (false, false) => format!("{title}\n\n{body}"),
        (false, true) => title.to_string(),
        (true, false) => body.to_string(),
        (true, true) => String::new(),
    };
    cap_embedding_text(text)
}

/// Embed up to `limit` pending history units (commits first, then docs).
///
/// Merge-commit noise is retired from the queue without a request and counted
/// in [`EmbedReport::skipped`]: it is not awaiting a vector, it is excluded
/// from having one, and leaving it armed would mean the pending count could
/// never reach zero.
pub fn embed_pending_history(
    store: &dyn HistoryStore,
    embedder: &KnowledgeEmbedder,
    cache: &KnowledgeVectorCache,
    limiter: &RateLimiter,
    limit: usize,
) -> Result<EmbedReport, CasError> {
    let mut report = EmbedReport {
        reindexed: cache.reindexed(),
        ..Default::default()
    };

    // A model change invalidates every cached vector, so the whole corpus must
    // be recomputed — not just the rows that happened to be pending.
    if cache.reindexed() {
        store.mark_all_pending_embedding().map_err(|e| {
            CasError::Other(format!("Failed to re-mark history for embedding: {e}"))
        })?;
    }

    let commits = store
        .list_pending_embedding_commits(limit)
        .map_err(|e| CasError::Other(format!("Failed to list pending commits: {e}")))?;

    let mut units: Vec<EmbedUnit> = Vec::with_capacity(commits.len());
    for commit in &commits {
        if is_noise_merge_subject(&commit.subject) {
            match store.skip_commit_embedding(&commit.sha) {
                Ok(()) => report.skipped += 1,
                Err(e) => report.errors.push((commit.sha.clone(), e.to_string())),
            }
            continue;
        }
        let text = commit_embedding_text(commit);
        if text.trim().is_empty() {
            // Nothing to embed and nothing to retry: a request would come back
            // as a zero vector at best.
            match store.skip_commit_embedding(&commit.sha) {
                Ok(()) => report.skipped += 1,
                Err(e) => report.errors.push((commit.sha.clone(), e.to_string())),
            }
            continue;
        }
        units.push(EmbedUnit::new(
            history_commit_key(&commit.sha),
            commit.sha.clone(),
            text,
        ));
    }

    {
        let mut mark = |sha: &str| store.mark_commit_embedded(sha).map_err(|e| e.to_string());
        let mut quarantine = |sha: &str, error: &str| {
            store
                .quarantine_commit_embedding(sha, error)
                .map_err(|e| e.to_string())
        };
        drain_units_with_quarantine(
            embedder,
            cache,
            &units,
            limiter,
            &mut mark,
            &mut Some(&mut quarantine),
            &mut report,
        );
    }

    // Docs are only attempted when the commit half did not hit a systemic
    // failure: the same auth/rate-limit problem would produce the identical
    // error on a second corpus, and repeating it is exactly what `drain_units`
    // halts to avoid.
    let doc_budget = limit.saturating_sub(commits.len());
    if report.request_errors.is_empty() && doc_budget > 0 {
        let docs = store
            .list_pending_embedding_docs(doc_budget)
            .map_err(|e| CasError::Other(format!("Failed to list pending docs: {e}")))?;

        let mut doc_units: Vec<EmbedUnit> = Vec::with_capacity(docs.len());
        for doc in &docs {
            let text = doc_embedding_text(doc);
            if text.trim().is_empty() {
                match store.mark_doc_embedded(&doc.id) {
                    Ok(()) => report.skipped += 1,
                    Err(e) => report.errors.push((doc.id.clone(), e.to_string())),
                }
                continue;
            }
            doc_units.push(EmbedUnit::new(
                history_doc_key(&doc.id),
                doc.id.clone(),
                text,
            ));
        }

        let mut mark = |id: &str| store.mark_doc_embedded(id).map_err(|e| e.to_string());
        let mut quarantine = |id: &str, error: &str| {
            store
                .quarantine_doc_embedding(id, error)
                .map_err(|e| e.to_string())
        };
        drain_units_with_quarantine(
            embedder,
            cache,
            &doc_units,
            limiter,
            &mut mark,
            &mut Some(&mut quarantine),
            &mut report,
        );
    }

    let (commits_pending, docs_pending) = store
        .count_pending_embedding()
        .map_err(|e| CasError::Other(format!("Failed to count pending history: {e}")))?;
    report.pending_after = (commits_pending + docs_pending).max(0) as usize;

    Ok(report)
}

/// Drain every pending vector this installation owns — the daemon-tick entry
/// point, and the whole point of M7.
///
/// Returns `Ok(DrainReport { capability_absent: true, .. })` when there is no
/// embedder: that is a state of the installation, not a failure of the tick,
/// and the daemon must keep ticking for every other subsystem.
pub fn drain_all_pending(cas_root: &Path, limit: usize) -> Result<DrainReport, CasError> {
    let config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root)
        .unwrap_or_default();

    // First gate: no auth, no embedder, no cache directory on disk, no request.
    let Some(embedder) = KnowledgeEmbedder::from_config(&config) else {
        return Ok(DrainReport {
            capability_absent: true,
            ..Default::default()
        });
    };

    drain_all_pending_with(cas_root, limit, &embedder)
}

/// [`drain_all_pending`] with the capability already resolved.
///
/// The seam exists so a test can drive the *same* code the daemon arm runs
/// against a mock endpoint, instead of proving something about a hand-rolled
/// copy of it. Production has exactly one caller: [`drain_all_pending`].
pub fn drain_all_pending_with(
    cas_root: &Path,
    limit: usize,
    embedder: &KnowledgeEmbedder,
) -> Result<DrainReport, CasError> {
    let mut report = DrainReport::default();
    let cache = KnowledgeVectorCache::open(cas_root, embedder.meta())?;
    // One limiter for the whole tick: the endpoint's budget is shared across
    // corpora, so two independently-paced drains would together exceed it.
    let limiter = RateLimiter::cloud();

    if let Ok(store) = SqliteKnowledgeStore::open(cas_root) {
        // Pages that will never be embedded (none today) would be `skipped`;
        // the knowledge path has no exclusions, so this is a straight drain.
        match embed_pending_pages(&store, embedder, &cache, limit) {
            Ok(page_report) => {
                report.capability_absent |= page_report.capability_absent;
                report.knowledge = Some(page_report);
            }
            Err(e) => {
                report.knowledge = Some(EmbedReport {
                    request_errors: vec![e.to_string()],
                    pending_after: store.count_pending_embedding().unwrap_or(0),
                    ..Default::default()
                });
            }
        }
    }

    if let Ok(store) = SqliteHistoryStore::open(cas_root) {
        match embed_pending_history(&store, embedder, &cache, &limiter, limit) {
            Ok(history_report) => {
                report.capability_absent |= history_report.capability_absent;
                report.history = Some(history_report);
            }
            Err(e) => {
                report.history = Some(EmbedReport {
                    request_errors: vec![e.to_string()],
                    ..Default::default()
                });
            }
        }

        // The honesty ledger (spec §10.1): record that the drain ran and what
        // went wrong, so a failing drain is a fact `cas doctor` can read rather
        // than an inference from a pending count that will not move.
        if let Ok(repo_root) = crate::history::repo_root_for(cas_root) {
            let repository = crate::history::repository_id(&repo_root);
            let problems = report.problems();
            let error = (!problems.is_empty()).then(|| problems.join("; "));
            let _ = store.record_attempt(&repository, SOURCE_EMBEDDINGS, error.as_deref());
        }
    }

    match crate::cloud::code_embeddings::embed_pending_code(cas_root, embedder, &limiter, limit) {
        Ok(code_report) => {
            report.capability_absent |= code_report.capability_absent;
            report.code = Some(code_report);
        }
        Err(e) => {
            report.code = Some(EmbedReport {
                request_errors: vec![e.to_string()],
                ..Default::default()
            });
        }
    }

    Ok(report)
}

/// Per-tick unit budget. A page/commit budget, not a request size: the drain
/// splits it into chunks of `MAX_EMBED_INPUTS_PER_REQUEST`, so this many units
/// costs `ceil(n / 32)` requests.
///
/// 512 puts a full backfill of this repo (~2,100 units, spec §7.1) about five
/// ticks out rather than in one burst, while steady state (~60 units/day) is
/// always covered by a single tick.
pub const DRAIN_BATCH: usize = DEFAULT_EMBED_BATCH;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::embeddings::{EmbeddingMeta, MAX_EMBED_INPUTS_PER_REQUEST, VectorNamespace};
    use cas_store::{IngestBatch, KnowledgePage, PageWrite, SqliteKnowledgeStore};
    use std::sync::{Arc, Mutex};

    /// Answers with one unit vector per input and records the size of every
    /// request it saw. That record is the chunking receipt (AC2).
    struct EchoEmbeddings {
        dims: usize,
        seen: Arc<Mutex<Vec<usize>>>,
    }

    impl wiremock::Respond for EchoEmbeddings {
        fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let n = body
                .get("input")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            self.seen.lock().unwrap().push(n);
            let vectors: Vec<Vec<f32>> = (0..n)
                .map(|i| {
                    let mut v = vec![0.0f32; self.dims];
                    // Vary the vector so two units are not identical rows.
                    v[i % self.dims] = 1.0;
                    v
                })
                .collect();
            wiremock::ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "embeddings": vectors }))
        }
    }

    fn seed_history(root: &Path, commits: &[HistoryCommit], docs: &[HistoryDoc]) {
        let store = cas_store::SqliteHistoryStore::open(root).unwrap();
        if !commits.is_empty() {
            let watermark = commits.last().unwrap().sha.clone();
            store
                .commit_batch("/repo", commits, &[], &watermark, true)
                .unwrap();
        }
        if !docs.is_empty() {
            store
                .upsert_docs("/repo", "github", docs, None, true)
                .unwrap();
        }
    }

    fn seed_pages(root: &Path, titles: &[&str]) {
        let store = SqliteKnowledgeStore::open(root).unwrap();
        store.init().unwrap();
        let pages: Vec<PageWrite> = titles
            .iter()
            .enumerate()
            .map(|(i, title)| {
                let mut page = KnowledgePage::new(format!("cas-kn10{i}"), "architecture", *title);
                page.snippet = format!("snippet for {title}");
                PageWrite {
                    page,
                    body: format!("body of {title}"),
                }
            })
            .collect();
        store
            .commit_ingest(&IngestBatch {
                pages,
                sources: Vec::new(),
                tombstones: Vec::new(),
            })
            .unwrap();
    }

    fn doc(id: &str, title: &str, body: &str) -> HistoryDoc {
        HistoryDoc {
            id: id.to_string(),
            doc_kind: "issue".to_string(),
            title: Some(title.to_string()),
            body: Some(body.to_string()),
            updated_at: Some("2026-08-08T00:00:00Z".to_string()),
            repository: "/repo".to_string(),
            source: "github".to_string(),
            ..Default::default()
        }
    }

    fn commit(sha: &str, subject: &str, body: Option<&str>) -> HistoryCommit {
        HistoryCommit {
            sha: sha.to_string(),
            short_sha: sha.chars().take(8).collect(),
            subject: subject.to_string(),
            body: body.map(str::to_string),
            committed_at: "2026-08-08T00:00:00Z".to_string(),
            repository: "/repo".to_string(),
            symbol_mapping: "pending".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn merge_exclusion_is_heuristic_not_structural() {
        // Git's own generated subjects: noise, skipped (spec §12 Q5).
        assert!(is_noise_merge_subject("Merge branch 'main' into feature"));
        assert!(is_noise_merge_subject(
            "Merge pull request #12 from octo/patch-1"
        ));
        assert!(is_noise_merge_subject("Merge branches 'a' and 'b'"));

        // A squash-merge commit is a merge commit carrying REAL content. The
        // ruling is explicit that these must still be embedded — a structural
        // `is_merge` filter would silently drop the entire history of any
        // squash-merge repository.
        assert!(!is_noise_merge_subject(
            "Merge the retry path into one place (#412)"
        ));
        assert!(!is_noise_merge_subject("fix(cas-db6e): drain on the tick"));
        assert!(!is_noise_merge_subject("Mergesort the candidates"));
    }

    #[test]
    fn commit_text_is_subject_plus_body() {
        assert_eq!(
            commit_embedding_text(&commit("a".repeat(40).as_str(), "subject", Some("body"))),
            "subject\nbody"
        );
        // An empty body must not leave a trailing newline: two commits with the
        // same subject would otherwise embed as different texts depending on
        // whether git recorded an empty body or none.
        assert_eq!(
            commit_embedding_text(&commit("b".repeat(40).as_str(), "subject", Some("  "))),
            "subject"
        );
        assert_eq!(
            commit_embedding_text(&commit("c".repeat(40).as_str(), "subject", None)),
            "subject"
        );
    }

    #[test]
    fn doc_text_joins_only_the_halves_that_exist() {
        let mut doc = HistoryDoc {
            id: "gh:issue:1".into(),
            title: Some("Title".into()),
            body: Some("Body".into()),
            ..Default::default()
        };
        assert_eq!(doc_embedding_text(&doc), "Title\n\nBody");

        doc.title = None;
        assert_eq!(doc_embedding_text(&doc), "Body");

        doc.body = None;
        doc.title = Some("Only title".into());
        assert_eq!(doc_embedding_text(&doc), "Only title");

        doc.title = None;
        assert_eq!(doc_embedding_text(&doc), "");
    }

    /// AC1 + AC2. The daemon's drain — no `cas cloud sync` anywhere — empties
    /// BOTH queues, and every request it issued stayed inside the endpoint's
    /// 32-input cap.
    #[tokio::test]
    async fn one_drain_empties_history_and_knowledge_and_never_oversends() {
        use wiremock::{Mock, MockServer, matchers::method, matchers::path};

        let seen = Arc::new(Mutex::new(Vec::new()));
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embeddings"))
            .respond_with(EchoEmbeddings {
                dims: 4,
                seen: Arc::clone(&seen),
            })
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let seen_for_task = Arc::clone(&seen);

        let (report, history_pending, pages_pending, cached) =
            tokio::task::spawn_blocking(move || {
                // 70 commits: 32 + 32 + 6 requests, never one oversized call.
                let commits: Vec<HistoryCommit> = (0..70)
                    .map(|i| {
                        commit(
                            &format!("{i:040x}"),
                            &format!("feat: change number {i}"),
                            Some("body text"),
                        )
                    })
                    .collect();
                let docs = vec![
                    doc("gh:issue:1", "First issue", "Something broke"),
                    doc("gh:issue:2", "Second issue", "Something else broke"),
                ];
                seed_history(&root, &commits, &docs);
                seed_pages(&root, &["Build System", "Retrieval"]);

                let embedder =
                    KnowledgeEmbedder::new(&endpoint, "test-token").with_model("test-model", 4);

                // Loop the tick, exactly as the daemon does: the property is
                // "drains to zero without a human", not "does it in one pass".
                let mut last = None;
                for _ in 0..5 {
                    let report = drain_all_pending_with(&root, DRAIN_BATCH, &embedder).unwrap();
                    let done = report.pending_after() == 0;
                    last = Some(report);
                    if done {
                        break;
                    }
                }

                let history = cas_store::SqliteHistoryStore::open(&root).unwrap();
                let (commits_pending, docs_pending) = history.count_pending_embedding().unwrap();
                let knowledge = SqliteKnowledgeStore::open(&root).unwrap();
                let cache = KnowledgeVectorCache::open(
                    &root,
                    EmbeddingMeta::new("cas-cloud", "test-model", 4),
                )
                .unwrap();
                (
                    last.unwrap(),
                    commits_pending + docs_pending,
                    knowledge.count_pending_embedding().unwrap(),
                    cache.count().unwrap(),
                )
            })
            .await
            .unwrap();

        assert!(!report.capability_absent);
        assert!(
            report.problems().is_empty(),
            "problems: {:?}",
            report.problems()
        );
        assert_eq!(history_pending, 0, "history queue must drain to zero");
        assert_eq!(pages_pending, 0, "knowledge queue must drain to zero");
        // 70 commits + 2 docs + 2 pages = 74 vectors.
        assert_eq!(cached, 74, "every embedded unit must be cached");
        assert_eq!(report.pending_after(), 0);

        let sizes = seen_for_task.lock().unwrap().clone();
        assert!(
            sizes.iter().all(|n| *n <= MAX_EMBED_INPUTS_PER_REQUEST),
            "no request may exceed the endpoint cap: {sizes:?}"
        );
        // The chunking receipt for the 70-commit corpus.
        assert!(
            sizes.contains(&MAX_EMBED_INPUTS_PER_REQUEST) && sizes.contains(&6),
            "expected 32/32/6 chunking for 70 commits, saw {sizes:?}"
        );
        assert_eq!(
            sizes.iter().sum::<usize>(),
            74,
            "every unit must have been sent exactly once: {sizes:?}"
        );
    }

    /// AC5. Merge noise is retired from the queue without ever being sent, and
    /// a merge commit carrying real prose still gets embedded.
    #[tokio::test]
    async fn merge_noise_is_skipped_but_still_leaves_the_queue() {
        use wiremock::{Mock, MockServer, matchers::method, matchers::path};

        let seen = Arc::new(Mutex::new(Vec::new()));
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embeddings"))
            .respond_with(EchoEmbeddings {
                dims: 4,
                seen: Arc::clone(&seen),
            })
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let (report, pending, cached_keys) = tokio::task::spawn_blocking(move || {
            let mut noise = commit(&format!("{:040x}", 1), "Merge branch 'main' into x", None);
            noise.is_merge = true;
            let mut squash = commit(
                &format!("{:040x}", 2),
                "Merge the retry paths (#412)",
                Some("real content"),
            );
            squash.is_merge = true;
            seed_history(&root, &[noise, squash], &[]);

            let embedder =
                KnowledgeEmbedder::new(&endpoint, "test-token").with_model("test-model", 4);
            let cache =
                KnowledgeVectorCache::open(&root, EmbeddingMeta::new("cas-cloud", "test-model", 4))
                    .unwrap();
            let store = cas_store::SqliteHistoryStore::open(&root).unwrap();
            let report =
                embed_pending_history(&store, &embedder, &cache, &RateLimiter::cloud(), 100)
                    .unwrap();
            let (commits_pending, docs_pending) = store.count_pending_embedding().unwrap();
            (
                report,
                commits_pending + docs_pending,
                cache.count().unwrap(),
            )
        })
        .await
        .unwrap();

        assert_eq!(report.skipped, 1, "git's generated merge subject is noise");
        assert_eq!(
            report.embedded, 1,
            "a merge commit with real content must still be embedded (§12 Q5)"
        );
        assert_eq!(
            pending, 0,
            "the skipped merge must leave the queue, or pending can never reach zero"
        );
        assert_eq!(cached_keys, 1, "the skipped merge must have no vector");
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[1],
            "the skipped merge must never reach the provider"
        );
    }

    /// A provider that refuses any request containing `poison`, in the exact
    /// shape the deployed cloud emits today: the upstream 400 arrives wrapped
    /// as a gateway 502 (`{"error":"Embedding provider returned 400"}`,
    /// GH #695). Every other request succeeds.
    struct RejectsPoison {
        dims: usize,
        poison: String,
        requests: std::sync::Arc<std::sync::Mutex<usize>>,
    }

    impl wiremock::Respond for RejectsPoison {
        fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
            *self.requests.lock().unwrap() += 1;
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let inputs = body.get("input").and_then(|v| v.as_array()).unwrap();
            if inputs
                .iter()
                .any(|i| i.as_str().is_some_and(|t| t.contains(&self.poison)))
            {
                return wiremock::ResponseTemplate::new(502).set_body_json(
                    serde_json::json!({"error": "Embedding provider returned 400"}),
                );
            }
            let vectors: Vec<Vec<f32>> = (0..inputs.len())
                .map(|i| {
                    let mut v = vec![0.0f32; self.dims];
                    v[i % self.dims] = 1.0;
                    v
                })
                .collect();
            wiremock::ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "embeddings": vectors }))
        }
    }

    /// GH #695, the whole defect in one test: one unit the provider refuses
    /// must not strand the corpus behind it. The refused unit is quarantined
    /// with the provider's message, every neighbour in the same chunk is
    /// embedded in the SAME run, and pending reaches zero.
    #[tokio::test]
    async fn one_refused_unit_is_quarantined_and_its_neighbours_still_embed() {
        use wiremock::{Mock, MockServer, matchers::method, matchers::path};

        let requests = Arc::new(Mutex::new(0usize));
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embeddings"))
            .respond_with(RejectsPoison {
                dims: 4,
                poison: "POISON-BODY".to_string(),
                requests: Arc::clone(&requests),
            })
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let (report, pending, quarantined, last_error) = tokio::task::spawn_blocking(move || {
            // 40 commits so the drain issues more than one 32-input chunk, with
            // the poison unit buried in the middle of the first one.
            let commits: Vec<HistoryCommit> = (0..40)
                .map(|i| {
                    commit(
                        &format!("{i:040x}"),
                        &format!("feat: change number {i}"),
                        Some(if i == 11 { "POISON-BODY" } else { "body text" }),
                    )
                })
                .collect();
            seed_history(&root, &commits, &[]);

            let embedder =
                KnowledgeEmbedder::new(&endpoint, "test-token").with_model("test-model", 4);
            let report = drain_all_pending_with(&root, DRAIN_BATCH, &embedder).unwrap();
            let store = cas_store::SqliteHistoryStore::open(&root).unwrap();
            let (c, d) = store.count_pending_embedding().unwrap();
            let (qc, qd) = store.count_quarantined_embedding().unwrap();
            (
                report,
                c + d,
                qc + qd,
                store.last_quarantined_embedding_error().unwrap(),
            )
        })
        .await
        .unwrap();

        assert_eq!(report.embedded(), 39, "every healthy unit embeds this run");
        assert_eq!(report.quarantined(), 1);
        assert_eq!(quarantined, 1, "the refusal is durable on the row");
        assert_eq!(
            pending, 0,
            "the backlog must reach zero instead of parking behind one unit"
        );
        assert!(
            last_error.is_some_and(|e| e.contains("provider returned 400")),
            "the provider's own words must survive for reporting"
        );
        assert!(
            report.problems().is_empty(),
            "a quarantined unit is a stored fact, not a permanent drain error: {:?}",
            report.problems()
        );
        // Bisecting 32 inputs costs a bounded handful of extra requests, not a
        // per-unit retry storm.
        let issued = *requests.lock().unwrap();
        assert!(
            (2..=16).contains(&issued),
            "expected a bisect, not a scan: {issued} requests"
        );
    }

    /// The docs half must not be starved by a refused commit. Before GH #695's
    /// fix a failing commit chunk set `request_errors`, and the doc queue was
    /// skipped entirely on every tick.
    #[tokio::test]
    async fn a_refused_commit_does_not_starve_the_doc_queue() {
        use wiremock::{Mock, MockServer, matchers::method, matchers::path};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embeddings"))
            .respond_with(RejectsPoison {
                dims: 4,
                poison: "POISON-BODY".to_string(),
                requests: Arc::new(Mutex::new(0)),
            })
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let (report, pending) = tokio::task::spawn_blocking(move || {
            seed_history(
                &root,
                &[commit(&format!("{:040x}", 1), "feat: poison", Some("POISON-BODY"))],
                &[doc("gh:issue:1", "An issue", "ordinary text")],
            );
            let embedder =
                KnowledgeEmbedder::new(&endpoint, "test-token").with_model("test-model", 4);
            let report = drain_all_pending_with(&root, DRAIN_BATCH, &embedder).unwrap();
            let store = cas_store::SqliteHistoryStore::open(&root).unwrap();
            let (c, d) = store.count_pending_embedding().unwrap();
            (report, c + d)
        })
        .await
        .unwrap();

        assert_eq!(report.quarantined(), 1);
        assert_eq!(report.embedded(), 1, "the doc embeds despite the bad commit");
        assert_eq!(pending, 0);
    }

    /// A gateway outage is not a refusal. A 502 whose body does not name a
    /// provider 4xx must keep its units pending for the next tick, because a
    /// retry genuinely can succeed.
    #[tokio::test]
    async fn a_plain_gateway_failure_still_defers_instead_of_quarantining() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method, matchers::path};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embeddings"))
            .respond_with(ResponseTemplate::new(502).set_body_string("upstream unavailable"))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let (report, pending, quarantined) = tokio::task::spawn_blocking(move || {
            seed_history(
                &root,
                &[commit(&format!("{:040x}", 2), "feat: a change", None)],
                &[],
            );
            let embedder =
                KnowledgeEmbedder::new(&endpoint, "test-token").with_model("test-model", 4);
            let report = drain_all_pending_with(&root, DRAIN_BATCH, &embedder).unwrap();
            let store = cas_store::SqliteHistoryStore::open(&root).unwrap();
            let (c, d) = store.count_pending_embedding().unwrap();
            let (qc, qd) = store.count_quarantined_embedding().unwrap();
            (report, c + d, qc + qd)
        })
        .await
        .unwrap();

        assert_eq!(report.quarantined(), 0, "an outage is not the unit's fault");
        assert_eq!(quarantined, 0);
        assert_eq!(pending, 1, "the unit waits for the next tick");
        assert!(report.problems().iter().any(|p| p.contains("502")));
    }

    /// The measured cliff from GH #695: bodies over ~34k chars are refused by
    /// the model. Capping the text keeps those commits searchable instead of
    /// quarantining them.
    #[test]
    fn oversized_commit_text_is_capped_on_a_char_boundary() {
        let huge = "é".repeat(MAX_EMBED_TEXT_CHARS * 2);
        let squashed = commit(&format!("{:040x}", 9), "squash: everything", Some(&huge));
        let text = commit_embedding_text(&squashed);

        assert_eq!(text.chars().count(), MAX_EMBED_TEXT_CHARS);
        assert!(
            text.starts_with("squash: everything"),
            "the subject is the most useful part and must survive the cap"
        );

        // Ordinary units are untouched.
        let small = commit(&format!("{:040x}", 10), "feat: small", Some("body"));
        assert_eq!(commit_embedding_text(&small), "feat: small\nbody");
    }

    /// AC3. A request-level failure is reported and the rows stay pending —
    /// never a silent "0 embedded".
    #[tokio::test]
    async fn a_failing_endpoint_is_reported_and_leaves_the_queue_intact() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method, matchers::path};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embeddings"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let (report, pending) = tokio::task::spawn_blocking(move || {
            seed_history(
                &root,
                &[commit(&format!("{:040x}", 7), "feat: a change", None)],
                &[doc("gh:issue:9", "An issue", "text")],
            );
            let embedder =
                KnowledgeEmbedder::new(&endpoint, "test-token").with_model("test-model", 4);
            let report = drain_all_pending_with(&root, DRAIN_BATCH, &embedder).unwrap();
            let store = cas_store::SqliteHistoryStore::open(&root).unwrap();
            let (c, d) = store.count_pending_embedding().unwrap();
            (report, c + d)
        })
        .await
        .unwrap();

        let problems = report.problems();
        assert!(
            problems.iter().any(|p| p.contains("500")),
            "the failure must be reported verbatim, got {problems:?}"
        );
        assert_eq!(report.embedded(), 0);
        assert_eq!(pending, 2, "nothing may be marked embedded after a failure");
        assert!(
            !report.capability_absent,
            "a 500 is a failure, not a boundary"
        );
    }

    /// AC3 boundary half. An endpoint with no embedding capability is a state
    /// of the installation, reported as such rather than as an error.
    #[tokio::test]
    async fn an_endpoint_without_the_capability_is_a_declared_boundary() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let report = tokio::task::spawn_blocking(move || {
            seed_history(
                &root,
                &[commit(&format!("{:040x}", 3), "feat: a change", None)],
                &[],
            );
            let embedder =
                KnowledgeEmbedder::new(&endpoint, "test-token").with_model("test-model", 4);
            drain_all_pending_with(&root, DRAIN_BATCH, &embedder).unwrap()
        })
        .await
        .unwrap();

        assert!(
            report.capability_absent,
            "404 means the endpoint has no /api/embeddings"
        );
        assert_eq!(report.embedded(), 0);
    }

    /// AC4. History vectors and knowledge vectors share one LMDB env and must
    /// never be read as each other.
    #[tokio::test]
    async fn history_vectors_are_invisible_to_the_knowledge_namespace() {
        use wiremock::{Mock, MockServer, matchers::method, matchers::path};

        let seen = Arc::new(Mutex::new(Vec::new()));
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embeddings"))
            .respond_with(EchoEmbeddings {
                dims: 4,
                seen: Arc::clone(&seen),
            })
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let (knowledge_hits, history_hits, knowledge_count, history_count) =
            tokio::task::spawn_blocking(move || {
                seed_history(
                    &root,
                    &[commit(&format!("{:040x}", 5), "feat: a change", None)],
                    &[doc("gh:issue:3", "An issue", "text")],
                );
                seed_pages(&root, &["Build System"]);

                let embedder =
                    KnowledgeEmbedder::new(&endpoint, "test-token").with_model("test-model", 4);
                drain_all_pending_with(&root, DRAIN_BATCH, &embedder).unwrap();

                let cache = KnowledgeVectorCache::open(
                    &root,
                    EmbeddingMeta::new("cas-cloud", "test-model", 4),
                )
                .unwrap();
                // A query that is close to everything: the filter, not the
                // scores, has to be what keeps the namespaces apart.
                let query = vec![1.0f32, 1.0, 1.0, 1.0];
                (
                    cache.nearest(&query, 50).unwrap(),
                    cache
                        .nearest_in(VectorNamespace::History, &query, 50)
                        .unwrap(),
                    cache.count_in(VectorNamespace::Knowledge).unwrap(),
                    cache.count_in(VectorNamespace::History).unwrap(),
                )
            })
            .await
            .unwrap();

        assert!(
            knowledge_hits
                .iter()
                .all(|(id, _)| !id.starts_with("history:")),
            "the knowledge channel resolves ids as page ids; a history key here is a wrong \
             answer, not a ranking nit: {knowledge_hits:?}"
        );
        assert!(
            !knowledge_hits.is_empty(),
            "the page vector must still be found"
        );
        assert!(
            history_hits
                .iter()
                .all(|(id, _)| id.starts_with("history:")),
            "{history_hits:?}"
        );
        assert_eq!(history_hits.len(), 2, "one commit + one doc");
        assert_eq!(knowledge_count, 1);
        assert_eq!(history_count, 2);
    }

    /// AC2's rate-limit half. The limiter blocks once the window is full and
    /// costs nothing while it is not.
    #[test]
    fn the_limiter_paces_only_when_the_window_is_full() {
        use std::time::{Duration, Instant};

        let limiter = RateLimiter::new(2, Duration::from_millis(200));
        let started = Instant::now();
        limiter.acquire();
        limiter.acquire();
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "requests under the cap must not sleep"
        );
        assert_eq!(limiter.in_window(), 2);

        // The third is over the cap and must wait out the window.
        limiter.acquire();
        assert!(
            started.elapsed() >= Duration::from_millis(150),
            "the third request must be paced, waited {:?}",
            started.elapsed()
        );

        // The production limiter is the endpoint's published contract.
        let cloud = RateLimiter::cloud();
        for _ in 0..10 {
            cloud.acquire();
        }
        assert_eq!(
            cloud.in_window(),
            10,
            "a small run stays well under 120/60s"
        );
    }
}
