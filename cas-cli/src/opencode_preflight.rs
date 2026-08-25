//! Bounded preflight for a locally served OpenAI-compatible OpenCode model.
//!
//! OpenCode's provider configuration is owned by the factory policy layer, so
//! this module accepts explicit endpoint/model values and has no dependency on
//! a particular TOML shape.  The environment names are only an interim input
//! adapter for callers that have not yet resolved project configuration.
//!
//! The probe is deliberately read-only: it lists models, then sends a one-token
//! chat completion to prove that the selected model is loaded and answerable.
//! It never sends credentials, persists response data, or includes response
//! bodies in errors.

use std::time::Duration;

use serde_json::Value;
use url::Url;

/// Interim environment input for the local OpenCode provider base URL.
pub const LOCAL_ENDPOINT_ENV: &str = "CAS_OPENCODE_LOCAL_ENDPOINT";

/// Interim environment input for the local OpenCode model selector.
pub const MODEL_ENV: &str = "CAS_OPENCODE_MODEL";

/// The local probe's default network bound.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

/// Evidence returned after both model discovery and a bounded answer probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEndpointPreflight {
    /// Endpoint with credentials/query fragments removed before display.
    pub endpoint: String,
    pub model: String,
    pub loaded_models: Vec<String>,
    pub answerable: bool,
}

/// Probe a configured local OpenAI-compatible endpoint and model.
///
/// `endpoint` is the provider base URL (for example
/// `http://127.0.0.1:8000/v1`), not a complete `/models` URL.  The endpoint
/// must expose the standard `/models` and `/chat/completions` paths.  A caller
/// can use a smaller timeout in tests; production callers should retain the
/// bounded default.
pub fn preflight_local_endpoint(
    endpoint: &str,
    model: &str,
    timeout: Duration,
) -> Result<LocalEndpointPreflight, String> {
    let endpoint_url = parse_endpoint(endpoint)?;
    let display_endpoint = safe_endpoint_display(&endpoint_url);
    let model = model.trim();
    if model.is_empty() {
        return Err(format!(
            "OpenCode local-model preflight cannot run against {display_endpoint}: model is empty"
        ));
    }
    if timeout.is_zero() {
        return Err(format!(
            "OpenCode local-model preflight cannot run against {display_endpoint}: timeout must be positive"
        ));
    }

    let agent = ureq::AgentBuilder::new().timeout(timeout).build();
    let models_url = endpoint_path(&endpoint_url, "models");
    let response = agent.get(&models_url).call().map_err(|error| {
        format!(
            "OpenCode local-model preflight could not reach {display_endpoint} at /models: {error_class} (start the local provider and verify the endpoint) ",
            error_class = transport_error_class(&error)
        )
    })?;
    let payload: Value = response.into_json().map_err(|error| {
        format!(
            "OpenCode local-model preflight reached {display_endpoint}, but /models returned invalid JSON: {error}"
        )
    })?;
    let loaded_models = model_ids(&payload);
    if !loaded_models.iter().any(|loaded| loaded == model) {
        let listed = if loaded_models.is_empty() {
            "none".to_string()
        } else {
            loaded_models.join(", ")
        };
        return Err(format!(
            "OpenCode local-model preflight reached {display_endpoint}, but model {model:?} is not loaded; endpoint reports: {listed}"
        ));
    }

    let completions_url = endpoint_path(&endpoint_url, "chat/completions");
    let probe = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "Reply with READY."}],
        "max_tokens": 1,
        "stream": false
    });
    let response = agent
        .post(&completions_url)
        .set("Content-Type", "application/json")
        .send_json(probe)
        .map_err(|error| {
            format!(
                "OpenCode local-model preflight found model {model:?} at {display_endpoint}, but the answer probe failed: {error_class} (check that the model is loaded and answerable)",
                error_class = transport_error_class(&error)
            )
        })?;
    let answer: Value = response.into_json().map_err(|error| {
        format!(
            "OpenCode local-model preflight found model {model:?} at {display_endpoint}, but the answer probe returned invalid JSON: {error}"
        )
    })?;
    if !answer
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|choices| !choices.is_empty())
    {
        return Err(format!(
            "OpenCode local-model preflight found model {model:?} at {display_endpoint}, but the answer probe returned no choices"
        ));
    }

    Ok(LocalEndpointPreflight {
        endpoint: display_endpoint,
        model: model.to_string(),
        loaded_models,
        answerable: true,
    })
}

/// Check a future cloud provider's environment-key path without ever exposing
/// the key value.  The caller supplies the resolved value so config/TOML and
/// environment adapters can share the same validation behavior.
pub fn preflight_provider_env_key(
    provider: &str,
    env_key: &str,
    value: Option<&str>,
) -> Result<(), String> {
    let provider = provider.trim();
    let env_key = env_key.trim();
    if provider.is_empty() || env_key.is_empty() {
        return Err("provider and environment key are required".to_string());
    }
    if value.is_none_or(|value| value.trim().is_empty()) {
        return Err(format!(
            "OpenCode provider {provider:?} is missing its required environment key {env_key}"
        ));
    }
    Ok(())
}

