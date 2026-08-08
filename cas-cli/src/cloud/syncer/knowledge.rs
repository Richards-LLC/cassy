//! Team distribution of distilled knowledge pages over the existing
//! `/api/sync` push/pull endpoints (T5).
//!
//! # What the cloud is, and is not, for knowledge
//!
//! Local SQLite plus the markdown bodies on disk are the **source of truth**.
//! The cloud is a transport: it carries pages to teammates and (separately)
//! computes embeddings. A machine that never talks to the cloud has a fully
//! working knowledge base; a machine that does gets the same pages its
//! teammates distilled.
//!
//! # Two invariants
//!
//! 1. **The `locked` bit rides along and is honoured on arrival.** A locked
//!    page is one a human took ownership of. Incoming pages are applied via
//!    [`KnowledgeStore::commit_ingest`], whose `ON CONFLICT ... WHERE
//!    knowledge_pages.locked = 0` clause means a teammate's copy can never
//!    overwrite a page you locked — the same guard that stops distillation
//!    from doing it. The bit itself is transmitted, so a page created from a
//!    remote copy arrives locked if it was locked upstream.
//! 2. **No auth, no calls.** Every entry point returns an empty result
//!    without touching the network when the user is not logged in.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::cloud::get_project_canonical_id;
use crate::cloud::syncer::{CloudSyncer, SyncResult};
use crate::error::CasError;
use cas_store::{IngestBatch, KnowledgePage, KnowledgeStore, PageWrite};
use cas_types::ShareScope;

/// Metadata key holding the high-water mark for knowledge pushes.
const LAST_PUSH_KEY: &str = "last_knowledge_push_at";
/// Metadata key holding the high-water mark for knowledge pulls.
const LAST_PULL_KEY: &str = "last_knowledge_pull_at";
/// Entity-type key used in the push payload and the pull response.
pub const KNOWLEDGE_ENTITY: &str = "knowledge_pages";

/// Percent-encode a query-string *value*.
///
/// Conservative allow-list: anything outside unreserved characters is escaped,
/// so a team id containing `&`, `=`, `/` or a space cannot smuggle an extra
/// query parameter into a scoped pull URL.
pub(crate) fn encode_query_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Wire shape for one knowledge page.
///
/// The body travels inline. Pages are distilled summaries (a page is
/// kilobytes, not megabytes) and the push path already gzips the payload, so
/// a second fetch round-trip per page would buy nothing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnowledgePageRecord {
    pub id: String,
    pub page_type: String,
    pub title: String,
    pub rel_path: String,
    #[serde(default)]
    pub snippet: String,
    #[serde(default)]
    pub body: String,
    /// User-sovereignty bit. Transmitted so a locked page stays locked for
    /// everyone who receives it.
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub sources: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Who may see this page — the same `private | team` vocabulary entries
    /// use. Absent on older payloads, which are treated as `private`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<ShareScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_canonical_id: Option<String>,
}

impl KnowledgePageRecord {
    /// Build a wire record from a stored page plus its body.
    pub fn from_page(page: &KnowledgePage, body: String, share: ShareScope) -> Self {
        Self {
            id: page.id.clone(),
            page_type: page.page_type.clone(),
            title: page.title.clone(),
            rel_path: page.rel_path.clone(),
            snippet: page.snippet.clone(),
            body,
            locked: page.locked,
            sources: page.sources.clone(),
            created_at: page.created_at,
            updated_at: page.updated_at,
            share: Some(share),
            project_canonical_id: get_project_canonical_id(),
        }
    }

