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
//! - 404 or 501 means that direction is not live: its cursor holds, without error.
//! - Turns have their own push cursor and unsupported-kind cooldown. A rejected
//!   Turn never holds ordinary pushes or prevents the independent pull attempt.
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
const TURN_PUSH_CURSOR_KEY: &str = "quality_events_turn_push_seq";
const TURN_ABSENT_UNTIL_KEY: &str = "quality_events_turn_absent_until";
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
    /// The server returned 404/501 for a direction of this route.
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
        if self
            .quality_metadata_time(ABSENT_UNTIL_KEY)
            .is_some_and(|until| now < until)
            && !force
        {
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

        // Pull is independent, even if push returns a validation/network error.
        let push = self.push_quality_events(&token, &project_id, &team_id, force, &mut summary);
        let pull = self.pull_quality_events(&token, &project_id, &team_id, &mut summary);
        let push_absent = push?;
        let pull_absent = pull?;
        let cursor_absent = push_absent || pull_absent;
        if cursor_absent {
            summary.capability_absent = true;
            let until = now + Duration::hours(ABSENT_RETRY);
            self.queue()
                .set_metadata(ABSENT_UNTIL_KEY, &until.to_rfc3339())?;
        } else {
            self.queue().delete_metadata(ABSENT_UNTIL_KEY)?;
        }
        self.queue()
            .set_metadata(LAST_ROUND_KEY, &now.to_rfc3339())?;
        Ok(summary)
    }

    /// Returns true when the route is not live.
    fn push_quality_events(
        &self,
        token: &str,
        project_id: &str,
        team_id: &str,
        force: bool,
        summary: &mut QualitySyncSummary,
    ) -> Result<bool, CasError> {
        if self.push_quality_event_kind(token, project_id, team_id, false, summary)? {
            return Ok(true);
        }
        if !force
            && self
                .quality_metadata_time(TURN_ABSENT_UNTIL_KEY)
                .is_some_and(|until| Utc::now() < until)
        {
            return Ok(false);
        }
        self.push_quality_event_kind(token, project_id, team_id, true, summary)
    }

    /// Separate cursors keep unsupported Turns retryable without blocking other
    /// kinds. The new Turn cursor starts at zero; old successes replay safely
    /// against Cloud's idempotent event ids.
    fn push_quality_event_kind(
        &self,
        token: &str,
        project_id: &str,
        team_id: &str,
        turns: bool,
        summary: &mut QualitySyncSummary,
    ) -> Result<bool, CasError> {
        let cursor_key = if turns {
            TURN_PUSH_CURSOR_KEY
        } else {
            PUSH_CURSOR_KEY
        };
        let cas_dir = self.queue.cas_dir().to_path_buf();
        let url = format!(
            "{}/api/teams/{}/quality-events",
            self.cloud_config.endpoint, team_id
        );
        let mut limit = QUALITY_EVENT_BATCH_LIMIT;
        for _ in 0..MAX_ROUND_BATCHES {
            let cursor = self
                .queue()
                .get_metadata(cursor_key)?
                .and_then(|raw| raw.parse::<i64>().ok())
                .unwrap_or(0);
            let events = quality_events_after(&cas_dir, cursor, limit)
                .map_err(|error| CasError::Other(error.to_string()))?;
            let Some(last_seq) = events.last().map(|event| event.seq) else {
                if turns {
                    self.queue().delete_metadata(TURN_ABSENT_UNTIL_KEY)?;
                }
                return Ok(false);
            };
            let wire: Vec<QualityEventWire> = events
                .iter()
                // cas-87a2: Cloud has no Friction kind yet. An older endpoint
                // may refuse a whole batch for an unknown kind, so Friction
                // stays local rather than stall the ordinary cursor. A future
                // lane can start its own cursor at zero; ids are idempotent.
                .filter(|event| match event.payload.kind() {
                    "friction" => false,
                    "turn" => turns,
                    _ => !turns,
                })
                .map(|event| {
                    let mut wire = event.to_wire();
                    wire.project_id = Some(project_id.to_string());
                    wire
                })
                .collect();
            if wire.is_empty() {
                self.queue()
                    .set_metadata(cursor_key, &last_seq.to_string())?;
                if events.len() < limit {
                    return Ok(false);
                }
                continue;
            }
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
            // A status alone says nothing about kind support. Preserve and
            // report unrelated validation errors rather than hiding them.
            let response = match response {
                Err(ureq::Error::Status(code @ (400 | 422), response)) if turns => {
                    let body = response.into_string().unwrap_or_default();
                    if turn_kind_unsupported(&body) {
                        summary.refused += wire.len();
                        self.defer_turn_push()?;
                        return Ok(false);
                    }
                    return Err(CasError::Other(format!(
                        "quality events push failed with status {code}: {body}"
                    )));
                }
                response => response,
            };
            let answer = match quality_http(response, "push")? {
                Http::Absent => return Ok(true),
                Http::TooLarge => {
                    // Hold the cursor. A single event over the limit cannot be
                    // split, so report it rather than stall silently forever.
                    if wire.len() == 1 {
                        return Err(CasError::Other(
                            "quality events push: one event exceeds the server's size limit".into(),
                        ));
                    }
                    limit = (wire.len() / 2).max(1);
                    continue;
                }
                Http::Ok(answer) => answer,
            };
            let answer: PushAnswer = serde_json::from_value(answer).unwrap_or_default();
            // Any refused Turn stays behind its own cursor. Ordinary kinds
            // have already advanced, and pull still gets its own attempt.
            let rejected_turn = events.iter().find(|event| {
                turns
                    && event.payload.kind() == "turn"
                    && answer.rejected.iter().any(|row| {
                        row.get("id").and_then(serde_json::Value::as_str) == Some(event.id.as_str())
                    })
            });
            // Other legacy row-level refusals preserve their prior behavior.
            if !answer.rejected.is_empty() {
                tracing::warn!(rejected = ?answer.rejected, "cloud refused quality events");
            }
            summary.refused += answer.rejected.len();
            summary.pushed += wire.len().saturating_sub(answer.rejected.len());
            let accepted_through = rejected_turn.map_or(last_seq, |turn| {
                events
                    .iter()
                    .take_while(|event| event.seq < turn.seq)
                    .last()
                    .map_or(cursor, |event| event.seq)
            });
            self.queue()
                .set_metadata(cursor_key, &accepted_through.to_string())?;
            if rejected_turn.is_some() {
                self.defer_turn_push()?;
                return Ok(false);
            }
            if turns {
                self.queue().delete_metadata(TURN_ABSENT_UNTIL_KEY)?;
            }
            if events.len() < limit {
                return Ok(false);
            }
        }
        Ok(false)
    }

    fn defer_turn_push(&self) -> Result<(), CasError> {
        let until = Utc::now() + Duration::hours(ABSENT_RETRY);
        self.queue()
            .set_metadata(TURN_ABSENT_UNTIL_KEY, &until.to_rfc3339())
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
            if page_len < PULL_PAGE_LIMIT {
                return Ok(false);
            }
        }
        Ok(false)
    }
}

/// Only an explicit kind diagnosis in the error message is a capability miss;
/// incidental mentions of Turn in the rejected payload are not evidence.
fn turn_kind_unsupported(body: &str) -> bool {
    let value = serde_json::from_str::<serde_json::Value>(body)
        .unwrap_or_else(|_| serde_json::Value::String(body.to_string()));
    let messages = [
        value.as_str(),
        value.get("error").and_then(serde_json::Value::as_str),
        value.get("message").and_then(serde_json::Value::as_str),
        value
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(serde_json::Value::as_str),
    ];
    messages.into_iter().flatten().any(|message| {
        let message = message.to_ascii_lowercase();
        let words: Vec<_> = message
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|word| !word.is_empty())
            .collect();
        words.windows(4).any(|w| {
            w == ["unsupported", "event", "kind", "turn"]
                || w == ["unknown", "event", "kind", "turn"]
                || w == ["kind", "turn", "is", "unsupported"]
        }) || words
            .windows(3)
            .any(|w| w == ["unsupported", "kind", "turn"] || w == ["unknown", "kind", "turn"])
    })
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

#[cfg(test)]
#[path = "quality_events_tests.rs"]
mod tests;
