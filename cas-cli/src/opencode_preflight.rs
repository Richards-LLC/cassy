//! Bounded preflight for OpenCode's local and hosted OpenAI-compatible routes.
//!
//! OpenCode's provider configuration is owned by the factory policy layer, so
//! this module accepts explicit endpoint/model values and has no dependency on
//! a particular TOML shape.  The environment names are only an interim input
//! adapter for callers that have not yet resolved project configuration.
//!
//! Local and pay-as-you-go probes list models, then send a one-token chat
//! completion. The Token Plan probe is intentionally smaller: the completion
//! itself is the remote auth/answerability check, and no model-list request is
//! made. In particular, the Token Plan Anthropic endpoint has no model-list
//! route and must never be probed here. No probe persists response data or
//! includes response bodies in errors.

use std::time::Duration;

use cas_pty::{ServingIdentity, ServingRoute};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

/// Interim environment input for the local OpenCode provider base URL.
pub const LOCAL_ENDPOINT_ENV: &str = "CAS_OPENCODE_LOCAL_ENDPOINT";

/// Interim environment input for the local OpenCode model selector.
pub const MODEL_ENV: &str = "CAS_OPENCODE_MODEL";

/// DashScope pay-as-you-go credentials. The value is read only at probe time
/// and is never copied into a receipt, generated OpenCode config, or an error.
pub const DASHSCOPE_API_KEY_ENV: &str = "DASHSCOPE_API_KEY";

/// QwenCloud Token Plan credentials. Token Plan keys are dedicated `sk-sp-`
/// keys and are not interchangeable with DashScope pay-as-you-go keys.
pub const QWENCLOUD_TOKEN_PLAN_API_KEY_ENV: &str = "QWENCLOUD_TOKEN_PLAN_API_KEY";

/// International DashScope OpenAI-compatible endpoint from Qwen Cloud's
/// current API documentation.
pub const DASHSCOPE_INTL_ENDPOINT: &str = "https://dashscope-intl.aliyuncs.com/compatible-mode/v1";

/// Mainland China DashScope OpenAI-compatible endpoint.  Region selection is
/// explicit through `alibaba-cn/...`; it is never inferred from a failed
/// international request.
pub const DASHSCOPE_CN_ENDPOINT: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";

/// QwenCloud Token Plan's OpenAI-compatible endpoint.
pub const QWENCLOUD_TOKEN_PLAN_ENDPOINT: &str =
    "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1";

/// Canonical OpenCode provider id for the operator's Token Plan route.
pub const QWENCLOUD_TOKEN_PLAN_PROVIDER: &str = "qwencloud";

/// Canonical OpenCode provider id for DashScope pay-as-you-go.
pub const HOSTED_PAYG_PROVIDER: &str = "alibaba";
/// Legacy T8 name retained for callers that used the DashScope provider.
pub const HOSTED_QWEN_PROVIDER: &str = HOSTED_PAYG_PROVIDER;
pub const HOSTED_QWEN_CN_PROVIDER: &str = "alibaba-cn";
pub const HOSTED_QWEN_MODEL: &str = "qwen3.8-max";

/// Alternate explicit spelling accepted for operator configuration. The
/// canonical selector remains `qwencloud/qwen3.8-max`.
pub const HOSTED_TOKEN_PLAN_PROVIDER: &str = "hosted-token-plan";

/// Alternate explicit spelling accepted for operator configuration.
pub const HOSTED_PAYG_SELECTOR_PROVIDER: &str = "hosted-payg";

/// Qwen3.8-Max's own reasoning variants.  These are intentionally separate
/// from the local server's probed effort set.
pub const HOSTED_PAYG_ACCEPTED_EFFORTS: [cas_mux::Effort; 3] = [
    cas_mux::Effort::Low,
    cas_mux::Effort::Medium,
    cas_mux::Effort::XHigh,
];

/// QwenCloud's Token Plan qwen3.8-max table. The values currently match the
/// pay-as-you-go model, but the wire contract is separate: Token Plan uses
/// OpenAI-compatible `enable_thinking` body configuration and must not inherit
/// a future pay-as-you-go remapping.
pub const HOSTED_TOKEN_PLAN_ACCEPTED_EFFORTS: [cas_mux::Effort; 3] = [
    cas_mux::Effort::Low,
    cas_mux::Effort::Medium,
    cas_mux::Effort::XHigh,
];