    /// Convert an incoming record into a writable page.
    ///
    /// `pending_embedding` is forced true: a vector computed on a teammate's
    /// machine lives in *their* local cache, so this machine has to embed the
    /// page itself before the semantic channel can retrieve it.
    pub fn into_page_write(self) -> PageWrite {
        let mut page = KnowledgePage::new(self.id, self.page_type, self.title);
        // Trust the sender's canonical path rather than recomputing it: a
        // future change to the slug rules must not silently fork a page into
        // two rel_paths across machines running different CAS versions.
        page.rel_path = self.rel_path;
        page.snippet = self.snippet;
        page.locked = self.locked;
        page.sources = self.sources;
        page.created_at = self.created_at;
        page.updated_at = self.updated_at;
        page.pending_embedding = true;
        PageWrite {
            page,
            body: self.body,
        }
    }
}

/// Outcome of a knowledge pull.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnowledgePullReport {
    /// Pages created or updated locally.
    pub applied: usize,
    /// Pages the local store refused to overwrite because the local copy is
    /// locked. Not an error — this is user sovereignty working.
    pub locked_preserved: usize,
    /// Per-page failures (rel_path, message).
    pub errors: Vec<(String, String)>,
}

/// Share scope for knowledge pages under the current cloud configuration.
///
/// Pages are distilled from the repository itself, so they are project data
/// by construction: when a team is configured they are team-visible, and
/// otherwise they stay private to the account. There is no per-page override
/// today — `KnowledgePage` has no `share` column — and inventing one here
/// would let the wire format claim a sovereignty guarantee the local store
/// cannot enforce.
pub fn knowledge_share_scope(team_configured: bool) -> ShareScope {
    if team_configured {
        ShareScope::Team
    } else {
        ShareScope::Private
    }
}

impl CloudSyncer {
    /// Push knowledge pages changed since the last successful push.
    ///
    /// Returns the number of pages sent. Returns `Ok(0)` without any network
    /// activity when the user is not logged in.
    pub fn push_knowledge_pages(&self, store: &dyn KnowledgeStore) -> Result<usize, CasError> {
        if !self.is_available() {
            return Ok(0);
        }
        let token = self
            .cloud_config
            .token
            .clone()
            .ok_or_else(|| CasError::Other("Not logged in".to_string()))?;

        let since = self
            .queue()
            .get_metadata(LAST_PUSH_KEY)?
            .and_then(|raw| DateTime::parse_from_rfc3339(&raw).ok())
            .map(|dt| dt.with_timezone(&Utc));

        let share = knowledge_share_scope(self.cloud_config.active_team_id().is_some());

        let pages = store
            .list_pages()
            .map_err(|e| CasError::Other(format!("Failed to list knowledge pages: {e}")))?;

        let mut records = Vec::new();
        for page in pages {
            if let Some(since) = since {
                if page.updated_at <= since {
                    continue;
                }
            }
            let body = match store.read_body(&page.rel_path) {
                Ok(body) => body,
                Err(e) => {
                    // A missing body is a local corruption, not a reason to
                    // abandon the whole push.
                    warn!(page = %page.id, error = %e, "skipping knowledge page with unreadable body");
                    continue;
                }
            };
            records.push(serde_json::to_value(KnowledgePageRecord::from_page(
                &page, body, share,
            ))?);
        }

        if records.is_empty() {
            return Ok(0);
        }

        let count = records.len();
        self.push_sub_batch(records, KNOWLEDGE_ENTITY, &token)?;
        let _ = self
            .queue()
            .set_metadata(LAST_PUSH_KEY, &Utc::now().to_rfc3339());
        Ok(count)
    }

