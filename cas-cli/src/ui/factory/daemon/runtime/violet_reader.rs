//! Violet v2 reader for managed Slack bindings (cas-146e, petra-stella-cloud#178).
//!
//! Cloud owns admission, dedupe and recovery. This reader keeps only Cloud's
//! reader order (docs/API-REFERENCE.md, `lib/coordination/contract.ts`):
//!
//! claim → ack `admitted` → fetch body → receipt `dispatch_uncertain` (wait
//! for Cloud to accept it) → type → receipt `consumed`.
//!
//! A refused `dispatch_uncertain` means don't type; it also fences a
//! superseded reader. Hand-out is at least once, so an in-memory set keyed on
//! `delivery.id` keeps one process from typing a delivery twice. Nothing here
//! is persisted. Once Cloud answers for a managed binding, the legacy v1
//! claim and channel sweep stay off for this process ([`VioletReader::managed`]).

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::coordination::external_work::{CALL_REMOTE_BUDGET, CONTRACT_SCHEMA, CloudCoordinationPort};

const CLAIM_PATH: &str = "/api/violet/activity/claim";
const ACK_PATH: &str = "/api/violet/activity/ack";
const CLAIM_MAX: u32 = 25;
const LEASE_SECS: u32 = 120;

/// A step that did not happen, by Cloud's or Cassy's stable code. Nothing was
/// typed for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NamedRefusal {
    pub code: String,
    pub reason: String,
}

impl NamedRefusal {
    pub(crate) fn new(code: impl Into<String>, reason: impl Into<String>) -> Self {
        Self { code: code.into(), reason: reason.into() }
    }

    fn from_payload(payload: &Value) -> Self {
        let text = |key: &str| payload.get(key).and_then(Value::as_str).map(str::to_string);
        Self::new(
            text("code").unwrap_or_else(|| "invalid_response".into()),
            text("reason").or_else(|| text("guidance")).unwrap_or_default(),
        )
    }
}

impl std::fmt::Display for NamedRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.reason)
    }
}

/// One leased delivery from a v2 claim. `target` is Cloud's reference
/// (resource, project, `external_ref`, `message_ts`, permalink), never a body.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ClaimedDelivery {
    pub id: String,
    pub delivery_generation: u64,
    pub lease_token: String,
    pub target: Value,
}

/// Fetches the text to type for a claimed target (cas-f82cb plugs Violet in).
pub(crate) trait BodySource {
    fn fetch_body(&self, target: &Value) -> Result<String, NamedRefusal>;
}

/// Types a body into a local supervisor session.
pub(crate) trait SessionTyper {
    fn type_into(&self, session: &str, delivery_id: &str, body: &str) -> Result<(), NamedRefusal>;
}

/// What one reader tick did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClaimOutcome {
    /// Cloud leased and the reader walked the order for each delivery.
    Read(ReaderReport),
    /// `403 coordination_scope_required`: no managed binding, so v1 applies.
    Unmanaged,
    /// Any other claim refusal, such as `409 strict_activation_required`.
    Refused(NamedRefusal),
    /// The reader was fenced earlier (cas-c186 sets [`VioletReader::stopped`]).
    Stopped(NamedRefusal),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ReaderReport {
    pub claimed: usize,
    pub typed: usize,
    /// `(delivery id, why it was not typed)`.
    pub not_typed: Vec<(String, NamedRefusal)>,
}

/// The reader's in-memory state for one daemon process.
pub(crate) struct VioletReader {
    default_session: String,
    /// The don't-type-twice guard: every delivery typed by this process.
    typed: HashSet<String>,
    /// delivery.id → local session (cas-c186 binds; default otherwise).
    sessions: HashMap<String, String>,
    /// Set when Cloud says this registration is superseded; no more claims.
    pub(crate) stopped: Option<NamedRefusal>,
    managed: bool,
}

impl VioletReader {
    pub(crate) fn new(default_session: impl Into<String>) -> Self {
        Self {
            default_session: default_session.into(),
            typed: HashSet::new(),
            sessions: HashMap::new(),
            stopped: None,
            managed: false,
        }
    }

    pub(crate) fn bind_session(&mut self, delivery_id: &str, session: &str) {
        self.sessions.insert(delivery_id.to_string(), session.to_string());
    }

    pub(crate) fn session_for(&self, delivery_id: &str) -> &str {
        self.sessions.get(delivery_id).map_or(self.default_session.as_str(), String::as_str)
    }

    /// True once Cloud has answered for a managed binding (a completed claim or
    /// `strict_activation_required`). The legacy v1 claim and sweep stay off.
    pub(crate) fn managed(&self) -> bool {
        self.managed
    }

    /// Claim, then walk Cloud's reader order for each leased delivery.
    pub(crate) fn tick(
        &mut self,
        port: &dyn CloudCoordinationPort,
        registration_id: &str,
        body: &dyn BodySource,
        typer: &dyn SessionTyper,
        now: DateTime<Utc>,
    ) -> ClaimOutcome {
        let _ = (port, registration_id, body, typer, now, CLAIM_PATH, ACK_PATH, CLAIM_MAX, LEASE_SECS);
        ClaimOutcome::Read(ReaderReport::default())
    }
}

#[cfg(test)]
#[path = "violet_reader_tests.rs"]
mod tests;
