//! PSC operator-inbox wire v1. Unknown response keys are additive; outcomes
//! remain closed. Decimal strings never pass through floating-point JSON.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

use super::{Failure, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position(u64);

impl Position {
    pub fn new(value: u64) -> Result<Self> {
        if value > 9_999_999_999_999_999_999 {
            return Err(Failure::Protocol("decimal range"));
        }
        Ok(Self(value))
    }

    pub fn value(self) -> u64 {
        self.0
    }
}

impl Serialize for Position {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for Position {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        if raw.is_empty()
            || raw.len() > 19
            || !raw.bytes().all(|b| b.is_ascii_digit())
            || (raw.len() > 1 && raw.starts_with('0'))
        {
            return Err(serde::de::Error::custom("invalid decimal string"));
        }
        let value = raw.parse::<u64>().map_err(serde::de::Error::custom)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

pub(super) fn identity(value: &str) -> Result<()> {
    if !(22..=64).contains(&value.len())
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(Failure::Protocol("opaque identity"));
    }
    Ok(())
}

/// The §4.2 decimal-string pattern `^(0|[1-9][0-9]{0,18})$`.
pub(super) fn decimal(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 19
        && value.bytes().all(|b| b.is_ascii_digit())
        && (value.len() == 1 || !value.starts_with('0'))
}

pub(super) fn route(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 200
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:@/-".contains(&b))
    {
        return Err(Failure::Protocol("routing identity"));
    }
    Ok(())
}

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub(super) fn error_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

pub(super) fn validate_digest(value: &str) -> Result<()> {
    if value.len() != 71
        || !value.starts_with("sha256:")
        || !value[7..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Failure::Protocol("ciphertext digest"));
    }
    Ok(())
}

/// Input must come from a verified binding and an interoperable sealer, with
/// these exact ciphertext bytes durably frozen before upload. This type alone
/// is neither enrollment proof nor evidence that supplied bytes are encrypted.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SealedEvent {
    pub event_id: String,
    pub hub_id: String,
    pub project_id: String,
    pub session_id: String,
    pub key_epoch: Position,
    pub ciphertext: String,
    pub digest: String,
    pub attachment_ids: Vec<String>,
}

