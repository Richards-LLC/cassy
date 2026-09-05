//! Static provider and lane routing policy.
//!
//! The registry in `policy/lane-registry.toml` is deliberately limited to
//! shipped policy: route recipes, lane ordering, and protected suspensions.
//! Runtime availability and conformance evidence belong in
//! [`CapabilitySnapshot`] and are not inferred from this file.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use cas_mux::{Effort, SupervisorCli, WorkerSpec};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The embedded, operator-reviewed static routing policy.
pub const LANE_REGISTRY_TOML: &str = include_str!("../policy/lane-registry.toml");

/// Compatibility alias for callers that describe the artifact as a route
/// table rather than a lane registry.
pub const REGISTRY_TOML: &str = LANE_REGISTRY_TOML;

/// A complete serving route identity.
///
/// Harness, provider, endpoint, model, and account profile are all part of the
/// key. In particular, a Qwen Token Plan route and a DashScope route must not
/// share evidence merely because they use the same harness and model.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RouteIdentity {
    pub harness: String,
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub account_profile: String,
}

impl RouteIdentity {
    pub fn new(
        harness: impl Into<String>,
        provider: impl Into<String>,
        endpoint: impl Into<String>,
        model: impl Into<String>,
        account_profile: impl Into<String>,
    ) -> Self {
        Self {
            harness: harness.into(),
            provider: provider.into(),
            endpoint: endpoint.into(),
            model: model.into(),
            account_profile: account_profile.into(),
        }
    }

    /// Stable, secret-free display key for logs and evidence references.
    pub fn key(&self) -> String {
        [
            self.harness.as_str(),
            self.provider.as_str(),
            self.endpoint.as_str(),
            self.model.as_str(),
            self.account_profile.as_str(),
        ]
        .join("|")
    }
}

/// Tri-state capability fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum CapabilityAvailability {
    Available,
    Unavailable,
    Unknown,
}

impl CapabilityAvailability {
    pub const fn default_ttl_ms(self) -> u64 {
        match self {
            Self::Available => CAPABILITY_AVAILABLE_TTL_MS,
            Self::Unavailable => CAPABILITY_UNAVAILABLE_TTL_MS,
            Self::Unknown => CAPABILITY_UNKNOWN_TTL_MS,
        }
    }
}

/// Default freshness windows for runtime capability evidence.
pub const CAPABILITY_AVAILABLE_TTL_MS: u64 = 5 * 60 * 1_000;
pub const CAPABILITY_UNAVAILABLE_TTL_MS: u64 = 60 * 1_000;
pub const CAPABILITY_UNKNOWN_TTL_MS: u64 = 5 * 1_000;

/// One observation for one complete route identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityEvidence {
    pub availability: CapabilityAvailability,
    pub observed_at_ms: u64,
    pub ttl_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

impl CapabilityEvidence {
    pub fn new(availability: CapabilityAvailability, observed_at_ms: u64) -> Self {
        Self {
            availability,
            observed_at_ms,
            ttl_ms: availability.default_ttl_ms(),
            reason: None,
            remediation: None,
        }
    }

    pub fn with_ttl_ms(mut self, ttl_ms: u64) -> Self {
        self.ttl_ms = ttl_ms;
        self
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    pub fn with_remediation(mut self, remediation: impl Into<String>) -> Self {
        self.remediation = Some(remediation.into());
        self
    }

    pub fn expires_at_ms(&self) -> u64 {
        self.observed_at_ms.saturating_add(self.ttl_ms)
    }

    pub fn is_stale_at(&self, now_ms: u64) -> bool {
        now_ms >= self.expires_at_ms()
    }

    /// Expired evidence is explicitly Unknown. Callers must not route from a
    /// stale Available or Unavailable observation.
    pub fn availability_at(&self, now_ms: u64) -> CapabilityAvailability {
        if self.is_stale_at(now_ms) {
            CapabilityAvailability::Unknown
        } else {
            self.availability
        }
    }
}

/// The externally useful projection of a route observation at a point in
/// time. `stale` stays visible even though stale evidence is classified as
/// `Unknown`, so doctor/preflight can explain why a prior verdict was dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityStatus {
    pub availability: CapabilityAvailability,
    pub stale: bool,
    pub observed_at_ms: u64,
    pub expires_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

/// Runtime capability evidence keyed by the complete route identity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilitySnapshot {
    pub availability: BTreeMap<RouteIdentity, CapabilityEvidence>,
}

impl CapabilitySnapshot {
    pub fn record(&mut self, identity: RouteIdentity, evidence: CapabilityEvidence) {
        self.availability.insert(identity, evidence);
    }

    pub fn get(&self, identity: &RouteIdentity) -> Option<&CapabilityEvidence> {
        self.availability.get(identity)
    }

    pub fn status_at(&self, identity: &RouteIdentity, now_ms: u64) -> Option<CapabilityStatus> {
        let evidence = self.get(identity)?;
        let stale = evidence.is_stale_at(now_ms);
        Some(CapabilityStatus {
            availability: evidence.availability_at(now_ms),
            stale,
            observed_at_ms: evidence.observed_at_ms,
            expires_at_ms: evidence.expires_at_ms(),
            reason: evidence.reason.clone(),
            remediation: evidence.remediation.clone(),
        })
    }

    pub fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis() as u64)
    }
}

/// Errors from parsing, validating, or using the static routing policy.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RoutingError {
    #[error("invalid lane registry: {0}")]
    Registry(String),

    #[error("unknown lane {0:?}")]
    UnknownLane(String),

    #[error("lane {lane:?} has no active recipe candidates")]
    NoActiveRecipe { lane: String },

    #[error("lane {lane:?} references unknown recipe or lane {reference:?}")]
    UnknownReference { lane: String, reference: String },

    #[error("lane fallback cycle detected: {0}")]
    FallbackCycle(String),

    #[error("routing policy violation: {0}")]
    Policy(String),
}

/// Static route status. This is policy, not a mutable support claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecipeStatus {
    Active,
    Suspended,
}

/// A typed route recipe from the embedded registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteRecipe {
    pub harness: SupervisorCli,
    pub provider: String,
    pub model: String,
    /// The default effort used when this recipe is selected by a lane.
    /// `effort` is accepted as a compatibility alias for early registry
    /// drafts, while the typed API exposes the unambiguous name.
    #[serde(alias = "effort", alias = "default")]
    pub default_effort: Effort,
    pub allowed_efforts: Vec<Effort>,
    pub status: RecipeStatus,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub required_capability: Option<String>,
}

/// Public short name for callers that use “recipe” as the domain term.
pub type Recipe = RouteRecipe;

/// A lane's ordered static candidates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lane {
    pub candidates: Vec<String>,
    /// Optional explicit fallback references. `candidates` remains the
    /// canonical ordered list; these fields allow future policy revisions to
    /// name fallback edges without making the parser permissive.
    #[serde(default)]
    pub fallback: Option<String>,
    #[serde(default)]
    pub fallbacks: Vec<String>,
    /// A later capability-aware resolver must not substitute a candidate for
    /// this lane when this flag is set.
    #[serde(default)]
    pub no_fallback: bool,
}

/// Public alias for callers that use “lane definition” as the domain term.
pub type LaneDefinition = Lane;

/// Per-harness policy defaults. These retain the existing spawn defaults,
/// independently of the lane recipes (including Fable/medium for taste).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerDefaults {
    pub model: String,
    pub effort: Effort,
}

