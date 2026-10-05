//! Bounded drain of a project's enrolled cloud outbox (cas-9b7d S2).
//!
//! One pass for one project database:
//!
//! 1. Fetch the active epoch policy (`GET /keys/epoch`) and verify its
//!    manifest: issuer `typ`, account, feed generation, epoch, suite, and a
//!    policy version never lower than the last one accepted (§6.2, §6.4).
//!    `upload_state = frozen` holds the lane; local recording continues.
//! 2. Claim up to 100 rows under a lease. Seal unsealed rows to the epoch
//!    public key **outside** any SQLite lock, then compare-and-set the bytes.
//! 3. Append the batch (≤ 100 events, ≤ 1 MiB, 10 s) and settle every row
//!    from its own receipt: `stored`/`duplicate`/`expired` end retry;
//!    `epoch_retired` re-seals the same frozen payload under the new epoch
//!    with the same `event_id`; other row refusals park with a closed reason.
//!    A whole-request failure leaves the bytes for an identical retry.
//!
//! No step here moves a device cursor, marks anything read, or claims a
//! device stored the event: a cloud receipt only means the cloud holds it.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cas_store::{
    OperatorCloudClaim, OperatorCloudSettlement, OperatorSealedBytes, SqlitePromptQueueStore,
};
use chrono::Utc;
use serde_json::{Value, json};

use super::jws::{IssuerKeys, TYP_EPOCH_MANIFEST};
use super::machine::{MachinePrincipal, MachineTransport};
use super::{AppendOutcome, Failure, MachineRelay, Position, SealedEvent};

const BATCH: usize = 100;
const LEASE_SECONDS: i64 = 60;

/// The verified active epoch for sealing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpochPolicy {
    pub feed_generation: String,
    pub epoch: String,
    pub policy_version: String,
    pub public_key: Vec<u8>,
    pub uploads_open: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DrainError {
    #[error("epoch policy refused: {0}")]
    Policy(&'static str),
    #[error("cloud transport: {0}")]
    Transport(Failure),
    #[error("local store: {0}")]
    Store(String),
    #[error("machine grant is no longer valid ({0}); re-enroll this hub")]
    GrantInvalid(String),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DrainReport {
    pub claimed: usize,
    pub sealed: usize,
    pub stored: usize,
    pub resealed: usize,
    pub parked: usize,
    pub retry: usize,
    pub frozen: bool,
}

fn decimal_claim(claims: &serde_json::Map<String, Value>, name: &str) -> Option<String> {
    claims
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| super::wire::decimal(value))
        .map(str::to_owned)
}

fn greater_or_equal(a: &str, b: &str) -> bool {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(a), Ok(b)) => a >= b,
        _ => false,
    }
}

/// `GET /keys/epoch` and the manifest checks of §6.2.
pub fn fetch_epoch_policy(
    transport: &MachineTransport,
    issuer: &IssuerKeys,
    principal: &MachinePrincipal,
) -> Result<EpochPolicy, DrainError> {
    let response = transport
        .exchange_blocking("GET", "/api/operator/keys/epoch", &[])
        .map_err(DrainError::Transport)?;
    let body: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
    if response.status != 200 {
        let code = body["error"].as_str().unwrap_or("unknown_error").to_owned();
        if matches!(
            code.as_str(),
            "grant_revoked" | "grant_expired" | "grant_unknown"
        ) {
            return Err(DrainError::GrantInvalid(code));
        }
        return Err(DrainError::Transport(Failure::Http {
            status: response.status,
            code,
            recovery: None,
        }));
    }
    let manifest = body["manifest"]
        .as_str()
        .ok_or(DrainError::Policy("manifest missing"))?;
    let verified = issuer
        .verify(manifest, TYP_EPOCH_MANIFEST, Utc::now().timestamp())
        .map_err(|_| DrainError::Policy("manifest signature"))?;
    let claims = &verified.claims;
    let feed_generation =
        decimal_claim(claims, "fgen").ok_or(DrainError::Policy("manifest fgen"))?;
    let epoch = decimal_claim(claims, "epoch").ok_or(DrainError::Policy("manifest epoch"))?;
    let policy_version =
        decimal_claim(claims, "policy_version").ok_or(DrainError::Policy("manifest policy"))?;
    if claims.get("acct").and_then(Value::as_str) != Some(principal.account_id.as_str()) {
        return Err(DrainError::Policy("manifest account"));
    }
    if body["active_epoch"].as_str() != Some(epoch.as_str())
        || body["feed_generation"].as_str() != Some(feed_generation.as_str())
        || claims.get("status").and_then(Value::as_str) != Some("active")
    {
        return Err(DrainError::Policy("manifest is not the active epoch"));
    }
    let suite = claims.get("suite").cloned().unwrap_or(Value::Null);
    if suite != json!({"kem": 16, "kdf": 1, "aead": 2}) {
        return Err(DrainError::Policy("manifest suite"));
    }
    if let Some(previous) = &principal.policy_version
        && !greater_or_equal(&policy_version, previous)
    {
        return Err(DrainError::Policy("policy version went backwards"));
    }
    let public_key = claims
        .get("pk")
        .and_then(Value::as_str)
        .and_then(|pk| URL_SAFE_NO_PAD.decode(pk).ok())
        .filter(|pk| pk.len() == 65)
        .ok_or(DrainError::Policy("manifest pk"))?;
    let upload_state = claims
        .get("upload_state")
        .and_then(Value::as_str)
        .or_else(|| body["upload_state"].as_str());
    Ok(EpochPolicy {
        feed_generation,
        epoch,
        policy_version,
        public_key,
        uploads_open: upload_state == Some("open"),
    })
}

