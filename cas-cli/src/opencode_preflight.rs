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

use cas_pty::{ServingIdentity, ServingRoute};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

/// Interim environment input for the local OpenCode provider base URL.
pub const LOCAL_ENDPOINT_ENV: &str = "CAS_OPENCODE_LOCAL_ENDPOINT";

/// Interim environment input for the local OpenCode model selector.
pub const MODEL_ENV: &str = "CAS_OPENCODE_MODEL";

/// Hosted Qwen route credentials.  The value is read only at probe time and
/// is never copied into a receipt, generated OpenCode config, or an error.
pub const DASHSCOPE_API_KEY_ENV: &str = "DASHSCOPE_API_KEY";

/// International DashScope OpenAI-compatible endpoint from Qwen Cloud's
/// current API documentation.
pub const DASHSCOPE_INTL_ENDPOINT: &str = "https://dashscope-intl.aliyuncs.com/compatible-mode/v1";

/// Mainland China DashScope OpenAI-compatible endpoint.  Region selection is
/// explicit through `alibaba-cn/...`; it is never inferred from a failed
/// international request.
pub const DASHSCOPE_CN_ENDPOINT: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";

pub const HOSTED_QWEN_PROVIDER: &str = "alibaba";
pub const HOSTED_QWEN_CN_PROVIDER: &str = "alibaba-cn";
pub const HOSTED_QWEN_MODEL: &str = "qwen3.8-max";

/// Qwen3.8-Max's own reasoning variants.  These are intentionally separate
/// from the local server's probed effort set.
pub const HOSTED_QWEN_ACCEPTED_EFFORTS: [cas_mux::Effort; 3] = [
    cas_mux::Effort::Low,
    cas_mux::Effort::Medium,
    cas_mux::Effort::XHigh,
];

/// A route selector carried by the OpenCode `provider/model` selector.
pub type OpenCodeRoute = ServingRoute;

/// Status of a route-specific support claim.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SupportClaimStatus {
    PendingKey,
    PendingConformance,
    Supported,
}

/// Secret-free, route-specific support evidence.
///
/// A local claim and a hosted claim are independent.  In particular, a
/// missing hosted key does not invalidate a local endpoint claim.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct OpenCodeSupportClaim {
    pub route: ServingRoute,
    pub serving_identity: ServingIdentity,
    pub status: SupportClaimStatus,
}

/// Evidence returned after hosted provider authentication and model probes.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct HostedEndpointPreflight {
    pub route: ServingRoute,
    pub serving_identity: ServingIdentity,
    pub loaded_models: Vec<String>,
    pub accepted_efforts: Vec<String>,
    pub authenticated: bool,
    pub answerable: bool,
}

/// The local probe's default network bound.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

/// Classify a complete OpenCode provider/model selector into one explicit
/// serving route.  Requiring the provider prefix is important: a hosted
/// failure must never silently fall back to a local endpoint (or vice versa).
pub fn opencode_route_for_selector(selector: &str) -> Result<ServingRoute, String> {
    let selector = selector.trim();
    let Some((provider, model)) = selector.split_once('/') else {
        return Err(format!(
            "OpenCode model selector {selector:?} must explicitly name a route as provider/model; use local/<model> or alibaba/qwen3.8-max"
        ));
    };
    let provider = provider.trim().to_ascii_lowercase();
    if provider.is_empty() || model.trim().is_empty() || model.contains('/') {
        return Err(format!(
            "OpenCode model selector {selector:?} must contain exactly one non-empty provider/model route"
        ));
    }
    match provider.as_str() {
        "local" => Ok(ServingRoute::Local),
        HOSTED_QWEN_PROVIDER | HOSTED_QWEN_CN_PROVIDER => Ok(ServingRoute::Hosted),
        _ => Err(format!(
            "OpenCode provider {provider:?} is not a supported route; choose explicit local/<model> or alibaba/qwen3.8-max"
        )),
    }
}

