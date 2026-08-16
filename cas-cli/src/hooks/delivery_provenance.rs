//! One-shot provenance envelopes for PTY-delivered factory turns.
//!
//! Harness hook JSON carries only the submitted prompt. The factory delivery
//! authority persists a short-lived envelope *before* it injects the exact
//! payload. Hook dispatch atomically consumes that envelope, keeping sender
//! authority out of rendered prompt text.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use cas_core::hooks::types::{MachinePromptOrigin, MachinePromptProvenance};

const ENVELOPE_DIR: &str = "hook-delivery-provenance";
const ENVELOPE_TTL_SECONDS: i64 = 30;

#[derive(Debug, Serialize, Deserialize)]
struct PendingEnvelope {
    nonce: Uuid,
    recipient: String,
    payload: String,
    provenance: MachinePromptProvenance,
    expires_at: DateTime<Utc>,
}

fn envelope_dir(cas_root: &Path) -> PathBuf {
    cas_root.join(ENVELOPE_DIR)
}

/// Classify a delivery source without looking at the delivered display text.
pub(crate) fn origin_for_source(source: &str) -> MachinePromptOrigin {
    if source.eq_ignore_ascii_case("director") {
        MachinePromptOrigin::DirectorGenerated
    } else if source.starts_with("lifecycle-wake:") {
        MachinePromptOrigin::LifecycleRelay
    } else {
        MachinePromptOrigin::AgentAuthored
    }
}

/// Register one exact machine payload for its recipient at the sole PTY
/// delivery boundary.
///
/// A random nonce makes identical payloads independently consumable. A crashed
/// turn leaves at most a short-lived envelope, then expiry prevents a later
/// operator prompt with the same text from being classified as a relay.
pub(crate) fn register(
    cas_root: &Path,
    recipient: &str,
    payload: &str,
    origin: MachinePromptOrigin,
    notification_id: Option<i64>,
) -> io::Result<()> {
    let now = Utc::now();
    let nonce = Uuid::new_v4();
    let generated_id = now.timestamp_nanos_opt().unwrap_or_default();
    let envelope = PendingEnvelope {
        nonce,
        recipient: recipient.to_string(),
        payload: payload.to_string(),
        provenance: MachinePromptProvenance {
            notification_id: notification_id.unwrap_or(generated_id),
            origin,
            queued_at: now.to_rfc3339(),
            delivery: "first-delivery".to_string(),
        },
        expires_at: now + Duration::seconds(ENVELOPE_TTL_SECONDS),
    };

    let directory = envelope_dir(cas_root);
    fs::create_dir_all(&directory)?;
    prune_expired(&directory, now);
    let temporary = directory.join(format!("{nonce}.tmp"));
    let path = directory.join(format!("{nonce}.json"));
    let write_result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        serde_json::to_writer(&mut file, &envelope).map_err(io::Error::other)?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        fs::rename(&temporary, path)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    write_result
}

/// Consume exactly one envelope for `payload`, if the delivery authority made
/// it. `remove_file` is the compare-and-consume step: concurrent or repeated
/// hook calls cannot both receive the same provenance.
pub(crate) fn consume(
    cas_root: &Path,
    recipient: &str,
    payload: &str,
) -> Option<MachinePromptProvenance> {
    let directory = envelope_dir(cas_root);
    let entries = fs::read_dir(&directory).ok()?;
    let now = Utc::now();
    let mut candidates = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let Ok(file) = OpenOptions::new().read(true).open(&path) else {
            continue;
        };
        let Ok(envelope) = serde_json::from_reader::<_, PendingEnvelope>(file) else {
            let _ = fs::remove_file(path);
            continue;
        };
        if envelope.expires_at <= now {
            let _ = fs::remove_file(path);
            continue;
        }
        if envelope.recipient == recipient && envelope.payload == payload {
            candidates.push((path, envelope));
        }
    }

    // Filename UUID order is immaterial. Every duplicate payload has an
    // independent one-shot envelope; consume one and leave the next delivery
    // for its own hook turn.
    for (path, envelope) in candidates {
        if fs::remove_file(path).is_ok() {
            return Some(envelope.provenance);
        }
    }
    None
}

fn prune_expired(directory: &Path, now: DateTime<Utc>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let Ok(file) = OpenOptions::new().read(true).open(&path) else {
            continue;
        };
        let remove = serde_json::from_reader::<_, PendingEnvelope>(file)
            .map(|envelope| envelope.expires_at <= now)
            .unwrap_or(true);
        if remove {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_payloads_are_consumed_one_at_a_time() {
        let root = tempfile::tempdir().unwrap();
        register(
            root.path(),
            "worker-a",
            "same payload",
            MachinePromptOrigin::AgentAuthored,
            Some(41),
        )
        .unwrap();
        register(
            root.path(),
            "worker-a",
            "same payload",
            MachinePromptOrigin::LifecycleRelay,
            Some(42),
        )
        .unwrap();

        assert!(
            consume(root.path(), "worker-b", "same payload").is_none(),
            "a pending relay for another worker cannot suppress this worker's operator text"
        );
        let first = consume(root.path(), "worker-a", "same payload").unwrap();
        let second = consume(root.path(), "worker-a", "same payload").unwrap();
        assert_ne!(first.notification_id, second.notification_id);
        assert!(consume(root.path(), "worker-a", "same payload").is_none());
    }
}