    /// Pull knowledge pages the team has shared since the last pull.
    ///
    /// Returns an empty report without any network activity when the user is
    /// not logged in.
    pub fn pull_knowledge_pages(
        &self,
        store: &dyn KnowledgeStore,
    ) -> Result<KnowledgePullReport, CasError> {
        let mut report = KnowledgePullReport::default();
        if !self.is_available() {
            return Ok(report);
        }
        let token = self
            .cloud_config
            .token
            .clone()
            .ok_or_else(|| CasError::Other("Not logged in".to_string()))?;

        let mut params = vec![format!("types={KNOWLEDGE_ENTITY}")];
        if let Some(since) = self.queue().get_metadata(LAST_PULL_KEY)? {
            params.push(format!("since={since}"));
        }
        // Send the active team so the server can narrow to it. Without this a
        // user who belongs to two teams that share one project_canonical_id
        // pulls the UNION of both teams' pages — cross-team knowledge bleed
        // (cas-f177). Project scope alone does not partition teams.
        //
        // Absent team_id keeps the previous server behaviour, so a personal
        // (teamless) install is unaffected.
        if let Some(team_id) = self
            .cloud_config
            .team_id
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            params.push(format!("team_id={}", encode_query_value(team_id)));
        }
        // Fail closed on an unresolvable project scope, exactly like the
        // canonical builder: without `project_id=` this would ask the server
        // for every project's knowledge pages (cas-2eb3 / cas-ed15).
        let (url, _project_id) =
            super::pull::build_scoped_pull_url(&self.cloud_config.endpoint, &params)?;

        let response = ureq::get(&url)
            .timeout(self.config.timeout)
            .set("Authorization", &format!("Bearer {token}"))
            .call();

        let body: serde_json::Value = match response {
            Ok(resp) => resp
                .into_json()
                .map_err(|e| CasError::Other(format!("Failed to parse pull response: {e}")))?,
            Err(ureq::Error::Status(code, resp)) => {
                let text = resp.into_string().unwrap_or_default();
                return Err(CasError::Other(format!(
                    "Knowledge pull failed with status {code}: {text}"
                )));
            }
            Err(ureq::Error::Transport(e)) => {
                return Err(CasError::Other(format!("Network error: {e}")));
            }
        };

        let incoming = body
            .get(KNOWLEDGE_ENTITY)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for raw in incoming {
            let record: KnowledgePageRecord = match serde_json::from_value(raw) {
                Ok(record) => record,
                Err(e) => {
                    report
                        .errors
                        .push((String::from("<unparseable>"), e.to_string()));
                    continue;
                }
            };
            let rel_path = record.rel_path.clone();
            match self.apply_knowledge_record(store, record) {
                Ok(true) => report.applied += 1,
                Ok(false) => report.locked_preserved += 1,
                Err(e) => report.errors.push((rel_path, e.to_string())),
            }
        }

        let _ = self
            .queue()
            .set_metadata(LAST_PULL_KEY, &Utc::now().to_rfc3339());

        Ok(report)
    }

    /// Apply one incoming page. `Ok(false)` means the local copy is locked and
    /// was deliberately preserved.
    ///
    /// Committed one page at a time on purpose: `commit_ingest` aborts the
    /// whole batch on an id/rel_path collision, and one teammate's odd page
    /// must not discard every other page in the same pull.
    fn apply_knowledge_record(
        &self,
        store: &dyn KnowledgeStore,
        record: KnowledgePageRecord,
    ) -> Result<bool, CasError> {
        let write = record.into_page_write();
        let batch = IngestBatch {
            pages: vec![write],
            sources: Vec::new(),
            tombstones: Vec::new(),
        };
        let report = store
            .commit_ingest(&batch)
            .map_err(|e| CasError::Other(format!("Failed to apply knowledge page: {e}")))?;
        Ok(report.pages_written > 0)
    }
}