/// Return the documented hosted endpoint for a provider selector.
pub fn hosted_endpoint_for_provider(provider: &str) -> Result<&'static str, String> {
    match provider.trim().to_ascii_lowercase().as_str() {
        HOSTED_QWEN_PROVIDER => Ok(DASHSCOPE_INTL_ENDPOINT),
        HOSTED_QWEN_CN_PROVIDER => Ok(DASHSCOPE_CN_ENDPOINT),
        other => Err(format!(
            "OpenCode hosted provider {other:?} is unsupported; expected {HOSTED_QWEN_PROVIDER:?} or {HOSTED_QWEN_CN_PROVIDER:?}"
        )),
    }
}

/// Validate the hosted route's model and return its secret-free identity.
pub fn hosted_serving_identity(selector: &str) -> Result<ServingIdentity, String> {
    let Some((provider, model)) = selector.trim().split_once('/') else {
        return Err(format!(
            "hosted OpenCode selector {selector:?} must be provider/model"
        ));
    };
    let provider = provider.trim().to_ascii_lowercase();
    let model = model.trim();
    if !matches!(
        provider.as_str(),
        HOSTED_QWEN_PROVIDER | HOSTED_QWEN_CN_PROVIDER
    ) {
        return Err(format!(
            "hosted OpenCode provider {provider:?} is unsupported"
        ));
    }
    if model != HOSTED_QWEN_MODEL {
        return Err(format!(
            "hosted OpenCode provider {provider:?} currently supports model {HOSTED_QWEN_MODEL:?}; received {model:?}"
        ));
    }
    Ok(ServingIdentity {
        provider: provider.clone(),
        model: model.to_string(),
        endpoint: hosted_endpoint_for_provider(provider.as_str())?.to_string(),
    })
}

/// Validate a requested shared effort against Qwen3.8-Max's hosted variants.
/// The provider's compatibility layer maps OpenAI `high`/`minimal` values;
/// rejecting them here preserves the exact Cassy request in the spawn spec.
pub fn validate_hosted_effort(effort: Option<cas_mux::Effort>) -> Result<(), String> {
    let Some(effort) = effort else {
        return Ok(());
    };
    if HOSTED_QWEN_ACCEPTED_EFFORTS.contains(&effort) {
        return Ok(());
    }
    Err(format!(
        "hosted OpenCode model {HOSTED_QWEN_MODEL:?} rejects effort {effort}; accepted hosted efforts: [low, medium, xhigh]. No effort remapping is performed."
    ))
}

/// Probe the hosted DashScope route with a key supplied by the caller.
///
/// The probe lists models and performs a one-token completion.  Only status
/// classes and secret-free metadata are returned in errors; the key is sent
/// solely as an Authorization header and is never interpolated into output.
pub fn preflight_hosted_endpoint(
    selector: &str,
    api_key: Option<&str>,
    timeout: Duration,
) -> Result<HostedEndpointPreflight, String> {
    let identity = hosted_serving_identity(selector)?;
    preflight_hosted_endpoint_at(selector, &identity.endpoint, api_key, timeout)
}