/// The complete typed static registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneRegistry {
    pub schema_version: u32,
    #[serde(default)]
    pub defaults: BTreeMap<String, WorkerDefaults>,
    pub recipes: BTreeMap<String, RouteRecipe>,
    pub lanes: BTreeMap<String, Lane>,
}

/// Decision returned by [`resolve_lane`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub lane: String,
    pub recipe_id: String,
    pub spec: WorkerSpec,
    pub warnings: Vec<String>,
}

/// Resolve one registry lane for each requested worker slot.
///
/// Keeping the slot expansion here means MCP and direct CLI callers use the
/// same lane spelling, registry lookup, and capability-aware decision seam.
/// The returned decisions are intentionally independent values: later callers
/// may attach worker names and account directories without mutating the
/// registry result or another slot's decision.
pub fn resolve_lane_specs(
    lane: &str,
    slots: usize,
    snapshot: &CapabilitySnapshot,
) -> Result<Vec<RoutingDecision>, RoutingError> {
    let decision = resolve_lane(lane, snapshot)?;
    Ok((0..slots).map(|_| decision.clone()).collect())
}

/// Reject mixing registry lane mode with explicit recipe controls.
///
/// A lane is an operator-level request for the registry to choose the whole
/// route. Accepting one explicit control alongside it would create a hybrid
/// recipe whose fallback and warning semantics are ambiguous. This validator
/// is deliberately independent of capability facts so every request surface
/// fails closed before queueing or launching.
pub fn validate_lane_request(
    lane: &str,
    cli_explicit: bool,
    model_explicit: bool,
    effort_explicit: bool,
) -> Result<(), RoutingError> {
    if lane.trim().is_empty() {
        return Err(RoutingError::UnknownLane(lane.to_string()));
    }
    if cli_explicit || model_explicit || effort_explicit {
        let mut fields = Vec::new();
        if cli_explicit {
            fields.push("cli");
        }
        if model_explicit {
            fields.push("model");
        }
        if effort_explicit {
            fields.push("effort");
        }
        return Err(RoutingError::Policy(format!(
            "lane={:?} cannot be combined with explicit {} recipe field(s); choose lane= or an explicit cli/model/effort recipe",
            lane.trim(),
            fields.join(", ")
        )));
    }
    Ok(())
}

/// Parse and semantically validate a registry document.
pub fn parse_registry(source: &str) -> Result<LaneRegistry, RoutingError> {
    let registry: LaneRegistry = toml::from_str(source)
        .map_err(|error| RoutingError::Registry(format!("TOML parse failed: {error}")))?;
    validate_registry(&registry)?;
    Ok(registry)
}

/// Validate schema version, route recipes, lane references, and fallback
/// cycles. Unknown TOML fields are rejected by `deny_unknown_fields` during
/// [`parse_registry`].
pub fn validate_registry(registry: &LaneRegistry) -> Result<(), RoutingError> {
    if registry.schema_version != 1 {
        return Err(RoutingError::Registry(format!(
            "unsupported schema_version {}; expected 1",
            registry.schema_version
        )));
    }
    if registry.recipes.is_empty() {
        return Err(RoutingError::Registry(
            "registry must declare at least one recipe".to_string(),
        ));
    }
    if registry.lanes.is_empty() {
        return Err(RoutingError::Registry(
            "registry must declare at least one lane".to_string(),
        ));
    }

    if let Some(name) = registry
        .recipes
        .keys()
        .find(|name| registry.lanes.contains_key(*name))
    {
        return Err(RoutingError::Registry(format!(
            "route recipe and lane IDs must be unique; {name:?} is declared in both"
        )));
    }

    for (name, defaults) in &registry.defaults {
        let cli = parse_cli_key(name)
            .ok_or_else(|| RoutingError::Registry(format!("unknown defaults harness {name:?}")))?;
        if defaults.model.trim().is_empty() {
            return Err(RoutingError::Registry(format!(
                "defaults for {} has an empty model",
                cli.backend().name()
            )));
        }
    }

    for (name, recipe) in &registry.recipes {
        if name.trim().is_empty() || name.chars().any(char::is_whitespace) {
            return Err(RoutingError::Registry(format!(
                "recipe ID {name:?} must be non-empty and contain no whitespace"
            )));
        }
        if recipe.provider.trim().is_empty() {
            return Err(RoutingError::Registry(format!(
                "recipe {name:?} has an empty provider"
            )));
        }
        if recipe.model.trim().is_empty() {
            return Err(RoutingError::Registry(format!(
                "recipe {name:?} has an empty model"
            )));
        }
        if recipe.harness == SupervisorCli::Claude
            && let Err(reason) = validate_model_slug(SupervisorCli::Claude, &recipe.model)
        {
            return Err(RoutingError::Registry(format!(
                "recipe {name:?} has an unrecognized Claude model: {reason}"
            )));
        }
        if recipe.allowed_efforts.is_empty() {
            return Err(RoutingError::Registry(format!(
                "recipe {name:?} must declare at least one allowed effort"
            )));
        }
        let mut efforts = Vec::new();
        for effort in &recipe.allowed_efforts {
            if efforts.contains(effort) {
                return Err(RoutingError::Registry(format!(
                    "recipe {name:?} repeats allowed effort {effort}"
                )));
            }
            efforts.push(*effort);
        }
        if !efforts.contains(&recipe.default_effort) {
            return Err(RoutingError::Registry(format!(
                "recipe {name:?} default effort {} is not in allowed_efforts",
                recipe.default_effort
            )));
        }
        if recipe.status == RecipeStatus::Suspended
            && recipe
                .reason
                .as_deref()
                .is_none_or(|reason| reason.trim().is_empty())
        {
            return Err(RoutingError::Registry(format!(
                "suspended recipe {name:?} must include a non-empty reason"
            )));
        }
        if recipe
            .required_capability
            .as_deref()
            .is_some_and(|capability| capability.trim().is_empty())
        {
            return Err(RoutingError::Registry(format!(
                "recipe {name:?} has an empty required_capability"
            )));
        }
    }

    for (name, lane) in &registry.lanes {
        if name.trim().is_empty() || name.chars().any(char::is_whitespace) {
            return Err(RoutingError::Registry(format!(
                "lane ID {name:?} must be non-empty and contain no whitespace"
            )));
        }
        let references = lane_references(lane);
        if references.is_empty() {
            return Err(RoutingError::Registry(format!(
                "lane {name:?} must declare at least one candidate"
            )));
        }
        let mut seen = BTreeSet::new();
        for reference in references {
            if !seen.insert(reference.clone()) {
                return Err(RoutingError::Registry(format!(
                    "lane {name:?} repeats candidate {reference:?}"
                )));
            }
        }
    }

    for lane_name in registry.lanes.keys() {
        let mut stack = Vec::new();
        let mut active = BTreeSet::new();
        validate_lane_references(registry, lane_name, &mut stack, &mut active)?;
    }
    Ok(())
}

fn parse_cli_key(value: &str) -> Option<SupervisorCli> {
    value.parse().ok()
}

fn cli_key(cli: SupervisorCli) -> &'static str {
    match cli {
        SupervisorCli::Claude => "claude",
        SupervisorCli::Codex => "codex",
        SupervisorCli::Grok => "grok",
        SupervisorCli::OpenCode => "opencode",
    }
}