/// T8 compatibility alias for the DashScope hosted effort table.
pub const HOSTED_QWEN_ACCEPTED_EFFORTS: [cas_mux::Effort; 3] = HOSTED_PAYG_ACCEPTED_EFFORTS;

/// Explicit hosted billing lane selected by the provider/model selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostedLane {
    TokenPlan,
    Payg,
}

impl HostedLane {
    pub const fn route(self) -> ServingRoute {
        match self {
            Self::TokenPlan => ServingRoute::HostedTokenPlan,
            Self::Payg => ServingRoute::HostedPayg,
        }
    }

    pub const fn key_env(self) -> &'static str {
        match self {
            Self::TokenPlan => QWENCLOUD_TOKEN_PLAN_API_KEY_ENV,
            Self::Payg => DASHSCOPE_API_KEY_ENV,
        }
    }

    pub const fn endpoint(self) -> &'static str {
        match self {
            Self::TokenPlan => QWENCLOUD_TOKEN_PLAN_ENDPOINT,
            Self::Payg => DASHSCOPE_INTL_ENDPOINT,
        }
    }

    pub const fn accepted_efforts(self) -> &'static [cas_mux::Effort] {
        match self {
            Self::TokenPlan => &HOSTED_TOKEN_PLAN_ACCEPTED_EFFORTS,
            Self::Payg => &HOSTED_PAYG_ACCEPTED_EFFORTS,
        }
    }

    pub const fn uses_model_list(self) -> bool {
        matches!(self, Self::Payg)
    }

    pub const fn wire_contract(self) -> &'static str {
        match self {
            Self::TokenPlan => "extra_body.enable_thinking",
            Self::Payg => "reasoning_effort",
        }
    }
}

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

/// Resolve the route-stamped typed receipt that is allowed to back a factory
/// support claim. A live auth probe proves only that the endpoint answered;
/// it cannot substitute for the full PTY/session/lifecycle matrix.
pub fn support_claim_for_selector(selector: &str) -> Result<OpenCodeSupportClaim, String> {
    let route = opencode_route_for_selector(selector)?;
    let serving_identity = match route {
        ServingRoute::HostedTokenPlan | ServingRoute::HostedPayg => {
            hosted_serving_identity(selector)?
        }
        ServingRoute::Local => {
            let (provider, model) = selector.split_once('/').ok_or_else(|| {
                format!("local OpenCode selector {selector:?} must be provider/model")
            })?;
            let endpoint = std::env::var(LOCAL_ENDPOINT_ENV)
                .ok()
                .and_then(|raw| parse_endpoint(&raw).ok())
                .map(|url| safe_endpoint_display(&url))
                .unwrap_or_default();
            ServingIdentity {
                provider: provider.to_string(),
                model: model.to_string(),
                endpoint,
            }
        }
        ServingRoute::Hosted => {
            return Err(
                "legacy OpenCode hosted route has no billing identity and cannot carry a support claim"
                    .to_string(),
            );
        }
    };
    let supported = cas_pty::harness_conformance_receipts()
        .map_err(|error| format!("OpenCode conformance receipt could not be decoded: {error}"))?
        .into_iter()
        .any(|receipt| {
            receipt.harness == cas_pty::Harness::OpenCode
                && receipt.route == Some(route)
                && receipt.serving_identity.as_ref() == Some(&serving_identity)
                && receipt.validates_pin()
        });
    Ok(OpenCodeSupportClaim {
        route,
        serving_identity,
        status: if supported {
            SupportClaimStatus::Supported
        } else {
            SupportClaimStatus::PendingConformance
        },
    })
}

