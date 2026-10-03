//! Machine-local Commander authorization state.
//!
//! Implements H2-PERM-01 through H2-AUDIT-06 from the binding Commander ADR.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::broadcast;

use crate::ui::factory::ClientMessage;
use std::fmt;

const PAIRING_TTL_MINUTES: i64 = 10;
const WS_TICKET_TTL_MINUTES: i64 = 5;
const CREDENTIAL_ABSOLUTE_DAYS: i64 = 90;
const CREDENTIAL_IDLE_DAYS: i64 = 30;
const CREDENTIAL_REFRESH_GRACE_DAYS: i64 = 7;
const DPOP_SKEW_SECONDS: i64 = 60;
const DPOP_REPLAY_MINUTES: i64 = 5;

#[derive(Debug, thiserror::Error)]
pub enum PairingExchangeError {
    /// Five exchanges from the same bound controller origin are already inside the one-minute window.
    #[error("pairing exchange throttled")]
    Throttled { retry_after_seconds: u64 },
    /// Every non-throttling failure retains its prior diagnostic for trusted in-process callers.
    #[error(transparent)]
    Opaque(#[from] anyhow::Error),
}

/// Why a DPoP-authenticated request was refused (cas-d636). Every refusal
/// used to be the same bare 401, and hub-web read each one as a revoked
/// pairing and stopped reconnecting, so a proof that was merely signed before
/// the phone slept and sent after it woke (soundwave, 2026-09-26 22:58Z)
/// silenced Commander until a reload. The 401 now names the reason, and only
/// the definitive ones ask for a new pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthRefusal {
    /// The proof's `iat` is outside the hub's skew window; retry with a fresh proof.
    #[error("authentication refused: stale proof ({skew_secs}s from the hub clock)")]
    StaleProof { skew_secs: i64 },
    /// The proof does not verify for this request (signature, method, target,
    /// access-token hash or a missing jti); retry with a fresh proof.
    #[error("authentication refused: invalid proof")]
    InvalidProof,
    /// The proof's jti was already used; retry with a fresh proof.
    #[error("authentication refused: proof replayed")]
    ProofReplay,
    /// The proof was signed by a key other than the paired one.
    #[error("authentication refused: proof key does not match the pairing")]
    KeyMismatch,
    /// No paired device holds this credential.
    #[error("authentication refused: unknown credential")]
    UnknownCredential,
    /// The Authorization header is not a DPoP credential.
    #[error("authentication refused: malformed credential")]
    MalformedCredential,
    #[error("authentication refused: pairing revoked")]
    Revoked,
    /// Past its absolute lifetime; the credential refresh route may still renew it.
    #[error("authentication refused: credential expired")]
    Expired,
    /// Unused for longer than the idle limit.
    #[error("authentication refused: credential idle too long")]
    Idle,
    #[error("authentication refused: origin does not match the pairing")]
    OriginMismatch,
}

impl AuthRefusal {
    /// The machine-readable reason carried on the 401 and in the audit row.
    pub fn code(self) -> &'static str {
        match self {
            Self::StaleProof { .. } => "stale_proof",
            Self::InvalidProof => "invalid_proof",
            Self::ProofReplay => "proof_replay",
            Self::KeyMismatch => "key_mismatch",
            Self::UnknownCredential => "unknown_credential",
            Self::MalformedCredential => "malformed_credential",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
            Self::Idle => "idle",
            Self::OriginMismatch => "origin_mismatch",
        }
    }

    /// Whether the same credential can succeed with a fresh proof.
    pub fn retryable(self) -> bool {
        matches!(self, Self::StaleProof { .. } | Self::InvalidProof | Self::ProofReplay)
    }

    /// RFC 9449 `WWW-Authenticate: DPoP error=...`: a proof problem is
    /// `invalid_dpop_proof`, a credential problem `invalid_token`.
    pub fn dpop_error(self) -> &'static str {
        if self.retryable() { "invalid_dpop_proof" } else { "invalid_token" }
    }

    fn detail(self) -> Option<String> {
        match self {
            Self::StaleProof { skew_secs } if skew_secs < 0 => {
                Some(format!("proof iat {}s behind the hub clock", -skew_secs))
            }
            Self::StaleProof { skew_secs } => Some(format!("proof iat {skew_secs}s ahead of the hub clock")),
            _ => None,
        }
    }
}

fn refusal_of(error: &anyhow::Error) -> AuthRefusal {
    error.downcast_ref::<AuthRefusal>().copied().unwrap_or(AuthRefusal::InvalidProof)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    MachineRead,
    SessionRead,
    SessionLaunch,
    PaneRead,
    PaneInput,
    MessageSend,
    PaneInterrupt,
    FactoryManage,
    HubAdmin,
}

impl Scope {
    pub fn default_read_only() -> BTreeSet<Self> {
        [Self::MachineRead, Self::SessionRead, Self::PaneRead]
            .into_iter()
            .collect()
    }

    pub fn parse(value: &str) -> Result<Self> {
        Ok(match value {
            "machine:read" | "machine-read" => Self::MachineRead,
            "session:read" | "session-read" => Self::SessionRead,
            "session:launch" | "session-launch" => Self::SessionLaunch,
            "pane:read" | "pane-read" => Self::PaneRead,
            "pane:input" | "pane-input" => Self::PaneInput,
            "message:send" | "message-send" => Self::MessageSend,
            "pane:interrupt" | "pane-interrupt" => Self::PaneInterrupt,
            "factory:manage" | "factory-manage" => Self::FactoryManage,
            "hub:admin" | "hub-admin" => Self::HubAdmin,
            _ => anyhow::bail!("unknown Commander scope '{value}'"),
        })
    }

    /// Wire spelling used by the pairing exchange payload and the invitation URL.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::MachineRead => "machine-read",
            Self::SessionRead => "session-read",
            Self::SessionLaunch => "session-launch",
            Self::PaneRead => "pane-read",
            Self::PaneInput => "pane-input",
            Self::MessageSend => "message-send",
            Self::PaneInterrupt => "pane-interrupt",
            Self::FactoryManage => "factory-manage",
            Self::HubAdmin => "hub-admin",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::MachineRead => "machine:read",
            Self::SessionRead => "session:read",
            Self::SessionLaunch => "session:launch",
            Self::PaneRead => "pane:read",
            Self::PaneInput => "pane:input",
            Self::MessageSend => "message:send",
            Self::PaneInterrupt => "pane:interrupt",
            Self::FactoryManage => "factory:manage",
            Self::HubAdmin => "hub:admin",
        }
    }
}