fn lane_references(lane: &Lane) -> Vec<String> {
    let mut references = lane.candidates.clone();
    if let Some(fallback) = &lane.fallback {
        references.push(fallback.clone());
    }
    references.extend(lane.fallbacks.iter().cloned());
    references
}

fn validate_lane_references(
    registry: &LaneRegistry,
    lane_name: &str,
    stack: &mut Vec<String>,
    active: &mut BTreeSet<String>,
) -> Result<(), RoutingError> {
    if !active.insert(lane_name.to_string()) {
        let start = stack
            .iter()
            .position(|name| name == lane_name)
            .unwrap_or_default();
        let mut cycle = stack[start..].to_vec();
        cycle.push(lane_name.to_string());
        return Err(RoutingError::FallbackCycle(cycle.join(" -> ")));
    }
    stack.push(lane_name.to_string());
    let lane = registry
        .lanes
        .get(lane_name)
        .expect("lane name came from registry map");
    for reference in lane_references(lane) {
        if registry.recipes.contains_key(&reference) {
            continue;
        }
        if registry.lanes.contains_key(&reference) {
            validate_lane_references(registry, &reference, stack, active)?;
            continue;
        }
        return Err(RoutingError::UnknownReference {
            lane: lane_name.to_string(),
            reference,
        });
    }
    stack.pop();
    active.remove(lane_name);
    Ok(())
}

fn flattened_candidates(registry: &LaneRegistry, lane_name: &str, output: &mut Vec<String>) {
    let lane = registry
        .lanes
        .get(lane_name)
        .expect("lane was semantically validated");
    for reference in lane_references(lane) {
        if registry.recipes.contains_key(&reference) {
            output.push(reference);
        } else {
            flattened_candidates(registry, &reference, output);
        }
    }
}

/// Access the embedded registry. Parsing occurs once per process; a malformed
/// shipped registry is therefore a fatal configuration error, not silently
/// ignored or reparsed differently by separate callers.
pub fn registry() -> Result<&'static LaneRegistry, RoutingError> {
    static REGISTRY: OnceLock<Result<LaneRegistry, RoutingError>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| parse_registry(LANE_REGISTRY_TOML))
        .as_ref()
        .map_err(Clone::clone)
}

/// Compatibility name for consumers that prefer an explicit embedded-registry
/// operation.
pub fn embedded_registry() -> Result<&'static LaneRegistry, RoutingError> {
    registry()
}

/// Return every harness represented by the embedded registry.
///
/// Defaults and recipes are both included because a harness may be present
/// before it has a lane recipe (Grok is currently in that state). Keeping this
/// catalog derived from the registry prevents doctor and preflight from
/// silently growing separate hard-coded harness lists.
pub fn registered_harnesses() -> Result<Vec<SupervisorCli>, RoutingError> {
    let registry = registry()?;
    let mut harnesses = BTreeSet::new();
    harnesses.extend(
        registry
            .defaults
            .keys()
            .filter_map(|key| parse_cli_key(key)),
    );
    harnesses.extend(registry.recipes.values().map(|recipe| recipe.harness));
    Ok(harnesses.into_iter().collect())
}

/// Build the canonical capability identity for a registry recipe.
///
/// The provider adapters live in `cas-cli`, while this crate owns the
/// registry and must remain usable by the MCP/CLI crates. Keep this mapping
/// here in lockstep with `cas_cli::capability::harness_route_identity` so the
/// snapshot lookup uses all route dimensions without making the factory crate
/// depend on a concrete probe implementation.
pub fn recipe_route_identity(recipe: &RouteRecipe, account_profile: &str) -> RouteIdentity {
    let (harness, endpoint) = match recipe.harness {
        SupervisorCli::Claude => ("claude", "https://api.anthropic.com"),
        SupervisorCli::Codex => ("codex", "https://api.openai.com"),
        SupervisorCli::Grok => ("grok", "https://api.x.ai"),
        SupervisorCli::OpenCode => (
            "opencode",
            "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
        ),
    };
    RouteIdentity::new(
        harness,
        recipe.provider.clone(),
        endpoint,
        recipe.model.clone(),
        account_profile,
    )
}

/// Resolve a lane from a caller-provided registry and capability snapshot.
///
/// This is the deep routing seam: callers provide immutable policy and live
/// evidence, while candidate ordering, suspension handling, tri-state
/// freshness, and warning text stay in one implementation. The embedded
/// [`resolve_lane`] wrapper below is the normal production entry point.
pub fn resolve_lane_from_registry(
    lane: &str,
    snapshot: &CapabilitySnapshot,
    registry: &LaneRegistry,
) -> Result<RoutingDecision, RoutingError> {
    let lane_name = lane.trim();
    let lane_definition = registry
        .lanes
        .get(lane_name)
        .ok_or_else(|| RoutingError::UnknownLane(lane.to_string()))?;
    let mut candidates = Vec::new();
    flattened_candidates(registry, lane_name, &mut candidates);
    let primary_recipe_id = candidates.first().cloned();
    let now_ms = CapabilitySnapshot::now_ms();
    let mut skipped = Vec::new();

    for (index, recipe_id) in candidates.into_iter().enumerate() {
        let recipe = &registry.recipes[&recipe_id];
        if recipe.status != RecipeStatus::Active {
            let reason = recipe
                .reason
                .as_deref()
                .unwrap_or("recipe is suspended")
                .trim();
            skipped.push(format!("recipe {recipe_id:?} is suspended ({reason})"));
            if lane_definition.no_fallback {
                break;
            }
            continue;
        }

        let identity = recipe_route_identity(recipe, "default");
        let status = snapshot.status_at(&identity, now_ms);
        let availability = status
            .as_ref()
            .map_or(CapabilityAvailability::Unknown, |status| {
                status.availability
            });
        match availability {
            CapabilityAvailability::Available => {
                let warning = if skipped.is_empty() {
                    Vec::new()
                } else {
                    vec![format!(
                        "lane={lane_name:?} selected fallback recipe {recipe_id:?} instead of primary recipe {:?}: {}",
                        primary_recipe_id.as_deref().unwrap_or("unknown"),
                        skipped.join("; ")
                    )]
                };
                return Ok(routing_decision(lane_name, recipe_id, recipe, warning));
            }
            CapabilityAvailability::Unavailable => {
                let reason = status
                    .as_ref()
                    .and_then(|status| status.reason.as_deref())
                    .unwrap_or("capability is unavailable");
                skipped.push(format!("recipe {recipe_id:?} is unavailable ({reason})"));
                if lane_definition.no_fallback {
                    break;
                }
            }
            CapabilityAvailability::Unknown => {
                if index == 0 || lane_definition.no_fallback {
                    // An unknown primary is allowed only when no substitution
                    // is being attempted. Unknown evidence must never justify
                    // selecting a later fallback route.
                    return Ok(routing_decision(lane_name, recipe_id, recipe, Vec::new()));
                }
                return Err(RoutingError::Policy(format!(
                    "lane {lane_name:?} cannot select fallback recipe {recipe_id:?}: capability availability is Unknown; refusing fallback after {}",
                    skipped.join("; ")
                )));
            }
        }
    }

    Err(RoutingError::NoActiveRecipe {
        lane: lane_name.to_string(),
    })
}