/// Fail closed before queue insertion when the selected route does not have a
/// matching passing typed receipt. This gate is deliberately independent of
/// credentials: a valid key and successful tiny completion are necessary but
/// do not prove the factory lifecycle.
pub fn require_supported_selector(selector: &str) -> Result<OpenCodeSupportClaim, String> {
    let claim = support_claim_for_selector(selector)?;
    if claim.status == SupportClaimStatus::Supported {
        return Ok(claim);
    }
    Err(format!(
        "OpenCode route {} for selector {selector:?} remains pending-conformance: no matching passing typed live receipt is embedded; factory spawn was not queued",
        match claim.route {
            ServingRoute::Local => "local",
            ServingRoute::HostedTokenPlan => "hosted-token-plan",
            ServingRoute::HostedPayg => "hosted-payg",
            ServingRoute::Hosted => "hosted",
        }
    ))
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
/// serving route. Requiring the provider prefix is important: a hosted
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
        QWENCLOUD_TOKEN_PLAN_PROVIDER | HOSTED_TOKEN_PLAN_PROVIDER => {
            Ok(ServingRoute::HostedTokenPlan)
        }
        HOSTED_PAYG_PROVIDER | HOSTED_QWEN_CN_PROVIDER | HOSTED_PAYG_SELECTOR_PROVIDER => {
            Ok(ServingRoute::HostedPayg)
        }
        _ => Err(format!(
            "OpenCode provider {provider:?} is not a supported route; choose explicit local/<model>, qwencloud/qwen3.8-max, or alibaba/qwen3.8-max"
        )),
    }
}

/// Return the explicit hosted billing lane selected by a provider/model
/// selector. A selector without a provider is rejected rather than inferred.
pub fn hosted_lane_for_selector(selector: &str) -> Result<HostedLane, String> {
    let Some((provider, _model)) = selector.trim().split_once('/') else {
        return Err(format!(
            "hosted OpenCode selector {selector:?} must explicitly name a lane as provider/model"
        ));
    };
    match provider.trim().to_ascii_lowercase().as_str() {
        QWENCLOUD_TOKEN_PLAN_PROVIDER | HOSTED_TOKEN_PLAN_PROVIDER => Ok(HostedLane::TokenPlan),
        HOSTED_PAYG_PROVIDER | HOSTED_QWEN_CN_PROVIDER | HOSTED_PAYG_SELECTOR_PROVIDER => {
            Ok(HostedLane::Payg)
        }
        other => Err(format!(
            "OpenCode hosted provider {other:?} is not a supported billing lane; choose {QWENCLOUD_TOKEN_PLAN_PROVIDER:?} for Token Plan or {HOSTED_PAYG_PROVIDER:?} for pay-as-you-go"
        )),
    }
}

