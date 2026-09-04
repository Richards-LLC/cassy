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

const PAIRING_TTL_MINUTES: i64 = 10;
const WS_TICKET_TTL_MINUTES: i64 = 5;
const CREDENTIAL_ABSOLUTE_DAYS: i64 = 90;
const CREDENTIAL_IDLE_DAYS: i64 = 30;
const CREDENTIAL_REFRESH_GRACE_DAYS: i64 = 7;
const DPOP_SKEW_SECONDS: i64 = 60;
const DPOP_REPLAY_MINUTES: i64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    MachineRead,
    SessionRead,
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
        | ClientMessage::ScrollbackRequest { .. } => Some(Scope::PaneRead),
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
        | ClientMessage::Ping => None,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize)]
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
        )
    }
}

fn pairing_invitation_url(
    target: PairingInvitationTarget,
    controller_origin: &str,
    token: &str,
    hub_id: &str,
    scopes: &BTreeSet<Scope>,
) -> String {
    let mut url = format!("{controller_origin}/#pair={token}&hub={hub_id}");
    if target == PairingInvitationTarget::LocalCommander {
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

#[derive(Debug, Clone, Serialize)]
pub struct DeviceCredential {
    pub device_id: String,
    pub credential_id: String,
    pub credential: String,
    pub expires_at: DateTime<Utc>,
    pub scopes: BTreeSet<Scope>,
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
}

struct AuthInner {
    root: PathBuf,
    machine_id: String,
    gate: Mutex<()>,
    lock_file: File,
    revocations: broadcast::Sender<String>,
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
        }));
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
            ),
            token,
            expires_at,
            scopes: max_scopes,
            controller_origin: controller_origin.to_owned(),
            hub_id: self.0.machine_id.clone(),
        })
    }

    pub fn exchange_pairing(
        &self,
        exchange: PairingExchange,
        now: DateTime<Utc>,
    ) -> Result<DeviceCredential> {
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
            .count();
        anyhow::ensure!(recent_source < 5, "pairing exchange refused");
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
            anyhow::bail!("pairing exchange refused")
        };
        let record = &mut state.pairings[index];
        let valid = record.consumed_at.is_none()
            && record.expires_at >= now
            && record.failed_attempts < 10
            && exchange.requested_scopes.is_subset(&record.max_scopes);
        if !valid {
            record.failed_attempts = record.failed_attempts.saturating_add(1);
            self.persist(&state)?;
            anyhow::bail!("pairing exchange refused")
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
        validate_origin(origin)?;
        let credential = authorization
            .strip_prefix("DPoP ")
            .context("authentication refused")?;
        let credential_hash = hash_b64(credential.as_bytes());
        let mut state = self.lock()?;
        let device_index = state
            .devices
            .iter()
            .position(|device| constant_time_eq(&device.credential_hash, &credential_hash));
        let Some(device_index) = device_index else {
            anyhow::bail!("authentication refused")
        };
        let device = state.devices[device_index].clone();
        anyhow::ensure!(
            device.revoked_at.is_none()
                && device.expires_at >= now
                && device.last_used_at + Duration::days(CREDENTIAL_IDLE_DAYS) >= now
                && device.controller_origin == origin,
            "authentication refused"
        );
        let context = AuthContext {
            device_id: device.device_id.clone(),
            credential_id: device.credential_id.clone(),
            device_label: device.device_label.clone(),
            operator_label: device.operator_label.clone(),
            controller_origin: device.controller_origin.clone(),
            scopes: device.scopes.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
        };
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
                self.audit(Some(&context), "denied", "dpop_auth", None, None, now)?;
                return Err(error);
            }
        };
        state.dpop_jtis.retain(|entry| entry.expires_at >= now);
        if state
            .dpop_jtis
            .iter()
            .any(|entry| entry.credential_id == device.credential_id && entry.jti == verified.jti)
        {
            drop(state);
            self.audit(Some(&context), "denied", "dpop_replay", None, None, now)?;
            anyhow::bail!("authentication refused")
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
        validate_origin(origin)?;
        let credential = authorization
            .strip_prefix("DPoP ")
            .context("authentication refused")?;
        let credential_hash = hash_b64(credential.as_bytes());
        let mut state = self.lock()?;
        let device_index = state
            .devices
            .iter()
            .position(|device| constant_time_eq(&device.credential_hash, &credential_hash))
            .context("authentication refused")?;
        let device = state.devices[device_index].clone();
        anyhow::ensure!(
            device.revoked_at.is_none()
                && device.controller_origin == origin
                && device.last_used_at + Duration::days(CREDENTIAL_IDLE_DAYS) >= now
                && device.expires_at + Duration::days(CREDENTIAL_REFRESH_GRACE_DAYS) >= now,
            "authentication refused"
        );
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
        anyhow::ensure!(
            !state.dpop_jtis.iter().any(|entry| {
                entry.credential_id == device.credential_id && entry.jti == verified.jti
            }),
            "authentication refused"
        );
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
        };
        let _state_lock = self.lock()?;
        append_private_json_line(&self.0.root.join("audit.jsonl"), &record)
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
    let parts: Vec<&str> = proof.split('.').collect();
    anyhow::ensure!(parts.len() == 3, "authentication refused");
    let header: DpopHeader = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0])?)?;
    let claims: DpopClaims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1])?)?;
    anyhow::ensure!(header.alg == "ES256", "authentication refused");
    let thumbprint = header.jwk.thumbprint()?;
    anyhow::ensure!(
        constant_time_eq(&thumbprint, stored_thumbprint)
            && constant_time_eq(&thumbprint, &stored_key.thumbprint()?),
        "authentication refused"
    );
    let signature_bytes = URL_SAFE_NO_PAD.decode(parts[2])?;
    let signature = Signature::from_slice(&signature_bytes).context("authentication refused")?;
    header
        .jwk
        .validate()?
        .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
        .context("authentication refused")?;
    anyhow::ensure!(
        claims.htm.eq_ignore_ascii_case(method)
            && claims.htu == target_uri
            && constant_time_eq(&claims.ath, &hash_b64(credential.as_bytes()))
            && (claims.iat - now.timestamp()).abs() <= DPOP_SKEW_SECONDS
            && !claims.jti.is_empty(),
        "authentication refused"
    );
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