fn routing_decision(
    lane: &str,
    recipe_id: String,
    recipe: &RouteRecipe,
    warnings: Vec<String>,
) -> RoutingDecision {
    let model = if recipe.harness == SupervisorCli::OpenCode && !recipe.model.contains('/') {
        format!("{}/{}", recipe.provider, recipe.model)
    } else {
        recipe.model.clone()
    };
    let spec = WorkerSpec {
        name: None,
        cli: recipe.harness,
        model: Some(model),
        effort: Some(recipe.default_effort),
        config_dir: None,
        requester_config_dir: None,
        requester_secure_storage_dir: None,
    };
    RoutingDecision {
        lane: lane.to_string(),
        recipe_id,
        spec,
        warnings,
    }
}

/// Resolve a lane's ordered candidates against capability evidence.
pub fn resolve_lane(
    lane: &str,
    snapshot: &CapabilitySnapshot,
) -> Result<RoutingDecision, RoutingError> {
    let registry = registry()?;
    resolve_lane_from_registry(lane, snapshot, registry)
}

/// Validate a resolved explicit worker recipe against static model, suspension,
/// and effort policy. Claude's CLI rejects family/version slugs such as
/// `opus-5`; reject those labels so a lane cannot look healthy in the registry
/// while dying during harness boot.
pub fn validate_explicit(
    spec: &WorkerSpec,
    _snapshot: &CapabilitySnapshot,
) -> Result<(), RoutingError> {
    let registry = registry()?;
    if let Some(model) = spec.model.as_deref() {
        if let Err(reason) = validate_model_slug(spec.cli, model) {
            return Err(RoutingError::Policy(policy_violation_with_alternatives(
                registry,
                reason,
                "harness model slug",
                Some(model),
            )));
        }
        if let Err(reason) = validate_model_is_active(model) {
            return Err(RoutingError::Policy(policy_violation_with_alternatives(
                registry,
                reason,
                "suspended recipe",
                Some(model),
            )));
        }
        if let Err(reason) = validate_model_effort_policy(model, spec.effort) {
            return Err(RoutingError::Policy(policy_violation_with_alternatives(
                registry,
                reason,
                "allowed effort",
                None,
            )));
        }
        if let Some((recipe_id, recipe)) = registry.recipes.iter().find(|(_, recipe)| {
            recipe.harness == spec.cli
                && (recipe.model.eq_ignore_ascii_case(model)
                    || format!("{}/{}", recipe.provider, recipe.model).eq_ignore_ascii_case(model))
        }) {
            if recipe.status != RecipeStatus::Active {
                return Err(RoutingError::Policy(policy_violation_with_alternatives(
                    registry,
                    format!(
                        "explicit recipe {recipe_id:?} is suspended ({})",
                        recipe.reason.as_deref().unwrap_or("no reason recorded")
                    ),
                    "recipe status",
                    Some(model),
                )));
            }
            if let Some(effort) = spec.effort
                && !recipe.allowed_efforts.contains(&effort)
            {
                let allowed = recipe
                    .allowed_efforts
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("|");
                return Err(RoutingError::Policy(policy_violation_with_alternatives(
                    registry,
                    format!(
                        "explicit recipe {recipe_id:?} rejects effort {effort}; allowed efforts are {allowed}"
                    ),
                    "recipe allowed efforts",
                    None,
                )));
            }
        }
    }
    Ok(())
}

