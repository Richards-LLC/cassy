//! Shared, opt-in AI enrichment transport and privacy boundary.
//!
//! Consumers supply a prompt prefix, strict JSON schema, and payload. This
//! module always redacts the payload immediately before serialization, so a
//! caller cannot accidentally bypass the egress guard.

use std::time::{Duration, Instant};

use cas_factory::AiEnrichmentConfig;
use regex::Regex;
use serde_json::{Map, Value, json};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) struct AiEnrichmentRequest<'a> {
    pub system_prefix: &'a str,
    pub schema_name: &'a str,
    pub schema: Value,
    /// May contain unredacted caller data. The provider structurally redacts
    /// it at the final egress boundary.
    pub payload: Value,
}

pub(crate) trait AiEnrichmentProvider: Send + Sync {
    fn complete_json(&self, request: AiEnrichmentRequest<'_>) -> anyhow::Result<Value>;
}

#[derive(Clone)]
pub(crate) struct HttpAiEnrichmentProvider {
    config: AiEnrichmentConfig,
}

impl HttpAiEnrichmentProvider {
    pub(crate) fn new(config: AiEnrichmentConfig) -> Self {
        Self { config }
    }
}

impl AiEnrichmentProvider for HttpAiEnrichmentProvider {
    fn complete_json(&self, request: AiEnrichmentRequest<'_>) -> anyhow::Result<Value> {
        anyhow::ensure!(self.config.enabled, "AI enrichment is disabled");
        anyhow::ensure!(
            self.config.provider == "openai" || self.config.provider == "openai-compatible",
            "unsupported AI enrichment provider"
        );
        anyhow::ensure!(
            self.config.effort == "low",
            "AI enrichment reasoning effort must be low"
        );

        let payload = redact_json(&request.payload);
        let payload_text = serde_json::to_string(&payload)?;
        let key = std::env::var(&self.config.api_key_env).unwrap_or_default();
        let started = Instant::now();
        let mut last_error = None;
        for retry in [false, true] {
            let remaining = REQUEST_TIMEOUT.saturating_sub(started.elapsed());
            anyhow::ensure!(
                !remaining.is_zero(),
                "AI enrichment request timed out after 3s"
            );
            let agent = ureq::AgentBuilder::new().timeout(remaining).build();
            let body = json!({
                "model": self.config.model,
                "reasoning": {"effort": self.config.effort},
                "input": [
                    {"role": "system", "content": [{"type": "input_text", "text": request.system_prefix}]},
                    {"role": "user", "content": [{"type": "input_text", "text": format!("{payload_text}{}", if retry { "\nJSON only." } else { "" })}]}
                ],
                "text": {"format": {"type": "json_schema", "name": request.schema_name, "strict": true, "schema": request.schema}},
                "max_output_tokens": 256
            });
            let mut outbound = agent
                .post(&self.config.endpoint)
                .set("content-type", "application/json");
            if !key.is_empty() {
                outbound = outbound.set("authorization", &format!("Bearer {key}"));
            }
            let response = outbound.send_json(body)?;
            let envelope: Value = response.into_json()?;
            let raw = extract_output_text(&envelope)
                .ok_or_else(|| anyhow::anyhow!("provider response had no output text"))?;
            match serde_json::from_str(raw) {
                Ok(value) => return Ok(value),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error
            .map(anyhow::Error::from)
            .unwrap_or_else(|| anyhow::anyhow!("invalid AI enrichment response")))
    }
}

fn extract_output_text(value: &Value) -> Option<&str> {
    value
        .get("output_text")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("output")?
                .as_array()?
                .iter()
                .flat_map(|item| {
                    item.get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                })
                .find_map(|content| content.get("text").and_then(Value::as_str))
        })
}

/// Redact secret assignments, bearer credentials, and PEM-style BEGIN blocks.
pub(crate) fn redact_string(text: &str) -> String {
    let assignment = Regex::new(r"(?i)\b(api[_-]?key|token|secret|password)\b\s*[:=]").unwrap();
    let bearer = Regex::new(r"(?i)(authorization\s*:\s*)?bearer\s+[A-Za-z0-9._~+/=-]+").unwrap();
    let mut in_begin_block = false;
    let mut output = Vec::new();
    for line in text.lines() {
        if line.contains("-----BEGIN ") {
            in_begin_block = true;
            output.push("[REDACTED BEGIN BLOCK]");
            continue;
        }
        if in_begin_block {
            if line.contains("-----END ") {
                in_begin_block = false;
            }
            continue;
        }
        if assignment.is_match(line) {
            output.push("[REDACTED SECRET]");
            continue;
        }
        output.push(if bearer.is_match(line) {
            "[REDACTED BEARER]"
        } else {
            line
        });
    }
    output.join("\n")
}

/// Recursively redact JSON. Secret-named fields are replaced regardless of
/// their value shape; every other string runs through the shared text pass.
pub(crate) fn redact_json(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(redact_string(text)),
        Value::Array(items) => Value::Array(items.iter().map(redact_json).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| {
                    let normalized = key.to_ascii_lowercase().replace(['-', '_'], "");
                    let redacted = if ["apikey", "token", "secret", "password", "authorization"]
                        .contains(&normalized.as_str())
                    {
                        Value::String("[REDACTED SECRET]".to_string())
                    } else {
                        redact_json(value)
                    };
                    (key.clone(), redacted)
                })
                .collect::<Map<_, _>>(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn recursive_redaction_covers_secret_keys_strings_and_blocks() {
        let value = json!({"api_key":"top-secret", "nested":["Authorization: Bearer abc.def", "-----BEGIN PRIVATE KEY-----\nkey\n-----END PRIVATE KEY-----"]});
        let redacted = redact_json(&value).to_string();
        assert!(!redacted.contains("top-secret"));
        assert!(!redacted.contains("abc.def"));
        assert!(!redacted.contains("PRIVATE KEY"));
    }

    #[test]
    fn planted_api_key_is_absent_from_actual_outbound_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                bytes.extend_from_slice(&buf[..n]);
                if let Some(split) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..split + 4]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap();
                    if bytes.len() >= split + 4 + length {
                        break;
                    }
                }
            }
            let captured = String::from_utf8_lossy(&bytes).to_string();
            let body = r#"{"output_text":"{\"title\":\"Safe summary\"}"}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            captured
        });
        let config = AiEnrichmentConfig {
            enabled: true,
            endpoint,
            ..Default::default()
        };
        let provider = HttpAiEnrichmentProvider::new(config);
        let result = provider
            .complete_json(AiEnrichmentRequest {
                system_prefix: "Stable prefix",
                schema_name: "capture",
                schema: json!({"type":"object"}),
                payload: json!({"transcript":"working\nAPI_KEY=planted-wire-secret\nnext"}),
            })
            .unwrap();
        assert_eq!(result["title"], "Safe summary");
        let captured = server.join().unwrap();
        assert!(
            !captured.contains("planted-wire-secret"),
            "secret leaked in captured HTTP request"
        );
        assert!(
            !captured.contains("API_KEY="),
            "secret assignment leaked in captured HTTP request"
        );
        assert!(captured.contains("gpt-5.6-luna"));
        assert!(captured.contains("\"effort\":\"low\""));
    }
}