pub fn required_scope(message: &ClientMessage) -> Option<Scope> {
    match message {
        ClientMessage::Input { .. }
        | ClientMessage::InputFocused { .. }
        | ClientMessage::Focus { .. }
        | ClientMessage::FocusNext
        | ClientMessage::FocusPrev
        | ClientMessage::Resize { .. } => Some(Scope::PaneInput),
        // Reporting the viewport is part of observing a pane, not terminal
        // input. The lease policy is enforced separately: an unleased pane
        // may follow an observer, while a leased pane follows its controller.
        ClientMessage::ResizePane { .. }
        | ClientMessage::RequestPaneKeyframe { .. }
        | ClientMessage::ScrollbackRequest { .. }
        | ClientMessage::ConversationHistoryRequest { .. } => Some(Scope::PaneRead),
        ClientMessage::SendMessage { .. } => Some(Scope::MessageSend),
        ClientMessage::InterruptPane { .. } => Some(Scope::PaneInterrupt),
        ClientMessage::SpawnWorkers { .. }
        | ClientMessage::ShutdownWorkers { .. }
        | ClientMessage::Inject { .. }
        | ClientMessage::SpawnShell { .. }
        | ClientMessage::KillShell { .. } => Some(Scope::FactoryManage),
        // The legacy focused-pane interrupt is intentionally never exposed.
        ClientMessage::Interrupt => None,
        ClientMessage::Attach { .. }
        | ClientMessage::Detach
        | ClientMessage::GetState
        | ClientMessage::Ping
        | ClientMessage::OperatorReplyDelivered { .. } => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicJwk {
    pub kty: String,
    pub crv: String,
    pub x: String,
    pub y: String,
}

impl PublicJwk {
    fn validate(&self) -> Result<VerifyingKey> {
        anyhow::ensure!(
            self.kty == "EC" && self.crv == "P-256",
            "invalid device key"
        );
        let x = URL_SAFE_NO_PAD
            .decode(&self.x)
            .context("invalid device key")?;
        let y = URL_SAFE_NO_PAD
            .decode(&self.y)
            .context("invalid device key")?;
        anyhow::ensure!(x.len() == 32 && y.len() == 32, "invalid device key");
        let mut point = Vec::with_capacity(65);
        point.push(4);
        point.extend_from_slice(&x);
        point.extend_from_slice(&y);
        VerifyingKey::from_sec1_bytes(&point).context("invalid device key")
    }

    fn thumbprint(&self) -> Result<String> {
        self.validate()?;
        let canonical = format!(
            r#"{{"crv":"P-256","kty":"EC","x":"{}","y":"{}"}}"#,
            self.x, self.y
        );
        Ok(hash_b64(canonical.as_bytes()))
    }

    #[cfg(test)]
    fn generator() -> Self {
        let x = hex::decode("6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296")
            .unwrap();
        let y = hex::decode("4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5")
            .unwrap();
        Self {
            kty: "EC".into(),
            crv: "P-256".into(),
            x: URL_SAFE_NO_PAD.encode(x),
            y: URL_SAFE_NO_PAD.encode(y),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PairingExchange {
    pub token: String,
    pub hub_id: String,
    pub controller_origin: String,
    pub public_key_jwk: PublicJwk,
    pub device_label: String,
    pub operator_label: String,
    pub requested_scopes: BTreeSet<Scope>,
    #[serde(default = "local_source")]
    pub source: String,
}

fn local_source() -> String {
    "local".into()
}

impl PairingExchange {
    #[cfg(test)]
    pub fn test_fixture(
        token: String,
        hub_id: &str,
        origin: &str,
        scopes: BTreeSet<Scope>,
    ) -> Self {
        Self {
            token,
            hub_id: hub_id.into(),
            controller_origin: origin.into(),
            public_key_jwk: PublicJwk::generator(),
            device_label: "test device".into(),
            operator_label: "test operator".into(),
            requested_scopes: scopes,
            source: "test".into(),
        }
    }
}

#[derive(Clone, Serialize)]
pub struct PairingInvitation {
    #[serde(skip_serializing)]
    pub token: String,
    pub url: String,
    pub expires_at: DateTime<Utc>,
    pub scopes: BTreeSet<Scope>,
    #[serde(skip_serializing)]
    controller_origin: String,
    #[serde(skip_serializing)]
    hub_id: String,
    #[serde(skip_serializing)]
    prefill: PairingPrefill,
}

/// Where the browser can reach this machine and what to call it. Both are
/// suggestions the pairing form prefills as editable values, never trusted
/// state: the exchange still goes to whatever address the operator confirms.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PairingPrefill {
    /// The machine's reachable hub origin (usually its Tailscale Serve URL).
    pub hub_url: Option<String>,
    /// The machine's display name.
    pub machine_label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairingInvitationTarget {
    LocalCommander,
    HostedRelay,
}

impl PairingInvitation {
    pub(crate) fn url_for(&self, target: PairingInvitationTarget) -> String {
        pairing_invitation_url(
            target,
            &self.controller_origin,
            &self.token,
            &self.hub_id,
            &self.scopes,
            &self.prefill,
        )
    }

    /// Carry the machine's hub address and display name in the printed link so
    /// the pairing form opens with both filled in. Older Commander builds ignore
    /// the extra fragment parameters; the hosted-relay URL never carries them
    /// because the relay delivers both through its own completion record.
    pub fn with_prefill(mut self, prefill: PairingPrefill) -> Self {
        self.prefill = prefill;
        self.url = self.url_for(PairingInvitationTarget::LocalCommander);
        self
    }
}

fn pairing_invitation_url(
    target: PairingInvitationTarget,
    controller_origin: &str,
    token: &str,
    hub_id: &str,
    scopes: &BTreeSet<Scope>,
    prefill: &PairingPrefill,
) -> String {
    let mut url = format!("{controller_origin}/#pair={token}&hub={hub_id}");
    if target == PairingInvitationTarget::LocalCommander {
        // Before `scopes`, which stays last so its value can be read to the end.
        for (key, value) in [
            ("hub_url", prefill.hub_url.as_deref()),
            ("machine", prefill.machine_label.as_deref()),
        ] {
            if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
                url.push('&');
                url.push_str(key);
                url.push('=');
                url.push_str(&urlencoding::encode(value));
            }
        }
        let declared_scopes = scopes
            .iter()
            .map(|scope| scope.as_wire())
            .collect::<Vec<_>>()
            .join(",");
        url.push_str("&scopes=");
        url.push_str(&declared_scopes);
    }
    url
}

#[derive(Clone, Serialize)]
pub struct DeviceCredential {
    pub device_id: String,
    pub credential_id: String,
    pub credential: String,
    pub expires_at: DateTime<Utc>,
    pub scopes: BTreeSet<Scope>,
}

// Every one of these carries a pairing capability: the token IS the authority to
// pair a device, so a derived Debug leaks an actionable credential.
impl fmt::Debug for PairingExchange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairingExchange")
            .field("token", &"[redacted]")
            .field("hub_id", &self.hub_id)
            .field("controller_origin", &self.controller_origin)
            .field("device_label", &self.device_label)
            .field("operator_label", &self.operator_label)
            .field("requested_scopes", &self.requested_scopes)
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for PairingInvitation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `url` embeds the same token, so it is redacted with it.
        f.debug_struct("PairingInvitation")
            .field("token", &"[redacted]")
            .field("url", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .field("scopes", &self.scopes)
            .field("controller_origin", &self.controller_origin)
            .field("hub_id", &self.hub_id)
            .field("prefill", &self.prefill)
            .finish()
    }
}

impl fmt::Debug for DeviceCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceCredential")
            .field("device_id", &self.device_id)
            .field("credential_id", &self.credential_id)
            .field("credential", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .field("scopes", &self.scopes)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceSession {
    pub device_id: String,
    pub credential_id: String,
    pub device_label: String,
    pub operator_label: String,
    pub controller_origin: String,
    pub scopes: BTreeSet<Scope>,
    pub issued_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    credential_hash: String,
    public_key: PublicJwk,
    public_key_thumbprint: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeviceSummary {
    pub device_id: String,
    pub credential_id: String,
    pub device_label: String,
    pub operator_label: String,
    pub controller_origin: String,
    pub scopes: BTreeSet<Scope>,
    pub issued_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthContext {
    pub device_id: String,
    pub credential_id: String,
    pub device_label: String,
    pub operator_label: String,
    pub controller_origin: String,
    pub scopes: BTreeSet<Scope>,
    pub request_id: String,
}

impl AuthContext {
    pub fn has(&self, scope: Scope) -> bool {
        self.scopes.contains(&scope)
    }

    #[cfg(test)]
    pub fn test_fixture(device: &str, origin: &str, scopes: BTreeSet<Scope>) -> Self {
        Self {
            device_id: device.into(),
            credential_id: "credential-test".into(),
            device_label: "test device".into(),
            operator_label: "test operator".into(),
            controller_origin: origin.into(),
            scopes,
            request_id: "request-test".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WsTicket {
    #[serde(skip_serializing)]
    pub ticket: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LeaseSummary {
    pub controller_device_id: Option<String>,
    pub controller_label: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub held_by_me: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistedState {
    pairings: Vec<PairingRecord>,
    devices: Vec<DeviceSession>,
    tickets: Vec<TicketRecord>,
    dpop_jtis: Vec<ReplayRecord>,
    source_attempts: Vec<SourceAttempt>,
    leases: BTreeMap<String, LeaseRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PairingRecord {
    token_hash: String,
    hub_id: String,
    controller_origin: String,
    max_scopes: BTreeSet<Scope>,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    consumed_at: Option<DateTime<Utc>>,
    failed_attempts: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TicketRecord {
    ticket_hash: String,
    context: AuthContext,
    session: String,
    endpoint: String,
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    consumed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReplayRecord {
    credential_id: String,
    jti: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SourceAttempt {
    source: String,
    at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LeaseRecord {
    device_id: String,
    expires_at: DateTime<Utc>,
}

/// File beside `audit.jsonl` that names a failing audit writer (cas-0140).
pub const AUDIT_HEALTH_FILE: &str = "audit-health.json";
/// The hub's append-only audit log.
pub const AUDIT_LOG_FILE: &str = "audit.jsonl";

/// A hub audit writer that is failing (cas-0140). An audit row that cannot be
/// written refuses the request it was guarding, and nothing said so: the
/// operator only saw requests fail and the log fall silent. The hub records
/// the failure here (and in its log) on the first failed write and removes
/// the file on the next successful one, so `cas hub status` and `cas doctor`
/// can tell a broken writer from a hub that simply had no audited traffic.
/// The file outlives a restart until a row is written again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditHealth {
    pub failing_since: DateTime<Utc>,
    pub last_failure_at: DateTime<Utc>,
    pub failures: u64,
    pub last_action: String,
    pub last_error: String,
}

/// The persisted audit-writer failure under a hub state root, if any.
pub fn read_audit_health(root: &Path) -> Result<Option<AuditHealth>> {
    let path = root.join(AUDIT_HEALTH_FILE);
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes)
                .with_context(|| format!("invalid {}", path.display()))?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// What `cas hub status` and `cas doctor` say about the audit writer
/// (cas-0140). A quiet log is not a failure: a hub with no authenticated
/// traffic writes no rows. Only a recorded write failure is.
#[derive(Debug, Clone, Serialize)]
pub struct AuditWriterReport {
    /// "ok", "failing" or "unknown" (the failure record could not be read).
    pub status: &'static str,
    pub path: String,
    pub last_row_at: Option<DateTime<Utc>>,
    pub failure: Option<AuditHealth>,
    pub message: String,
}

impl AuditWriterReport {
    pub fn is_failure(&self) -> bool {
        self.status == "failing"
    }
}

/// Build the audit-writer report for a hub state root.
pub fn audit_writer_report(root: &Path, now: DateTime<Utc>) -> AuditWriterReport {
    let log = root.join(AUDIT_LOG_FILE);
    let last_row_at = fs::metadata(&log)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(DateTime::<Utc>::from);
    let path = log.display().to_string();
    let quiet = match last_row_at {
        Some(at) => format!(
            "last row {} ago; a hub writes rows only for authenticated requests",
            age_label(now.signed_duration_since(at))
        ),
        None => "no audit rows yet".to_owned(),
    };
    match read_audit_health(root) {
        Ok(None) => AuditWriterReport { status: "ok", path, last_row_at, failure: None, message: quiet },
        Ok(Some(failure)) => AuditWriterReport {
            status: "failing",
            message: format!(
                "writes failing since {} ({} failure{}, last on {}): {}. Audited requests are refused until a row can be written; check that {} is a regular 0600 file you own on a writable disk",
                failure.failing_since.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                failure.failures,
                if failure.failures == 1 { "" } else { "s" },
                failure.last_action,
                failure.last_error,
                path,
            ),
            path,
            last_row_at,
            failure: Some(failure),
        },
        Err(error) => AuditWriterReport {
            status: "unknown",
            message: format!("cannot read the audit failure record: {error:#}; {quiet}"),
            path,
            last_row_at,
            failure: None,
        },
    }
}

fn age_label(age: Duration) -> String {
    let seconds = age.num_seconds().max(0);
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3_599 => format!("{}m", seconds / 60),
        3_600..=172_799 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

#[derive(Debug, Serialize)]
struct AuditRecord<'a> {
    timestamp: DateTime<Utc>,
    machine_id: &'a str,
    request_id: &'a str,
    outcome: &'a str,
    action: &'a str,
    required_scope: Option<&'a str>,
    device_id: Option<&'a str>,
    credential_id: Option<&'a str>,
    device_label: Option<&'a str>,
    operator_label: Option<&'a str>,
    controller_origin: Option<&'a str>,
    target_session: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    supervisor_cli: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    placement: Option<&'a str>,
    /// cas-d636: why an authentication was denied (an AuthRefusal code).
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

struct AuthInner {
    root: PathBuf,
    machine_id: String,
    gate: Mutex<()>,
    lock_file: File,
    revocations: broadcast::Sender<String>,
    /// The writer's failure as last recorded, loaded from AUDIT_HEALTH_FILE at
    /// open so a success after a restart still clears it (cas-0140).
    audit_health: Mutex<Option<AuditHealth>>,
}

struct AuthFileLock<'a>(&'a File);

impl Drop for AuthFileLock<'_> {
    fn drop(&mut self) {
        let _ = FileExt::unlock(self.0);
    }
}

struct LockedState<'a> {
    state: PersistedState,
    _file_lock: AuthFileLock<'a>,
    _gate: MutexGuard<'a, ()>,
}

impl Deref for LockedState<'_> {
    type Target = PersistedState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for LockedState<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

#[derive(Clone)]
pub struct AuthStore(Arc<AuthInner>);

impl AuthStore {
    pub fn open(root: impl AsRef<Path>, machine_id: impl Into<String>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        super::ensure_private_dir(&root)?;
        let lock_path = root.join("auth.lock");
        if lock_path.exists() {
            secure_regular_file(&lock_path)?;
        }
        let mut lock_options = OpenOptions::new();
        lock_options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            lock_options.mode(0o600);
        }
        let lock_file = lock_options.open(&lock_path)?;
        secure_regular_file(&lock_path)?;
        let (revocations, _) = broadcast::channel(64);
        let store = Self(Arc::new(AuthInner {
            root,
            machine_id: machine_id.into(),
            gate: Mutex::new(()),
            lock_file,
            revocations,
            audit_health: Mutex::new(None),
        }));
        // A failure recorded by a previous hub process stays visible until
        // this one writes a row. An unreadable record says nothing true, so
        // it is dropped rather than left to fail every status check.
        match read_audit_health(&store.0.root) {
            Ok(previous) => {
                *store
                    .0
                    .audit_health
                    .lock()
                    .map_err(|_| anyhow::anyhow!("hub audit health poisoned"))? = previous;
            }
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "dropping an unreadable hub audit failure record");
                let _ = fs::remove_file(store.0.root.join(AUDIT_HEALTH_FILE));
            }
        }
        let state_path = store.0.root.join("auth.json");
        let state = store.lock()?;
        if !state_path.exists() {
            store.persist(&state)?;
        }
        drop(state);
        Ok(store)
    }

    pub fn mint_pairing(
        &self,
        controller_origin: &str,
        max_scopes: BTreeSet<Scope>,
        now: DateTime<Utc>,
    ) -> Result<PairingInvitation> {
        validate_origin(controller_origin)?;
        anyhow::ensure!(
            !max_scopes.is_empty(),
            "pairing scope ceiling cannot be empty"
        );
        let token = random_secret();
        let expires_at = now + Duration::minutes(PAIRING_TTL_MINUTES);
        let mut state = self.lock()?;
        state.pairings.push(PairingRecord {
            token_hash: hash_b64(token.as_bytes()),
            hub_id: self.0.machine_id.clone(),
            controller_origin: controller_origin.into(),
            max_scopes: max_scopes.clone(),
            created_at: now,
            expires_at,
            consumed_at: None,
            failed_attempts: 0,
        });
        self.persist(&state)?;
        Ok(PairingInvitation {
            url: pairing_invitation_url(
                PairingInvitationTarget::LocalCommander,
                controller_origin,
                &token,
                &self.0.machine_id,
                &max_scopes,
                &PairingPrefill::default(),
            ),
            token,
            expires_at,
            scopes: max_scopes,
            controller_origin: controller_origin.to_owned(),
            hub_id: self.0.machine_id.clone(),
            prefill: PairingPrefill::default(),
        })
    }

    pub fn exchange_pairing(
        &self,
        exchange: PairingExchange,
        now: DateTime<Utc>,
    ) -> std::result::Result<DeviceCredential, PairingExchangeError> {
        validate_origin(&exchange.controller_origin)?;
        let token_hash = hash_b64(exchange.token.as_bytes());
        let mut state = self.lock()?;
        state
            .source_attempts
            .retain(|attempt| attempt.at > now - Duration::hours(1));
        let recent_source = state
            .source_attempts
            .iter()
            .filter(|attempt| {
                attempt.source == exchange.source && attempt.at > now - Duration::minutes(1)
            })
            .collect::<Vec<_>>();
        if recent_source.len() >= 5 {
            let oldest = recent_source
                .iter()
                .map(|attempt| attempt.at)
                .min()
                .expect("five recent attempts have an oldest timestamp");
            let remaining_millis = (oldest + Duration::minutes(1) - now).num_milliseconds();
            let retry_after_seconds = ((remaining_millis.max(1) + 999) / 1_000).clamp(1, 60) as u64;
            return Err(PairingExchangeError::Throttled {
                retry_after_seconds,
            });
        }
        state.source_attempts.push(SourceAttempt {
            source: exchange.source.clone(),
            at: now,
        });
        let matching = state.pairings.iter().position(|record| {
            constant_time_eq(&record.token_hash, &token_hash)
                && record.hub_id == exchange.hub_id
                && record.controller_origin == exchange.controller_origin
        });
        let Some(index) = matching else {
            self.persist(&state)?;
            return Err(anyhow::anyhow!("pairing exchange refused").into());
        };
        let record = &mut state.pairings[index];
        let valid = record.consumed_at.is_none()
            && record.expires_at >= now
            && record.failed_attempts < 10
            && exchange.requested_scopes.is_subset(&record.max_scopes);
        if !valid {
            record.failed_attempts = record.failed_attempts.saturating_add(1);
            self.persist(&state)?;
            return Err(anyhow::anyhow!("pairing exchange refused").into());
        }
        let thumbprint = exchange.public_key_jwk.thumbprint()?;
        record.consumed_at = Some(now);
        let credential = random_secret();
        let device_id = uuid::Uuid::new_v4().to_string();
        let credential_id = uuid::Uuid::new_v4().to_string();
        let expires_at = now + Duration::days(CREDENTIAL_ABSOLUTE_DAYS);
        state.devices.push(DeviceSession {
            device_id: device_id.clone(),
            credential_id: credential_id.clone(),
            device_label: sanitize_label(&exchange.device_label),
            operator_label: sanitize_label(&exchange.operator_label),
            controller_origin: exchange.controller_origin,
            scopes: exchange.requested_scopes.clone(),
            issued_at: now,
            last_used_at: now,
            expires_at,
            revoked_at: None,
            credential_hash: hash_b64(credential.as_bytes()),
            public_key: exchange.public_key_jwk,
            public_key_thumbprint: thumbprint,
        });
        self.persist(&state)?;
        drop(state);
        self.audit(None, "allowed", "pairing_exchange", None, None, now)?;
        Ok(DeviceCredential {
            device_id,
            credential_id,
            credential,
            expires_at,
            scopes: exchange.requested_scopes,
        })
    }

    pub fn pairing_exchange_matches(
        &self,
        token: &str,
        hub_id: &str,
        controller_origin: &str,
    ) -> Result<bool> {
        let token_hash = hash_b64(token.as_bytes());
        Ok(self.lock()?.pairings.iter().any(|record| {
            constant_time_eq(&record.token_hash, &token_hash)
                && record.hub_id == hub_id
                && record.controller_origin == controller_origin
        }))
    }

    pub fn authenticate_dpop(
        &self,
        authorization: &str,
        proof: &str,
        origin: &str,
        method: &str,
        target_uri: &str,
        now: DateTime<Utc>,
    ) -> Result<AuthContext> {
        validate_origin(origin).map_err(|_| AuthRefusal::OriginMismatch)?;
        let credential = authorization
            .strip_prefix("DPoP ")
            .ok_or(AuthRefusal::MalformedCredential)?;
        let credential_hash = hash_b64(credential.as_bytes());
        let mut state = self.lock()?;
        let device_index = state
            .devices
            .iter()
            .position(|device| constant_time_eq(&device.credential_hash, &credential_hash));
        let Some(device_index) = device_index else {
            return Err(AuthRefusal::UnknownCredential.into());
        };
        let device = state.devices[device_index].clone();
        let context = AuthContext {
            device_id: device.device_id.clone(),
            credential_id: device.credential_id.clone(),
            device_label: device.device_label.clone(),
            operator_label: device.operator_label.clone(),
            controller_origin: device.controller_origin.clone(),
            scopes: device.scopes.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
        };
        // cas-d636: each credential refusal names itself, so the client can
        // tell a revocation (re-pair) from an expiry (refresh).
        let standing = if device.revoked_at.is_some() {
            Some(AuthRefusal::Revoked)
        } else if device.controller_origin != origin {
            Some(AuthRefusal::OriginMismatch)
        } else if device.last_used_at + Duration::days(CREDENTIAL_IDLE_DAYS) < now {
            Some(AuthRefusal::Idle)
        } else if device.expires_at < now {
            Some(AuthRefusal::Expired)
        } else {
            None
        };
        if let Some(refused) = standing {
            drop(state);
            self.audit_refusal(&context, "dpop_auth", refused, now)?;
            return Err(refused.into());
        }
        let verified = match verify_dpop(
            proof,
            credential,
            &device.public_key,
            &device.public_key_thumbprint,
            method,
            target_uri,
            now,
        ) {
            Ok(verified) => verified,
            Err(error) => {
                drop(state);
                let refused = refusal_of(&error);
                self.audit_refusal(&context, "dpop_auth", refused, now)?;
                return Err(refused.into());
            }
        };
        state.dpop_jtis.retain(|entry| entry.expires_at >= now);
        if state
            .dpop_jtis
            .iter()
            .any(|entry| entry.credential_id == device.credential_id && entry.jti == verified.jti)
        {
            drop(state);
            self.audit_refusal(&context, "dpop_replay", AuthRefusal::ProofReplay, now)?;
            return Err(AuthRefusal::ProofReplay.into());
        }
        state.dpop_jtis.push(ReplayRecord {
            credential_id: context.credential_id.clone(),
            jti: verified.jti,
            expires_at: now + Duration::minutes(DPOP_REPLAY_MINUTES),
        });
        state.devices[device_index].last_used_at = now;
        self.persist(&state)?;
        drop(state);
        self.audit(Some(&context), "allowed", "dpop_auth", None, None, now)?;
        Ok(context)
    }

    /// Rotate an otherwise-valid device credential without requiring a new
    /// machine-side pairing. Expiry has a short, bounded recovery grace; a
    /// revoked credential, changed origin, bad proof, or idle credential can
    /// never use this path.
    pub fn refresh_device_credential(
        &self,
        authorization: &str,
        proof: &str,
        origin: &str,
        method: &str,
        target_uri: &str,
        now: DateTime<Utc>,
    ) -> Result<DeviceCredential> {
        validate_origin(origin).map_err(|_| AuthRefusal::OriginMismatch)?;
        let credential = authorization
            .strip_prefix("DPoP ")
            .ok_or(AuthRefusal::MalformedCredential)?;
        let credential_hash = hash_b64(credential.as_bytes());
        let mut state = self.lock()?;
        let device_index = state
            .devices
            .iter()
            .position(|device| constant_time_eq(&device.credential_hash, &credential_hash))
            .ok_or(AuthRefusal::UnknownCredential)?;
        let device = state.devices[device_index].clone();
        // cas-d636: a refresh refusal names itself too. Past the refresh
        // grace, an expired credential is as final as a revoked one.
        if device.revoked_at.is_some() {
            return Err(AuthRefusal::Revoked.into());
        }
        if device.controller_origin != origin {
            return Err(AuthRefusal::OriginMismatch.into());
        }
        if device.last_used_at + Duration::days(CREDENTIAL_IDLE_DAYS) < now {
            return Err(AuthRefusal::Idle.into());
        }
        if device.expires_at + Duration::days(CREDENTIAL_REFRESH_GRACE_DAYS) < now {
            return Err(AuthRefusal::Expired.into());
        }
        let verified = verify_dpop(
            proof,
            credential,
            &device.public_key,
            &device.public_key_thumbprint,
            method,
            target_uri,
            now,
        )?;
        state.dpop_jtis.retain(|entry| entry.expires_at >= now);
        if state
            .dpop_jtis
            .iter()
            .any(|entry| entry.credential_id == device.credential_id && entry.jti == verified.jti)
        {
            return Err(AuthRefusal::ProofReplay.into());
        }
        state.dpop_jtis.push(ReplayRecord {
            credential_id: device.credential_id.clone(),
            jti: verified.jti,
            expires_at: now + Duration::minutes(DPOP_REPLAY_MINUTES),
        });
        let rotated = random_secret();
        let expires_at = now + Duration::days(CREDENTIAL_ABSOLUTE_DAYS);
        state.devices[device_index].credential_hash = hash_b64(rotated.as_bytes());
        state.devices[device_index].last_used_at = now;
        state.devices[device_index].expires_at = expires_at;
        self.persist(&state)?;
        Ok(DeviceCredential {
            device_id: device.device_id,
            credential_id: device.credential_id,
            credential: rotated,
            expires_at,
            scopes: device.scopes,
        })
    }

    pub fn issue_ws_ticket(
        &self,
        context: &AuthContext,
        session: &str,
        endpoint: &str,
        now: DateTime<Utc>,
    ) -> Result<WsTicket> {
        anyhow::ensure!(context.has(Scope::PaneRead), "authorization refused");
        let mut state = self.lock()?;
        Self::ensure_active_context_in_state(&state, context, now)?;
        let ticket = random_secret();
        let expires_at = now + Duration::minutes(WS_TICKET_TTL_MINUTES);
        state.tickets.push(TicketRecord {
            ticket_hash: hash_b64(ticket.as_bytes()),
            context: context.clone(),
            session: session.into(),
            endpoint: endpoint.into(),
            issued_at: now,
            expires_at,
            consumed_at: None,
        });
        self.persist(&state)?;
        Ok(WsTicket { ticket, expires_at })
    }

    pub fn consume_ws_ticket(
        &self,
        ticket: &str,
        origin: &str,
        session: &str,
        endpoint: &str,
        now: DateTime<Utc>,
    ) -> Result<AuthContext> {
        let hash = hash_b64(ticket.as_bytes());
        let mut state = self.lock()?;
        let index = state
            .tickets
            .iter()
            .position(|candidate| constant_time_eq(&candidate.ticket_hash, &hash))
            .context("websocket ticket refused")?;
        let record = &state.tickets[index];
        anyhow::ensure!(
            record.consumed_at.is_none()
                && record.expires_at >= now
                && record.context.controller_origin == origin
                && record.session == session
                && record.endpoint == endpoint,
            "websocket ticket refused"
        );
        let context = record.context.clone();
        Self::ensure_active_context_in_state(&state, &context, now)?;
        state.tickets[index].consumed_at = Some(now);
        self.persist(&state)?;
        Ok(context)
    }

    pub fn list_devices(&self) -> Result<Vec<DeviceSummary>> {
        Ok(self
            .lock()?
            .devices
            .iter()
            .map(|device| DeviceSummary {
                device_id: device.device_id.clone(),
                credential_id: device.credential_id.clone(),
                device_label: device.device_label.clone(),
                operator_label: device.operator_label.clone(),
                controller_origin: device.controller_origin.clone(),
                scopes: device.scopes.clone(),
                issued_at: device.issued_at,
                last_used_at: device.last_used_at,
                expires_at: device.expires_at,
                revoked_at: device.revoked_at,
            })
            .collect())
    }

    /// A device with all three pane controls may add only session launch to
    /// its own credential. Recheck the persisted device under the store lock:
    /// the authenticated context can predate a concurrent revocation.
    pub fn grant_own_session_launch(
        &self,
        context: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<BTreeSet<Scope>> {
        let mut state = self.lock()?;
        Self::ensure_active_context_in_state(&state, context, now)?;
        let device = state
            .devices
            .iter_mut()
            .find(|device| {
                device.device_id == context.device_id
                    && device.credential_id == context.credential_id
            })
            .context("authorization refused")?;
        anyhow::ensure!(
            [Scope::PaneInput, Scope::MessageSend, Scope::PaneInterrupt]
                .into_iter()
                .all(|scope| device.scopes.contains(&scope)),
            "scope denied"
        );
        if device.scopes.contains(&Scope::SessionLaunch) {
            return Ok(device.scopes.clone());
        }
        // The audit must succeed before the capability becomes durable.
        // This is the same append-only audit used by session launch itself.
        let record = AuditRecord {
            timestamp: now,
            machine_id: &self.0.machine_id,
            request_id: &context.request_id,
            outcome: "allowed",
            action: "self_grant_session_launch",
            required_scope: Some(Scope::SessionLaunch.as_str()),
            device_id: Some(&context.device_id),
            credential_id: Some(&context.credential_id),
            device_label: Some(&context.device_label),
            operator_label: Some(&context.operator_label),
            controller_origin: Some(&context.controller_origin),
            target_session: None,
            project: None,
            supervisor_cli: None,
            profile: None,
            placement: None,
            reason: None,
            detail: None,
        };
        let written = append_private_json_line(&self.0.root.join(AUDIT_LOG_FILE), &record);
        self.record_audit_outcome("self_grant_session_launch", now, written.as_ref().err());
        written?;
        device.scopes.insert(Scope::SessionLaunch);
        let scopes = device.scopes.clone();
        self.persist(&state)?;
        Ok(scopes)
    }

    pub fn is_paired_origin(&self, origin: &str, now: DateTime<Utc>) -> Result<bool> {
        Ok(self.lock()?.devices.iter().any(|device| {
            device.controller_origin == origin
                && device.revoked_at.is_none()
                && device.expires_at >= now
                && device.last_used_at + Duration::days(CREDENTIAL_IDLE_DAYS) >= now
        }))
    }

    pub fn revoke_device(&self, device_id: &str, now: DateTime<Utc>) -> Result<()> {
        let mut state = self.lock()?;
        let device = state
            .devices
            .iter_mut()
            .find(|device| device.device_id == device_id)
            .context("device not found")?;
        device.revoked_at = Some(now);
        state.leases.retain(|_, lease| lease.device_id != device_id);
        self.persist(&state)?;
        let _ = self.0.revocations.send(device_id.to_owned());
        drop(state);
        self.audit(
            None,
            "allowed",
            "device_revoke",
            Some(Scope::HubAdmin),
            None,
            now,
        )
    }

    pub fn subscribe_revocations(&self) -> broadcast::Receiver<String> {
        self.0.revocations.subscribe()
    }

    pub fn acquire_lease(
        &self,
        context: &AuthContext,
        session: &str,
        now: DateTime<Utc>,
    ) -> Result<DateTime<Utc>> {
        self.acquire_or_force_lease(context, session, now, false)
    }

    pub fn acquire_or_force_lease(
        &self,
        context: &AuthContext,
        session: &str,
        now: DateTime<Utc>,
        force: bool,
    ) -> Result<DateTime<Utc>> {
        let mut state = self.lock()?;
        Self::ensure_active_context_in_state(&state, context, now)?;
        anyhow::ensure!(
            !force || context.has(Scope::HubAdmin),
            "authorization refused"
        );
        if let Some(existing) = state.leases.get(session) {
            anyhow::ensure!(
                force || existing.expires_at < now || existing.device_id == context.device_id,
                "controller lease held by another device"
            );
        }
        let expires_at = now + Duration::seconds(30);
        state.leases.insert(
            session.into(),
            LeaseRecord {
                device_id: context.device_id.clone(),
                expires_at,
            },
        );
        self.persist(&state)?;
        Ok(expires_at)
    }

    pub fn lease_status(
        &self,
        context: &AuthContext,
        session: &str,
        now: DateTime<Utc>,
    ) -> Result<LeaseSummary> {
        let state = self.lock()?;
        Self::ensure_active_context_in_state(&state, context, now)?;
        let active = state
            .leases
            .get(session)
            .filter(|lease| lease.expires_at >= now);
        let controller = active.and_then(|lease| {
            state
                .devices
                .iter()
                .find(|device| device.device_id == lease.device_id)
                .map(|device| (lease, device))
        });
        Ok(LeaseSummary {
            controller_device_id: controller.map(|(_, device)| device.device_id.clone()),
            controller_label: controller.map(|(_, device)| device.device_label.clone()),
            expires_at: controller.map(|(lease, _)| lease.expires_at),
            held_by_me: controller.is_some_and(|(_, device)| device.device_id == context.device_id),
        })
    }

    pub fn release_lease(
        &self,
        context: &AuthContext,
        session: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let mut state = self.lock()?;
        Self::ensure_active_context_in_state(&state, context, now)?;
        let lease = state
            .leases
            .get(session)
            .context("controller lease unavailable")?;
        anyhow::ensure!(
            lease.device_id == context.device_id,
            "authorization refused"
        );
        state.leases.remove(session);
        self.persist(&state)
    }

    pub fn has_active_lease(
        &self,
        context: &AuthContext,
        session: &str,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let state = self.lock()?;
        Self::ensure_active_context_in_state(&state, context, now)?;
        Ok(state
            .leases
            .get(session)
            .is_some_and(|lease| lease.device_id == context.device_id && lease.expires_at >= now))
    }

    /// Whether this viewer owns the shared PTY geometry for a session.
    ///
    /// With no controller, any pane reader may establish a usable viewport.
    /// Once a controller lease exists, only that device may resize; otherwise
    /// a phone observer could unexpectedly reflow a desktop controller's TUI.
    pub fn may_resize_panes(
        &self,
        context: &AuthContext,
        session: &str,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        if !context.has(Scope::PaneRead) {
            return Ok(false);
        }
        let lease = self.lease_status(context, session, now)?;
        Ok(lease.controller_device_id.is_none() || lease.held_by_me)
    }

    pub fn audit(
        &self,
        context: Option<&AuthContext>,
        outcome: &str,
        action: &str,
        required_scope: Option<Scope>,
        target_session: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.write_audit(
            context, outcome, action, required_scope, target_session,
            None, None, None, None, None, now,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn audit_launch(
        &self,
        context: &AuthContext,
        outcome: &str,
        project: &str,
        supervisor_cli: &str,
        profile: Option<&str>,
        session: Option<&str>,
        now: DateTime<Utc>,
        placement: Option<&str>,
    ) -> Result<()> {
        self.write_audit(
            Some(context), outcome, "session_launch", Some(Scope::SessionLaunch),
            session, Some(project), Some(supervisor_cli), profile, placement, None, now,
        )
    }

    /// One row of a structured fleet operation (cas-566b): `requested`
    /// before it runs, then its outcome. `detail` names the operation's
    /// subject (task, epic) and, on failure, why.
    #[allow(clippy::too_many_arguments)]
    pub fn audit_operation(
        &self,
        context: &AuthContext,
        outcome: &str,
        action: &str,
        required_scope: Scope,
        target_session: &str,
        detail: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.write_audit_record(
            Some(context), outcome, action, Some(required_scope), Some(target_session),
            None, None, None, None, None, detail, now,
        )
    }

    /// A denied authentication, with its reason (cas-d636).
    fn audit_refusal(
        &self,
        context: &AuthContext,
        action: &str,
        refusal: AuthRefusal,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.write_audit(
            Some(context), "denied", action, None, None,
            None, None, None, None, Some(refusal), now,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn write_audit(
        &self,
        context: Option<&AuthContext>,
        outcome: &str,
        action: &str,
        required_scope: Option<Scope>,
        target_session: Option<&str>,
        project: Option<&str>,
        supervisor_cli: Option<&str>,
        profile: Option<&str>,
        placement: Option<&str>,
        refusal: Option<AuthRefusal>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.write_audit_record(
            context, outcome, action, required_scope, target_session, project,
            supervisor_cli, profile, placement, refusal, None, now,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn write_audit_record(
        &self,
        context: Option<&AuthContext>,
        outcome: &str,
        action: &str,
        required_scope: Option<Scope>,
        target_session: Option<&str>,
        project: Option<&str>,
        supervisor_cli: Option<&str>,
        profile: Option<&str>,
        placement: Option<&str>,
        refusal: Option<AuthRefusal>,
        detail: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let record = AuditRecord {
            timestamp: now,
            machine_id: &self.0.machine_id,
            request_id: context.map_or("unattributed", |value| value.request_id.as_str()),
            outcome,
            action,
            required_scope: required_scope.map(Scope::as_str),
            device_id: context.map(|value| value.device_id.as_str()),
            credential_id: context.map(|value| value.credential_id.as_str()),
            device_label: context.map(|value| value.device_label.as_str()),
            operator_label: context.map(|value| value.operator_label.as_str()),
            controller_origin: context.map(|value| value.controller_origin.as_str()),
            target_session,
            project,
            supervisor_cli,
            profile,
            placement,
            reason: refusal.map(AuthRefusal::code),
            detail: detail.or_else(|| refusal.and_then(AuthRefusal::detail)),
        };
        let written = self
            .lock()
            .and_then(|_state_lock| append_private_json_line(&self.0.root.join(AUDIT_LOG_FILE), &record));
        self.record_audit_outcome(action, now, written.as_ref().err());
        written
    }

    /// The audit writer's health as this process knows it (cas-0140).
    pub fn audit_health(&self) -> Option<AuditHealth> {
        self.0
            .audit_health
            .lock()
            .ok()
            .and_then(|health| health.clone())
    }

    /// Record whether an audit row was written. A failure is logged at error
    /// level and persisted to AUDIT_HEALTH_FILE; the first success afterwards
    /// clears both. Only transitions touch the disk, so a healthy writer adds
    /// no I/O per row. The health record never refuses the request itself:
    /// the audit error already does that.
    fn record_audit_outcome(&self, action: &str, now: DateTime<Utc>, error: Option<&anyhow::Error>) {
        let Ok(mut health) = self.0.audit_health.lock() else {
            return;
        };
        let path = self.0.root.join(AUDIT_HEALTH_FILE);
        match error {
            None => {
                if health.take().is_some() {
                    if let Err(remove) = fs::remove_file(&path)
                        && remove.kind() != std::io::ErrorKind::NotFound
                    {
                        tracing::warn!(error = %remove, path = %path.display(), "hub audit writer recovered but its failure record could not be removed");
                    }
                    tracing::info!(action, "hub audit writer recovered; rows are being written again");
                }
            }
            Some(error) => {
                let message = format!("{error:#}");
                let next = match health.take() {
                    Some(previous) => AuditHealth {
                        failing_since: previous.failing_since,
                        last_failure_at: now,
                        failures: previous.failures.saturating_add(1),
                        last_action: action.to_owned(),
                        last_error: message.clone(),
                    },
                    None => AuditHealth {
                        failing_since: now,
                        last_failure_at: now,
                        failures: 1,
                        last_action: action.to_owned(),
                        last_error: message.clone(),
                    },
                };
                tracing::error!(action, failures = next.failures, error = %message, "hub audit row could not be written; the audited request is refused");
                let persisted = serde_json::to_vec_pretty(&next)
                    .map_err(anyhow::Error::from)
                    .and_then(|bytes| {
                        let temporary = self.0.root.join(format!(
                            ".audit-health.{}.{}.tmp",
                            std::process::id(),
                            uuid::Uuid::new_v4()
                        ));
                        write_private_file(&temporary, &bytes, true)?;
                        fs::rename(&temporary, &path)?;
                        Ok(())
                    });
                if let Err(persist) = persisted {
                    tracing::error!(error = %persist, path = %path.display(), "hub audit failure record could not be written");
                }
                *health = Some(next);
            }
        }
    }

    pub fn ensure_active_context(&self, context: &AuthContext, now: DateTime<Utc>) -> Result<()> {
        let state = self.lock()?;
        Self::ensure_active_context_in_state(&state, context, now)
    }

    fn ensure_active_context_in_state(
        state: &PersistedState,
        context: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let device = state
            .devices
            .iter()
            .find(|device| device.device_id == context.device_id)
            .context("authorization refused")?;
        anyhow::ensure!(
            device.credential_id == context.credential_id
                && device.controller_origin == context.controller_origin
                && context.scopes.is_subset(&device.scopes)
                && device.revoked_at.is_none()
                && device.expires_at >= now
                && device.last_used_at + Duration::days(CREDENTIAL_IDLE_DAYS) >= now,
            "authorization refused"
        );
        Ok(())
    }

    fn lock(&self) -> Result<LockedState<'_>> {
        let gate = self
            .0
            .gate
            .lock()
            .map_err(|_| anyhow::anyhow!("hub auth state poisoned"))?;
        self.0.lock_file.lock_exclusive()?;
        let file_lock = AuthFileLock(&self.0.lock_file);
        let target = self.0.root.join("auth.json");
        let state = if target.exists() {
            secure_regular_file(&target)?;
            serde_json::from_slice(&fs::read(&target)?).context("invalid hub auth state")?
        } else {
            PersistedState::default()
        };
        Ok(LockedState {
            state,
            _file_lock: file_lock,
            _gate: gate,
        })
    }

    fn persist(&self, state: &PersistedState) -> Result<()> {
        let target = self.0.root.join("auth.json");
        let temporary = self.0.root.join(format!(
            ".auth.{}.{}.tmp",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        write_private_file(&temporary, &serde_json::to_vec_pretty(state)?, true)?;
        fs::rename(&temporary, &target)?;
        secure_regular_file(&target)
    }
}

#[derive(Deserialize)]
struct DpopHeader {
    alg: String,
    jwk: PublicJwk,
}

#[derive(Deserialize)]
struct DpopClaims {
    htm: String,
    htu: String,
    iat: i64,
    jti: String,
    ath: String,
}

struct VerifiedDpop {
    jti: String,
}

fn verify_dpop(
    proof: &str,
    credential: &str,
    stored_key: &PublicJwk,
    stored_thumbprint: &str,
    method: &str,
    target_uri: &str,
    now: DateTime<Utc>,
) -> Result<VerifiedDpop> {
    let invalid = |_| AuthRefusal::InvalidProof;
    let parts: Vec<&str> = proof.split('.').collect();
    if parts.len() != 3 {
        return Err(AuthRefusal::InvalidProof.into());
    }
    let header: DpopHeader = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).map_err(invalid)?)
        .map_err(|_| AuthRefusal::InvalidProof)?;
    let claims: DpopClaims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).map_err(invalid)?)
        .map_err(|_| AuthRefusal::InvalidProof)?;
    if header.alg != "ES256" {
        return Err(AuthRefusal::InvalidProof.into());
    }
    let thumbprint = header.jwk.thumbprint().map_err(|_| AuthRefusal::InvalidProof)?;
    // A different key cannot become right on retry: the browser lost or
    // replaced the key it paired with.
    if !(constant_time_eq(&thumbprint, stored_thumbprint)
        && constant_time_eq(&thumbprint, &stored_key.thumbprint()?))
    {
        return Err(AuthRefusal::KeyMismatch.into());
    }
    let signature_bytes = URL_SAFE_NO_PAD.decode(parts[2]).map_err(invalid)?;
    let signature = Signature::from_slice(&signature_bytes).map_err(|_| AuthRefusal::InvalidProof)?;
    header
        .jwk
        .validate()
        .map_err(|_| AuthRefusal::InvalidProof)?
        .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
        .map_err(|_| AuthRefusal::InvalidProof)?;
    if !(claims.htm.eq_ignore_ascii_case(method)
        && claims.htu == target_uri
        && constant_time_eq(&claims.ath, &hash_b64(credential.as_bytes()))
        && !claims.jti.is_empty())
    {
        return Err(AuthRefusal::InvalidProof.into());
    }
    // Checked last, so a stale verdict means the proof was otherwise good:
    // most often signed before the device slept and sent after it woke.
    let skew_secs = claims.iat - now.timestamp();
    if skew_secs.abs() > DPOP_SKEW_SECONDS {
        return Err(AuthRefusal::StaleProof { skew_secs }.into());
    }
    Ok(VerifiedDpop { jti: claims.jti })
}

fn validate_origin(origin: &str) -> Result<()> {
    let parsed = url::Url::parse(origin).context("invalid controller origin")?;
    anyhow::ensure!(
        (parsed.scheme() == "https" || parsed.scheme() == "http")
            && parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.path() == "/"
            && parsed.query().is_none()
            && parsed.fragment().is_none(),
        "invalid controller origin"
    );
    Ok(())
}

fn secure_regular_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "hub auth state is not a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            metadata.mode() & 0o777 == 0o600,
            "hub auth state must have mode 0600"
        );
        anyhow::ensure!(
            metadata.uid() == unsafe { libc::geteuid() },
            "hub auth state has the wrong owner"
        );
    }
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8], create_new: bool) -> Result<()> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .truncate(!create_new)
        .create_new(create_new);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn append_private_json_line<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if path.exists() {
        secure_regular_file(path)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_data()?;
    Ok(())
}

fn random_secret() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}

fn hash_b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(bytes))
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    left.as_bytes().ct_eq(right.as_bytes()).into()
}

fn sanitize_label(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(80)
        .collect()
}

#[cfg(test)]
mod credential_redaction_tests {
    use super::*;

    fn jwk() -> PublicJwk {
        PublicJwk {
            kty: "EC".to_string(),
            crv: "P-256".to_string(),
            x: "x".to_string(),
            y: "y".to_string(),
        }
    }

    #[test]
    fn the_pairing_exchange_debug_never_prints_the_pairing_token() {
        let exchange = PairingExchange {
            token: "SECRET-tok-9f3a1c".to_string(),
            hub_id: "hub-1".to_string(),
            controller_origin: "https://hub.example".to_string(),
            public_key_jwk: jwk(),
            device_label: "Laptop".to_string(),
            operator_label: "Daniel".to_string(),
            requested_scopes: Default::default(),
            source: "local".to_string(),
        };
        let rendered = format!("{exchange:?}");
        assert!(!rendered.contains("SECRET-tok-9f3a1c"), "{rendered}");
        assert!(rendered.contains("[redacted]"), "{rendered}");
        assert!(rendered.contains("hub-1"), "{rendered}");
    }

    #[test]
    fn the_pairing_invitation_debug_redacts_the_url_as_well_as_the_token() {
        let invitation = PairingInvitation {
            token: "SECRET-tok-9f3a1c".to_string(),
            url: "https://hub.example/pair#SECRET-tok-9f3a1c".to_string(),
            expires_at: chrono::Utc::now(),
            scopes: Default::default(),
            controller_origin: "https://hub.example".to_string(),
            hub_id: "hub-1".to_string(),
            prefill: PairingPrefill::default(),
        };
        let rendered = format!("{invitation:?}");
        assert!(
            !rendered.contains("SECRET-tok-9f3a1c"),
            "the URL embeds the same token, so redacting only the field would still leak: {rendered}"
        );
        assert!(rendered.contains("hub-1"), "{rendered}");
    }

    #[test]
    fn the_device_credential_debug_never_prints_the_credential() {
        let credential = DeviceCredential {
            device_id: "dev-1".to_string(),
            credential_id: "cred-1".to_string(),
            credential: "SECRET-tok-9f3a1c".to_string(),
            expires_at: chrono::Utc::now(),
            scopes: Default::default(),
        };
        let rendered = format!("{credential:?}");
        assert!(!rendered.contains("SECRET-tok-9f3a1c"), "{rendered}");
        assert!(
            rendered.contains("cred-1"),
            "the credential ID is a handle, not the secret: {rendered}"
        );
    }
}