/// Probe a hosted selector against an explicit OpenAI-compatible endpoint.
///
/// The explicit endpoint form supports workspace/region-specific DashScope
/// domains and makes the probe deterministic in tests.  The selector still
/// controls the route and model identity; this endpoint is never allowed to
/// change a hosted request into a local one.
pub fn preflight_hosted_endpoint_at(
    selector: &str,
    endpoint: &str,
    api_key: Option<&str>,
    timeout: Duration,
) -> Result<HostedEndpointPreflight, String> {
    let mut identity = hosted_serving_identity(selector)?;
    let key = api_key
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .ok_or_else(|| {
            format!(
                "OpenCode hosted route {selector:?} requires {DASHSCOPE_API_KEY_ENV}; live hosted conformance remains pending-key"
            )
        })?;
    if timeout.is_zero() {
        return Err(format!(
            "OpenCode hosted preflight for {selector:?} requires a positive timeout"
        ));
    }

    let endpoint_url = parse_provider_endpoint(endpoint)?;
    let display_endpoint = safe_endpoint_display(&endpoint_url);
    identity.endpoint = display_endpoint.clone();
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();
    let models_url = endpoint_path(&endpoint_url, "models");
    let response = agent
        .get(&models_url)
        .set("Authorization", &format!("Bearer {key}"))
        .call()
        .map_err(|error| {
            format!(
                "OpenCode hosted preflight could not validate {HOSTED_QWEN_PROVIDER} authentication at {display_endpoint}: {} (check {DASHSCOPE_API_KEY_ENV} and network access)",
                hosted_transport_error_class(&error)
            )
        })?;
    let payload: Value = response.into_json().map_err(|_| {
        format!(
            "OpenCode hosted preflight reached {display_endpoint}, but the model listing was not valid JSON"
        )
    })?;
    let loaded_models = model_ids(&payload);
    if !loaded_models.iter().any(|model| model == HOSTED_QWEN_MODEL) {
        let listed = if loaded_models.is_empty() {
            "none".to_string()
        } else {
            loaded_models.join(", ")
        };
        return Err(format!(
            "OpenCode hosted preflight authenticated at {display_endpoint}, but model {HOSTED_QWEN_MODEL:?} is not available; endpoint reports: {listed}"
        ));
    }

    let completions_url = endpoint_path(&endpoint_url, "chat/completions");
    let probe = serde_json::json!({
        "model": HOSTED_QWEN_MODEL,
        "messages": [{"role": "user", "content": "Reply with READY."}],
        "max_tokens": 1,
        "stream": false
    });
    let response = agent
        .post(&completions_url)
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", "application/json")
        .send_json(probe)
        .map_err(|error| {
            format!(
                "OpenCode hosted preflight found model {HOSTED_QWEN_MODEL:?}, but its answer probe failed at {display_endpoint}: {}",
                hosted_transport_error_class(&error)
            )
        })?;
    let answer: Value = response.into_json().map_err(|_| {
        format!(
            "OpenCode hosted preflight found model {HOSTED_QWEN_MODEL:?}, but its answer probe returned invalid JSON"
        )
    })?;
    if !answer
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|choices| !choices.is_empty())
    {
        return Err(format!(
            "OpenCode hosted preflight found model {HOSTED_QWEN_MODEL:?}, but its answer probe returned no choices"
        ));
    }

    Ok(HostedEndpointPreflight {
        route: ServingRoute::Hosted,
        serving_identity: identity,
        loaded_models,
        accepted_efforts: HOSTED_QWEN_ACCEPTED_EFFORTS
            .iter()
            .map(ToString::to_string)
            .collect(),
        authenticated: true,
        answerable: true,
    })
}

/// Probe the configured hosted key without exposing it to callers.
pub fn preflight_hosted_from_env(
    selector: &str,
    timeout: Duration,
) -> Result<HostedEndpointPreflight, String> {
    preflight_hosted_endpoint(
        selector,
        std::env::var(DASHSCOPE_API_KEY_ENV).ok().as_deref(),
        timeout,
    )
}

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

