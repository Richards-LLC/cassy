//! Contract adapter for PSC operator inbox v1 (cloud cas-8a96@5fd3f9a50).
//!
//! No HTTP implementation, scheduler, key custody, enrollment or plaintext
//! conversion is supplied here. Production admission must first persist a
//! verified audience and a stable encrypted envelope. Phase 2a unenrolled
//! snapshots cannot be passed to this API as account events.
//!
//! A separately owned transport performs PSC-PoP with dedicated relay keys,
//! freshly checks its grant/role, and binds the exact method, path and body
//! digest below. It must never forward a hub credential or hub DPoP proof.
//! Browser persistence is a separate module; calling `ack_persisted` requires
//! its transaction to have committed. Nothing here advances a local cursor.

use std::{future::Future, pin::Pin, time::Duration};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::de::DeserializeOwned;
use serde_json::json;
use sha2::{Digest, Sha256};

pub mod assertion;
pub mod commands;
pub mod drain;
pub mod jws;
pub mod machine;
pub mod presence;
mod wire;
pub use wire::{
    AckOutcome, AckReceipt, AckRow, AppendOutcome, AppendReceipt, AppendRow, Cursor,
    ExpiredInterval, ExpiryReason, PersistedEvent, Position, ReadMark, ReplayEvent, ReplayPage,
    SealedEvent,
};