/// The encrypted plaintext of one session event (DESIGN D3). Readable names
/// travel only here, never in routing IDs.
pub fn event_plaintext(claim: &OperatorCloudClaim) -> Result<Vec<u8>, DrainError> {
    let snapshot: Value = serde_json::from_str(&claim.payload_snapshot)
        .map_err(|_| DrainError::Store("frozen snapshot is not JSON".into()))?;
    serde_json::to_vec(&json!({
        "type": "cas.operator.turn",
        "v": 1,
        "event_id": claim.event_id,
        "session_name": claim.factory_session,
        "snapshot": snapshot,
    }))
    .map_err(|_| DrainError::Store("plaintext encoding".into()))
}

fn seal_claim(
    claim: &OperatorCloudClaim,
    policy: &EpochPolicy,
) -> Result<OperatorSealedBytes, DrainError> {
    let plaintext = event_plaintext(claim)?;
    let sealed = cas_operator_crypto::seal_event(
        &policy.public_key,
        &plaintext,
        &cas_operator_crypto::EventIds {
            account_id: &claim.account_id,
            feed_generation: &policy.feed_generation,
            key_epoch: &policy.epoch,
            event_id: &claim.event_id,
            hub_id: &claim.hub_id,
            project_id: &claim.project_id,
            session_id: &claim.session_id,
        },
    )
    .map_err(|error| DrainError::Store(format!("seal: {error}")))?;
    Ok(OperatorSealedBytes {
        feed_generation: policy.feed_generation.clone(),
        key_epoch: policy.epoch.clone(),
        ciphertext: URL_SAFE_NO_PAD.encode(&sealed.bytes),
        digest: sealed.digest,
    })
}

fn park_reason(code: &str) -> &'static str {
    match code {
        "event_conflict" => "event_conflict",
        "epoch_unknown" => "epoch_unknown",
        "hub_mismatch" => "hub_mismatch",
        "project_not_granted" => "project_not_granted",
        "attachment_unknown" => "attachment_unknown",
        "event_too_large" => "event_too_large",
        "invalid_event" => "invalid_event",
        "digest_mismatch" => "digest_mismatch",
        "scope_not_allowed" => "scope_not_allowed",
        _ => "rejected_unknown",
    }
}

fn store_error(error: cas_store::StoreError) -> DrainError {
    DrainError::Store(error.to_string())
}

