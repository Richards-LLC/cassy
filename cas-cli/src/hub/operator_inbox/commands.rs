//! Offline-machine commands, machine side (cloud contract §10.2–§10.4).
//!
//! A device queued an `operator_message` while this hub was off. When the hub
//! is back it lists its commands, then for each one:
//!
//! 1. reserves it (the fence: once reserved, no cancel or expiry can race the
//!    admission) and receives the ciphertext plus a signed admission;
//! 2. verifies the admission JWS (`psc-op-command-admission+jwt`) and that its
//!    command, account, machine, routing, operation and digest all match, and
//!    that the digest is the SHA-256 of the ciphertext it was handed;
//! 3. decrypts it with the command key and checks the session routing ID is
//!    the hash of the session name inside it;
//! 4. admits it in **one** project transaction (queue row, admission row and
//!    receipt ID), so a crash anywhere replays to the same single admission;
//! 5. posts the receipt. Only `accepted` makes the device say "Accepted by
//!    machine"; the receipt never claims the supervisor acted on it.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cas_store::{AdmissionOutcome, OperatorCommandAdmission, SqlitePromptQueueStore};
use chrono::Utc;
use serde_json::{Value, json};

use super::Failure;
use super::drain::DrainError;
use super::jws::{IssuerKeys, TYP_COMMAND_ADMISSION};
use super::machine::{MachinePrincipal, MachineTransport};

pub const OPERATION: &str = "operator_message";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CommandReport {
    pub listed: usize,
    pub admitted: usize,
    pub replayed: usize,
    pub rejected: usize,
    pub skipped: usize,
}

/// Why a reserved command is refused by this machine (receipt `reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    Admission,
    Digest,
    Decrypt,
    Session,
    Project,
    Payload,
}

impl Refusal {
    fn reason(self) -> &'static str {
        match self {
            Self::Admission => "admission_invalid",
            Self::Digest => "digest_mismatch",
            Self::Decrypt => "undecryptable",
            Self::Session => "session_mismatch",
            Self::Project => "project_not_bound",
            Self::Payload => "payload_invalid",
        }
    }
}

fn call(
    transport: &MachineTransport,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<(u16, Value), DrainError> {
    let bytes = body
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|_| DrainError::Store("request encoding".into()))?
        .unwrap_or_default();
    let response = transport
        .exchange_blocking(method, path, &bytes)
        .map_err(DrainError::Transport)?;
    let value: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
    if response.status == 401 {
        let code = value["error"]
            .as_str()
            .unwrap_or("unknown_error")
            .to_owned();
        if matches!(
            code.as_str(),
            "grant_revoked" | "grant_expired" | "grant_unknown"
        ) {
            return Err(DrainError::GrantInvalid(code));
        }
    }
    Ok((response.status, value))
}

