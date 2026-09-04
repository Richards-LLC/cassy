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
use cas_store::{
    IngestBatch, KnowledgePage, KnowledgePageOrigin, KnowledgeStore, PageWrite,
    TombstoneApplyOutcome,
};
use cas_types::ShareScope;

/// Metadata key holding the high-water mark for knowledge pushes.
const LAST_PUSH_KEY: &str = "last_knowledge_push_at";
/// Metadata key holding the high-water mark for knowledge pulls.
const LAST_PULL_KEY: &str = "last_knowledge_pull_at";
/// Canonical project id used by the last knowledge push the server accepted.
///
/// Recorded so the starvation detector can name BOTH ids when pulls go quiet:
/// "we push as X and pull as Y" is the actionable form of the warning.
const LAST_PUSH_PROJECT_KEY: &str = "last_knowledge_push_project_id";
/// Count of consecutive knowledge pulls that returned a completely empty
/// envelope.
const EMPTY_PULL_STREAK_KEY: &str = "knowledge_empty_pull_streak";

/// Consecutive empty pulls tolerated before we say something.
///
/// Not 1: an empty envelope is the NORMAL steady state once a project is fully
/// synced and nobody has written a page since. The signal is only meaningful
/// when it persists *and* we have evidence there should be something there —
/// hence the paired "a push was accepted" condition below.
const EMPTY_PULL_STREAK_THRESHOLD: u32 = 5;
/// Entity-type key used in the push payload and the pull response.
pub const KNOWLEDGE_ENTITY: &str = "knowledge_pages";
/// Companion key in the existing knowledge push/pull envelope. Tombstones are
/// separate from page records because the page row and body no longer exist.
pub const KNOWLEDGE_TOMBSTONE_ENTITY: &str = "knowledge_tombstones";

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
        Self::from_page_for_project(page, body, share, get_project_canonical_id())
    }

    /// Build a wire record with the identity of the project being synced.
    ///
    /// The compatibility [`Self::from_page`] helper remains for callers that
    /// construct an isolated record, but network push paths must use this
    /// root-explicit form so a multi-root refresh cannot inherit process cwd.
    pub fn from_page_for_project(
        page: &KnowledgePage,
        body: String,
        share: ShareScope,
        project_canonical_id: Option<String>,
    ) -> Self {
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
            project_canonical_id,
        }
    }

    /// Convert an incoming record into a writable page.
    ///
    /// `pending_embedding` is forced true: a vector computed on a teammate's
    /// machine lives in *their* local cache, so this machine has to embed the
    /// page itself before the semantic channel can retrieve it.
    pub fn into_page_write(self) -> PageWrite {
        let origin_project_id = self.project_canonical_id.clone();
        let mut page = KnowledgePage::new(self.id, self.page_type, self.title);
        // Trust the sender's canonical path rather than recomputing it: a
        // future change to the slug rules must not silently fork a page into
        // two rel_paths across machines running different Cassy versions.
        page.rel_path = self.rel_path;
        page.snippet = self.snippet;
        page.locked = self.locked;
        page.sources = self.sources;
        page.created_at = self.created_at;
        page.updated_at = self.updated_at;
        page.pending_embedding = true;
        page.origin = KnowledgePageOrigin::CloudPull;
        page.origin_project_id = origin_project_id;
        PageWrite {
            page,
            body: self.body,
        }
    }
}

/// Wire shape for a knowledge-page deletion.
///
/// The enclosing push envelope supplies the project/team scope. Pull records
/// additionally carry that scope at the raw JSON level and are checked before
/// this type is deserialized or applied.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnowledgePageTombstoneRecord {
    pub id: String,
    pub deleted_at: DateTime<Utc>,
}