impl SealedEvent {
    pub(super) fn validate(&self) -> Result<usize> {
        identity(&self.event_id)?;
        for field in [&self.hub_id, &self.project_id, &self.session_id] {
            route(field)?;
        }
        if self.key_epoch.value() == 0 || self.attachment_ids.len() > 16 {
            return Err(Failure::Protocol("event metadata"));
        }
        let mut ids = std::collections::HashSet::new();
        for id in &self.attachment_ids {
            identity(id)?;
            if !ids.insert(id) {
                return Err(Failure::Protocol("duplicate attachment"));
            }
        }
        if self.ciphertext.len() > 87_382 {
            return Err(Failure::Protocol("ciphertext size"));
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(&self.ciphertext)
            .map_err(|_| Failure::Protocol("ciphertext encoding"))?;
        if bytes.is_empty()
            || bytes.len() > 65_536
            || URL_SAFE_NO_PAD.encode(&bytes) != self.ciphertext
        {
            return Err(Failure::Protocol("ciphertext size or encoding"));
        }
        if digest(&bytes) != self.digest {
            return Err(Failure::Protocol("ciphertext digest"));
        }
        Ok(bytes.len())
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AppendOutcome {
    Stored {
        sequence: Position,
        digest: String,
        key_epoch: Position,
        stored_at: String,
        expires_at: String,
    },
    Duplicate {
        sequence: Position,
        digest: String,
        key_epoch: Position,
        stored_at: String,
        expires_at: String,
    },
    Expired {
        sequence: Position,
        digest: String,
        expired_at: String,
    },
    Rejected {
        error: String,
        #[serde(default)]
        active_epoch: Option<Position>,
        #[serde(default)]
        policy_version: Option<Position>,
    },
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AppendRow {
    pub event_id: String,
    #[serde(flatten)]
    pub outcome: AppendOutcome,
}

#[derive(Debug, Deserialize)]
pub struct AppendReceipt {
    pub wire_version: u8,
    pub feed_generation: Position,
    pub rows: Vec<AppendRow>,
}

#[derive(Clone, Deserialize)]
pub struct ReplayEvent {
    pub sequence: Position,
    #[serde(flatten)]
    pub event: SealedEvent,
    pub stored_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExpiryReason {
    Retention,
    AccountDeleted,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExpiredInterval {
    pub from: Position,
    pub to: Position,
    pub reason: ExpiryReason,
}

#[derive(Clone, Deserialize)]
pub struct ReplayPage {
    pub wire_version: u8,
    pub feed_generation: Position,
    pub retained_floor: Position,
    pub head: Position,
    pub events: Vec<ReplayEvent>,
    pub expired_intervals: Vec<ExpiredInterval>,
    pub next_cursor: Position,
    pub has_more: bool,
    pub poll_after_ms: u64,
}

impl ReplayPage {
    /// Validates complete, non-overlapping coverage without expanding intervals
    /// (a huge expired interval must not cause unbounded iteration/allocation).
    pub(super) fn validate(&self, generation: Position, after: Position, limit: u16) -> Result<()> {
        if self.wire_version != 1
            || self.feed_generation != generation
            || generation.value() == 0
            || self.retained_floor.value() == 0
            || self.retained_floor.value() > self.head.value() + 1
            || after > self.head
            || after.value() < self.retained_floor.value() - 1
            || self.next_cursor < after
            || self.next_cursor > self.head
            || self.events.len() > usize::from(limit)
            || self.has_more != (self.next_cursor < self.head)
        {
            return Err(Failure::Protocol("replay bounds"));
        }
        let mut coverage = Vec::with_capacity(self.events.len() + self.expired_intervals.len());
        let mut prior = after.value();
        let mut ids = std::collections::HashSet::new();
        for row in &self.events {
            row.event.validate()?;
            if row.sequence.value() <= prior
                || row.sequence < self.retained_floor
                || !ids.insert(&row.event.event_id)
                || row.stored_at.is_empty()
                || row.expires_at.is_empty()
            {
                return Err(Failure::Protocol("replay event identity or order"));
            }
            prior = row.sequence.value();
            coverage.push((prior, prior));
        }
        let mut prior_end = after.value();
        for gap in &self.expired_intervals {
            if gap.from.value() <= prior_end || gap.from > gap.to {
                return Err(Failure::Protocol("expired interval order"));
            }
            prior_end = gap.to.value();
            coverage.push((gap.from.value(), gap.to.value()));
        }
        coverage.sort_unstable();
        let mut covered = after.value();
        for (from, to) in coverage {
            if from != covered + 1 || to > self.next_cursor.value() {
                return Err(Failure::Protocol("replay coverage"));
            }
            covered = to;
        }
        if covered != self.next_cursor.value() || (covered == after.value() && self.has_more) {
            return Err(Failure::Protocol("replay coverage"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedEvent {
    pub event_id: String,
    pub digest: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AckOutcome {
    Acked,
    AlreadyAcked,
    Rejected { error: String },
}

impl AckOutcome {
    /// Expiry is an accepted no-op under §8.2, not retained content or read.
    pub fn is_acknowledged(&self) -> bool {
        matches!(self, Self::Acked | Self::AlreadyAcked)
            || matches!(self, Self::Rejected { error } if error == "event_expired")
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AckRow {
    pub event_id: String,
    #[serde(flatten)]
    pub outcome: AckOutcome,
}

#[derive(Debug, Deserialize)]
pub struct AckReceipt {
    pub wire_version: u8,
    pub rows: Vec<AckRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cursor {
    pub wire_version: u8,
    pub feed_generation: Position,
    pub cursor: Option<Position>,
    pub accepted_expired_through: Position,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadMark {
    pub wire_version: u8,
    pub feed_generation: Position,
    pub hub_id: String,
    pub project_id: String,
    pub session_id: String,
    pub sequence: Position,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by_device_id: Option<String>,
}