fn parse_provider_endpoint(raw: &str) -> Result<Url, String> {
    let trimmed = raw.trim();
    let url = Url::parse(trimmed)
        .map_err(|error| format!("OpenCode hosted provider endpoint is invalid: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("OpenCode hosted provider endpoint must be an http(s) URL".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("OpenCode hosted provider endpoint must not contain credentials".to_string());
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

fn hosted_transport_error_class(error: &ureq::Error) -> &'static str {
    match error {
        ureq::Error::Status(code, _) => match *code {
            401 | 403 => "DASHSCOPE_API_KEY was rejected",
            404 => "the hosted provider endpoint returned not found",
            500..=599 => "the hosted provider returned a server error",
            _ => "the hosted provider returned an HTTP error",
        },
        ureq::Error::Transport(_) => "the hosted provider could not be reached or timed out",
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
        respond_body(stream, body);
    }

    fn respond_body(mut stream: TcpStream, body: &str) {
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

    #[test]
    fn hosted_selectors_are_explicit_and_route_specific() {
        assert_eq!(
            opencode_route_for_selector("local/qwen3.8"),
            Ok(ServingRoute::Local)
        );
        assert_eq!(
            opencode_route_for_selector("alibaba/qwen3.8-max"),
            Ok(ServingRoute::Hosted)
        );
        assert_eq!(
            opencode_route_for_selector("alibaba-cn/qwen3.8-max"),
            Ok(ServingRoute::Hosted)
        );
        let error = opencode_route_for_selector("qwen3.8-max").unwrap_err();
        assert!(error.contains("provider/model"), "{error}");
        let error = opencode_route_for_selector("cloud/qwen3.8-max").unwrap_err();
        assert!(error.contains("supported route"), "{error}");
    }

    #[test]
    fn hosted_qwen_effort_table_rejects_openai_compatibility_remaps() {
        for effort in HOSTED_QWEN_ACCEPTED_EFFORTS {
            assert!(validate_hosted_effort(Some(effort)).is_ok());
        }
        for effort in [cas_mux::Effort::Minimal, cas_mux::Effort::High] {
            let error = validate_hosted_effort(Some(effort)).unwrap_err();
            assert!(error.contains("accepted hosted efforts: [low, medium, xhigh]"));
            assert!(error.contains("No effort remapping"));
        }
        assert!(validate_hosted_effort(None).is_ok());
    }

    #[test]
    fn hosted_preflight_proves_auth_and_answerability_without_receipt_secret() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (requests_tx, requests_rx) = std::sync::mpsc::channel();
        let thread = thread::spawn(move || {
            for body in [
                r#"{"object":"list","data":[{"id":"qwen3.8-max"}]}"#,
                r#"{"choices":[{"message":{"content":"READY"}}]}"#,
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                let size = stream.read(&mut request).unwrap();
                requests_tx
                    .send(String::from_utf8_lossy(&request[..size]).into_owned())
                    .unwrap();
                respond_body(stream, body);
            }
        });

        let result = preflight_hosted_endpoint_at(
            "alibaba/qwen3.8-max",
            &endpoint,
            Some("sk-hosted-secret"),
            Duration::from_secs(2),
        )
        .unwrap();
        thread.join().unwrap();
        let requests: Vec<_> = requests_rx.into_iter().collect();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("Authorization: Bearer sk-hosted-secret"));
        assert!(requests[1].contains("Authorization: Bearer sk-hosted-secret"));
        assert_eq!(result.route, ServingRoute::Hosted);
        assert_eq!(result.serving_identity.provider, "alibaba");
        assert_eq!(result.serving_identity.model, HOSTED_QWEN_MODEL);
        assert_eq!(result.serving_identity.endpoint, format!("{endpoint}/"));
        assert!(result.authenticated && result.answerable);
        assert_eq!(
            result.accepted_efforts,
            vec!["low".to_string(), "medium".to_string(), "xhigh".to_string()]
        );
        let receipt = serde_json::to_string(&result).unwrap();
        assert!(!receipt.contains("sk-hosted-secret"));
    }

    #[test]
    fn hosted_preflight_without_key_is_pending_and_secret_safe() {
        let error =
            preflight_hosted_endpoint("alibaba/qwen3.8-max", Some("  "), Duration::from_secs(1))
                .unwrap_err();
        assert!(error.contains(DASHSCOPE_API_KEY_ENV), "{error}");
        assert!(error.contains("pending-key"), "{error}");
        assert!(!error.contains("Bearer"), "{error}");
    }
}
