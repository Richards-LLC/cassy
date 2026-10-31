//! Batched team sync of the friction ledger (`quality_events`).
//!
//! Client half of the contract agreed with Cloud:
//!
//! - `POST /api/teams/{teamId}/quality-events`, gzip JSON
//!   `{project_canonical_id, git_remote?, client_version, events: [..]}` with at
//!   most 200 events. Idempotent on `(team_id, id)`, first write wins, so event
//!   ids carry the project (see `NewQualityEvent::scope`). Answers
//!   `{accepted, duplicates, rejected: [{id, reason}]}`. A bad envelope is a 400
//!   for the whole batch; 413 means split the batch.
//! - `GET /api/teams/{teamId}/quality-events?cursor=&limit=` answers
//!   `{events, next_cursor}` across the team's projects, ordered by server ingest.
//!   The cursor is opaque here. Server fields on events are ignored.
//! - 404 or 501 means the route is not live: both cursors hold and it is no error.
//!
//! Scope A: an event is `QualityEventWire`: metrics, timings, titles and links.
//! `local_note` has no wire field. A project with no active team stays local.
//!
//! Lean by design: no per-event writes. The daemon sends at most one round per
//! `PUSH_INTERVAL`, a round is bounded, and a route that is not live is not asked
//! again for `ABSENT_RETRY`.

use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::cloud::syncer::CloudSyncer;
use crate::error::CasError;
use cas_store::{
    QUALITY_EVENT_BATCH_LIMIT, QualityEventWire, import_quality_events, quality_events_after,
};

const PULL_PAGE_LIMIT: usize = 500;
const PROJECT_ID_MAX_CHARS: usize = 200;
const PUSH_CURSOR_KEY: &str = "quality_events_push_seq";
const PULL_CURSOR_KEY: &str = "quality_events_pull_cursor";
const LAST_ROUND_KEY: &str = "quality_events_last_round_at";
const ABSENT_UNTIL_KEY: &str = "quality_events_absent_until";
const PUSH_INTERVAL: i64 = 15;
const ABSENT_RETRY: i64 = 6;
const MAX_ROUND_BATCHES: usize = 5;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct QualitySyncSummary {
    pub pushed: usize,
    pub pulled: usize,
    pub refused: usize,
    /// The server has not shipped this route; both cursors held.
    pub capability_absent: bool,
}

#[derive(Debug, Deserialize)]
struct PullPage {
    #[serde(default)]
    events: Vec<serde_json::Value>,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct PushAnswer {
    #[serde(default)]
    rejected: Vec<serde_json::Value>,
}

enum Http<T> {
    Ok(T),
    /// 404 or 501: the route is not live yet.
    Absent,
    /// 413: split the batch.
    TooLarge,
}

impl CloudSyncer {
    fn quality_metadata_time(&self, key: &str) -> Option<DateTime<Utc>> {
        self.queue()
            .get_metadata(key)
            .ok()
            .flatten()
            .and_then(|raw| DateTime::parse_from_rfc3339(&raw).ok())
            .map(|time| time.with_timezone(&Utc))
    }

