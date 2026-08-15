//! Opt-in, server-side session-card enrichment.
//!
//! Privacy boundary: raw terminal bytes never leave this module. The request
//! is assembled only after ANSI removal, blank-line compaction, secret
//! redaction, and a hard 6 KiB transcript cap.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cas_factory::AiEnrichmentConfig;
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use super::relay::PaneBuffer;
use crate::ai_enrichment::{
    AiEnrichmentProvider, AiEnrichmentRequest, HttpAiEnrichmentProvider, redact_string,
};
use crate::ui::factory::protocol::SessionCardSummary;

const MIN_NEW_BYTES: usize = 2 * 1024;
const MIN_CALL_INTERVAL: Duration = Duration::from_secs(90);
const HARD_CALL_CAP: Duration = Duration::from_secs(60);
const IDLE_AFTER: Duration = Duration::from_secs(5 * 60);
const MAX_TRANSCRIPT_BYTES: usize = 6 * 1024;
const SYSTEM_PROMPT: &str = "Summarize this live coding-agent session for an operator. Be concrete and terse. Describe current work, not personality. Return only the required JSON object.";

pub(in crate::ui::factory::daemon) struct SessionSummarizer {
    config: AiEnrichmentConfig,
    provider: Arc<dyn AiEnrichmentProvider>,
    new_bytes: usize,
    last_output_at: Instant,
    last_call_at: Option<Instant>,
    force_due: bool,
    in_flight: Option<JoinHandle<anyhow::Result<SessionCardSummary>>>,
    last_summary: Option<SessionCardSummary>,
}

impl SessionSummarizer {
    pub(in crate::ui::factory::daemon) fn new(config: AiEnrichmentConfig) -> Self {
        let provider = Arc::new(HttpAiEnrichmentProvider::new(config.clone()));
        Self {
            config,
            provider,
            new_bytes: 0,
            last_output_at: Instant::now(),
            last_call_at: None,
            force_due: false,
            in_flight: None,
            last_summary: None,
        }
    }

    #[cfg(test)]
    fn with_provider(config: AiEnrichmentConfig, provider: Arc<dyn AiEnrichmentProvider>) -> Self {
        let mut summarizer = Self::new(config);
        summarizer.provider = provider;
        summarizer
    }

    pub(super) fn note_output(&mut self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        self.new_bytes = self.new_bytes.saturating_add(bytes);
        self.last_output_at = Instant::now();
    }

    /// Mark an orchestrator checkpoint/task/error transition as meaningful.
    /// The hard one-call-per-minute cap still applies.
    pub(super) fn note_semantic_event(&mut self) {
        self.force_due = true;
    }

    pub(super) async fn poll(
        &mut self,
        pane_buffers: &HashMap<String, PaneBuffer>,
        metadata: &str,
    ) -> Option<SessionCardSummary> {
        if self.in_flight.as_ref().is_some_and(JoinHandle::is_finished) {
            let task = self.in_flight.take().expect("checked above");
            match task.await {
                Ok(Ok(summary)) => {
                    self.last_summary = Some(summary.clone());
                    return Some(summary);
                }
                Ok(Err(error)) => tracing::debug!(%error, "session summary update dropped"),
                Err(error) => tracing::debug!(%error, "session summary task failed"),
            }
        }

        // Idle is a local state transition: it never spends a model call.
        if self.last_output_at.elapsed() >= IDLE_AFTER {
            if let Some(previous) = self.last_summary.as_ref()
                && previous.phase != "idle"
            {
                let mut idle = previous.clone();
                idle.phase = "idle".to_string();
                idle.blocked_on = None;
                idle.generated_at = chrono::Utc::now().to_rfc3339();
                self.last_summary = Some(idle.clone());
                return Some(idle);
            }
            return None;
        }

        if !self.config.enabled || self.in_flight.is_some() {
            return None;
        }
        let since_call = self.last_call_at.map(|at| at.elapsed());
        let normal_due = self.new_bytes >= MIN_NEW_BYTES
            && since_call.is_none_or(|elapsed| elapsed >= MIN_CALL_INTERVAL);
        let forced_due =
            self.force_due && since_call.is_none_or(|elapsed| elapsed >= HARD_CALL_CAP);
        if !normal_due && !forced_due {
            return None;
        }

        let transcript = build_transcript(pane_buffers);
        if transcript.trim().is_empty() {
            return None;
        }
        let provider = Arc::clone(&self.provider);
        let metadata = metadata.to_string();
        self.last_call_at = Some(Instant::now());
        self.new_bytes = 0;
        self.force_due = false;
        self.in_flight = Some(tokio::task::spawn_blocking(move || {
            request_summary(provider.as_ref(), &metadata, &transcript)
        }));
        None
    }
}

fn build_transcript(pane_buffers: &HashMap<String, PaneBuffer>) -> String {
    let mut panes: Vec<_> = pane_buffers.iter().collect();
    panes.sort_by_key(|(name, _)| *name);
    let joined = panes
        .into_iter()
        .map(|(name, buffer)| format!("[{name}]\n{}", buffer.as_plain_text()))
        .collect::<Vec<_>>()
        .join("\n");
    let compact = collapse_blank_runs(&redact_string(&joined));
    utf8_tail(&compact, MAX_TRANSCRIPT_BYTES).to_string()
}

fn collapse_blank_runs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = false;
    for line in text.lines() {
        if line.trim().is_empty() {
            if !blank {
                out.push('\n');
            }
            blank = true;
        } else {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(line.trim_end());
            out.push('\n');
            blank = false;
        }
    }
    out
}

