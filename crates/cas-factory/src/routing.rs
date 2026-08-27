//! Static provider and lane routing policy.
//!
//! The registry in `policy/lane-registry.toml` is deliberately limited to
//! shipped policy: route recipes, lane ordering, and protected suspensions.
//! Runtime availability and conformance evidence belong in
//! [`CapabilitySnapshot`] and are not inferred from this file.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use cas_mux::{Effort, SupervisorCli, WorkerSpec};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The embedded, operator-reviewed static routing policy.
pub const LANE_REGISTRY_TOML: &str = include_str!("../policy/lane-registry.toml");

/// Compatibility alias for callers that describe the artifact as a route
/// table rather than a lane registry.
pub const REGISTRY_TOML: &str = LANE_REGISTRY_TOML;

/// Availability facts are intentionally supplied by a later capability layer.
/// R1 only needs a typed argument at the seam; it must not probe a provider or
/// claim that a route is available merely because it is in the registry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySnapshot {
    /// Optional route-keyed facts supplied by a capability probe.
    ///
    /// The map is public so the R4 capability layer can add facts without
    /// changing the `resolve_lane`/`validate_explicit` call shape. R1 does not
    /// read it when selecting the static primary recipe.
    pub availability: BTreeMap<String, CapabilityAvailability>,
}

/// Tri-state capability fact reserved for the capability-aware routing layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum CapabilityAvailability {
    Available,
    Unavailable,
    Unknown,
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
/// which are intentionally distinct from the taste recipe's `opus-5` model.
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

/// Resolve a lane's first active static recipe into a worker spec.
///
/// R1 deliberately does not perform availability probing or fallback policy;
/// the snapshot is part of the seam so later capability-aware work can make
/// that decision without changing callers.
pub fn resolve_lane(
    lane: &str,
    _snapshot: &CapabilitySnapshot,
) -> Result<RoutingDecision, RoutingError> {
    let registry = registry()?;
    let lane_name = lane.trim();
    let lane_definition = registry
        .lanes
        .get(lane_name)
        .ok_or_else(|| RoutingError::UnknownLane(lane.to_string()))?;
    let mut candidates = Vec::new();
    flattened_candidates(registry, lane_name, &mut candidates);
    for recipe_id in candidates {
        let recipe = &registry.recipes[&recipe_id];
        if recipe.status != RecipeStatus::Active {
            if lane_definition.no_fallback {
                break;
            }
            continue;
        }
        let spec = WorkerSpec {
            name: None,
            cli: recipe.harness,
            model: Some(recipe.model.clone()),
            effort: Some(recipe.default_effort),
            config_dir: None,
            requester_config_dir: None,
        };
        return Ok(RoutingDecision {
            lane: lane_name.to_string(),
            recipe_id,
            spec,
            warnings: Vec::new(),
        });
    }
    Err(RoutingError::NoActiveRecipe {
        lane: lane_name.to_string(),
    })
}

/// Validate a resolved explicit worker recipe against static suspension and
/// effort policy. Unknown models remain accepted, preserving the existing
/// spawn behavior while the registry is being introduced.
pub fn validate_explicit(
    spec: &WorkerSpec,
    _snapshot: &CapabilitySnapshot,
) -> Result<(), RoutingError> {
    let registry = registry()?;
    if let Some(model) = spec.model.as_deref() {
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
    }
    Ok(())
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
            },
            |defaults| defaults.effort,
        )
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
        assert_eq!(registry.lanes["light"].candidates, ["claude_haiku"]);
        assert_eq!(registry.lanes["standard"].candidates, ["codex_luna"]);
        assert_eq!(registry.lanes["taste"].candidates, ["claude_opus"]);
        assert!(registry.lanes["taste"].no_fallback);
        assert_eq!(registry.lanes["heavy"].candidates, ["codex_sol"]);

        let haiku = &registry.recipes["claude_haiku"];
        assert_eq!(haiku.model, "haiku-4.5");
        assert_eq!(haiku.allowed_efforts, [Effort::Low, Effort::Medium]);
        assert_eq!(haiku.default_effort, Effort::Low);
        assert_eq!(registry.recipes["claude_opus"].model, "opus-5");
        assert_eq!(
            registry.recipes["codex_terra"].status,
            RecipeStatus::Suspended
        );
        assert_eq!(
            registry.recipes["codex_terra"].reason.as_deref(),
            Some("Standing operator suspension (2026-08-27)")
        );
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
        assert_eq!(decision.spec.model.as_deref(), Some("haiku-4.5"));
        assert_eq!(decision.spec.effort, Some(Effort::Low));
        assert!(decision.warnings.is_empty());
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
        };
        let before = terra.clone();
        let error = validate_explicit(&terra, &CapabilitySnapshot::default())
            .expect_err("suspended Terra must fail closed")
            .to_string();

        assert_eq!(terra, before, "validation must not rewrite an explicit spec");
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
        };
        let error = validate_explicit(&luna, &CapabilitySnapshot::default())
            .expect_err("Luna below xhigh must fail closed")
            .to_string();

        assert!(error.contains("Luna is only permitted"), "{error}");
        assert!(error.contains("routing rule 'allowed effort'"), "{error}");
        assert!(error.contains("codex_luna"), "{error}");
        assert!(error.contains("effort=xhigh"), "{error}");
    }
}