    /// One bounded push-and-pull round. `force` is an explicit request
    /// (`cas cloud sync`, `cas ledger report`), which skips the daemon's pacing.
    pub fn sync_quality_events(&self, force: bool) -> Result<QualitySyncSummary, CasError> {
        let mut summary = QualitySyncSummary::default();
        crate::store::detect::guard_implicit_worktree_write(
            self.queue.cas_dir(),
            "start cloud sync",
        )?;
        self.cloud_config
            .validate_team_only()
            .map_err(CasError::Other)?;
        if !self.is_available() {
            return Ok(summary);
        }
        // The route is team-scoped. A teamless project keeps its ledger local.
        let Some(team_id) = self.cloud_config.active_team_id() else {
            return Ok(summary);
        };
        if let Some(refusal) = self.ephemeral_project_refusal() {
            tracing::warn!("[Cassy sync] {refusal}");
            return Ok(summary);
        }
        let now = Utc::now();
        if self.quality_metadata_time(ABSENT_UNTIL_KEY).is_some_and(|until| now < until) && !force {
            summary.capability_absent = true;
            return Ok(summary);
        }
        if !force
            && self
                .quality_metadata_time(LAST_ROUND_KEY)
                .is_some_and(|last| now - last < Duration::minutes(PUSH_INTERVAL))
        {
            return Ok(summary);
        }
        let token = self
            .cloud_config
            .token
            .clone()
            .ok_or_else(|| CasError::Other("Not logged in".to_string()))?;
        let project_id = self.personal_push_project_id()?;
        if project_id.chars().count() > PROJECT_ID_MAX_CHARS {
            return Err(CasError::Other(format!(
                "quality events: project id exceeds {PROJECT_ID_MAX_CHARS} characters"
            )));
        }

        let cursor_absent = self.push_quality_events(&token, &project_id, &team_id, &mut summary)?
            || self.pull_quality_events(&token, &project_id, &team_id, &mut summary)?;
        if cursor_absent {
            summary.capability_absent = true;
            let until = now + Duration::hours(ABSENT_RETRY);
            self.queue().set_metadata(ABSENT_UNTIL_KEY, &until.to_rfc3339())?;
        } else {
            self.queue().delete_metadata(ABSENT_UNTIL_KEY)?;
        }
        self.queue().set_metadata(LAST_ROUND_KEY, &now.to_rfc3339())?;
        Ok(summary)
    }

    /// Returns true when the route is not live.
    fn push_quality_events(
        &self,
        token: &str,
        project_id: &str,
        team_id: &str,
        summary: &mut QualitySyncSummary,
    ) -> Result<bool, CasError> {
        let cas_dir = self.queue.cas_dir().to_path_buf();
        let url = format!(
            "{}/api/teams/{}/quality-events",
            self.cloud_config.endpoint, team_id
        );
        let mut limit = QUALITY_EVENT_BATCH_LIMIT;
        for _ in 0..MAX_ROUND_BATCHES {
            let cursor = self
                .queue()
                .get_metadata(PUSH_CURSOR_KEY)?
                .and_then(|raw| raw.parse::<i64>().ok())
                .unwrap_or(0);
            let events = quality_events_after(&cas_dir, cursor, limit)
                .map_err(|error| CasError::Other(error.to_string()))?;
            let Some(last_seq) = events.last().map(|event| event.seq) else {
                return Ok(false);
            };
            let wire: Vec<QualityEventWire> = events
                .iter()
                .map(|event| {
                    let mut wire = event.to_wire();
                    wire.project_id = Some(project_id.to_string());
                    wire
                })
                .collect();
            let mut payload = serde_json::Map::new();
            payload.insert("project_canonical_id".into(), serde_json::json!(project_id));
            payload.insert("events".into(), serde_json::to_value(&wire)?);
            if let Some(remote) = self.personal_push_git_remote.as_deref() {
                payload.insert("git_remote".into(), serde_json::json!(remote));
            }
            Self::insert_client_version(&mut payload);
            let body = Self::gzip_json(&serde_json::to_vec(&payload)?)?;
            let response = ureq::post(&url)
                .timeout(self.config.timeout.min(StdDuration::from_secs(30)))
                .set("Authorization", &format!("Bearer {token}"))
                .set("Content-Type", "application/json")
                .set("Content-Encoding", "gzip")
                .send_bytes(&body);
            let answer = match quality_http(response, "push")? {
                Http::Absent => return Ok(true),
                Http::TooLarge => {
                    // Hold the cursor. A single event over the limit cannot be
                    // split, so report it rather than stall silently forever.
                    if events.len() == 1 {
                        return Err(CasError::Other(
                            "quality events push: one event exceeds the server's size limit".into(),
                        ));
                    }
                    limit = (events.len() / 2).max(1);
                    continue;
                }
                Http::Ok(answer) => answer,
            };
            let answer: PushAnswer = serde_json::from_value(answer).unwrap_or_default();
            // A row-level refusal is permanent: holding the cursor for it would
            // stall every later event, and nothing here can make it acceptable.
            if !answer.rejected.is_empty() {
                tracing::warn!(rejected = ?answer.rejected, "cloud refused quality events");
            }
            summary.refused += answer.rejected.len();
            summary.pushed += wire.len().saturating_sub(answer.rejected.len());
            self.queue().set_metadata(PUSH_CURSOR_KEY, &last_seq.to_string())?;
            if events.len() < limit {
                return Ok(false);
            }
        }
        Ok(false)
    }