fn utf8_tail(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

fn summary_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["title", "description", "phase", "blocked_on"],
        "properties": {
            "title": {"type": "string", "maxLength": 60},
            "description": {"type": "string", "maxLength": 200},
            "phase": {"type": "string", "enum": ["planning", "editing", "testing", "building", "blocked", "reviewing", "idle"]},
            "blocked_on": {"type": ["string", "null"], "maxLength": 80}
        }
    })
}

fn request_summary(
    provider: &dyn AiEnrichmentProvider,
    metadata: &str,
    transcript: &str,
) -> anyhow::Result<SessionCardSummary> {
    let value = provider.complete_json(AiEnrichmentRequest {
        system_prefix: SYSTEM_PROMPT,
        schema_name: "session_summary",
        schema: summary_schema(),
        payload: json!({"metadata": metadata, "transcript": transcript}),
    })?;
    parse_summary(&value)
}

fn parse_summary(value: &Value) -> anyhow::Result<SessionCardSummary> {
    let phase = value
        .get("phase")
        .and_then(Value::as_str)
        .unwrap_or_default();
    anyhow::ensure!(
        matches!(
            phase,
            "planning" | "editing" | "testing" | "building" | "blocked" | "reviewing" | "idle"
        ),
        "invalid phase"
    );
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    anyhow::ensure!(
        !title.is_empty() && title.chars().count() <= 60,
        "invalid title"
    );
    anyhow::ensure!(
        !description.is_empty() && description.chars().count() <= 200,
        "invalid description"
    );
    let blocked_on = value
        .get("blocked_on")
        .and_then(Value::as_str)
        .map(str::to_string);
    anyhow::ensure!(
        phase == "blocked" || blocked_on.is_none(),
        "blocked_on is only valid for blocked phase"
    );
    anyhow::ensure!(
        blocked_on.as_ref().is_none_or(|s| s.chars().count() <= 80),
        "blocked_on too long"
    );
    Ok(SessionCardSummary {
        title: title.to_string(),
        description: description.to_string(),
        phase: phase.to_string(),
        blocked_on,
        generated_at: chrono::Utc::now().to_rfc3339(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingProvider(Arc<AtomicUsize>);

    impl AiEnrichmentProvider for CountingProvider {
        fn complete_json(&self, _request: AiEnrichmentRequest<'_>) -> anyhow::Result<Value> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(json!({
                "title": "Active work",
                "description": "Editing the shared enrichment path.",
                "phase": "editing",
                "blocked_on": null
            }))
        }
    }

    #[test]
    fn defaults_are_private_and_explicitly_low_effort() {
        let config = AiEnrichmentConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.model, "gpt-5.6-luna");
        assert_eq!(config.effort, "low");
    }

    #[test]
    fn transcript_is_redacted_compact_and_bounded() {
        let text = format!(
            "ok\n\n\nAPI_KEY=hunter2\nAuthorization: Bearer abc.def\n-----BEGIN PRIVATE KEY-----\nsecret\n-----END PRIVATE KEY-----\n{}",
            "x".repeat(7000)
        );
        let redacted = collapse_blank_runs(&redact_string(&text));
        let tail = utf8_tail(&redacted, MAX_TRANSCRIPT_BYTES);
        assert!(!redacted.contains("hunter2"));
        assert!(!redacted.contains("abc.def"));
        assert!(!redacted.contains("PRIVATE KEY"));
        assert!(tail.len() <= MAX_TRANSCRIPT_BYTES);
        assert!(!redacted.contains("\n\n\n"));
    }

    #[tokio::test]
    async fn idle_sessions_make_zero_provider_calls() {
        let calls = Arc::new(AtomicUsize::new(0));
        let config = AiEnrichmentConfig {
            enabled: true,
            ..Default::default()
        };
        let mut summarizer = SessionSummarizer::with_provider(
            config,
            Arc::new(CountingProvider(Arc::clone(&calls))),
        );
        summarizer.new_bytes = MIN_NEW_BYTES;
        summarizer.last_output_at = Instant::now() - IDLE_AFTER;
        let mut pane = PaneBuffer::default();
        pane.append(b"meaningful output");
        assert!(
            summarizer
                .poll(
                    &HashMap::from([("supervisor".into(), pane)]),
                    "session=test"
                )
                .await
                .is_none()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(summarizer.in_flight.is_none());
    }

    #[tokio::test]
    async fn meaningful_output_is_summarized_once_server_side() {
        let calls = Arc::new(AtomicUsize::new(0));
        let config = AiEnrichmentConfig {
            enabled: true,
            ..Default::default()
        };
        let mut summarizer = SessionSummarizer::with_provider(
            config,
            Arc::new(CountingProvider(Arc::clone(&calls))),
        );
        summarizer.new_bytes = MIN_NEW_BYTES;
        let mut pane = PaneBuffer::default();
        pane.append(b"meaningful output");
        let buffers = HashMap::from([("supervisor".into(), pane)]);
        assert!(summarizer.poll(&buffers, "session=test").await.is_none());
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(1)).await;
            if let Some(summary) = summarizer.poll(&buffers, "session=test").await {
                assert_eq!(summary.phase, "editing");
                assert_eq!(calls.load(Ordering::SeqCst), 1);
                return;
            }
        }
        panic!("summary task did not complete");
    }
}