impl SyncResult {
    /// Fold a knowledge push/pull into an existing sync result.
    pub fn with_knowledge(mut self, pushed: usize, pulled: &KnowledgePullReport) -> Self {
        self.pushed_knowledge_pages = pushed;
        self.pulled_knowledge_pages = pulled.applied;
        for (rel_path, message) in &pulled.errors {
            self.errors
                .push(format!("Knowledge page {rel_path} error: {message}"));
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::{CloudConfig, CloudSyncerConfig, SyncQueue};
    use cas_store::SqliteKnowledgeStore;
    use std::sync::Arc;

    fn syncer(endpoint: Option<&str>, root: &std::path::Path) -> CloudSyncer {
        let queue = Arc::new(SyncQueue::open(root).unwrap());
        queue.init().unwrap();
        let config = CloudConfig {
            endpoint: endpoint.unwrap_or("https://example.invalid").to_string(),
            token: endpoint.map(|_| "test-token".to_string()),
            ..Default::default()
        };
        CloudSyncer::new(queue, config, CloudSyncerConfig::default())
    }

    fn seeded_store(root: &std::path::Path) -> SqliteKnowledgeStore {
        let store = SqliteKnowledgeStore::open(root).unwrap();
        let mut page = KnowledgePage::new("cas-kn001", "architecture", "Build System");
        page.snippet = "How the build works".to_string();
        store
            .commit_ingest(&IngestBatch {
                pages: vec![PageWrite {
                    page,
                    body: "# Build System\n\nZig linker.".to_string(),
                }],
                sources: Vec::new(),
                tombstones: Vec::new(),
            })
            .unwrap();
        store
    }

    fn remote_record(title: &str, body: &str, locked: bool) -> serde_json::Value {
        let mut page = KnowledgePage::new("cas-kn900", "architecture", title);
        page.snippet = "remote snippet".to_string();
        page.locked = locked;
        serde_json::to_value(KnowledgePageRecord::from_page(
            &page,
            body.to_string(),
            ShareScope::Team,
        ))
        .unwrap()
    }

    #[test]
    fn share_scope_follows_team_configuration() {
        assert_eq!(knowledge_share_scope(true), ShareScope::Team);
        assert_eq!(knowledge_share_scope(false), ShareScope::Private);
    }

    #[test]
    fn wire_record_round_trips_the_locked_bit() {
        let mut page = KnowledgePage::new("cas-kn001", "architecture", "Build System");
        page.locked = true;
        let record = KnowledgePageRecord::from_page(&page, "body".into(), ShareScope::Team);
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["locked"], serde_json::json!(true));
        assert_eq!(json["share"], serde_json::json!("team"));
        let back: KnowledgePageRecord = serde_json::from_value(json).unwrap();
        assert_eq!(back, record);
        assert!(back.into_page_write().page.locked);
    }

    #[test]
    fn incoming_pages_always_arrive_pending_embedding() {
        // A teammate's vector lives in a teammate's cache; this machine has
        // to compute its own or the page is semantically invisible here.
        let mut page = KnowledgePage::new("cas-kn001", "architecture", "Build System");
        page.pending_embedding = false;
        let record = KnowledgePageRecord::from_page(&page, "body".into(), ShareScope::Team);
        assert!(record.into_page_write().page.pending_embedding);
    }

    #[test]
    fn logged_out_push_and_pull_make_no_network_calls() {
        // The endpoint is unroutable: if either path touched the network this
        // would fail rather than return an empty result.
        let tmp = tempfile::tempdir().unwrap();
        let store = seeded_store(tmp.path());
        let syncer = syncer(None, tmp.path());
        assert_eq!(syncer.push_knowledge_pages(&store).unwrap(), 0);
        assert_eq!(
            syncer.pull_knowledge_pages(&store).unwrap(),
            KnowledgePullReport::default()
        );
    }

    #[tokio::test]
    async fn pushes_pages_and_advances_the_high_water_mark() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/sync/push"))
            .and(header("Authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let (first, second) = tokio::task::spawn_blocking(move || {
            let store = seeded_store(&root);
            let syncer = syncer(Some(&endpoint), &root);
            let first = syncer.push_knowledge_pages(&store).unwrap();
            // Nothing changed since the mark was written.
            let second = syncer.push_knowledge_pages(&store).unwrap();
            (first, second)
        })
        .await
        .unwrap();

        assert_eq!(first, 1, "the seeded page must be pushed");
        assert_eq!(second, 0, "an unchanged page must not be re-pushed");
    }

