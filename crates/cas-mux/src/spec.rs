//! Per-worker configuration types.
//!
//! [`WorkerSpec`] is the resolved, per-worker view of `{cli, model, effort}`.
//! It is produced by the cascade resolver in `cas-factory` and consumed at
//! spawn time by `Mux::factory`.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::SupervisorCli;

/// Reasoning effort level, shared across backends.
///
/// Backends map this shared value to their own CLI/config syntax through
/// [`Backend::effort_arg`](crate::Backend::effort_arg).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Minimal,
    Low,
    Medium,
    High,
    /// Extra-high; serialised/parsed as `"xhigh"`.
    #[serde(rename = "xhigh")]
    XHigh,
}

impl Effort {
    /// Canonical configuration spelling shared by serialization and display.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
        }
    }
}

impl FromStr for Effort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" | "x-high" => Ok(Self::XHigh),
            other => Err(format!(
                "unsupported effort level {other:?}; expected one of minimal|low|medium|high|xhigh"
            )),
        }
    }
}

impl std::fmt::Display for Effort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Resolved per-worker configuration.
///
/// Produced by `cas_factory::spec_resolver::resolve_specs` after applying the
/// 5-layer config cascade, and consumed at spawn time.
///
/// `None` in any field means "use the backend's own default", not "still
/// needs resolution".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSpec {
    /// Optional name for this worker slot (e.g. `"alice"`).
    /// `None` means the factory assigns a generated name at spawn time.
    pub name: Option<String>,
    /// CLI backend (Claude, Codex, or Grok).
    pub cli: SupervisorCli,
    /// Model name (e.g. `"claude-opus-4-5"` or `"gpt-5.5"`).
    /// `None` = use the backend's own default.
    pub model: Option<String>,
    /// Reasoning effort. `None` = use the backend's own default.
    pub effort: Option<Effort>,
    /// Explicit account directory for this spawn — `CLAUDE_CONFIG_DIR` when
    /// `cli == Claude`, `CODEX_HOME` when `cli == Codex` (cas-9cc3). `None`
    /// preserves inherited env-var behavior. Also consulted by
    /// `apply_codex_fallback`'s auth probe (cas-4a5e) to check the RIGHT
    /// account's login state, not just the default `~/.codex`.
    #[serde(default)]
    pub config_dir: Option<String>,
    /// Requesting supervisor's own account directory (same provider-scoped
    /// meaning as `config_dir` above — `CLAUDE_CONFIG_DIR` or `CODEX_HOME`
    /// depending on `cli`), captured when the spawn request was enqueued.
    /// Explicit `config_dir` takes precedence.
    #[serde(default)]
    pub requester_config_dir: Option<String>,
    /// Requesting Claude supervisor's independent secure-storage selector,
    /// captured when the spawn request was enqueued. `None` means the
    /// variable was unset, `Some("")` means it was explicitly empty, and
    /// `Some(path)` preserves the selected credential-store directory.
    /// Explicit `config_dir` takes precedence over this field for the worker's
    /// derived secure-storage policy.
    #[serde(default)]
    pub requester_secure_storage_dir: Option<String>,
}

impl WorkerSpec {
    /// Construct the built-in default spec: Claude / no model / High effort.
    pub fn builtin_default() -> Self {
        Self {
            name: None,
            cli: SupervisorCli::Claude,
            model: None,
            effort: Some(Effort::High),
            config_dir: None,
            requester_config_dir: None,
            requester_secure_storage_dir: None,
        }
    }

    /// Construct a named Codex spec with no model or effort override.
    ///
    /// Convenience constructor for tests and callers that want to pin a
    /// specific worker slot to the Codex backend without specifying model or
    /// effort (both will use the Codex binary's own defaults).
    ///
    /// # Example
    /// ```
    /// use cas_mux::{WorkerSpec, SupervisorCli};
    /// let spec = WorkerSpec::codex_default("alice");
    /// assert_eq!(spec.name.as_deref(), Some("alice"));
    /// assert_eq!(spec.cli, SupervisorCli::Codex);
    /// assert!(spec.model.is_none());
    /// assert!(spec.effort.is_none());
    /// ```
    pub fn codex_default(name: &str) -> Self {
        Self {
            name: Some(name.to_string()),
            cli: SupervisorCli::Codex,
            model: None,
            effort: None,
            config_dir: None,
            requester_config_dir: None,
            requester_secure_storage_dir: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_roundtrip_all_variants() {
        let cases = [
            (Effort::Minimal, "minimal"),
            (Effort::Low, "low"),
            (Effort::Medium, "medium"),
            (Effort::High, "high"),
            (Effort::XHigh, "xhigh"),
        ];
        for (variant, s) in cases {
            assert_eq!(
                variant.as_str(),
                s,
                "canonical spelling mismatch for {variant:?}"
            );
            for cli in [
                SupervisorCli::Claude,
                SupervisorCli::Codex,
                SupervisorCli::Grok,
            ] {
                assert_eq!(
                    cli.backend().effort_arg(variant),
                    s,
                    "backend effort mismatch for {cli:?}/{variant:?}"
                );
            }
            assert_eq!(
                s.parse::<Effort>().unwrap(),
                variant,
                "parse mismatch for {s:?}"
            );
            assert_eq!(variant.to_string(), s, "display mismatch for {variant:?}");
        }
    }

    #[test]
    fn effort_parse_xhigh_alias() {
        assert_eq!("x-high".parse::<Effort>().unwrap(), Effort::XHigh);
    }

    #[test]
    fn effort_parse_case_insensitive() {
        assert_eq!("HIGH".parse::<Effort>().unwrap(), Effort::High);
        assert_eq!("  Low  ".parse::<Effort>().unwrap(), Effort::Low);
    }

    #[test]
    fn effort_parse_invalid() {
        assert!("extreme".parse::<Effort>().is_err());
        assert!("".parse::<Effort>().is_err());
    }

    #[test]
    fn worker_spec_builtin_default() {
        let spec = WorkerSpec::builtin_default();
        assert_eq!(spec.cli, SupervisorCli::Claude);
        assert_eq!(spec.model, None);
        assert_eq!(spec.effort, Some(Effort::High));
        assert_eq!(spec.name, None);
    }

    #[test]
    fn worker_spec_codex_default() {
        let spec = WorkerSpec::codex_default("alice");
        assert_eq!(spec.cli, SupervisorCli::Codex);
        assert_eq!(spec.model, None);
        assert_eq!(spec.effort, None);
        assert_eq!(spec.name.as_deref(), Some("alice"));
    }

    /// EPIC cas-8888 (cas-9a31, Phase 1): `WorkerSpec.cli` derives its
    /// Serialize/Deserialize straight from `SupervisorCli`, which carries
    /// `#[serde(rename_all = "lowercase")]` — so `"grok"` round-trips with
    /// zero extra wiring here. This is the concrete proof cited in the
    /// task description (crates/cas-mux/src/spec.rs:87-98).
    #[test]
    fn worker_spec_grok_round_trips_through_json() {
        let spec = WorkerSpec {
            name: Some("bob".to_string()),
            cli: SupervisorCli::Grok,
            model: Some("grok-4.5".to_string()),
            effort: Some(Effort::Medium),
            config_dir: None,
            requester_config_dir: None,
            requester_secure_storage_dir: None,
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(
            json.contains("\"cli\":\"grok\""),
            "expected lowercase 'grok' in serialized WorkerSpec: {json}"
        );
        let back: WorkerSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back, spec);
    }
}