/// One bounded pass. Callers schedule it (the hub's drain loop) and back off
/// on `Err`; per-row outcomes are already persisted when it returns.
pub async fn drain_project(
    store: &SqlitePromptQueueStore,
    relay: &MachineRelay<MachineTransport>,
    policy: &EpochPolicy,
    principal: &MachinePrincipal,
) -> Result<DrainReport, DrainError> {
    let mut report = DrainReport::default();
    if !policy.uploads_open {
        report.frozen = true;
        return Ok(report);
    }
    let claims = store
        .claim_operator_cloud(&principal.machine_id, Utc::now(), BATCH, LEASE_SECONDS)
        .map_err(store_error)?;
    report.claimed = claims.len();
    if claims.is_empty() {
        return Ok(report);
    }
    let mut batch: Vec<(OperatorCloudClaim, SealedEvent)> = Vec::new();
    for mut claim in claims {
        if claim.account_id != principal.account_id || claim.hub_id != principal.hub_id {
            // Pending rows keep their audience; a different machine identity
            // must not upload them.
            store
                .settle_operator_cloud(
                    &claim,
                    &OperatorCloudSettlement::Park {
                        reason: "audience_changed",
                    },
                    Utc::now(),
                )
                .map_err(store_error)?;
            report.parked += 1;
            continue;
        }
        let sealed = match claim.sealed.clone() {
            Some(sealed) if sealed.feed_generation == policy.feed_generation => sealed,
            Some(_) => {
                // Sealed under an older feed generation: the reset tombstoned
                // that history; this event can never be appended there.
                store
                    .settle_operator_cloud(
                        &claim,
                        &OperatorCloudSettlement::Park {
                            reason: "feed_generation_changed",
                        },
                        Utc::now(),
                    )
                    .map_err(store_error)?;
                report.parked += 1;
                continue;
            }
            None => {
                let sealed = seal_claim(&claim, policy)?;
                if !store
                    .seal_operator_cloud(&claim, &sealed, Utc::now())
                    .map_err(store_error)?
                {
                    continue; // lease lost to another drain
                }
                report.sealed += 1;
                claim.sealed = Some(sealed.clone());
                sealed
            }
        };
        let epoch = sealed
            .key_epoch
            .parse::<u64>()
            .ok()
            .and_then(|value| Position::new(value).ok())
            .ok_or(DrainError::Store("stored key epoch".into()))?;
        let event = SealedEvent {
            event_id: claim.event_id.clone(),
            hub_id: claim.hub_id.clone(),
            project_id: claim.project_id.clone(),
            session_id: claim.session_id.clone(),
            key_epoch: epoch,
            ciphertext: sealed.ciphertext.clone(),
            digest: sealed.digest.clone(),
            attachment_ids: Vec::new(),
        };
        batch.push((claim, event));
    }
    if batch.is_empty() {
        return Ok(report);
    }
    let generation = policy
        .feed_generation
        .parse::<u64>()
        .ok()
        .and_then(|value| Position::new(value).ok())
        .ok_or(DrainError::Policy("feed generation"))?;
    let events: Vec<SealedEvent> = batch.iter().map(|(_, event)| event.clone()).collect();
    let receipt = match relay.append(generation, &events).await {
        Ok(receipt) => receipt,
        Err(failure) => {
            for (claim, _) in &batch {
                store
                    .settle_operator_cloud(claim, &OperatorCloudSettlement::Retry, Utc::now())
                    .map_err(store_error)?;
                report.retry += 1;
            }
            if let Failure::Http { code, .. } = &failure
                && matches!(
                    code.as_str(),
                    "grant_revoked" | "grant_expired" | "grant_unknown"
                )
            {
                return Err(DrainError::GrantInvalid(code.clone()));
            }
            if matches!(&failure, Failure::Http { code, .. } if code == "uploads_frozen") {
                report.frozen = true;
                return Ok(report);
            }
            return Err(DrainError::Transport(failure));
        }
    };
    for ((claim, _), row) in batch.iter().zip(receipt.rows.iter()) {
        let settlement = match &row.outcome {
            AppendOutcome::Stored { sequence, .. } => OperatorCloudSettlement::Receipt {
                kind: "stored",
                sequence: sequence.value().to_string(),
            },
            AppendOutcome::Duplicate { sequence, .. } => OperatorCloudSettlement::Receipt {
                kind: "duplicate",
                sequence: sequence.value().to_string(),
            },
            AppendOutcome::Expired { sequence, .. } => OperatorCloudSettlement::Receipt {
                kind: "expired",
                sequence: sequence.value().to_string(),
            },
            AppendOutcome::Rejected { error, .. } if error == "epoch_retired" => {
                OperatorCloudSettlement::Reseal
            }
            AppendOutcome::Rejected { error, .. }
                if matches!(
                    error.as_str(),
                    "grant_revoked" | "grant_expired" | "grant_generation_stale"
                ) =>
            {
                OperatorCloudSettlement::Retry
            }
            AppendOutcome::Rejected { error, .. } => OperatorCloudSettlement::Park {
                reason: park_reason(error),
            },
        };
        match &settlement {
            OperatorCloudSettlement::Receipt { .. } => report.stored += 1,
            OperatorCloudSettlement::Reseal => report.resealed += 1,
            OperatorCloudSettlement::Park { .. } => report.parked += 1,
            OperatorCloudSettlement::Retry => report.retry += 1,
        }
        store
            .settle_operator_cloud(claim, &settlement, Utc::now())
            .map_err(store_error)?;
    }
    Ok(report)
}