pub type Result<T> = std::result::Result<T, Failure>;
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);
const MAX_RESPONSE: usize = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Failure {
    #[error("operator inbox transport unavailable")]
    Unavailable,
    #[error("operator inbox request deadline exceeded")]
    Deadline,
    #[error("operator inbox contract refusal: {0}")]
    Protocol(&'static str),
    #[error("operator inbox HTTP refusal {status}: {code}")]
    Http {
        status: u16,
        code: String,
        recovery: Option<Recovery>,
    },
}

impl Failure {
    /// Unknown 4xx codes are terminal. Known 409/410 recovery is explicit;
    /// it never silently changes a persisted cursor or retries a mutation.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Unavailable | Self::Deadline)
            || matches!(
                self,
                Self::Http {
                    status: 429 | 500..=599,
                    ..
                }
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovery {
    HistoryExpired {
        generation: Position,
        floor: Position,
        head: Position,
        expired_through: Position,
    },
    GenerationChanged {
        generation: Position,
        start: Position,
    },
    CursorAhead {
        head: Position,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Machine,
    Device,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
}
impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
        }
    }
}

/// Deliberately no Debug: body bytes are ciphertext or private routing metadata.
/// No compression is allowed. The signer uses `body_digest` as PSC-PoP `bdg`.
pub struct Request {
    pub role: Role,
    pub method: Method,
    pub path: String,
    pub body: Vec<u8>,
    pub body_digest: String,
    pub max_response_bytes: usize,
}

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

pub trait AuthenticatedTransport: Send + Sync {
    /// Resolve only a current grant of `request.role`; sign a fresh nonce with
    /// typ=psc-op-pop+jwt, htm, exact htp and bdg, gid/gen and cloud-only aud.
    /// Device requests use their exact enrolled Origin. This adapter bounds
    /// the whole future, including signing, fetch and body consumption.
    /// Cap body consumption at `max_response_bytes` before allocating a full
    /// response; dropping the future must cancel its request. Do not detach
    /// a background mutation that survives this request's cancellation.
    fn exchange(
        &self,
        request: Request,
    ) -> Pin<Box<dyn Future<Output = Result<Response>> + Send + '_>>;
}

async fn exchange<T: AuthenticatedTransport, R: DeserializeOwned>(
    transport: &T,
    role: Role,
    method: Method,
    path: String,
    body: Vec<u8>,
) -> Result<R> {
    if body.len() > 1_572_864 {
        return Err(Failure::Protocol("request size"));
    }
    let request = Request {
        role,
        method,
        path,
        body_digest: URL_SAFE_NO_PAD.encode(Sha256::digest(&body)),
        max_response_bytes: MAX_RESPONSE,
        body,
    };
    let response = tokio::time::timeout(REQUEST_DEADLINE, transport.exchange(request))
        .await
        .map_err(|_| Failure::Deadline)??;
    if response.body.len() > MAX_RESPONSE {
        return Err(Failure::Protocol("response size"));
    }
    if response.status != 200 {
        return Err(http_failure(response));
    }
    serde_json::from_slice(&response.body).map_err(|_| Failure::Protocol("response shape"))
}

fn http_failure(response: Response) -> Failure {
    let value: serde_json::Value = serde_json::from_slice(&response.body).unwrap_or_default();
    // Never expose raw error_description/body, which may carry sensitive data.
    let code = value
        .get("error")
        .and_then(|v| v.as_str())
        .filter(|s| wire::error_code(s))
        .unwrap_or("unknown_error")
        .to_owned();
    let position = |key: &str| {
        value
            .get(key)
            .cloned()
            .and_then(|v| serde_json::from_value::<Position>(v).ok())
    };
    let recovery = match (response.status, code.as_str()) {
        (410, "history_expired") => (|| {
            let generation = position("feed_generation")?;
            let floor = position("retained_floor")?;
            let head = position("head")?;
            let expired_through = position("expired_through")?;
            if generation.value() == 0
                || floor.value() == 0
                || floor.value() > head.value() + 1
                || expired_through.value() != floor.value() - 1
            {
                return None;
            }
            Some(Recovery::HistoryExpired {
                generation,
                floor,
                head,
                expired_through,
            })
        })(),
        (409, "feed_generation_changed") => (|| {
            let generation = position("feed_generation")?;
            if generation.value() == 0 {
                return None;
            }
            Some(Recovery::GenerationChanged {
                generation,
                start: position("start_sequence")?,
            })
        })(),
        (409, "cursor_ahead") => position("head").map(|head| Recovery::CursorAhead { head }),
        _ => None,
    };
    Failure::Http {
        status: response.status,
        code,
        recovery,
    }
}

fn encode(value: impl serde::Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(&value).map_err(|_| Failure::Protocol("request serialization"))
}
fn generation(value: Position) -> Result<()> {
    if value.value() == 0 {
        return Err(Failure::Protocol("feed generation"));
    }
    Ok(())
}

/// Machine scope exposes append only, never feed replay or shared read state.
pub struct MachineRelay<T> {
    transport: T,
}
impl<T: AuthenticatedTransport> MachineRelay<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub async fn append(
        &self,
        expected_generation: Position,
        events: &[SealedEvent],
    ) -> Result<AppendReceipt> {
        generation(expected_generation)?;
        if !(1..=100).contains(&events.len()) {
            return Err(Failure::Protocol("append count"));
        }
        let mut total = 0;
        let mut ids = std::collections::HashSet::new();
        for event in events {
            total += event.validate()?;
            if !ids.insert(&event.event_id) {
                return Err(Failure::Protocol("duplicate batch identity"));
            }
        }
        if total > 1_048_576 {
            return Err(Failure::Protocol("append bytes"));
        }
        let body = encode(json!({"wire_version":1,"events":events}))?;
        let receipt: AppendReceipt = exchange(
            &self.transport,
            Role::Machine,
            Method::Post,
            "/api/operator/feed/events".into(),
            body,
        )
        .await?;
        if receipt.wire_version != 1
            || receipt.feed_generation != expected_generation
            || receipt.rows.len() != events.len()
        {
            return Err(Failure::Protocol("append receipt scope"));
        }
        for (row, event) in receipt.rows.iter().zip(events) {
            if row.event_id != event.event_id {
                return Err(Failure::Protocol("append receipt identity"));
            }
            match &row.outcome {
                AppendOutcome::Stored {
                    sequence,
                    digest,
                    key_epoch,
                    stored_at,
                    expires_at,
                }
                | AppendOutcome::Duplicate {
                    sequence,
                    digest,
                    key_epoch,
                    stored_at,
                    expires_at,
                } => {
                    if sequence.value() == 0
                        || digest != &event.digest
                        || key_epoch != &event.key_epoch
                        || stored_at.is_empty()
                        || expires_at.is_empty()
                    {
                        return Err(Failure::Protocol("append receipt binding"));
                    }
                }
                AppendOutcome::Expired {
                    sequence,
                    digest,
                    expired_at,
                } => {
                    if sequence.value() == 0 || digest != &event.digest || expired_at.is_empty() {
                        return Err(Failure::Protocol("expired receipt binding"));
                    }
                }
                AppendOutcome::Rejected { error, .. } if !wire::error_code(error) => {
                    return Err(Failure::Protocol("append rejection"));
                }
                AppendOutcome::Rejected { .. } => {}
            }
        }
        Ok(receipt)
    }
}