fn parse_endpoint(raw: &str) -> Result<Url, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(
            "OpenCode local-model preflight requires a configured endpoint (set the local provider base URL)"
                .to_string(),
        );
    }
    let url = Url::parse(trimmed).map_err(|error| {
        format!("OpenCode local-model preflight endpoint {trimmed:?} is invalid: {error}")
    })?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(format!(
            "OpenCode local-model preflight endpoint {} must be an http(s) URL",
            safe_endpoint_display(&url)
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!(
            "OpenCode local-model preflight endpoint {} must not contain credentials; configure local access without embedding secrets in the URL",
            safe_endpoint_display(&url)
        ));
    }
    Ok(url)
}

fn endpoint_path(base: &Url, suffix: &str) -> String {
    let mut url = base.clone();
    let path = format!(
        "/{}/{}",
        base.path().trim_matches('/'),
        suffix.trim_matches('/')
    );
    url.set_path(path.trim_start_matches('/'));
    // Query strings are not part of the local provider contract. Dropping one
    // also guarantees that a token-like query value cannot reach the probe.
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn safe_endpoint_display(url: &Url) -> String {
    let mut safe = url.clone();
    let _ = safe.set_username("");
    let _ = safe.set_password(None);
    safe.set_query(None);
    safe.set_fragment(None);
    safe.to_string()
}

fn model_ids(payload: &Value) -> Vec<String> {
    payload
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect()
}

fn transport_error_class(error: &ureq::Error) -> &'static str {
    match error {
        ureq::Error::Status(code, _) => match *code {
            401 | 403 => "provider rejected the request",
            404 => "provider endpoint returned not found",
            500..=599 => "provider returned a server error",
            _ => "provider returned an HTTP error",
        },
        ureq::Error::Transport(_) => "connection failed or timed out",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    fn respond(mut stream: TcpStream, body: &str) {
        let mut request = [0; 4096];
        let _ = stream.read(&mut request);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    }

    #[test]
    fn local_endpoint_probe_passes_against_stub_and_does_not_persist_data() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!(
            "http://{}/v1?token=must-not-leak",
            listener.local_addr().unwrap()
        );
        let thread = thread::spawn(move || {
            let (models, _) = listener.accept().unwrap();
            respond(models, r#"{"object":"list","data":[{"id":"qwen-local"}]}"#);
            let (completion, _) = listener.accept().unwrap();
            respond(
                completion,
                r#"{"choices":[{"message":{"content":"READY"}}]}"#,
            );
        });

        let result =
            preflight_local_endpoint(&endpoint, "qwen-local", Duration::from_secs(2)).unwrap();
        thread.join().unwrap();
        assert_eq!(result.model, "qwen-local");
        assert_eq!(result.loaded_models, vec!["qwen-local"]);
        assert!(result.answerable);
        assert!(!result.endpoint.contains("token"));
    }

    #[test]
    fn unreachable_endpoint_names_remediation_without_secret_query() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let error = preflight_local_endpoint(
            &format!("http://127.0.0.1:{port}/v1?api_key=secret"),
            "qwen-local",
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(error.contains(&format!("127.0.0.1:{port}")), "{error}");
        assert!(error.contains("start the local provider"), "{error}");
        assert!(!error.contains("secret"), "secret leaked: {error}");
    }

    #[test]
    fn endpoint_credentials_are_rejected_without_echoing_them() {
        let error = preflight_local_endpoint(
            "http://operator:secret@127.0.0.1:8000/v1",
            "qwen-local",
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(error.contains("must not contain credentials"), "{error}");
        assert!(!error.contains("operator"), "username leaked: {error}");
        assert!(!error.contains(":secret@"), "password leaked: {error}");
    }

    #[test]
    fn model_must_be_reported_as_loaded() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let thread = thread::spawn(move || {
            let (models, _) = listener.accept().unwrap();
            respond(models, r#"{"data":[{"id":"other-model"}]}"#);
        });
        let error =
            preflight_local_endpoint(&endpoint, "qwen-local", Duration::from_secs(2)).unwrap_err();
        thread.join().unwrap();
        assert!(error.contains("qwen-local"), "{error}");
        assert!(error.contains("not loaded"), "{error}");
    }

    #[test]
    fn cloud_provider_key_validation_never_echoes_the_value() {
        assert!(preflight_provider_env_key("cloud", "PROVIDER_KEY", Some("secret")).is_ok());
        let error = preflight_provider_env_key("cloud", "PROVIDER_KEY", None).unwrap_err();
        assert!(error.contains("PROVIDER_KEY"), "{error}");
        assert!(!error.contains("secret"), "secret leaked: {error}");
    }
}