    /// Returns true when the route is not live.
    fn pull_quality_events(
        &self,
        token: &str,
        project_id: &str,
        team_id: &str,
        summary: &mut QualitySyncSummary,
    ) -> Result<bool, CasError> {
        let cas_dir = self.queue.cas_dir().to_path_buf();
        for _ in 0..MAX_ROUND_BATCHES {
            let mut url = format!(
                "{}/api/teams/{}/quality-events?limit={PULL_PAGE_LIMIT}",
                self.cloud_config.endpoint, team_id
            );
            let cursor = self.queue().get_metadata(PULL_CURSOR_KEY)?;
            if let Some(cursor) = cursor.as_deref() {
                url.push_str(&format!("&cursor={}", super::knowledge::encode_query_value(cursor)));
            }
            let response = ureq::get(&url)
                .timeout(self.config.timeout.min(StdDuration::from_secs(30)))
                .set("Authorization", &format!("Bearer {token}"))
                .call();
            let page = match quality_http(response, "pull")? {
                Http::Absent => return Ok(true),
                Http::TooLarge => {
                    return Err(CasError::Other("quality events pull: unexpected 413".into()));
                }
                Http::Ok(page) => page,
            };
            let page: PullPage = serde_json::from_value(page)
                .map_err(|error| CasError::Other(format!("quality events pull: {error}")))?;
            let page_len = page.events.len();
            // One bad row must not strand the rest of a page.
            let mut rows = Vec::with_capacity(page_len);
            for raw in page.events {
                match serde_json::from_value::<QualityEventWire>(raw) {
                    Ok(row) => rows.push(row),
                    Err(_) => summary.refused += 1,
                }
            }
            let (inserted, refused) = import_quality_events(&cas_dir, &rows, project_id)
                .map_err(|error| CasError::Other(error.to_string()))?;
            summary.pulled += inserted;
            summary.refused += refused;
            let Some(next) = page.next_cursor.as_deref().filter(|next| Some(*next) != cursor.as_deref())
            else {
                return Ok(false);
            };
            self.queue().set_metadata(PULL_CURSOR_KEY, next)?;
            if page_len == 0 {
                return Ok(false);
            }
        }
        Ok(false)
    }
}

fn quality_http(
    response: Result<ureq::Response, ureq::Error>,
    action: &str,
) -> Result<Http<serde_json::Value>, CasError> {
    match response {
        Ok(response) => Ok(Http::Ok(
            response
                .into_json()
                .map_err(|error| CasError::Other(format!("quality events {action}: {error}")))?,
        )),
        Err(ureq::Error::Status(404 | 501, _)) => Ok(Http::Absent),
        Err(ureq::Error::Status(413, _)) => Ok(Http::TooLarge),
        Err(ureq::Error::Status(code, response)) => Err(CasError::Other(format!(
            "quality events {action} failed with status {code}: {}",
            response.into_string().unwrap_or_default()
        ))),
        Err(ureq::Error::Transport(error)) => {
            Err(CasError::Other(format!("Network error: {error}")))
        }
    }
}