    /// The body a page carries must survive push → wire → pull unchanged,
    /// byte for byte. Anything less and a teammate's copy quietly differs from
    /// yours: trailing whitespace stripped, CRLF rewritten, a missing final
    /// newline added back. The page IS the body; a lossy transport is a
    /// corrupted wiki.
    #[tokio::test]
    async fn a_page_body_round_trips_byte_identically_through_push_and_pull() {
        use std::io::Read;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Deliberately hostile: CRLF, a lone CR, tabs, trailing spaces, an
        // unterminated last line, non-ASCII, and a NUL-adjacent control char.
        let body = "# Build System\r\n\ttabbed\ttext   \nunicode: → ✅ ünïcødé\rlone-cr\n\n\ntrailing spaces:   \nno trailing newline";

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/sync/push"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let push_root = tempfile::tempdir().unwrap();
        let push_path = push_root.path().to_path_buf();
        let body_for_push = body.to_string();
        tokio::task::spawn_blocking(move || {
            let store = SqliteKnowledgeStore::open(&push_path).unwrap();
            let mut page = KnowledgePage::new("cas-kn001", "architecture", "Build System");
            page.snippet = "How the build works".to_string();
            store
                .commit_ingest(&IngestBatch {
                    pages: vec![PageWrite {
                        page,
                        body: body_for_push,
                    }],
                    sources: Vec::new(),
                    tombstones: Vec::new(),
                })
                .unwrap();
            let syncer = syncer(Some(&endpoint), &push_path);
            assert_eq!(syncer.push_knowledge_pages(&store).unwrap(), 1);
        })
        .await
        .unwrap();

        // Take the record straight off the wire — not a re-serialization of the
        // local page — so the assertion covers what actually got sent.
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(&requests[0].body[..])
            .read_to_end(&mut raw)
            .expect("push payload must be gzip");
        let payload: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let pushed = payload[KNOWLEDGE_ENTITY].as_array().unwrap()[0].clone();
        assert_eq!(
            pushed["body"].as_str().unwrap(),
            body,
            "the body must reach the wire unmodified"
        );

        // Now serve that exact record back to a fresh machine.
        let pull_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(super::super::pull::PULL_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "knowledge_pages": [pushed]
            })))
            .mount(&pull_server)
            .await;
        let pull_endpoint = pull_server.uri();

        let pull_root = tempfile::tempdir().unwrap();
        let pull_path = pull_root.path().to_path_buf();
        let landed = tokio::task::spawn_blocking(move || {
            let store = SqliteKnowledgeStore::open(&pull_path).unwrap();
            let syncer = syncer(Some(&pull_endpoint), &pull_path);
            let report = syncer.pull_knowledge_pages(&store).unwrap();
            assert_eq!(report.applied, 1, "errors: {:?}", report.errors);
            let page = store.get_page_by_rel_path("architecture/build-system.md");
            let page = page.unwrap().expect("rel_path identity must be preserved");
            store.read_body(&page.rel_path).unwrap()
        })
        .await
        .unwrap();

        assert_eq!(
            landed, body,
            "the pulled body must be byte-identical to the pushed one"
        );
    }

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(encode_query_value("team-abc_123.x~y"), "team-abc_123.x~y");
        // A value that could otherwise smuggle a second parameter.
        assert_eq!(
            encode_query_value("a&project_id=other"),
            "a%26project_id%3Dother"
        );
        assert_eq!(encode_query_value("a b/c"), "a%20b%2Fc");
    }

    #[tokio::test]
    async fn knowledge_pull_sends_the_active_team_id() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(super::super::pull::PULL_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "knowledge_pages": []
            })))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        tokio::task::spawn_blocking(move || {
            let store = seeded_store(&root);
            let queue = Arc::new(SyncQueue::open(&root).unwrap());
            queue.init().unwrap();
            let config = CloudConfig {
                endpoint: endpoint.clone(),
                token: Some("test-token".to_string()),
                team_id: Some("team-42".to_string()),
                ..Default::default()
            };
            let syncer = CloudSyncer::new(queue, config, CloudSyncerConfig::default());
            syncer.pull_knowledge_pages(&store).unwrap();
        })
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let query = requests[0].url.query().unwrap_or_default().to_string();
        assert!(
            query.contains("team_id=team-42"),
            "cross-team bleed guard: the pull must name the active team — got {query}"
        );
        assert!(
            query.contains("project_id="),
            "project scoping must still be present — got {query}"
        );
    }

    #[tokio::test]
    async fn knowledge_pull_omits_team_id_for_a_personal_install() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(super::super::pull::PULL_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "knowledge_pages": []
            })))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        tokio::task::spawn_blocking(move || {
            let store = seeded_store(&root);
            let syncer = syncer(Some(&endpoint), &root);
            syncer.pull_knowledge_pages(&store).unwrap();
        })
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        let query = requests[0].url.query().unwrap_or_default().to_string();
        assert!(
            !query.contains("team_id"),
            "a teamless install must not claim a team — got {query}"
        );
    }

    #[tokio::test]
    async fn pulls_pages_and_preserves_a_locked_local_page() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Two remote pages: one brand new, one colliding with a page the user
        // locked locally.
        let new_page = remote_record("Retrieval Pipeline", "# Retrieval\n\nremote body", false);
        let mut collide = remote_record("Build System", "# REMOTE OVERWRITE", false);
        collide["id"] = serde_json::json!("cas-kn901");

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(super::super::pull::PULL_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "knowledge_pages": [new_page, collide]
            })))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let (report, local_body, titles, locked_page_still_locked) =
            tokio::task::spawn_blocking(move || {
                let store = seeded_store(&root);
                store.set_locked("cas-kn001", true).unwrap();
                let syncer = syncer(Some(&endpoint), &root);
                let report = syncer.pull_knowledge_pages(&store).unwrap();
                let local = store.get_page("cas-kn001").unwrap();
                let body = store.read_body(&local.rel_path).unwrap();
                let titles: Vec<String> = store
                    .list_pages()
                    .unwrap()
                    .into_iter()
                    .map(|p| p.title)
                    .collect();
                (report, body, titles, local.locked)
            })
            .await
            .unwrap();

        assert_eq!(report.applied, 1, "the new page must land");
        assert_eq!(
            report.locked_preserved, 1,
            "the locked page must be counted as preserved, not applied"
        );
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        assert!(
            local_body.contains("Zig linker"),
            "a locked page's body must survive a remote push: {local_body}"
        );
        assert!(!local_body.contains("REMOTE OVERWRITE"));
        assert!(locked_page_still_locked);
        assert!(titles.iter().any(|t| t == "Retrieval Pipeline"));
        assert!(titles.iter().any(|t| t == "Build System"));
    }

    #[tokio::test]
    async fn a_pulled_page_round_trips_its_locked_bit() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let locked_remote = remote_record("Team Charter", "# Charter\n\nhuman written", true);

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(super::super::pull::PULL_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "knowledge_pages": [locked_remote]
            })))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let landed = tokio::task::spawn_blocking(move || {
            let store = SqliteKnowledgeStore::open(&root).unwrap();
            let syncer = syncer(Some(&endpoint), &root);
            let report = syncer.pull_knowledge_pages(&store).unwrap();
            assert_eq!(report.applied, 1, "errors: {:?}", report.errors);
            store
                .list_pages()
                .unwrap()
                .into_iter()
                .find(|p| p.title == "Team Charter")
                .unwrap()
        })
        .await
        .unwrap();

        assert!(
            landed.locked,
            "a page locked upstream must arrive locked, or the next local \
             distillation pass would silently overwrite a human-owned page"
        );
    }
}