/// Device operations have distinct result types and never share an implicit
/// high-water mark. The host browser owns atomic projection and durable ACKs.
pub struct DeviceInbox<T> {
    transport: T,
}
impl<T: AuthenticatedTransport> DeviceInbox<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub async fn replay(
        &self,
        feed_generation: Position,
        after: Position,
        limit: u16,
    ) -> Result<ReplayPage> {
        generation(feed_generation)?;
        if !(1..=500).contains(&limit) {
            return Err(Failure::Protocol("page limit"));
        }
        let path = format!(
            "/api/operator/feed?generation={}&after={}&limit={limit}",
            feed_generation.value(),
            after.value()
        );
        let page: ReplayPage =
            exchange(&self.transport, Role::Device, Method::Get, path, Vec::new()).await?;
        page.validate(feed_generation, after, limit)?;
        Ok(page)
    }

    /// Precondition: each event/digest was committed in this device's local
    /// projection. A socket write or a shared read watermark is insufficient.
    pub async fn ack_persisted(
        &self,
        feed_generation: Position,
        acks: &[PersistedEvent],
    ) -> Result<AckReceipt> {
        generation(feed_generation)?;
        if !(1..=500).contains(&acks.len()) {
            return Err(Failure::Protocol("ack count"));
        }
        let mut ids = std::collections::HashSet::new();
        for ack in acks {
            wire::identity(&ack.event_id)?;
            wire::validate_digest(&ack.digest)?;
            if !ids.insert(&ack.event_id) {
                return Err(Failure::Protocol("duplicate ack identity"));
            }
        }
        let receipt: AckReceipt = exchange(
            &self.transport,
            Role::Device,
            Method::Post,
            "/api/operator/feed/acks".into(),
            encode(json!({"wire_version":1,"feed_generation":feed_generation,"acks":acks}))?,
        )
        .await?;
        if receipt.wire_version != 1
            || receipt.rows.len() != acks.len()
            || receipt
                .rows
                .iter()
                .zip(acks)
                .any(|(row, ack)| row.event_id != ack.event_id)
        {
            return Err(Failure::Protocol("ack receipt binding"));
        }
        if receipt.rows.iter().any(|row| matches!(&row.outcome, AckOutcome::Rejected { error } if !wire::error_code(error))) {
            return Err(Failure::Protocol("ack rejection"));
        }
        Ok(receipt)
    }

    pub async fn cursor(&self, expected_generation: Position) -> Result<Cursor> {
        generation(expected_generation)?;
        let cursor: Cursor = exchange(
            &self.transport,
            Role::Device,
            Method::Get,
            "/api/operator/devices/me/cursor".into(),
            Vec::new(),
        )
        .await?;
        if cursor.wire_version != 1
            || cursor.feed_generation != expected_generation
            || cursor
                .cursor
                .is_some_and(|v| cursor.accepted_expired_through > v)
        {
            return Err(Failure::Protocol("cursor receipt scope"));
        }
        Ok(cursor)
    }

    /// Call after the local page transaction, including an explicitly accepted
    /// expiry gap. This receipt never proves another device stored the page.
    pub async fn save_cursor(&self, committed: &Cursor) -> Result<Cursor> {
        generation(committed.feed_generation)?;
        let Some(position) = committed.cursor else {
            return Err(Failure::Protocol("null cursor write"));
        };
        if committed.wire_version != 1 || committed.accepted_expired_through > position {
            return Err(Failure::Protocol("cursor write"));
        }
        let body = encode(
            json!({"wire_version":1,"feed_generation":committed.feed_generation,"cursor":position,"accepted_expired_through":committed.accepted_expired_through}),
        )?;
        let receipt: Cursor = exchange(
            &self.transport,
            Role::Device,
            Method::Put,
            "/api/operator/devices/me/cursor".into(),
            body,
        )
        .await?;
        if receipt.wire_version != 1
            || receipt.feed_generation != committed.feed_generation
            || receipt.cursor.is_none_or(|v| v < position)
            || receipt.accepted_expired_through < committed.accepted_expired_through
            || receipt
                .cursor
                .is_some_and(|v| receipt.accepted_expired_through > v)
        {
            return Err(Failure::Protocol("cursor receipt binding"));
        }
        Ok(receipt)
    }

    /// Human read state only; lower writes can return a higher existing mark.
    /// No replay or persistence side effect is performed by this method.
    pub async fn mark_read(&self, mark: &ReadMark) -> Result<ReadMark> {
        generation(mark.feed_generation)?;
        for field in [&mark.hub_id, &mark.project_id, &mark.session_id] {
            wire::route(field)?;
        }
        if mark.wire_version != 1 {
            return Err(Failure::Protocol("read version"));
        }
        let body = encode(
            json!({"wire_version":1,"feed_generation":mark.feed_generation,"hub_id":mark.hub_id,"project_id":mark.project_id,"session_id":mark.session_id,"sequence":mark.sequence}),
        )?;
        let receipt: ReadMark = exchange(
            &self.transport,
            Role::Device,
            Method::Put,
            "/api/operator/read-marks".into(),
            body,
        )
        .await?;
        if receipt.wire_version != 1
            || receipt.feed_generation != mark.feed_generation
            || receipt.hub_id != mark.hub_id
            || receipt.project_id != mark.project_id
            || receipt.session_id != mark.session_id
            || receipt.sequence < mark.sequence
        {
            return Err(Failure::Protocol("read receipt binding"));
        }
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests;