/// Return the documented hosted endpoint for a provider selector.
pub fn hosted_endpoint_for_provider(provider: &str) -> Result<&'static str, String> {
    match provider.trim().to_ascii_lowercase().as_str() {
        QWENCLOUD_TOKEN_PLAN_PROVIDER | HOSTED_TOKEN_PLAN_PROVIDER => {
            Ok(QWENCLOUD_TOKEN_PLAN_ENDPOINT)
        }
        HOSTED_PAYG_PROVIDER | HOSTED_PAYG_SELECTOR_PROVIDER => Ok(DASHSCOPE_INTL_ENDPOINT),
        HOSTED_QWEN_CN_PROVIDER => Ok(DASHSCOPE_CN_ENDPOINT),
        other => Err(format!(
            "OpenCode hosted provider {other:?} is unsupported; expected {QWENCLOUD_TOKEN_PLAN_PROVIDER:?}, {HOSTED_PAYG_PROVIDER:?}, or {HOSTED_QWEN_CN_PROVIDER:?}"
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
    let lane = hosted_lane_for_selector(selector)?;
    if model != HOSTED_QWEN_MODEL {
        return Err(format!(
            "hosted OpenCode provider {provider:?} on the {} lane currently supports model {HOSTED_QWEN_MODEL:?}; received {model:?}",
            lane_name(lane)
        ));
    }
    let endpoint = hosted_endpoint_for_provider(&provider)?.to_string();
    Ok(ServingIdentity {
        provider,
        model: model.to_string(),
        endpoint,
    })
}

/// Validate a requested effort against the selected hosted lane's own table.
/// The provider compatibility layer may map OpenAI values; rejecting them here
/// preserves the exact Cassy request in the spawn spec.
pub fn validate_hosted_effort(effort: Option<cas_mux::Effort>) -> Result<(), String> {
    validate_hosted_effort_for_lane(HostedLane::Payg, effort)
}

pub fn validate_hosted_effort_for_lane(
    lane: HostedLane,
    effort: Option<cas_mux::Effort>,
) -> Result<(), String> {
    let Some(effort) = effort else {
        return Ok(());
    };
    if lane.accepted_efforts().contains(&effort) {
        return Ok(());
    }
    Err(format!(
        "hosted OpenCode {} lane model {HOSTED_QWEN_MODEL:?} rejects effort {effort}; accepted {} efforts: [{}] (accepted hosted efforts: [{}]). No effort remapping is performed.",
        lane_name(lane),
        lane_name(lane),
        format_efforts(lane.accepted_efforts()),
        format_efforts(lane.accepted_efforts())
    ))
}

fn lane_name(lane: HostedLane) -> &'static str {
    match lane {
        HostedLane::TokenPlan => "hosted-token-plan",
        HostedLane::Payg => "hosted-payg",
    }
}

fn format_efforts(efforts: &[cas_mux::Effort]) -> String {
    efforts
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Validate only the key's non-secret prefix against its explicit hosted lane.
/// This is intentionally separate from network probing so an accidental key
/// mix-up cannot consume a provider request or silently change billing.
pub fn validate_hosted_api_key(lane: HostedLane, api_key: Option<&str>) -> Result<(), String> {
    let key = api_key
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .ok_or_else(|| {
            format!(
                "{} lane requires {}; live hosted conformance remains pending-key",
                lane_name(lane),
                lane.key_env()
            )
        })?;
    let observed = key_prefix_class(key);
    let valid = match lane {
        HostedLane::TokenPlan => observed == "sk-sp-",
        HostedLane::Payg => matches!(observed, "sk-" | "sk-ws-"),
    };
    if valid {
        return Ok(());
    }
    let expected = match lane {
        HostedLane::TokenPlan => "sk-sp-",
        HostedLane::Payg => "sk- or sk-ws-",
    };
    Err(format!(
        "{} lane key prefix mismatch: expected {expected}, received {observed}; set {} for this lane. No secret was inspected or echoed.",
        lane_name(lane),
        lane.key_env()
    ))
}

fn key_prefix_class(key: &str) -> &'static str {
    if key.starts_with("sk-sp-") {
        "sk-sp-"
    } else if key.starts_with("sk-ws-") {
        "sk-ws-"
    } else if key.starts_with("sk-") {
        "sk-"
    } else {
        "unknown"
    }
}

/// Probe a hosted route with a key supplied by the caller.
///
/// Pay-as-you-go performs the T8 model-list plus one-token completion probe.
/// Token Plan performs only one-token completion: that request validates both
/// authentication and answerability, and deliberately avoids model discovery.
/// Only status classes and secret-free metadata are returned in errors; the key
/// is sent solely as an Authorization header and is never interpolated into
/// output.
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
    let lane = hosted_lane_for_selector(selector)?;
    let mut identity = hosted_serving_identity(selector)?;
    validate_hosted_api_key(lane, api_key)?;
    let key = api_key
        .expect("validated hosted API key must be present")
        .trim();
    if timeout.is_zero() {
        return Err(format!(
            "OpenCode hosted preflight for {selector:?} requires a positive timeout"
        ));
    }

    let endpoint_url = parse_provider_endpoint(endpoint)?;
    if endpoint_url.path().contains("/apps/anthropic") {
        return Err(format!(
            "{} lane preflight requires the OpenAI-compatible endpoint ending in /compatible-mode/v1; the /apps/anthropic endpoint has no model-list route and is not probed",
            lane_name(lane)
        ));
    }
    let display_endpoint = safe_endpoint_display(&endpoint_url);
    identity.endpoint = display_endpoint.clone();
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();
    let mut loaded_models = Vec::new();
    if lane.uses_model_list() {
        let models_url = endpoint_path(&endpoint_url, "models");
        let response = agent
            .get(&models_url)
            .set("Authorization", &format!("Bearer {key}"))
            .call()
            .map_err(|error| {
                format!(
                    "OpenCode {} preflight could not validate authentication at {display_endpoint}: {} (check {})",
                    lane_name(lane),
                    hosted_transport_error_class(&error),
                    lane.key_env()
                )
            })?;
        let payload: Value = response.into_json().map_err(|_| {
            format!(
                "OpenCode {} preflight reached {display_endpoint}, but the model listing was not valid JSON",
                lane_name(lane)
            )
        })?;
        loaded_models = model_ids(&payload);
        if !loaded_models.iter().any(|model| model == HOSTED_QWEN_MODEL) {
            let listed = if loaded_models.is_empty() {
                "none".to_string()
            } else {
                loaded_models.join(", ")
            };
            return Err(format!(
                "OpenCode {} preflight authenticated at {display_endpoint}, but model {HOSTED_QWEN_MODEL:?} is not available; endpoint reports: {listed}",
                lane_name(lane)
            ));
        }
    }

    let completions_url = endpoint_path(&endpoint_url, "chat/completions");
    let mut probe = serde_json::json!({
        "model": HOSTED_QWEN_MODEL,
        "messages": [{"role": "user", "content": "Reply with READY."}],
        "max_tokens": 1,
        "stream": false
    });
    if matches!(lane, HostedLane::TokenPlan) {
        // This is the direct HTTP equivalent of the SDK's extra_body. The
        // endpoint is OpenAI-compatible and QwenCloud uses this body flag to
        // pin the Token Plan thinking contract; no pay-as-you-go remapping is
        // inherited.
        probe["enable_thinking"] = Value::Bool(true);
    }
    let response = agent
        .post(&completions_url)
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", "application/json")
        .send_json(probe)
        .map_err(|error| {
            format!(
                "OpenCode {} preflight {} model {HOSTED_QWEN_MODEL:?}, but its answer probe failed at {display_endpoint}: {}",
                lane_name(lane),
                if lane.uses_model_list() {
                    "found"
                } else {
                    "authenticated for"
                },
                hosted_transport_error_class(&error)
            )
        })?;
    let answer: Value = response.into_json().map_err(|_| {
        format!(
            "OpenCode {} preflight answer probe for model {HOSTED_QWEN_MODEL:?} returned invalid JSON",
            lane_name(lane)
        )
    })?;
    if !answer
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|choices| !choices.is_empty())
    {
        return Err(format!(
            "OpenCode {} preflight answer probe for model {HOSTED_QWEN_MODEL:?} returned no choices",
            lane_name(lane)
        ));
    }

    Ok(HostedEndpointPreflight {
        route: lane.route(),
        serving_identity: identity,
        loaded_models,
        accepted_efforts: lane
            .accepted_efforts()
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
    let lane = hosted_lane_for_selector(selector)?;
    let key = std::env::var(lane.key_env()).ok();
    preflight_hosted_endpoint(selector, key.as_deref(), timeout)
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
            401 | 403 => "the hosted API key was rejected",
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
            Ok(ServingRoute::HostedPayg)
        );
        assert_eq!(
            opencode_route_for_selector("alibaba-cn/qwen3.8-max"),
            Ok(ServingRoute::HostedPayg)
        );
        assert_eq!(
            opencode_route_for_selector("qwencloud/qwen3.8-max"),
            Ok(ServingRoute::HostedTokenPlan)
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
            Some("sk-payg-test"),
            Duration::from_secs(2),
        )
        .unwrap();
        thread.join().unwrap();
        let requests: Vec<_> = requests_rx.into_iter().collect();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("Authorization: Bearer sk-payg-test"));
        assert!(requests[1].contains("Authorization: Bearer sk-payg-test"));
        assert_eq!(result.route, ServingRoute::HostedPayg);
        assert_eq!(result.serving_identity.provider, "alibaba");
        assert_eq!(result.serving_identity.model, HOSTED_QWEN_MODEL);
        assert_eq!(result.serving_identity.endpoint, format!("{endpoint}/"));
        assert!(result.authenticated && result.answerable);
        assert_eq!(
            result.accepted_efforts,
            vec!["low".to_string(), "medium".to_string(), "xhigh".to_string()]
        );
        let receipt = serde_json::to_string(&result).unwrap();
        assert!(!receipt.contains("sk-payg-test"));
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

    #[test]
    fn hosted_lanes_refuse_cross_billing_key_prefixes_without_secret_echo() {
        assert!(validate_hosted_api_key(HostedLane::TokenPlan, Some("sk-sp-token")).is_ok());
        assert!(validate_hosted_api_key(HostedLane::Payg, Some("sk-payg-token")).is_ok());
        assert!(validate_hosted_api_key(HostedLane::Payg, Some("sk-ws-token")).is_ok());

        let token_error =
            validate_hosted_api_key(HostedLane::TokenPlan, Some("sk-payg-mismatch")).unwrap_err();
        assert!(token_error.contains("hosted-token-plan"), "{token_error}");
        assert!(token_error.contains("sk-sp-"), "{token_error}");
        assert!(token_error.contains("sk-"), "{token_error}");
        assert!(
            !token_error.contains("sk-payg-mismatch"),
            "key leaked: {token_error}"
        );

        let payg_error =
            validate_hosted_api_key(HostedLane::Payg, Some("sk-sp-mismatch")).unwrap_err();
        assert!(payg_error.contains("hosted-payg"), "{payg_error}");
        assert!(payg_error.contains("sk-sp-"), "{payg_error}");
        assert!(payg_error.contains("sk- or sk-ws-"), "{payg_error}");
        assert!(
            !payg_error.contains("sk-sp-mismatch"),
            "key leaked: {payg_error}"
        );
    }

    #[test]
    fn token_plan_preflight_is_one_openai_completion_without_model_listing() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (request_tx, request_rx) = std::sync::mpsc::channel();
        let thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let size = stream.read(&mut buffer).unwrap();
                if size == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..size]);
                let Some(header_end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("Content-Length: "))
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0);
                if request.len() >= header_end + 4 + content_length {
                    break;
                }
            }
            request_tx
                .send(String::from_utf8_lossy(&request).into_owned())
                .unwrap();
            respond_body(stream, r#"{"choices":[{"message":{"content":"READY"}}]}"#);
        });

        let result = preflight_hosted_endpoint_at(
            "qwencloud/qwen3.8-max",
            &endpoint,
            Some("sk-sp-token-plan-test"),
            Duration::from_secs(2),
        )
        .unwrap();
        thread.join().unwrap();
        let request = request_rx.recv().unwrap();
        assert_eq!(request.matches("HTTP/1.1").count(), 1);
        assert!(
            request.starts_with("POST /chat/completions HTTP/1.1"),
            "{request}"
        );
        assert!(
            !request.contains("/models"),
            "Token Plan must not list models: {request}"
        );
        assert!(request.contains("enable_thinking"), "{request}");
        assert_eq!(result.route, ServingRoute::HostedTokenPlan);
        assert!(result.loaded_models.is_empty());
        assert!(result.authenticated && result.answerable);
        let receipt = serde_json::to_string(&result).unwrap();
        assert!(!receipt.contains("sk-sp-token-plan-test"));
    }

    #[test]
    fn token_plan_anthropic_endpoint_is_rejected_before_any_probe() {
        let error = preflight_hosted_endpoint_at(
            "qwencloud/qwen3.8-max",
            "https://token-plan.ap-southeast-1.maas.aliyuncs.com/apps/anthropic",
            Some("sk-sp-token"),
            Duration::from_secs(2),
        )
        .unwrap_err();
        assert!(error.contains("OpenAI-compatible"), "{error}");
        assert!(error.contains("/apps/anthropic"), "{error}");
        assert!(!error.contains("sk-sp-token"), "secret leaked: {error}");
    }

    #[test]
    fn support_claim_gate_accepts_only_the_receipted_token_plan_route() {
        let token_plan = support_claim_for_selector("qwencloud/qwen3.8-max").unwrap();
        assert_eq!(token_plan.route, ServingRoute::HostedTokenPlan);
        assert_eq!(token_plan.status, SupportClaimStatus::Supported);
        assert!(require_supported_selector("qwencloud/qwen3.8-max").is_ok());

        let payg = support_claim_for_selector("alibaba/qwen3.8-max").unwrap();
        assert_eq!(payg.route, ServingRoute::HostedPayg);
        assert_eq!(payg.status, SupportClaimStatus::PendingConformance);
        let error = require_supported_selector("alibaba/qwen3.8-max").unwrap_err();
        assert!(error.contains("hosted-payg"), "{error}");
        assert!(error.contains("pending-conformance"), "{error}");
        assert!(error.contains("was not queued"), "{error}");

        let local = support_claim_for_selector("local/qwen3.8").unwrap();
        assert_eq!(local.route, ServingRoute::Local);
        assert_eq!(local.status, SupportClaimStatus::PendingConformance);
        assert!(require_supported_selector("local/qwen3.8").is_err());
    }
}