/// Outcome of a knowledge pull.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnowledgePullReport {
    /// Pages created or updated locally.
    pub applied: usize,
    /// Pages the local store refused to overwrite because the local copy is
    /// locked. Not an error — this is user sovereignty working.
    pub locked_preserved: usize,
    /// Incoming tombstones that deleted a local page (or established a guard
    /// for a page not currently present).
    pub tombstones_applied: usize,
    /// Tombstones that deliberately did not delete a locally locked page.
    pub tombstones_locked_preserved: usize,
    /// Page records refused because a tombstone was already applied. This is
    /// surfaced rather than silently ignored because it proves stale data was
    /// actively prevented from resurrecting a deleted page.
    pub tombstoned_pages_refused: usize,
    /// Per-page failures (rel_path, message).
    pub errors: Vec<(String, String)>,
    /// Rows refused at ingest because they do not belong to this project.
    ///
    /// Never silently dropped and never written: each one is counted here and
    /// named in [`Self::refused_foreign_ids`], because a zero here is a claim
    /// that the pull was clean and must be backed by a real check.
    pub refused_foreign: usize,
    /// The page ids refused above, for the operator-facing report.
    pub refused_foreign_ids: Vec<String>,
    /// Set when pulls have been persistently empty while pushes are being
    /// accepted — the signature of a project-id divergence, which presents as
    /// silence rather than as an error. See [`CloudSyncer::pull_knowledge_pages`].
    pub starvation_warning: Option<String>,
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

        // Resolve once: every team-visible page must be published into the
        // same active team that the pull path selects. Consulting the raw
        // project `team_id` again would lose opted-in user-default and
        // sole-team fallbacks (and could bypass the kill switch).
        let active_team_id = self.cloud_config.active_team_id();
        let share = knowledge_share_scope(active_team_id.is_some());

        let pages = store
            .list_pages()
            .map_err(|e| CasError::Other(format!("Failed to list knowledge pages: {e}")))?;
        let tombstones = store
            .list_pending_page_tombstones()
            .map_err(|e| CasError::Other(format!("Failed to list knowledge tombstones: {e}")))?;

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
            records.push(serde_json::to_value(
                KnowledgePageRecord::from_page_for_project(
                    &page,
                    body,
                    share,
                    Some(self.personal_push_project_id()?),
                ),
            )?);
        }

        if records.is_empty() && tombstones.is_empty() {
            return Ok(0);
        }

        let count = records.len();
        let tombstone_ids: Vec<String> = tombstones.iter().map(|t| t.id.clone()).collect();
        let tombstone_records = tombstones
            .into_iter()
            .map(|t| KnowledgePageTombstoneRecord {
                id: t.id,
                deleted_at: t.deleted_at,
            })
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()?;
        let response = self.push_knowledge_batch(
            records,
            tombstone_records,
            active_team_id.as_deref(),
            &token,
        )?;

        // Read the refusal count instead of discarding the response. The
        // knowledge push has no per-row queue to leave un-marked, so its only
        // retry lever is the watermark: advancing it past a page the server
        // REFUSED (its locked guard) means that page is never sent again until
        // a human happens to edit it. Holding the mark makes the next run
        // re-offer the same window — the same conservative choice the generic
        // path makes when it leaves a sub-batch un-synced (push.rs).
        let skipped = response
            .skipped_count_for(KNOWLEDGE_ENTITY)
            .map_err(CasError::Other)?
            + response
                .skipped_count_for(KNOWLEDGE_TOMBSTONE_ENTITY)
                .map_err(CasError::Other)?;
        if skipped > 0 {
            warn!(
                skipped,
                batch_size = count,
                "cloud refused {skipped} of {count} knowledge page(s); holding the push watermark \
                 so they are re-offered next run"
            );
            return Ok(count.saturating_sub(skipped));
        }

        store
            .mark_page_tombstones_pushed(&tombstone_ids)
            .map_err(|e| {
                CasError::Other(format!("Failed to mark knowledge tombstones pushed: {e}"))
            })?;

        let _ = self
            .queue()
            .set_metadata(LAST_PUSH_KEY, &Utc::now().to_rfc3339());
        // Record WHICH project id the server accepted pages under. If pulls
        // later go silent, this is half of the evidence that names the cause.
        let project_id = self.personal_push_project_id()?;
        let _ = self
            .queue()
            .set_metadata(LAST_PUSH_PROJECT_KEY, &project_id);
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
        if let Some(team_id) = self.cloud_config.active_team_id() {
            params.push(format!("team_id={}", encode_query_value(&team_id)));
        }
        // Fail closed on an unresolvable project scope, exactly like the
        // canonical builder: without `project_id=` this would ask the server
        // for every project's knowledge pages (cas-2eb3 / cas-ed15).
        //
        // The resolved id is KEPT, not discarded: it is the second line of
        // defence below. Asking the server for one project's pages and then
        // trusting whatever comes back is exactly the gap that let a foreign
        // page overwrite a local one (cas-2cc5).
        let project_id = self.personal_push_project_id()?;
        let (url, project_id) = super::pull::build_scoped_pull_url_with(
            &self.cloud_config.endpoint,
            &params,
            || Some(project_id),
        )?;

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
        let incoming_tombstones = body
            .get(KNOWLEDGE_TOMBSTONE_ENTITY)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        // Empty BEFORE any client-side filtering: rows we refused still prove
        // the read channel works, and must not be read as starvation.
        let envelope_was_empty = incoming.is_empty() && incoming_tombstones.is_empty();

        // Tombstones MUST win over pages in one envelope regardless of JSON
        // key order. Establishing the durable guard first makes a stale page
        // record in the same response harmless instead of resurrecting it.
        for raw in incoming_tombstones {
            if !super::pull::entity_matches_project(&raw, &project_id, "knowledge tombstone") {
                report.refused_foreign += 1;
                report.refused_foreign_ids.push(
                    raw.get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("<unknown>")
                        .to_string(),
                );
                continue;
            }
            let tombstone: KnowledgePageTombstoneRecord = match serde_json::from_value(raw) {
                Ok(tombstone) => tombstone,
                Err(e) => {
                    report
                        .errors
                        .push((String::from("<unparseable tombstone>"), e.to_string()));
                    continue;
                }
            };
            match store
                .apply_remote_page_tombstone(&tombstone.id, tombstone.deleted_at)
                .map_err(|e| CasError::Other(format!("Failed to apply knowledge tombstone: {e}")))?
            {
                TombstoneApplyOutcome::Applied => report.tombstones_applied += 1,
                TombstoneApplyOutcome::LockedPreserved => report.tombstones_locked_preserved += 1,
            }
        }

        for raw in incoming {
            // FAIL CLOSED, BEFORE PARSING OR WRITING. A knowledge page is
            // merged on `rel_path` (knowledge_store.rs) and written to both
            // cas.db and disk, so a foreign page with a colliding path
            // OVERWRITES the local one unless it happens to be locked — and
            // durable attribution now lets doctor detect one after the fact,
            // but detection cannot undo an overwrite; the only safe place to
            // refuse is still here.
            if !super::pull::entity_matches_project(&raw, &project_id, "knowledge page") {
                report.refused_foreign += 1;
                report.refused_foreign_ids.push(
                    raw.get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("<unknown>")
                        .to_string(),
                );
                continue;
            }

            let record: KnowledgePageRecord = match serde_json::from_value(raw) {
                Ok(record) => record,
                Err(e) => {
                    report
                        .errors
                        .push((String::from("<unparseable>"), e.to_string()));
                    continue;
                }
            };
            if store
                .is_page_tombstoned(&record.id)
                .map_err(|e| CasError::Other(format!("Failed to check knowledge tombstone: {e}")))?
            {
                report.tombstoned_pages_refused += 1;
                continue;
            }
            let rel_path = record.rel_path.clone();
            match self.apply_knowledge_record(store, record) {
                Ok(true) => report.applied += 1,
                Ok(false) => report.locked_preserved += 1,
                Err(e) => report.errors.push((rel_path, e.to_string())),
            }
        }

        // Advance the watermark to the SERVER's clock, not ours. Client
        // wall-clock here meant any skew between this machine and the server
        // silently widened or narrowed the next `since` window — rows created
        // in the gap are never pulled again. The entity pull has always used
        // the server value (pull.rs); knowledge now matches it. If the server
        // sends no `pulled_at` the mark is left alone, so the next pull
        // re-requests the same window rather than skipping it.
        if let Some(pulled_at) = body.get("pulled_at").and_then(|v| v.as_str()) {
            let _ = self.queue().set_metadata(LAST_PULL_KEY, pulled_at);
        }

        report.starvation_warning = self.check_pull_starvation(envelope_was_empty, &project_id);

        Ok(report)
    }

    /// Detect the failure mode that has no error to report: **silent
    /// starvation**.
    ///
    /// The server filters pulls on the project id the client *sends*. If rows
    /// were ever stored under a different canonical id than the one we send —
    /// and `resolveCanonicalProject` can legitimately return one, which is the
    /// point of its alias and conflict branches — we do not receive foreign
    /// rows to reject. We receive an **empty envelope, forever**, and both
    /// sides consider the sync successful. Nothing throws, nothing logs, and
    /// the account looks synced while nothing arrives.
    ///
    /// So the detector infers it from a shape rather than an error: pulls
    /// persistently empty *while pushes are being accepted*. Either condition
    /// alone is unremarkable — an empty pull is the normal steady state, and a
    /// successful push says nothing about the read path — but together they
    /// describe a channel that only works in one direction, which is exactly
    /// what an id divergence looks like from here.
    ///
    /// Deliberately advisory: it names both ids and stops. The client cannot
    /// tell a divergence from "genuinely nothing new for a while", so escalating
    /// to an error would eventually cry wolf at every quiet project.
    fn check_pull_starvation(
        &self,
        envelope_was_empty: bool,
        pull_project_id: &str,
    ) -> Option<String> {
        if !envelope_was_empty {
            let _ = self.queue().set_metadata(EMPTY_PULL_STREAK_KEY, "0");
            return None;
        }

        let streak = self
            .queue()
            .get_metadata(EMPTY_PULL_STREAK_KEY)
            .ok()
            .flatten()
            .and_then(|raw| raw.parse::<u32>().ok())
            .unwrap_or(0)
            .saturating_add(1);
        let _ = self
            .queue()
            .set_metadata(EMPTY_PULL_STREAK_KEY, &streak.to_string());

        if streak < EMPTY_PULL_STREAK_THRESHOLD {
            return None;
        }
        // Without an accepted push on record there is no evidence anything
        // should be coming back, and a quiet project is not a bug.
        let pushed_as = self
            .queue()
            .get_metadata(LAST_PUSH_PROJECT_KEY)
            .ok()
            .flatten()?;

        let mismatch = if pushed_as == pull_project_id {
            "The ids match locally, so the divergence (if any) is server-side: rows may be \
             stored under a canonical id different from the one this client sends."
        } else {
            "THESE IDS DIFFER — that is the likely cause: pages are being stored under one \
             project and requested under another."
        };
        Some(format!(
            "{streak} consecutive knowledge pulls returned nothing while pushes are being \
             accepted. Pushing as '{pushed_as}', pulling as '{pull_project_id}'. {mismatch} \
             A project-id divergence does not surface as an error — it presents exactly like \
             this, as silence. Check the canonical id pin before assuming there is simply \
             nothing new."
        ))
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

    /// Send live pages and newly authored tombstones in one normal sync push
    /// envelope. The cloud proposal intentionally reuses this transport so
    /// auth, gzip, project scope and server-side `skipped` reporting stay
    /// identical for both kinds of change, while knowledge alone may add its
    /// resolved active-team scope.
    fn push_knowledge_batch(
        &self,
        pages: Vec<serde_json::Value>,
        tombstones: Vec<serde_json::Value>,
        team_id: Option<&str>,
        token: &str,
    ) -> Result<super::PushResponse, CasError> {
        let payload = self.build_team_scoped_push_payload_fields(
            [
                (KNOWLEDGE_ENTITY.to_string(), pages),
                (KNOWLEDGE_TOMBSTONE_ENTITY.to_string(), tombstones),
            ],
            team_id,
        )?;
        self.push_personal_payload(payload, token)
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
    use crate::cloud::{CloudConfig, CloudSyncerConfig, SyncQueue, TeamInfo};
    use cas_store::SqliteKnowledgeStore;
    use std::sync::Arc;

    fn syncer(endpoint: Option<&str>, root: &std::path::Path) -> CloudSyncer {
        // These fixtures are constructed through the legacy `from_page` helper,
        // whose records intentionally carry the current checkout's identity.
        // Pin the synthetic queue root to that same identity so the tests model
        // an explicitly configured project rather than an unrelated temp path.
        let fixture_project_id = get_project_canonical_id().expect("tests run in a Cassy project");
        crate::cloud::set_canonical_id_in_config_toml(root, &fixture_project_id).unwrap();
        let queue = Arc::new(SyncQueue::open(root).unwrap());
        queue.init().unwrap();
        let config = CloudConfig {
            endpoint: endpoint.unwrap_or("https://example.invalid").to_string(),
            token: endpoint.map(|_| "test-token".to_string()),
            ..Default::default()
        };
        CloudSyncer::new(queue, config, CloudSyncerConfig::default())
    }

    async fn knowledge_pull_query_with_user_config(
        mut project_config: CloudConfig,
        user_config: CloudConfig,
    ) -> String {
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

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let user_cloud_json = tmp.path().join("user-cloud.json");
        user_config.save_to(&user_cloud_json).unwrap();
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("CAS_USER_CLOUD_JSON", &user_cloud_json);

        project_config.endpoint = server.uri();
        project_config.token = Some("test-token".to_string());
        tokio::task::spawn_blocking(move || {
            let store = seeded_store(&root);
            let queue = Arc::new(SyncQueue::open(&root).unwrap());
            queue.init().unwrap();
            let syncer = CloudSyncer::new(queue, project_config, CloudSyncerConfig::default());
            syncer.pull_knowledge_pages(&store).unwrap();
        })
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        requests[0].url.query().unwrap_or_default().to_string()
    }

    async fn knowledge_push_payload_with_user_config(
        mut project_config: CloudConfig,
        user_config: CloudConfig,
    ) -> serde_json::Value {
        use std::io::Read;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/sync/push"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let user_cloud_json = tmp.path().join("user-cloud.json");
        user_config.save_to(&user_cloud_json).unwrap();
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("CAS_USER_CLOUD_JSON", &user_cloud_json);

        project_config.endpoint = server.uri();
        project_config.token = Some("test-token".to_string());
        tokio::task::spawn_blocking(move || {
            let store = seeded_store(&root);
            let queue = Arc::new(SyncQueue::open(&root).unwrap());
            queue.init().unwrap();
            let syncer = CloudSyncer::new(queue, project_config, CloudSyncerConfig::default());
            assert_eq!(syncer.push_knowledge_pages(&store).unwrap(), 1);
        })
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(&requests[0].body[..])
            .read_to_end(&mut raw)
            .unwrap();
        serde_json::from_slice(&raw).unwrap()
    }

    async fn assert_knowledge_push_pull_team_parity(
        project_config: CloudConfig,
        user_config: CloudConfig,
        expected_team_id: Option<&str>,
    ) {
        let payload =
            knowledge_push_payload_with_user_config(project_config.clone(), user_config.clone())
                .await;
        let query = knowledge_pull_query_with_user_config(project_config, user_config).await;
        let pushed_page = &payload[KNOWLEDGE_ENTITY][0];

        match expected_team_id {
            Some(team_id) => {
                assert_eq!(payload["team_id"], team_id);
                assert_eq!(pushed_page["share"], "team");
                assert!(
                    query.contains(&format!("team_id={}", encode_query_value(team_id))),
                    "pull must select the same active team as push — got {query}"
                );
            }
            None => {
                assert!(
                    payload.get("team_id").is_none(),
                    "an unscoped knowledge push must omit team_id — got {payload}"
                );
                assert_eq!(pushed_page["share"], "private");
                assert!(
                    !query.contains("team_id="),
                    "an unscoped knowledge pull must omit team_id — got {query}"
                );
            }
        }
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

    fn remote_tombstone(id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "deleted_at": "2026-08-08T12:00:00Z",
            "project_canonical_id": get_project_canonical_id()
                .expect("knowledge pull tests run from a Cassy project"),
        })
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

    #[tokio::test]
    async fn local_page_delete_emits_one_tombstone_and_marks_it_delivered() {
        use std::io::Read;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/sync/push"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let deleted_id = tokio::task::spawn_blocking(move || {
            let store = seeded_store(&root);
            let page = store.list_pages().unwrap().remove(0);
            store.delete_page(&page.id).unwrap();
            let syncer = syncer(Some(&endpoint), &root);
            assert_eq!(syncer.push_knowledge_pages(&store).unwrap(), 0);
            assert!(store.list_pending_page_tombstones().unwrap().is_empty());
            assert!(store.is_page_tombstoned(&page.id).unwrap());
            page.id
        })
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "the delete must be a real push request");
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(&requests[0].body[..])
            .read_to_end(&mut raw)
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(payload[KNOWLEDGE_ENTITY], serde_json::json!([]));
        let tombstones = payload[KNOWLEDGE_TOMBSTONE_ENTITY].as_array().unwrap();
        assert_eq!(tombstones.len(), 1);
        assert_eq!(tombstones[0]["id"], deleted_id);
        assert!(
            tombstones[0]["deleted_at"].as_str().is_some(),
            "wire tombstone must carry its deletion time"
        );
    }

    #[tokio::test]
    async fn pulled_tombstone_wins_over_a_stale_page_in_the_same_envelope() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mut stale = remote_record("Build System", "# STALE RESURRECTION", false);
        stale["id"] = serde_json::json!("cas-kn001");
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(super::super::pull::PULL_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "knowledge_tombstones": [remote_tombstone("cas-kn001")],
                "knowledge_pages": [stale],
            })))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let report = tokio::task::spawn_blocking(move || {
            let store = seeded_store(&root);
            let syncer = syncer(Some(&endpoint), &root);
            let report = syncer.pull_knowledge_pages(&store).unwrap();
            assert!(store.get_page("cas-kn001").is_err());
            assert!(store.is_page_tombstoned("cas-kn001").unwrap());
            report
        })
        .await
        .unwrap();
        assert_eq!(report.tombstones_applied, 1);
        assert_eq!(report.tombstoned_pages_refused, 1);
        assert_eq!(report.applied, 0);
    }

    #[tokio::test]
    async fn pulled_tombstone_never_deletes_a_locked_page_without_local_action() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(super::super::pull::PULL_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "knowledge_tombstones": [remote_tombstone("cas-kn001")],
                "knowledge_pages": [],
            })))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let (report, body) = tokio::task::spawn_blocking(move || {
            let store = seeded_store(&root);
            store.set_locked("cas-kn001", true).unwrap();
            let syncer = syncer(Some(&endpoint), &root);
            let report = syncer.pull_knowledge_pages(&store).unwrap();
            let page = store.get_page("cas-kn001").unwrap();
            (report, store.read_body(&page.rel_path).unwrap())
        })
        .await
        .unwrap();
        assert_eq!(report.tombstones_locked_preserved, 1);
        assert_eq!(body, "# Build System\n\nZig linker.");
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
        let expected_origin_project_id = pushed["project_canonical_id"]
            .as_str()
            .expect("push wire row must carry its project identity")
            .to_string();
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
        let (landed, origin, origin_project_id) = tokio::task::spawn_blocking(move || {
            let store = SqliteKnowledgeStore::open(&pull_path).unwrap();
            let syncer = syncer(Some(&pull_endpoint), &pull_path);
            let report = syncer.pull_knowledge_pages(&store).unwrap();
            assert_eq!(report.applied, 1, "errors: {:?}", report.errors);
            let page = store.get_page_by_rel_path("architecture/build-system.md");
            let page = page.unwrap().expect("rel_path identity must be preserved");
            (
                store.read_body(&page.rel_path).unwrap(),
                page.origin,
                page.origin_project_id,
            )
        })
        .await
        .unwrap();

        assert_eq!(
            landed, body,
            "the pulled body must be byte-identical to the pushed one"
        );
        assert_eq!(origin, KnowledgePageOrigin::CloudPull);
        assert_eq!(
            origin_project_id.as_deref(),
            Some(expected_origin_project_id.as_str()),
            "pull provenance must retain the exact project id accepted by the ingest guard"
        );
    }

    /// The contamination case, end to end. A foreign page whose `rel_path`
    /// COLLIDES with a local page is the dangerous one: pages merge on
    /// rel_path and are written to both cas.db and disk, so before this guard
    /// it silently overwrote the local body. Durable provenance gives doctor a
    /// second line of defence, but cannot restore the body, so ingest must
    /// still refuse before commit.
    #[tokio::test]
    async fn a_foreign_project_page_is_refused_at_ingest_and_never_written() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Same rel_path as the seeded local page ("Build System"), different
        // project. This is the overwrite that must not happen.
        let mut foreign = remote_record("Build System", "# FOREIGN OVERWRITE", false);
        foreign["id"] = serde_json::json!("cas-kn999");
        foreign["project_canonical_id"] = serde_json::json!("github.com/someone-else/other-repo");
        // A legitimate page in the same envelope must still land: refusing the
        // foreign row must not become "drop the whole pull".
        let mut legit = remote_record(
            "Retrieval Pipeline",
            "# Retrieval\n\nlocal-project body",
            false,
        );
        legit["id"] = serde_json::json!("cas-kn902");

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(super::super::pull::PULL_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "knowledge_pages": [foreign, legit]
            })))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let (report, local_body, titles) = tokio::task::spawn_blocking(move || {
            let store = seeded_store(&root);
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
            (report, body, titles)
        })
        .await
        .unwrap();

        assert_eq!(
            report.refused_foreign, 1,
            "the foreign page must be counted, not silently dropped"
        );
        assert_eq!(
            report.refused_foreign_ids,
            vec!["cas-kn999".to_string()],
            "the refusal must name the page, so the operator can act on it"
        );
        assert_eq!(
            local_body, "# Build System\n\nZig linker.",
            "the local body must be untouched — a foreign page must never overwrite it"
        );
        assert_eq!(
            report.applied, 1,
            "the legitimate page in the same envelope must still land"
        );
        assert!(
            titles.contains(&"Retrieval Pipeline".to_string()),
            "titles: {titles:?}"
        );
        assert!(
            !titles.contains(&"FOREIGN".to_string()),
            "no foreign page may reach the store: {titles:?}"
        );
    }

    #[tokio::test]
    async fn a_page_with_no_project_id_is_refused_rather_than_assumed_local() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mut unscoped = remote_record("Orphan Page", "# no scope", false);
        unscoped["id"] = serde_json::json!("cas-kn998");
        // Field absent entirely — an older server, or a bug. Fail closed.
        unscoped
            .as_object_mut()
            .unwrap()
            .remove("project_canonical_id");

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(super::super::pull::PULL_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "knowledge_pages": [unscoped]
            })))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let (report, titles) = tokio::task::spawn_blocking(move || {
            let store = seeded_store(&root);
            let syncer = syncer(Some(&endpoint), &root);
            let report = syncer.pull_knowledge_pages(&store).unwrap();
            let titles: Vec<String> = store
                .list_pages()
                .unwrap()
                .into_iter()
                .map(|p| p.title)
                .collect();
            (report, titles)
        })
        .await
        .unwrap();

        assert_eq!(report.refused_foreign, 1);
        assert_eq!(report.applied, 0);
        assert!(
            !titles.contains(&"Orphan Page".to_string()),
            "an unscoped row is foreign until proven otherwise: {titles:?}"
        );
    }

    #[tokio::test]
    async fn the_pull_watermark_comes_from_the_server_not_the_local_clock() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(super::super::pull::PULL_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "knowledge_pages": [],
                "pulled_at": "2020-01-02T03:04:05Z"
            })))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let mark = tokio::task::spawn_blocking(move || {
            let store = seeded_store(&root);
            let queue = Arc::new(SyncQueue::open(&root).unwrap());
            queue.init().unwrap();
            let config = CloudConfig {
                endpoint: endpoint.clone(),
                token: Some("test-token".to_string()),
                ..Default::default()
            };
            let syncer = CloudSyncer::new(queue.clone(), config, CloudSyncerConfig::default());
            syncer.pull_knowledge_pages(&store).unwrap();
            queue.get_metadata(LAST_PULL_KEY).unwrap()
        })
        .await
        .unwrap();

        assert_eq!(
            mark.as_deref(),
            Some("2020-01-02T03:04:05Z"),
            "the server's pulled_at is authoritative; client wall-clock skew must not move the window"
        );
    }

    #[tokio::test]
    async fn a_server_refused_push_does_not_advance_the_watermark() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/sync/push"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "skipped": { "knowledge_pages": 1 }
            })))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let (first, second, mark) = tokio::task::spawn_blocking(move || {
            let store = seeded_store(&root);
            let queue = Arc::new(SyncQueue::open(&root).unwrap());
            queue.init().unwrap();
            let config = CloudConfig {
                endpoint: endpoint.clone(),
                token: Some("test-token".to_string()),
                ..Default::default()
            };
            let syncer = CloudSyncer::new(queue.clone(), config, CloudSyncerConfig::default());
            let first = syncer.push_knowledge_pages(&store).unwrap();
            // The page was refused, so the next run must offer it again.
            let second = syncer.push_knowledge_pages(&store).unwrap();
            (first, second, queue.get_metadata(LAST_PUSH_KEY).unwrap())
        })
        .await
        .unwrap();

        assert_eq!(first, 0, "a refused page must not be counted as pushed");
        assert_eq!(
            second, 0,
            "the page is re-offered and refused again — never silently abandoned"
        );
        assert!(
            mark.is_none(),
            "the watermark must NOT advance past a refused page, or it is never retried \
             until a human edits it — got {mark:?}"
        );
    }

    /// Starvation has no error to catch — it presents as a successful, quiet
    /// sync. This pins the inference: persistently empty pulls WHILE pushes
    /// are accepted, and the warning must name both ids.
    #[tokio::test]
    async fn persistently_empty_pulls_after_an_accepted_push_warn_about_id_divergence() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/sync/push"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
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

        let (early, at_threshold) = tokio::task::spawn_blocking(move || {
            let store = seeded_store(&root);
            let syncer = syncer(Some(&endpoint), &root);
            // A push the server accepts: this is the evidence that the write
            // channel works, without which silence means nothing.
            assert_eq!(syncer.push_knowledge_pages(&store).unwrap(), 1);

            let mut early = None;
            for _ in 1..EMPTY_PULL_STREAK_THRESHOLD {
                early = syncer
                    .pull_knowledge_pages(&store)
                    .unwrap()
                    .starvation_warning;
            }
            let at_threshold = syncer
                .pull_knowledge_pages(&store)
                .unwrap()
                .starvation_warning;
            (early, at_threshold)
        })
        .await
        .unwrap();

        assert!(
            early.is_none(),
            "a few empty pulls are the normal steady state and must stay quiet — got {early:?}"
        );
        let warning = at_threshold.expect("the streak must eventually be reported");
        assert!(
            warning.contains("Pushing as") && warning.contains("pulling as"),
            "the warning must name BOTH ids or it is not actionable: {warning}"
        );
        assert!(
            warning.contains("does not surface as an error"),
            "the warning must explain why nothing else caught this: {warning}"
        );
    }

    #[tokio::test]
    async fn a_non_empty_pull_clears_the_starvation_streak() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/sync/push"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        // Rows arrive — even though this one is refused as foreign, the read
        // channel demonstrably works, so it must NOT count as starvation.
        let mut foreign = remote_record("Foreign", "# foreign", false);
        foreign["id"] = serde_json::json!("cas-kn997");
        foreign["project_canonical_id"] = serde_json::json!("github.com/other/repo");
        Mock::given(method("GET"))
            .and(path(super::super::pull::PULL_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "knowledge_pages": [foreign]
            })))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let warnings = tokio::task::spawn_blocking(move || {
            let store = seeded_store(&root);
            let syncer = syncer(Some(&endpoint), &root);
            syncer.push_knowledge_pages(&store).unwrap();
            let mut warnings = Vec::new();
            for _ in 0..(EMPTY_PULL_STREAK_THRESHOLD + 2) {
                warnings.push(
                    syncer
                        .pull_knowledge_pages(&store)
                        .unwrap()
                        .starvation_warning,
                );
            }
            warnings
        })
        .await
        .unwrap();

        assert!(
            warnings.iter().all(|w| w.is_none()),
            "rows arriving — even refused ones — prove the read channel works: {warnings:?}"
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
    async fn knowledge_push_and_pull_use_explicit_active_team() {
        let project_config = CloudConfig {
            team_id: Some("team-42".to_string()),
            ..Default::default()
        };
        assert_knowledge_push_pull_team_parity(
            project_config,
            CloudConfig::default(),
            Some("team-42"),
        )
        .await;
    }

    #[tokio::test]
    async fn knowledge_push_and_pull_obey_active_team_kill_switch() {
        let mut project_config = CloudConfig::default();
        project_config.team_id = Some("explicit-team".to_string());
        project_config.team_auto_promote = Some(false);

        assert_knowledge_push_pull_team_parity(project_config, CloudConfig::default(), None).await;
    }

    #[tokio::test]
    async fn knowledge_push_and_pull_use_opted_in_user_default_team() {
        let mut project_config = CloudConfig::default();
        project_config.team_auto_promote = Some(true);
        let mut user_config = CloudConfig::default();
        user_config.default_team_id = Some("user-default-team".to_string());

        assert_knowledge_push_pull_team_parity(
            project_config,
            user_config,
            Some("user-default-team"),
        )
        .await;
    }

    #[tokio::test]
    async fn knowledge_push_and_pull_use_opted_in_single_team_fallback() {
        let mut project_config = CloudConfig::default();
        project_config.team_auto_promote = Some(true);
        let mut user_config = CloudConfig::default();
        user_config.teams = vec![TeamInfo {
            id: "solo-team".to_string(),
            slug: "solo".to_string(),
            name: "Solo".to_string(),
            role: "member".to_string(),
        }];

        assert_knowledge_push_pull_team_parity(project_config, user_config, Some("solo-team"))
            .await;
    }

    #[tokio::test]
    async fn knowledge_push_and_pull_remain_unscoped_without_an_active_team() {
        assert_knowledge_push_pull_team_parity(
            CloudConfig::default(),
            CloudConfig::default(),
            None,
        )
        .await;
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
