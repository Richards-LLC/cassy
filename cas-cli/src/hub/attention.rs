//! Best-effort AI structuring for long-tail Commander attention events.
//!
//! Events are already stored and broadcast before they enter this worker. A
//! provider failure therefore only clears the transient pending marker; it can
//! never suppress or delay the raw, actionable event.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::{
    AttentionAction, AttentionEnrichment, AttentionSeverity, MachineEvent, MachineEventBus,
};
use crate::ai_enrichment::{AiEnrichmentProvider, AiEnrichmentRequest};

const BATCH_WINDOW: Duration = Duration::from_secs(2);
const SYSTEM_PREFIX: &str = "Structure operational events for a coding-session operator. Preserve concrete failure identity across rewordings. Return only the required JSON object.";

#[derive(Debug, Deserialize)]
struct BatchResponse {
    events: Vec<EnrichedEvent>,
}

#[derive(Debug, Deserialize)]
struct EnrichedEvent {
    sequence: u64,
    severity: AttentionSeverity,
    summary: String,
    detail: Option<String>,
    action: AttentionAction,
    fingerprint: String,
}

pub(crate) fn spawn_attention_enricher(
    events: MachineEventBus,
    receiver: mpsc::UnboundedReceiver<MachineEvent>,
    provider: Arc<dyn AiEnrichmentProvider>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run_attention_enricher(events, receiver, provider))
}

async fn run_attention_enricher(
    events: MachineEventBus,
    mut receiver: mpsc::UnboundedReceiver<MachineEvent>,
    provider: Arc<dyn AiEnrichmentProvider>,
) {
    while let Some(first) = receiver.recv().await {
        let mut batch = vec![first];
        let deadline = tokio::time::Instant::now() + BATCH_WINDOW;
        loop {
            tokio::select! {
                biased;
                next = receiver.recv() => match next {
                    Some(event) => batch.push(event),
                    None => break,
                },
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }

        let provider = Arc::clone(&provider);
        let request_batch = batch.clone();
        let result =
            tokio::task::spawn_blocking(move || enrich_batch(provider.as_ref(), &request_batch))
                .await;
        let mut enriched = match result {
            Ok(Ok(items)) => items,
            Ok(Err(error)) => {
                tracing::debug!(%error, count = batch.len(), "attention enrichment dropped");
                HashMap::new()
            }
            Err(error) => {
                tracing::debug!(%error, count = batch.len(), "attention enrichment task failed");
                HashMap::new()
            }
        };
        for event in batch {
            events.finish_enrichment(event.sequence, enriched.remove(&event.sequence));
        }
    }
}

fn enrich_batch(
    provider: &dyn AiEnrichmentProvider,
    events: &[MachineEvent],
) -> anyhow::Result<HashMap<u64, AttentionEnrichment>> {
    let payload = json!({
        "events": events.iter().map(event_prompt_value).collect::<Vec<_>>(),
    });
    let response = provider.complete_json(AiEnrichmentRequest {
        system_prefix: SYSTEM_PREFIX,
        schema_name: "commander_attention_batch",
        schema: attention_schema(),
        payload,
    })?;
    let response: BatchResponse = serde_json::from_value(response)?;
    let requested: std::collections::HashSet<_> =
        events.iter().map(|event| event.sequence).collect();
    let mut output = HashMap::new();
    for item in response.events {
        if !requested.contains(&item.sequence) || output.contains_key(&item.sequence) {
            continue;
        }
        if let Some(enrichment) = validate_enrichment(item) {
            output.insert(enrichment.0, enrichment.1);
        }
    }
    Ok(output)
}

fn event_prompt_value(event: &MachineEvent) -> Value {
    json!({
        "sequence": event.sequence,
        "type": event.kind,
        "session": event.session,
        "pane_id": event.pane_id,
        "payload": event.payload,
        "diagnostic": event.diagnostic,
        "session_context": event.session_context,
    })
}

fn validate_enrichment(item: EnrichedEvent) -> Option<(u64, AttentionEnrichment)> {
    let summary = item
        .summary
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let detail = item
        .detail
        .map(|detail| detail.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|detail| !detail.is_empty());
    let fingerprint = item.fingerprint.trim().to_ascii_lowercase();
    if summary.is_empty()
        || summary.chars().count() > 90
        || summary.contains('{')
        || summary.contains('}')
        || detail
            .as_ref()
            .is_some_and(|detail| detail.chars().count() > 120)
        || fingerprint.is_empty()
        || fingerprint.len() > 160
        || !fingerprint
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".:_-".contains(character))
    {
        return None;
    }
    Some((
        item.sequence,
        AttentionEnrichment {
            severity: item.severity,
            summary,
            detail,
            action: item.action,
            fingerprint,
        },
    ))
}