/// Return whether a model uses a Claude Code-recognized model shape.
///
/// Claude Code accepts the short family aliases and canonical IDs whose
/// family is followed by one or more numeric version components, optionally
/// followed by its `[1m]` context suffix. Keep this shape-based check open to
/// new releases instead of enumerating today's IDs in a list that will go
/// stale on the next Claude Code release.
pub fn is_claude_model_slug(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    if matches!(normalized.as_str(), "opus" | "sonnet" | "haiku") {
        return true;
    }

    let canonical = normalized.strip_suffix("[1m]").unwrap_or(&normalized);
    let Some(versioned) = canonical.strip_prefix("claude-") else {
        return false;
    };
    let mut components = versioned.split('-');
    let Some(family) = components.next() else {
        return false;
    };
    if !matches!(family, "opus" | "sonnet" | "haiku" | "fable" | "mythos") {
        return false;
    }

    let version_components: Vec<_> = components.collect();
    !version_components.is_empty()
        && version_components.iter().all(|component| {
            !component.is_empty() && component.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// Validate a model slug for its selected harness.
pub fn validate_model_slug(cli: SupervisorCli, model: &str) -> Result<(), String> {
    validate_model_slug_with(cli, model, is_claude_model_slug)
}

/// Validate a model slug against a harness-provided acceptance probe.
///
/// Production uses [`is_claude_model_slug`] as the forward-compatible Claude
/// Code acceptance rule. Tests and future capability checks can supply a stub
/// or refreshed acceptance set without changing the error/remediation
/// contract.
pub fn validate_model_slug_with(
    cli: SupervisorCli,
    model: &str,
    accepted: impl Fn(&str) -> bool,
) -> Result<(), String> {
    if cli != SupervisorCli::Claude {
        return Ok(());
    }
    if accepted(model.trim()) {
        return Ok(());
    }

    let canonical_hint = canonical_hint_for_rejected_claude_slug(model);
    Err(format!(
        "invalid Claude model slug {model:?}: Claude Code does not recognize this value; use {canonical_hint}"
    ))
}

fn canonical_hint_for_rejected_claude_slug(model: &str) -> String {
    let normalized = model.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "opus-5" => "claude-opus-5".to_string(),
        "sonnet-5" => "claude-sonnet-5".to_string(),
        "haiku-4.5" => "claude-haiku-4-5-20251001".to_string(),
        _ if is_bare_claude_family_version(&normalized) => format!("claude-{normalized}"),
        _ => "a canonical claude-* model ID or the opus/sonnet/haiku alias".to_string(),
    }
}

fn is_bare_claude_family_version(model: &str) -> bool {
    let mut components = model.split('-');
    let Some(family) = components.next() else {
        return false;
    };
    if !matches!(family, "opus" | "sonnet" | "haiku" | "fable" | "mythos") {
        return false;
    }

    let version_components: Vec<_> = components.collect();
    !version_components.is_empty()
        && version_components.iter().all(|component| {
            !component.is_empty() && component.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// Add the violated registry rule and copyable active recipe alternatives to a
/// policy rejection. The alternatives are derived from the embedded registry,
/// so a policy change updates every caller's remediation without maintaining a
/// second hand-written list in an MCP or CLI layer.
fn policy_violation_with_alternatives(
    registry: &LaneRegistry,
    reason: String,
    rule: &str,
    excluded_model: Option<&str>,
) -> String {
    let mut alternatives = Vec::new();
    for (recipe_id, recipe) in &registry.recipes {
        if recipe.status != RecipeStatus::Active
            || excluded_model.is_some_and(|model| recipe.model.eq_ignore_ascii_case(model))
        {
            continue;
        }
        alternatives.push(format!(
            "{recipe_id} (model={}, effort={})",
            recipe.model, recipe.default_effort
        ));
    }

    // A Luna effort violation includes Luna xhigh itself, while a suspension
    // omits the suspended Terra recipe. Keep the list stable by recipe ID
    // (the registry is a BTreeMap) and state the rule even if a future
    // registry has no active alternatives.
    let alternatives = if alternatives.is_empty() {
        "none declared".to_string()
    } else {
        alternatives.join(", ")
    };
    format!(
        "{reason}; routing rule '{rule}' violated; available registry alternatives: {alternatives}"
    )
}

/// Reject the standing Terra suspension, retaining the byte-for-byte message
/// used by the MCP spawn path before this policy moved into cas-factory.
pub fn validate_model_is_active(model: &str) -> Result<(), String> {
    if model.trim().eq_ignore_ascii_case("gpt-5.6-terra") {
        return Err(
            "invalid spawn_workers model \"gpt-5.6-terra\": Terra is suspended as a routing target (2026-08-25; operator decision pending); use gpt-5.6-luna with effort=xhigh or another active tier".to_string(),
        );
    }
    Ok(())
}

/// Enforce Luna's existing xhigh-only effort policy.
pub fn validate_model_effort_policy(model: &str, effort: Option<Effort>) -> Result<(), String> {
    if model.trim().eq_ignore_ascii_case("gpt-5.6-luna") && effort != Some(Effort::XHigh) {
        return Err(
            "invalid spawn_workers effort for gpt-5.6-luna: Luna is only permitted at its current maximum, effort=xhigh".to_string(),
        );
    }
    Ok(())
}

/// Return the behavior-preserving stock model for a harness.
pub fn default_worker_model_for_cli(cli: SupervisorCli) -> &'static str {
    registry()
        .expect("embedded lane registry must be valid")
        .defaults
        .get(cli_key(cli))
        .map_or_else(
            || match cli {
                SupervisorCli::Claude => "opus",
                SupervisorCli::Codex => "gpt-5.6-luna",
                SupervisorCli::Grok => "grok-4.5",
                SupervisorCli::OpenCode => "qwencloud/qwen3.8-max",
            },
            |defaults| defaults.model.as_str(),
        )
}

/// Return the behavior-preserving stock effort for a harness.
pub fn default_worker_effort_for_cli(cli: SupervisorCli) -> Effort {
    registry()
        .expect("embedded lane registry must be valid")
        .defaults
        .get(cli_key(cli))
        .map_or_else(
            || match cli {
                SupervisorCli::Codex => Effort::XHigh,
                SupervisorCli::Claude | SupervisorCli::Grok => Effort::High,
                SupervisorCli::OpenCode => Effort::Medium,
            },
            |defaults| defaults.effort,
        )
}

const GENERATED_ROUTE_TABLE_START: &str =
    "<!-- BEGIN GENERATED ROUTE TABLE: cas-factory lane registry -->";
const GENERATED_ROUTE_TABLE_END: &str = "<!-- END GENERATED ROUTE TABLE -->";
const GENERATED_SPAWN_RECIPES_START: &str =
    "<!-- BEGIN GENERATED SPAWN RECIPES: cas-factory lane registry -->";
const GENERATED_SPAWN_RECIPES_END: &str = "<!-- END GENERATED SPAWN RECIPES -->";

/// Render the route table embedded in supervisor guidance.
///
/// The surrounding prose remains human-authored. This small generated block
/// is the one source of truth for lane, recipe, provider, harness, model, and
/// default-effort values shown in the supervisor-facing documentation.
pub fn render_route_table() -> Result<String, RoutingError> {
    let registry = registry()?;
    let mut output = String::from(GENERATED_ROUTE_TABLE_START);
    output.push_str(
        "\n| Lane | Recipe | Provider | CLI | Model | Effort | Status | Fallback | Notes |\n|---|---|---|---|---|---|---|---|---|\n",
    );

    let mut assigned_recipes = BTreeSet::new();

    for lane_name in ordered_lane_names(registry) {
        let decision = resolve_lane(lane_name, &CapabilitySnapshot::default())?;
        let mut lane_recipes = Vec::new();
        flattened_candidates(registry, lane_name, &mut lane_recipes);
        assigned_recipes.extend(lane_recipes);
        let recipe = &registry.recipes[&decision.recipe_id];
        output.push_str(&format!(
            "| `{lane_name}` | `{}` | `{}` | `{}` | `{}` | `{}` | `{}` | `{}` |  |\n",
            decision.recipe_id,
            recipe.provider,
            recipe.harness.backend().name(),
            recipe.model,
            recipe.default_effort,
            recipe_status_name(recipe.status),
            if registry.lanes[lane_name].no_fallback {
                "disabled"
            } else {
                "ordered candidates"
            },
        ));
    }

    for (recipe_id, recipe) in &registry.recipes {
        if assigned_recipes.contains(recipe_id) {
            continue;
        }
        output.push_str(&format!(
            "| `— (explicit only)` | `{recipe_id}` | `{}` | `{}` | `{}` | `{}` | `{}` | `not lane-routed` | {} |\n",
            recipe.provider,
            recipe.harness.backend().name(),
            recipe.model,
            recipe.default_effort,
            recipe_status_name(recipe.status),
            recipe.reason.as_deref().unwrap_or(""),
        ));
    }

    output.push_str(
        "\nLane request mode: call `coordination spawn_workers` with `lane=<lane>`. The registry resolves the ordered candidates; any non-primary selection is reported as a warning with the selected recipe and reason. Lanes marked `disabled` fail closed when their primary is unavailable.\n",
    );

    output.push_str(GENERATED_ROUTE_TABLE_END);
    Ok(output)
}

/// Render copyable `spawn_workers` commands for every active registry lane.
///
/// `tool_prefix` is the harness-specific MCP namespace (`mcp__cas__`,
/// `mcp__cs__`, or `cas__`). The command's route fields always come from the
/// embedded registry, and every generated command pins all three controls.
pub fn render_spawn_recipes(tool_prefix: &str) -> Result<String, RoutingError> {
    let registry = registry()?;
    let mut output = String::from(GENERATED_SPAWN_RECIPES_START);
    output.push_str(
        "\nCopy-paste commands generated from the registry; every recipe pins `cli`, `model`, and `effort`:\n\n```text\n",
    );

    for lane_name in ordered_lane_names(registry) {
        let decision = resolve_lane(lane_name, &CapabilitySnapshot::default())?;
        let recipe = &registry.recipes[&decision.recipe_id];
        output.push_str(&format!(
            "# {lane_name} — recipe {}\n{tool_prefix}coordination action=spawn_workers count=1 isolate=true cli={} model={} effort={}\n\n",
            decision.recipe_id,
            recipe.harness.backend().name(),
            recipe.model,
            recipe.default_effort,
        ));
    }

    output.push_str("```");
    output.push('\n');
    output.push_str(GENERATED_SPAWN_RECIPES_END);
    Ok(output)
}

fn ordered_lane_names(registry: &LaneRegistry) -> Vec<&str> {
    let mut names: Vec<_> = registry.lanes.keys().map(String::as_str).collect();
    names.sort_by_key(|name| match *name {
        "light" => (0, *name),
        "standard" => (1, *name),
        "taste" => (2, *name),
        "heavy" => (3, *name),
        _ => (4, *name),
    });
    names
}

fn recipe_status_name(status: RecipeStatus) -> &'static str {
    match status {
        RecipeStatus::Active => "active",
        RecipeStatus::Suspended => "suspended",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_registry() -> &'static str {
        r#"
schema_version = 1

[recipes.codex_luna]
harness = "codex"
provider = "openai"
model = "gpt-5.6-luna"
effort = "xhigh"
allowed_efforts = ["xhigh"]
status = "active"

[lanes.standard]
candidates = ["codex_luna"]
"#
    }

    #[test]
    fn embedded_registry_has_decided_lanes_and_recipes() {
        let registry = registry().expect("embedded registry validates");
        assert_eq!(registry.schema_version, 1);
        assert_eq!(
            registry.lanes["light"].candidates,
            ["claude_haiku", "codex_luna"]
        );
        assert_eq!(
            registry.lanes["standard"].candidates,
            ["codex_luna", "claude_opus"]
        );
        assert_eq!(registry.lanes["taste"].candidates, ["claude_fable"]);
        assert_eq!(registry.lanes["taste"].fallbacks, ["claude_opus"]);
        assert!(registry.lanes["taste"].no_fallback);
        assert_eq!(registry.lanes["supervisor"].candidates, ["claude_fable"]);
        assert_eq!(registry.lanes["supervisor"].fallbacks, ["claude_opus"]);
        assert!(registry.lanes["supervisor"].no_fallback);
        assert_eq!(
            registry.lanes["heavy"].candidates,
            ["codex_sol", "codex_luna"]
        );

        let haiku = &registry.recipes["claude_haiku"];
        assert_eq!(haiku.model, "claude-haiku-4-5-20251001");
        assert_eq!(haiku.allowed_efforts, [Effort::Low, Effort::Medium]);
        assert_eq!(haiku.default_effort, Effort::Low);
        assert_eq!(registry.recipes["claude_opus"].model, "claude-opus-5");
        let fable = &registry.recipes["claude_fable"];
        assert_eq!(fable.harness, SupervisorCli::Claude);
        assert_eq!(fable.provider, "anthropic");
        assert_eq!(fable.model, "claude-fable-5-1");
        assert_eq!(fable.default_effort, Effort::Medium);
        assert_eq!(fable.allowed_efforts, [Effort::Medium, Effort::High]);
        assert_eq!(fable.required_capability.as_deref(), Some("claude-account"));
        assert!(registry.recipes.contains_key("codex_astra"));
        assert_eq!(
            registry.recipes["codex_astra"].reason.as_deref(),
            Some(
                "Not routed for supervisor or taste: observed 2026-09-05 to hold finished workers and stop driving epics; explicit-request only."
            )
        );
        assert!(
            !lane_references(&registry.lanes["taste"])
                .iter()
                .any(|candidate| candidate == "codex_astra")
        );
        assert_eq!(
            registry.recipes["codex_terra"].status,
            RecipeStatus::Suspended
        );
        assert_eq!(
            registry.recipes["codex_terra"].reason.as_deref(),
            Some("Standing operator suspension (2026-08-27)")
        );
        let qwen = &registry.recipes["qwencloud_qwen"];
        assert_eq!(qwen.harness, SupervisorCli::OpenCode);
        assert_eq!(qwen.provider, "qwencloud");
        assert_eq!(qwen.model, "qwen3.8-max");
        assert_eq!(qwen.default_effort, Effort::Medium);
        assert_eq!(
            qwen.allowed_efforts,
            [Effort::Low, Effort::Medium, Effort::XHigh]
        );
        assert_eq!(
            qwen.required_capability.as_deref(),
            Some("qwencloud-token-plan-key")
        );
        assert!(registry.lanes.values().all(|lane| {
            !lane_references(lane)
                .iter()
                .any(|candidate| candidate == "qwencloud_qwen")
        }));
    }

    #[test]
    fn taste_lane_resolves_fable_medium_and_fails_closed_when_unavailable() {
        let registry = registry().unwrap();
        let recipe = &registry.recipes["claude_fable"];
        assert_eq!(recipe.harness, SupervisorCli::Claude);
        assert_eq!(recipe.provider, "anthropic");
        assert_eq!(recipe.model, "claude-fable-5-1");
        assert_eq!(recipe.default_effort, Effort::Medium);
        assert_eq!(recipe.allowed_efforts, [Effort::Medium, Effort::High]);
        assert_eq!(
            recipe.required_capability.as_deref(),
            Some("claude-account")
        );
        let now = CapabilitySnapshot::now_ms();
        for availability in [
            CapabilityAvailability::Unknown,
            CapabilityAvailability::Available,
        ] {
            let mut snapshot = CapabilitySnapshot::default();
            snapshot.record(
                recipe_route_identity(recipe, "default"),
                CapabilityEvidence::new(availability, now),
            );
            for decision in resolve_lane_specs("taste", 2, &snapshot).unwrap() {
                assert_eq!(decision.recipe_id, "claude_fable");
                assert_eq!(decision.spec.cli, SupervisorCli::Claude);
                assert_eq!(decision.spec.model.as_deref(), Some("claude-fable-5-1"));
                assert_eq!(decision.spec.effort, Some(Effort::Medium));
                assert!(decision.warnings.is_empty());
                validate_explicit(&decision.spec, &snapshot).unwrap();
                let mut explicit = decision.spec;
                for effort in [Effort::Medium, Effort::High] {
                    explicit.effort = Some(effort);
                    validate_explicit(&explicit, &snapshot).unwrap();
                }
            }
        }
        let mut snapshot = CapabilitySnapshot::default();
        snapshot.record(
            recipe_route_identity(recipe, "default"),
            CapabilityEvidence::new(CapabilityAvailability::Unavailable, now),
        );
        for alternative in ["claude_opus"] {
            snapshot.record(
                recipe_route_identity(&registry.recipes[alternative], "default"),
                CapabilityEvidence::new(CapabilityAvailability::Available, now),
            );
        }
        assert!(matches!(
            resolve_lane("taste", &snapshot),
            Err(RoutingError::NoActiveRecipe { .. })
        ));
    }

    #[test]
    fn supervisor_lane_resolves_fable_medium_and_fails_closed_when_unavailable() {
        let registry = registry().unwrap();
        let recipe = &registry.recipes["claude_fable"];
        let now = CapabilitySnapshot::now_ms();
        let mut available = CapabilitySnapshot::default();
        available.record(
            recipe_route_identity(recipe, "default"),
            CapabilityEvidence::new(CapabilityAvailability::Available, now),
        );
        let decision = resolve_lane("supervisor", &available).unwrap();
        assert_eq!(decision.recipe_id, "claude_fable");
        assert_eq!(decision.spec.cli, SupervisorCli::Claude);
        assert_eq!(decision.spec.model.as_deref(), Some("claude-fable-5-1"));
        assert_eq!(decision.spec.effort, Some(Effort::Medium));

        let mut unavailable = CapabilitySnapshot::default();
        unavailable.record(
            recipe_route_identity(recipe, "default"),
            CapabilityEvidence::new(CapabilityAvailability::Unavailable, now),
        );
        assert!(matches!(
            resolve_lane("supervisor", &unavailable),
            Err(RoutingError::NoActiveRecipe { .. })
        ));
    }

    #[test]
    fn explicit_opencode_recipe_is_registry_validated_without_becoming_a_lane() {
        let valid = WorkerSpec {
            name: None,
            cli: SupervisorCli::OpenCode,
            model: Some("qwencloud/qwen3.8-max".to_string()),
            effort: Some(Effort::XHigh),
            config_dir: None,
            requester_config_dir: None,
            requester_secure_storage_dir: None,
        };
        validate_explicit(&valid, &CapabilitySnapshot::default())
            .expect("receipted recipe accepts every registry-declared effort");

        let mut invalid = valid;
        invalid.effort = Some(Effort::High);
        let error = validate_explicit(&invalid, &CapabilitySnapshot::default())
            .expect_err("undeclared OpenCode effort must fail registry policy")
            .to_string();
        assert!(error.contains("qwencloud_qwen"), "{error}");
        assert!(error.contains("low|medium|xhigh"), "{error}");
    }

    #[test]
    fn defaults_and_legacy_policy_messages_are_preserved() {
        assert_eq!(default_worker_model_for_cli(SupervisorCli::Claude), "opus");
        assert_eq!(
            default_worker_model_for_cli(SupervisorCli::Codex),
            "gpt-5.6-luna"
        );
        assert_eq!(
            default_worker_model_for_cli(SupervisorCli::Grok),
            "grok-4.5"
        );
        assert_eq!(
            default_worker_effort_for_cli(SupervisorCli::Codex),
            Effort::XHigh
        );
        assert_eq!(
            default_worker_effort_for_cli(SupervisorCli::Claude),
            Effort::High
        );
        assert!(
            validate_model_is_active("gpt-5.6-terra")
                .expect_err("Terra is suspended")
                .contains("operator decision pending")
        );
        assert!(validate_model_effort_policy("gpt-5.6-luna", Some(Effort::High)).is_err());
    }

    #[test]
    fn registered_harnesses_are_derived_from_the_embedded_registry() {
        assert_eq!(
            registered_harnesses().unwrap(),
            vec![
                SupervisorCli::Claude,
                SupervisorCli::Codex,
                SupervisorCli::Grok,
                SupervisorCli::OpenCode,
            ]
        );
    }

    #[test]
    fn parse_registry_rejects_unknown_nested_keys() {
        let source =
            valid_registry().replace("status = \"active\"", "status = \"active\"\nunknown = true");
        let error = parse_registry(&source).expect_err("unknown recipe keys are fatal");
        assert!(error.to_string().contains("unknown field"), "{error}");
    }

    #[test]
    fn parse_registry_rejects_unknown_references() {
        let source = valid_registry().replace(
            "[lanes.standard]\ncandidates = [\"codex_luna\"]",
            "[lanes.standard]\ncandidates = [\"missing\"]",
        );
        let error = parse_registry(&source).expect_err("unknown recipe references are fatal");
        assert!(error.to_string().contains("missing"), "{error}");
    }

    #[test]
    fn parse_registry_rejects_unrecognized_claude_model() {
        let source = r#"
schema_version = 1

[recipes.claude_bad]
harness = "claude"
provider = "anthropic"
model = "opus-5"
default_effort = "high"
allowed_efforts = ["high"]
status = "active"

[lanes.taste]
candidates = ["claude_bad"]
"#;
        let error = parse_registry(source).expect_err("invalid Claude IDs must fail registry load");
        assert!(error.to_string().contains("claude-opus-5"), "{error}");
    }

    #[test]
    fn parse_registry_rejects_fallback_cycles() {
        let source = r#"
schema_version = 1

[recipes.codex_luna]
harness = "codex"
provider = "openai"
model = "gpt-5.6-luna"
effort = "xhigh"
allowed_efforts = ["xhigh"]
status = "active"

[lanes.first]
candidates = ["second"]

[lanes.second]
candidates = ["first"]
"#;
        let error = parse_registry(source).expect_err("fallback cycles are fatal");
        assert!(
            error.to_string().contains("first -> second -> first"),
            "{error}"
        );
    }

    #[test]
    fn resolve_lane_builds_typed_recipe_decision() {
        let decision = resolve_lane("light", &CapabilitySnapshot::default()).unwrap();
        assert_eq!(decision.recipe_id, "claude_haiku");
        assert_eq!(decision.spec.cli, SupervisorCli::Claude);
        assert_eq!(
            decision.spec.model.as_deref(),
            Some("claude-haiku-4-5-20251001")
        );
        assert_eq!(decision.spec.effort, Some(Effort::Low));
        assert!(decision.warnings.is_empty());
    }

    #[test]
    fn explicit_claude_model_validation_rejects_noncanonical_lane_slugs() {
        for (model, canonical) in [
            ("opus-5", "claude-opus-5"),
            ("haiku-4.5", "claude-haiku-4-5-20251001"),
            ("sonnet-5", "claude-sonnet-5"),
        ] {
            let invalid = WorkerSpec {
                name: None,
                cli: SupervisorCli::Claude,
                model: Some(model.to_string()),
                effort: Some(Effort::High),
                config_dir: None,
                requester_config_dir: None,
                requester_secure_storage_dir: None,
            };
            let error = validate_explicit(&invalid, &CapabilitySnapshot::default())
                .expect_err("Claude Code rejects the old lane slug")
                .to_string();
            assert!(
                error.contains(canonical),
                "{model} remediation must name {canonical}: {error}"
            );
        }

        for model in [
            "claude-opus-5",
            "claude-haiku-4-5-20251001",
            "claude-sonnet-5",
            "claude-opus-4-5",
            "claude-opus-4-5-20251101",
            "claude-sonnet-4-6",
            "claude-opus-4-8",
            "claude-fable-5",
            "claude-opus-5[1m]",
            "opus",
            "haiku",
            "sonnet",
        ] {
            let valid = WorkerSpec {
                name: None,
                cli: SupervisorCli::Claude,
                model: Some(model.to_string()),
                effort: None,
                config_dir: None,
                requester_config_dir: None,
                requester_secure_storage_dir: None,
            };
            validate_explicit(&valid, &CapabilitySnapshot::default()).unwrap_or_else(|error| {
                panic!("recognized Claude model {model} rejected: {error}")
            });
        }

        for (model, hint) in [
            ("opus-4-5", "claude-opus-4-5"),
            ("fable-5", "claude-fable-5"),
            ("mythos-5", "claude-mythos-5"),
            ("claude-opus", "a canonical claude-* model ID"),
            ("claude-unknown-5", "a canonical claude-* model ID"),
            ("claude-opus-4.5", "a canonical claude-* model ID"),
            ("claude-opus-5[2m]", "a canonical claude-* model ID"),
        ] {
            let invalid = WorkerSpec {
                name: None,
                cli: SupervisorCli::Claude,
                model: Some(model.to_string()),
                effort: None,
                config_dir: None,
                requester_config_dir: None,
                requester_secure_storage_dir: None,
            };
            let error = validate_explicit(&invalid, &CapabilitySnapshot::default())
                .expect_err("unrecognized Claude model shape must be rejected")
                .to_string();
            assert!(
                error.contains(hint),
                "{model} hint must name {hint}: {error}"
            );
        }
    }

    #[test]
    fn resolve_lane_specs_expands_one_registry_decision_per_slot() {
        let decisions = resolve_lane_specs("standard", 3, &CapabilitySnapshot::default())
            .expect("standard lane resolves");

        assert_eq!(decisions.len(), 3);
        assert!(decisions.iter().all(|decision| {
            decision.lane == "standard"
                && decision.recipe_id == "codex_luna"
                && decision.spec.model.as_deref() == Some("gpt-5.6-luna")
        }));
    }

    #[test]
    fn resolve_lane_selects_available_fallback_with_reasoned_warning() {
        let registry = parse_registry(
            r#"
schema_version = 1

[recipes.primary]
harness = "codex"
provider = "openai"
model = "primary"
effort = "high"
allowed_efforts = ["high"]
status = "active"

[recipes.fallback]
harness = "claude"
provider = "anthropic"
model = "claude-opus-5"
effort = "high"
allowed_efforts = ["high"]
status = "active"

[lanes.test]
candidates = ["primary", "fallback"]
"#,
        )
        .unwrap();
        let now = CapabilitySnapshot::now_ms();
        let mut snapshot = CapabilitySnapshot::default();
        snapshot.record(
            recipe_route_identity(&registry.recipes["primary"], "default"),
            CapabilityEvidence::new(CapabilityAvailability::Unavailable, now)
                .with_reason("Codex account is logged out"),
        );
        snapshot.record(
            recipe_route_identity(&registry.recipes["fallback"], "default"),
            CapabilityEvidence::new(CapabilityAvailability::Available, now),
        );

        let decision = resolve_lane_from_registry("test", &snapshot, &registry).unwrap();
        assert_eq!(decision.recipe_id, "fallback");
        assert_eq!(decision.warnings.len(), 1);
        assert!(decision.warnings[0].contains("primary"));
        assert!(decision.warnings[0].contains("fallback"));
        assert!(decision.warnings[0].contains("Codex account is logged out"));
    }

    #[test]
    fn resolve_lane_refuses_unknown_fallback_availability() {
        let registry = parse_registry(
            r#"
schema_version = 1

[recipes.primary]
harness = "codex"
provider = "openai"
model = "primary"
effort = "high"
allowed_efforts = ["high"]
status = "active"

[recipes.fallback]
harness = "claude"
provider = "anthropic"
model = "claude-opus-5"
effort = "high"
allowed_efforts = ["high"]
status = "active"

[lanes.test]
candidates = ["primary", "fallback"]
"#,
        )
        .unwrap();
        let now = CapabilitySnapshot::now_ms();
        let mut snapshot = CapabilitySnapshot::default();
        snapshot.record(
            recipe_route_identity(&registry.recipes["primary"], "default"),
            CapabilityEvidence::new(CapabilityAvailability::Unavailable, now),
        );

        let error = resolve_lane_from_registry("test", &snapshot, &registry)
            .expect_err("unknown fallback must fail closed")
            .to_string();
        assert!(error.contains("availability is Unknown"), "{error}");
        assert!(error.contains("refusing fallback"), "{error}");
    }

    #[test]
    fn resolve_lane_respects_no_fallback_for_unavailable_primary() {
        let registry = parse_registry(
            r#"
schema_version = 1

[recipes.primary]
harness = "claude"
provider = "anthropic"
model = "claude-opus-5"
effort = "high"
allowed_efforts = ["high"]
status = "active"

[recipes.fallback]
harness = "codex"
provider = "openai"
model = "fallback"
effort = "high"
allowed_efforts = ["high"]
status = "active"

[lanes.test]
candidates = ["primary", "fallback"]
no_fallback = true
"#,
        )
        .unwrap();
        let now = CapabilitySnapshot::now_ms();
        let mut snapshot = CapabilitySnapshot::default();
        snapshot.record(
            recipe_route_identity(&registry.recipes["primary"], "default"),
            CapabilityEvidence::new(CapabilityAvailability::Unavailable, now),
        );
        snapshot.record(
            recipe_route_identity(&registry.recipes["fallback"], "default"),
            CapabilityEvidence::new(CapabilityAvailability::Available, now),
        );

        let error = resolve_lane_from_registry("test", &snapshot, &registry)
            .expect_err("no-fallback lane must fail closed")
            .to_string();
        assert!(error.contains("no active recipe candidates"), "{error}");
    }

    #[test]
    fn lane_request_rejects_explicit_recipe_controls() {
        let error = validate_lane_request("heavy", false, true, false)
            .expect_err("lane and explicit model are ambiguous")
            .to_string();

        assert!(error.contains("lane=\"heavy\""), "{error}");
        assert!(error.contains("model"), "{error}");
        assert!(error.contains("choose lane="), "{error}");
    }

    #[test]
    fn lane_request_rejects_empty_lane() {
        let error = validate_lane_request("  ", false, false, false)
            .expect_err("empty lane is not a request")
            .to_string();

        assert!(error.contains("unknown lane"), "{error}");
    }

    #[test]
    fn validate_explicit_preserves_static_rejections() {
        let spec = WorkerSpec {
            name: None,
            cli: SupervisorCli::Codex,
            model: Some("gpt-5.6-luna".to_string()),
            effort: Some(Effort::High),
            config_dir: None,
            requester_config_dir: None,
            requester_secure_storage_dir: None,
        };
        assert!(validate_explicit(&spec, &CapabilitySnapshot::default()).is_err());
    }

    #[test]
    fn validate_explicit_reports_registry_alternatives_without_mutating_spec() {
        let terra = WorkerSpec {
            name: Some("terra".to_string()),
            cli: SupervisorCli::Codex,
            model: Some("gpt-5.6-terra".to_string()),
            effort: Some(Effort::XHigh),
            config_dir: Some("/accounts/codex".to_string()),
            requester_config_dir: Some("/accounts/requester".to_string()),
            requester_secure_storage_dir: None,
        };
        let before = terra.clone();
        let error = validate_explicit(&terra, &CapabilitySnapshot::default())
            .expect_err("suspended Terra must fail closed")
            .to_string();

        assert_eq!(
            terra, before,
            "validation must not rewrite an explicit spec"
        );
        assert!(error.contains("Terra is suspended"), "{error}");
        assert!(error.contains("routing rule 'suspended recipe'"), "{error}");
        assert!(error.contains("codex_luna"), "{error}");
        assert!(error.contains("effort=xhigh"), "{error}");
    }

    #[test]
    fn validate_explicit_reports_luna_rule_and_active_alternatives() {
        let luna = WorkerSpec {
            name: None,
            cli: SupervisorCli::Codex,
            model: Some("gpt-5.6-luna".to_string()),
            effort: Some(Effort::High),
            config_dir: None,
            requester_config_dir: None,
            requester_secure_storage_dir: None,
        };
        let error = validate_explicit(&luna, &CapabilitySnapshot::default())
            .expect_err("Luna below xhigh must fail closed")
            .to_string();

        assert!(error.contains("Luna is only permitted"), "{error}");
        assert!(error.contains("routing rule 'allowed effort'"), "{error}");
        assert!(error.contains("codex_luna"), "{error}");
        assert!(error.contains("effort=xhigh"), "{error}");
    }

    #[test]
    fn generated_route_table_and_recipes_follow_embedded_registry() {
        let registry = registry().expect("embedded registry validates");
        let table = render_route_table().expect("route table renders");
        let recipes = render_spawn_recipes("mcp__cas__").expect("spawn recipes render");

        for lane_name in registry.lanes.keys() {
            let decision = resolve_lane(lane_name, &CapabilitySnapshot::default())
                .expect("decided lane has an active recipe");
            let recipe = &registry.recipes[&decision.recipe_id];
            assert!(table.contains(&format!("`{lane_name}`")));
            assert!(table.contains(&format!("`{}`", recipe.model)));
            assert!(table.contains(&format!("`{}`", recipe.default_effort)));
            assert!(recipes.contains(&format!("# {lane_name} — recipe {}", decision.recipe_id)));
            assert!(recipes.contains(&format!(
                "cli={} model={} effort={}",
                recipe.harness.backend().name(),
                recipe.model,
                recipe.default_effort
            )));
        }

        assert!(!recipes.contains("gpt-5.6-terra"));
        assert!(recipes.contains("mcp__cas__coordination action=spawn_workers"));
        assert!(table.contains("Lane request mode"));
        assert!(table.contains("Fallback"));
        assert!(table.contains("disabled"));
        assert!(table.contains("`qwencloud_qwen`"));
        assert!(table.contains("Receipt-gated by opencode-1.18.23-hosted-token-plan-2026-08-27"));
        assert!(!recipes.contains("qwencloud_qwen"));
    }
}