fn safe_id(value: &str) -> bool {
    (22..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

struct Reserved {
    command_id: String,
    project_id: String,
    session_id: String,
    ciphertext: Vec<u8>,
    digest: String,
    device_id: String,
    history_event_id: String,
}

fn verify_reservation(
    issuer: &IssuerKeys,
    principal: &MachinePrincipal,
    listed: &Value,
    reply: &Value,
) -> Result<Reserved, Refusal> {
    let command_id = listed["command_id"].as_str().ok_or(Refusal::Payload)?;
    let admission = reply["admission_authorization"]
        .as_str()
        .ok_or(Refusal::Admission)?;
    let verified = issuer
        .verify(admission, TYP_COMMAND_ADMISSION, Utc::now().timestamp())
        .map_err(|_| Refusal::Admission)?;
    let claim = |name: &str| verified.claims.get(name).and_then(Value::as_str);
    let project_id = listed["project_id"].as_str().ok_or(Refusal::Payload)?;
    let session_id = listed["session_id"].as_str().ok_or(Refusal::Payload)?;
    let bound = [
        ("cmd", command_id),
        ("acct", principal.account_id.as_str()),
        ("mch", principal.machine_id.as_str()),
        ("hub", principal.hub_id.as_str()),
        ("proj", project_id),
        ("sess", session_id),
        ("op", OPERATION),
    ];
    if bound
        .iter()
        .any(|(name, expected)| claim(name) != Some(expected))
    {
        return Err(Refusal::Admission);
    }
    let digest = reply["machine_digest"].as_str().ok_or(Refusal::Digest)?;
    let ciphertext = reply["machine_ciphertext"]
        .as_str()
        .and_then(|text| URL_SAFE_NO_PAD.decode(text).ok())
        .ok_or(Refusal::Digest)?;
    if claim("digest") != Some(digest) || cas_operator_crypto::digest(&ciphertext) != digest {
        return Err(Refusal::Digest);
    }
    let device_id = claim("dev").ok_or(Refusal::Admission)?.to_owned();
    let history_event_id = claim("hist").ok_or(Refusal::Admission)?.to_owned();
    Ok(Reserved {
        command_id: command_id.to_owned(),
        project_id: project_id.to_owned(),
        session_id: session_id.to_owned(),
        ciphertext,
        digest: digest.to_owned(),
        device_id,
        history_event_id,
    })
}

/// Decrypt and check the command plaintext; returns `(session_name, body)`.
fn open_command(
    principal: &MachinePrincipal,
    reserved: &Reserved,
) -> Result<(String, String), Refusal> {
    let secret = principal.command_secret().ok_or(Refusal::Decrypt)?;
    let plain = cas_operator_crypto::open_command(
        &secret,
        &reserved.ciphertext,
        &cas_operator_crypto::CommandIds {
            account_id: &principal.account_id,
            machine_id: &principal.machine_id,
            command_id: &reserved.command_id,
            hub_id: &principal.hub_id,
            project_id: &reserved.project_id,
            session_id: &reserved.session_id,
            operation: OPERATION,
            machine_key_id: &principal.command_key_id,
        },
    )
    .map_err(|_| Refusal::Decrypt)?;
    let value: Value = serde_json::from_slice(&plain).map_err(|_| Refusal::Payload)?;
    if value["type"] != "cas.operator.command" || value["v"] != 1 || value["operation"] != OPERATION
    {
        return Err(Refusal::Payload);
    }
    let session_name = value["session_name"].as_str().ok_or(Refusal::Payload)?;
    let body = value["body"].as_str().ok_or(Refusal::Payload)?;
    if body.trim().is_empty() || body.len() > 32 * 1024 {
        return Err(Refusal::Payload);
    }
    if cas_store::session_routing_id(session_name) != reserved.session_id {
        return Err(Refusal::Session);
    }
    Ok((session_name.to_owned(), body.to_owned()))
}

/// A rejection is decided again identically after a crash, so its receipt
/// ID is derived from the command ID: a resend is idempotent (§10.4).
fn rejection_receipt_id(command_id: &str) -> String {
    use sha2::Digest;
    format!(
        "rej-{}",
        URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(command_id.as_bytes()))
    )
}

fn send_receipt(
    transport: &MachineTransport,
    command_id: &str,
    receipt_id: &str,
    digest: &str,
    refusal: Option<Refusal>,
) -> Result<bool, DrainError> {
    let mut body = json!({
        "wire_version": 1,
        "receipt_id": receipt_id,
        "outcome": if refusal.is_some() { "rejected" } else { "accepted" },
        "machine_digest": digest,
    });
    if let Some(refusal) = refusal {
        body["reason"] = json!(refusal.reason());
    }
    let (status, value) = call(
        transport,
        "POST",
        &format!("/api/operator/machine/commands/{command_id}/receipt"),
        Some(&body),
    )?;
    // `command_terminal` with our receipt means an earlier attempt landed.
    Ok(status == 200
        || (status == 409
            && value["error"] == "command_terminal"
            && value["receipt"]["receipt_id"] == receipt_id))
}

/// One pass over this machine's pending and reserved commands.
/// `queue_for(project_id)` opens the bound project database, if any.
pub fn process_commands(
    transport: &MachineTransport,
    issuer: &IssuerKeys,
    principal: &MachinePrincipal,
    queue_for: &dyn Fn(&str) -> Option<SqlitePromptQueueStore>,
) -> Result<CommandReport, DrainError> {
    let mut report = CommandReport::default();
    let (status, listing) = call(
        transport,
        "GET",
        "/api/operator/machine/commands?limit=50",
        None,
    )?;
    if status != 200 {
        return Err(DrainError::Transport(Failure::Http {
            status,
            code: listing["error"]
                .as_str()
                .unwrap_or("unknown_error")
                .to_owned(),
            recovery: None,
        }));
    }
    let commands = listing["commands"].as_array().cloned().unwrap_or_default();
    report.listed = commands.len();
    for listed in commands {
        let Some(command_id) = listed["command_id"].as_str().filter(|id| safe_id(id)) else {
            report.skipped += 1;
            continue;
        };
        if listed["hub_id"].as_str() != Some(principal.hub_id.as_str())
            || listed["operation"].as_str() != Some(OPERATION)
        {
            report.skipped += 1;
            continue;
        }
        let (status, reply) = call(
            transport,
            "POST",
            &format!("/api/operator/machine/commands/{command_id}/reserve"),
            Some(&json!({"wire_version": 1})),
        )?;
        if status != 200 {
            // Expired, cancelled, revoked submitter or already terminal:
            // nothing to admit (§10.3). Never retried blindly.
            report.skipped += 1;
            continue;
        }
        let digest_for_receipt = reply["machine_digest"].as_str().unwrap_or("").to_owned();
        let verified = verify_reservation(issuer, principal, &listed, &reply);
        let reserved = match verified {
            Ok(reserved) => reserved,
            Err(refusal) => {
                let receipt = rejection_receipt_id(command_id);
                if send_receipt(
                    transport,
                    command_id,
                    &receipt,
                    &digest_for_receipt,
                    Some(refusal),
                )? {
                    report.rejected += 1;
                }
                continue;
            }
        };
        let Some(queue) = queue_for(&reserved.project_id) else {
            let receipt = rejection_receipt_id(command_id);
            if send_receipt(
                transport,
                command_id,
                &receipt,
                &reserved.digest,
                Some(Refusal::Project),
            )? {
                report.rejected += 1;
            }
            continue;
        };
        let (session_name, body) = match open_command(principal, &reserved) {
            Ok(opened) => opened,
            Err(refusal) => {
                let receipt = rejection_receipt_id(command_id);
                if send_receipt(
                    transport,
                    command_id,
                    &receipt,
                    &reserved.digest,
                    Some(refusal),
                )? {
                    report.rejected += 1;
                }
                continue;
            }
        };
        let outcome = queue
            .admit_operator_command(&OperatorCommandAdmission {
                command_id: &reserved.command_id,
                machine_digest: &reserved.digest,
                history_event_id: &reserved.history_event_id,
                device_id: &reserved.device_id,
                device_label: "Commander",
                factory_session: &session_name,
                body: &body,
            })
            .map_err(|error| DrainError::Store(error.to_string()))?;
        let (receipt_id, sent_already) = match outcome {
            AdmissionOutcome::Admitted { receipt_id, .. } => {
                report.admitted += 1;
                (receipt_id, false)
            }
            AdmissionOutcome::Existing {
                receipt_id,
                receipt_sent,
                ..
            } => {
                report.replayed += 1;
                (receipt_id, receipt_sent)
            }
            AdmissionOutcome::Conflict => {
                report.skipped += 1;
                continue;
            }
        };
        if !sent_already
            && send_receipt(transport, command_id, &receipt_id, &reserved.digest, None)?
        {
            queue
                .mark_command_receipt_sent(command_id, Utc::now())
                .map_err(|error| DrainError::Store(error.to_string()))?;
        }
    }
    Ok(report)
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