#[cfg(test)]
#[path = "drain_tests.rs"]
mod tests;

/// How often the hub drains every bound project. Upload is independent of
/// viewer presence: it runs whether or not any Commander is connected.
pub const DRAIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

/// The hub's background drain. Each tick: load the machine principal (none →
/// idle), fetch and verify the epoch policy once, then drain every known
/// project whose database is bound to this machine. Errors are logged with
/// closed reasons only and retried next tick; a revoked grant stops the loop
/// until the hub restarts (re-enrollment writes a new principal).
pub fn spawn_drain_loop(hub_state_dir: std::path::PathBuf) -> tokio::task::JoinHandle<()> {
    use super::machine::{HttpClient, PrincipalStore, UreqHttp, issuer_keys};
    use cas_store::PromptQueueStore as _;
    use std::sync::Arc;
    tokio::spawn(async move {
        let store = PrincipalStore::new(&hub_state_dir);
        let http: Arc<dyn HttpClient> = Arc::new(UreqHttp::default());
        let mut issuer: Option<(String, Arc<IssuerKeys>)> = None;
        let mut interval = tokio::time::interval(DRAIN_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let loaded = {
                let store = store.clone();
                tokio::task::spawn_blocking(move || store.load()).await
            };
            let Ok(Ok(Some(mut principal))) = loaded else {
                continue;
            };
            if issuer
                .as_ref()
                .is_none_or(|(origin, _)| origin != &principal.cloud_origin)
            {
                issuer = Some((
                    principal.cloud_origin.clone(),
                    Arc::new(issuer_keys(Arc::clone(&http), &principal.cloud_origin)),
                ));
            }
            let Some(keys) = issuer.as_ref().map(|(_, keys)| Arc::clone(keys)) else {
                continue;
            };
            let Ok(transport) =
                MachineTransport::new(Arc::clone(&http), principal.clone(), Some(store.clone()))
            else {
                tracing::warn!("operator inbox: machine principal keys are unreadable");
                continue;
            };
            let fetched = {
                let (transport, principal) = (transport.clone(), principal.clone());
                tokio::task::spawn_blocking(move || {
                    fetch_epoch_policy(&transport, &keys, &principal)
                })
                .await
            };
            let Ok(fetched) = fetched else {
                continue;
            };
            let policy = match fetched {
                Ok(policy) => policy,
                Err(DrainError::GrantInvalid(code)) => {
                    tracing::warn!(%code, "operator inbox: machine grant refused; re-enroll this hub");
                    return;
                }
                Err(error) => {
                    tracing::debug!(%error, "operator inbox: epoch policy unavailable");
                    continue;
                }
            };
            if principal.policy_version.as_deref() != Some(policy.policy_version.as_str()) {
                principal.policy_version = Some(policy.policy_version.clone());
                let save = store.clone();
                let updated = principal.clone();
                let _ = tokio::task::spawn_blocking(move || save.save(&updated)).await;
            }
            let roots = tokio::task::spawn_blocking(|| {
                crate::hub::projects::list_projects(&[])
                    .map(|projects| {
                        projects
                            .into_iter()
                            .map(|project| project.path.join(".cas"))
                            .filter(|root| root.is_dir())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            let relay = MachineRelay::new(transport.clone());
            for root in roots {
                let Ok(queue) = SqlitePromptQueueStore::open(&root) else {
                    continue;
                };
                if queue.init().is_err() {
                    continue;
                }
                match queue.operator_feed_binding() {
                    Ok(Some(binding)) if binding.machine_id == principal.machine_id => {}
                    _ => {
                        // Rows recorded under a binding that was later detached
                        // still drain to their original audience.
                        if queue
                            .operator_cloud_backlog()
                            .map(|backlog| backlog.pending == 0)
                            .unwrap_or(true)
                        {
                            continue;
                        }
                    }
                }
                match drain_project(&queue, &relay, &policy, &principal).await {
                    Ok(report) if report.stored + report.parked + report.resealed > 0 => {
                        tracing::info!(
                            stored = report.stored,
                            parked = report.parked,
                            resealed = report.resealed,
                            "operator inbox: drained"
                        );
                    }
                    Ok(_) => {}
                    Err(DrainError::GrantInvalid(code)) => {
                        tracing::warn!(%code, "operator inbox: machine grant refused; re-enroll this hub");
                        return;
                    }
                    Err(error) => tracing::debug!(%error, "operator inbox: drain will retry"),
                }
            }
        }
    })
}