fn attention_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["events"],
        "properties": {
            "events": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["sequence", "severity", "summary", "detail", "action", "fingerprint"],
                    "properties": {
                        "sequence": {"type": "integer", "minimum": 1},
                        "severity": {"type": "string", "enum": ["critical", "warning", "info"]},
                        "summary": {"type": "string", "minLength": 1, "maxLength": 90},
                        "detail": {"type": ["string", "null"], "maxLength": 120},
                        "action": {"type": "string", "enum": ["repair", "view_pane", "retry", "open_pr", "none"]},
                        "fingerprint": {"type": "string", "minLength": 1, "maxLength": 160}
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::ai_enrichment::HttpAiEnrichmentProvider;
    use crate::ui::factory::DaemonMessage;
    use cas_factory::AiEnrichmentConfig;

    struct RecordingProvider {
        calls: AtomicUsize,
        batch_sizes: Mutex<Vec<usize>>,
    }

    async fn advance_until_pending_cleared(events: &MachineEventBus) {
        // With Tokio's paused clock, advancing before the spawned worker has
        // received its first event can move past the deadline it will later
        // register. Advance in bounded windows while yielding to the worker,
        // so the test waits on the actual completion state rather than one
        // scheduler-specific ordering.
        for _ in 0..16 {
            tokio::time::advance(BATCH_WINDOW).await;
            for _ in 0..128 {
                if !events
                    .history()
                    .iter()
                    .any(|event| event.enrichment_pending)
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        }
    }

    async fn wait_for_enrichment_patch(
        updates: &mut tokio::sync::broadcast::Receiver<MachineEvent>,
        sequence: u64,
    ) -> MachineEvent {
        // Keep advancing the paused clock until the broadcast emitted after
        // finish_enrichment observes the pending flag write. The provider is
        // blocking I/O, so a fixed yield budget can expire before that patch
        // task gets scheduled under parallel test load.
        let clock_driver = tokio::spawn(async {
            for _ in 0..16 {
                tokio::task::yield_now().await;
            }
            loop {
                tokio::time::advance(BATCH_WINDOW).await;
                for _ in 0..128 {
                    tokio::task::yield_now().await;
                }
            }
        });
        let (timeout_tx, timeout_rx) = std::sync::mpsc::channel();
        let mut timeout = tokio::task::spawn_blocking(move || {
            timeout_rx.recv_timeout(Duration::from_secs(5)).is_err()
        });
        let patched = loop {
            tokio::select! {
                biased;
                result = updates.recv() => match result {
                    Ok(event) if event.sequence == sequence && !event.enrichment_pending => {
                        break event;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        panic!("missed {skipped} machine event updates")
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        panic!("machine event updates closed before enrichment finished")
                    }
                },
                _ = &mut timeout => {
                    clock_driver.abort();
                    panic!("machine event enrichment patch did not arrive within 5 seconds")
                }
            }
        };
        let _ = timeout_tx.send(());
        let _ = timeout.await;
        clock_driver.abort();
        patched
    }

    impl AiEnrichmentProvider for RecordingProvider {
        fn complete_json(&self, request: AiEnrichmentRequest<'_>) -> anyhow::Result<Value> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let events = request.payload["events"].as_array().unwrap();
            self.batch_sizes.lock().unwrap().push(events.len());
            Ok(json!({
                "events": events.iter().map(|event| json!({
                    "sequence": event["sequence"],
                    "severity": "critical",
                    "summary": "Daemon failed while testing authentication",
                    "detail": "auth.rs:44 serde panic",
                    "action": "retry",
                    "fingerprint": "auth.rs-serde-panic"
                })).collect::<Vec<_>>()
            }))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn twenty_event_burst_is_one_call_and_every_event_is_patched() {
        let events = MachineEventBus::new(32);
        let receiver = events.enable_enrichment();
        let provider = Arc::new(RecordingProvider {
            calls: AtomicUsize::new(0),
            batch_sizes: Mutex::new(Vec::new()),
        });
        let task = spawn_attention_enricher(events.clone(), receiver, provider.clone());
        for index in 0..20 {
            events.observe_daemon(
                "factory-a",
                &DaemonMessage::Error {
                    message: format!("wording {index}"),
                },
            );
        }

        advance_until_pending_cleared(&events).await;
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(*provider.batch_sizes.lock().unwrap(), vec![20]);
        assert_eq!(
            events
                .history()
                .iter()
                .filter(|event| event.enrichment.is_some())
                .count(),
            20
        );
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn killed_http_api_clears_pending_and_preserves_the_raw_event() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
        // Accept the request and close the socket immediately. A dropped
        // listener can leave the kernel's refused-connection path pending
        // long enough for a paused-clock test to finish before ureq reports
        // the provider failure.
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("test API accepts one request");
            stream
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .expect("test API response");
        });
        let provider = HttpAiEnrichmentProvider::new(AiEnrichmentConfig {
            enabled: true,
            endpoint,
            ..Default::default()
        });
        let events = MachineEventBus::new(4);
        let receiver = events.enable_enrichment();
        let mut updates = events.subscribe();
        let task = spawn_attention_enricher(events.clone(), receiver, Arc::new(provider));
        events.observe_daemon(
            "factory-a",
            &DaemonMessage::Error {
                message: "raw error still visible".into(),
            },
        );
        let raw = updates.recv().await.expect("raw event broadcast");
        let patch = wait_for_enrichment_patch(&mut updates, raw.sequence).await;
        assert!(!patch.enrichment_pending);
        assert!(patch.enrichment.is_none());

        let event = &events.history()[0];
        assert!(!event.enrichment_pending);
        assert!(event.enrichment.is_none());
        assert_eq!(
            event.payload.as_ref().unwrap()["message"],
            "raw error still visible"
        );
        server.join().expect("test API thread");
        task.abort();
    }

    #[test]
    fn invalid_domain_fields_are_rejected_instead_of_repaired_silently() {
        assert!(
            validate_enrichment(EnrichedEvent {
                sequence: 1,
                severity: AttentionSeverity::Info,
                summary: "x".repeat(91),
                detail: None,
                action: AttentionAction::None,
                fingerprint: "valid-fingerprint".into(),
            })
            .is_none()
        );
        assert!(
            validate_enrichment(EnrichedEvent {
                sequence: 1,
                severity: AttentionSeverity::Info,
                summary: "Useful summary".into(),
                detail: None,
                action: AttentionAction::None,
                fingerprint: "spaces are unstable".into(),
            })
            .is_none()
        );
    }
}
