//! Cascade resolver for [`WorkerSpec`].
//!
//! Resolves per-worker configuration by merging six layers in order (last
//! wins):
//!
//! 1. **Built-in defaults** — Claude / no model / High effort.
//! 2. **User config** — `~/.cas/config.toml` `[factory.defaults]`.
//! 3. **Project config** — `<cwd>/.cas/config.toml` `[factory.defaults]`.
//! 4. **Project per-worker** — `[[factory.workers]]` entries (by position).
//! 5. **CLI flags** — `--worker-cli`, `--worker-model`, `--worker-effort`.
//! 6. **Per-worker JSON** — `--worker-spec '{"name":"alice","cli":"codex"}'`
//!    (repeatable; matched by name then sequential position).

use std::io;
use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

use cas_mux::{Effort, SupervisorCli, WorkerSpec};

// ─────────────────────────────────────────────────────────────────────────────
// Error type
// ─────────────────────────────────────────────────────────────────────────────

/// Errors produced by the cascade resolver.
#[derive(Error, Debug)]
pub enum SpecResolverError {
    /// A config file could not be read from disk.
    #[error("failed to read config file {path}: {source}")]
    ReadConfig {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A config file could not be parsed as TOML.
    #[error("failed to parse config file {path}: {source}")]
    ParseConfig {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    /// A `--worker-spec` value was not valid JSON.
    #[error("invalid --worker-spec JSON: {0}")]
    InvalidWorkerSpec(String),

    /// An effort string was not recognised.
    #[error("invalid effort value {0:?}: {1}")]
    InvalidEffort(String, String),

    /// A CLI string was not recognised.
    #[error("invalid cli value {0:?}: expected 'claude' or 'codex'")]
    InvalidCli(String),

    /// A spec requested Codex, Codex is unavailable on this host, and
    /// strict-CLI mode (`--strict-cli` / `[factory] strict_cli`) is set —
    /// so the resolver bails instead of silently falling back to Claude
    /// (cas-7199 / cas-a487).
    #[error(
        "worker {worker:?} requests codex, but codex is unavailable ({reason}), and \
         --strict-cli / [factory] strict_cli is set — refusing to silently fall back. \
         Install codex from https://developers.openai.com/codex and complete `codex login`, \
         or drop --strict-cli to allow falling back to claude."
    )]
    CodexUnavailableStrict { worker: String, reason: String },
}

// ─────────────────────────────────────────────────────────────────────────────
// TOML config file schema (crate-private)
// ─────────────────────────────────────────────────────────────────────────────

/// `[factory.defaults]` section — all fields optional.
#[derive(Debug, Default, Deserialize)]
struct FactoryDefaultsToml {
    cli: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

/// One `[[factory.workers]]` entry — all fields optional.
#[derive(Debug, Default, Deserialize)]
struct FactoryWorkerToml {
    name: Option<String>,
    cli: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

/// `[factory.supervisor]` table — all fields optional.
///
/// Overrides `[factory.defaults]` for the supervisor agent only.
#[derive(Debug, Default, Deserialize)]
struct FactorySupervisorToml {
    cli: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

/// `[factory]` table.
#[derive(Debug, Default, Deserialize)]
struct FactoryToml {
    defaults: Option<FactoryDefaultsToml>,
    #[serde(default)]
    workers: Vec<FactoryWorkerToml>,
    supervisor: Option<FactorySupervisorToml>,
}

/// Minimal wrapper so we can ignore non-`factory` sections.
#[derive(Debug, Default, Deserialize)]
struct ConfigFileToml {
    factory: Option<FactoryToml>,
}

// ─────────────────────────────────────────────────────────────────────────────
// `--worker-spec` JSON schema (crate-private)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct WorkerSpecJson {
    name: Option<String>,
    cli: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// All sources fed into the cascade resolver.
///
/// All fields have sensible `Default` values (skip layers whose paths don't
/// exist, no CLI overrides, no JSON overrides).
#[derive(Debug, Default)]
pub struct ConfigSources {
    /// Path to the user config file.
    ///
    /// `None` → use `~/.cas/config.toml` (resolved at call time via
    /// [`dirs::home_dir`]).  Pass a path that does not exist to skip this
    /// layer in tests.
    pub user_config: Option<PathBuf>,

    /// Path to the project config file.
    ///
    /// `None` → skip the project layer entirely.  Callers should pass
    /// `Some(cwd.join(".cas/config.toml"))` when the project root is known.
    pub project_config: Option<PathBuf>,

    /// Global `--worker-cli` override — applied to every slot.
    pub cli_flag: Option<SupervisorCli>,

    /// Global `--worker-model` override — applied to every slot.
    pub model_flag: Option<String>,

    /// Global `--worker-effort` override — applied to every slot.
    pub effort_flag: Option<Effort>,

    /// Raw JSON strings from repeated `--worker-spec` occurrences.
    ///
    /// Each string must deserialise as a JSON object with optional fields
    /// `name`, `cli`, `model`, `effort`.
    pub worker_spec_jsons: Vec<String>,

    /// Raw JSON string from a single `--supervisor-spec` flag.
    ///
    /// Applied as layer 6 of the supervisor cascade (overrides everything else).
    /// Ignored by `resolve_specs`; consumed only by `resolve_supervisor_spec`.
    pub supervisor_spec_json: Option<String>,
}

/// Resolve `workers` [`WorkerSpec`] slots from the layered config sources.
///
/// Returns a `Vec<WorkerSpec>` of length `workers`.  Returns an empty vec
/// when `workers == 0`.
///
/// # Errors
///
/// - A config file that exists but cannot be read or parsed produces
///   [`SpecResolverError::ReadConfig`] / [`SpecResolverError::ParseConfig`].
/// - An unparseable `--worker-spec` JSON produces
///   [`SpecResolverError::InvalidWorkerSpec`].
/// - Unknown `cli` or `effort` string values in any layer produce
///   [`SpecResolverError::InvalidCli`] / [`SpecResolverError::InvalidEffort`].
pub fn resolve_specs(
    workers: usize,
    sources: ConfigSources,
) -> Result<Vec<WorkerSpec>, SpecResolverError> {
    if workers == 0 {
        return Ok(vec![]);
    }

    // ── Layer 1: built-in defaults ────────────────────────────────────────
    let mut specs: Vec<WorkerSpec> = (0..workers)
        .map(|_| WorkerSpec::builtin_default())
        .collect();

    // ── Layer 2: user config (~/.cas/config.toml [factory.defaults]) ──────
    let user_path = sources
        .user_config
        .clone()
        .or_else(|| dirs::home_dir().map(|h| h.join(".cas").join("config.toml")));

    if let Some(ref path) = user_path {
        if let Some((defaults, _per_worker, _supervisor)) = load_config_file(path)? {
            if let Some(d) = defaults {
                apply_defaults_to_all(&mut specs, &d)?;
            }
        }
    }

    // ── Layer 3 + 4: project config (.cas/config.toml) ───────────────────
    if let Some(ref path) = sources.project_config {
        if let Some((defaults, per_worker, _supervisor)) = load_config_file(path)? {
            // 3. [factory.defaults]
            if let Some(d) = defaults {
                apply_defaults_to_all(&mut specs, &d)?;
            }
            // 4. [[factory.workers]] — by position
            for (i, wt) in per_worker.iter().enumerate() {
                if let Some(slot) = specs.get_mut(i) {
                    apply_worker_toml(slot, wt)?;
                }
            }
        }
    }

    // ── Layer 5: CLI flags (apply to every slot) ──────────────────────────
    for spec in specs.iter_mut() {
        if let Some(cli) = sources.cli_flag {
            spec.cli = cli;
        }
        if let Some(ref model) = sources.model_flag {
            spec.model = Some(model.clone());
        }
        if let Some(effort) = sources.effort_flag {
            spec.effort = Some(effort);
        }
    }

    // ── Layer 6: --worker-spec JSON overrides ─────────────────────────────
    //
    // Named specs: find an existing slot by name, or claim the next
    // positional slot and assign the name.  Unnamed specs: claim the next
    // positional slot.  A shared cursor tracks sequential slot consumption.
    let mut cursor: usize = 0;

    for json_str in &sources.worker_spec_jsons {
        let parsed: WorkerSpecJson = serde_json::from_str(json_str)
            .map_err(|e| SpecResolverError::InvalidWorkerSpec(e.to_string()))?;

        let target_idx: Option<usize> = if let Some(ref name) = parsed.name {
            // Prefer an existing named slot; fall back to cursor.
            specs
                .iter()
                .position(|s| s.name.as_deref() == Some(name.as_str()))
                .or_else(|| (cursor < specs.len()).then_some(cursor))
        } else {
            // No name: take the next cursor slot.
            (cursor < specs.len()).then_some(cursor)
        };

        if let Some(i) = target_idx {
            apply_json_spec(&mut specs[i], &parsed)?;
            // Advance cursor only when we consumed a positional (non-name-matched) slot.
            // A name-matched slot is one that already existed before cursor reached it
            // (i.e. i < cursor).  A cursor-consumed slot is i == cursor.
            let name_matched = parsed.name.is_some() && i < cursor;
            if !name_matched && i == cursor {
                cursor += 1;
            }
        }
    }

    Ok(specs)
}

/// Return whether any parsed config layer explicitly sets `cli` for a worker
/// slot.
///
/// This deliberately reads through the same TOML schema as the resolver instead
/// of scanning raw text. Callers use it only to distinguish the built-in
/// resolver default from an explicit configured `cli = "..."`
pub fn worker_slot_cli_configured(
    slot: usize,
    sources: &ConfigSources,
) -> Result<bool, SpecResolverError> {
    worker_slot_configured(
        slot,
        sources,
        |defaults| defaults.cli.is_some(),
        |worker| worker.cli.is_some(),
    )
}

/// Return whether any parsed config layer explicitly sets `effort` for a
/// worker slot.
///
/// This deliberately reads through the same TOML schema as the resolver instead
/// of scanning raw text. Callers use it only to distinguish a built-in default
/// from an explicit configured `effort = "high"`.
pub fn worker_slot_effort_configured(
    slot: usize,
    sources: &ConfigSources,
) -> Result<bool, SpecResolverError> {
    worker_slot_configured(
        slot,
        sources,
        |defaults| defaults.effort.is_some(),
        |worker| worker.effort.is_some(),
    )
}

fn worker_slot_configured(
    slot: usize,
    sources: &ConfigSources,
    defaults_has_field: impl Fn(&FactoryDefaultsToml) -> bool,
    worker_has_field: impl Fn(&FactoryWorkerToml) -> bool,
) -> Result<bool, SpecResolverError> {
    let user_path = sources
        .user_config
        .clone()
        .or_else(|| dirs::home_dir().map(|h| h.join(".cas").join("config.toml")));

    if let Some(ref path) = user_path
        && let Some((defaults, _per_worker, _supervisor)) = load_config_file(path)?
        && defaults.as_ref().is_some_and(&defaults_has_field)
    {
        return Ok(true);
    }

    if let Some(ref path) = sources.project_config
        && let Some((defaults, per_worker, _supervisor)) = load_config_file(path)?
    {
        if defaults.as_ref().is_some_and(&defaults_has_field) {
            return Ok(true);
        }
        if per_worker.get(slot).is_some_and(&worker_has_field) {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Resolve a single [`WorkerSpec`] for the supervisor agent.
///
/// Uses the same 6-layer cascade as [`resolve_specs`], but reads
/// `[factory.supervisor]` from the project config (layer 4) instead of
/// `[[factory.workers]]`, and accepts a single `--supervisor-spec` JSON
/// override (layer 6) via `sources.supervisor_spec_json`.
///
/// # Errors
///
/// Same error kinds as [`resolve_specs`].
pub fn resolve_supervisor_spec(sources: ConfigSources) -> Result<WorkerSpec, SpecResolverError> {
    // ── Layer 1: built-in defaults ────────────────────────────────────────
    let mut spec = WorkerSpec::builtin_default();

    // ── Layer 2: user config (~/.cas/config.toml [factory.defaults]) ──────
    let user_path = sources
        .user_config
        .clone()
        .or_else(|| dirs::home_dir().map(|h| h.join(".cas").join("config.toml")));

    if let Some(ref path) = user_path {
        if let Some((defaults, _per_worker, _supervisor)) = load_config_file(path)? {
            if let Some(d) = defaults {
                apply_defaults_to_all(std::slice::from_mut(&mut spec), &d)?;
            }
        }
    }

    // ── Layer 3 + 4: project config (.cas/config.toml) ───────────────────
    if let Some(ref path) = sources.project_config {
        if let Some((defaults, _per_worker, supervisor)) = load_config_file(path)? {
            // 3. [factory.defaults]
            if let Some(d) = defaults {
                apply_defaults_to_all(std::slice::from_mut(&mut spec), &d)?;
            }
            // 4. [factory.supervisor] — supervisor-specific overrides
            if let Some(s) = supervisor {
                apply_supervisor_toml(&mut spec, &s)?;
            }
        }
    }

    // ── Layer 5: CLI flags ────────────────────────────────────────────────
    if let Some(cli) = sources.cli_flag {
        spec.cli = cli;
    }
    if let Some(ref model) = sources.model_flag {
        spec.model = Some(model.clone());
    }
    if let Some(effort) = sources.effort_flag {
        spec.effort = Some(effort);
    }

    // ── Layer 6: --supervisor-spec JSON override ──────────────────────────
    if let Some(ref json_str) = sources.supervisor_spec_json {
        let parsed: WorkerSpecJson = serde_json::from_str(json_str)
            .map_err(|e| SpecResolverError::InvalidWorkerSpec(e.to_string()))?;
        apply_json_spec(&mut spec, &parsed)?;
    }

    Ok(spec)
}

// ─────────────────────────────────────────────────────────────────────────────
// Codex availability fallback (cas-7199 / cas-a487)
// ─────────────────────────────────────────────────────────────────────────────

/// Apply the post-cascade Codex availability fallback to a batch of already-
/// resolved specs, IN PLACE.
///
/// Any layer of [`resolve_specs`]'s cascade (built-in default, config file,
/// CLI flag, or a per-worker `--worker-spec`/`[[factory.workers]]` override)
/// can independently land a spec on `cli = Codex` — cas-fbac made it the
/// built-in default too, so this now runs on effectively every fresh
/// install. Call this once, after the cascade has fully resolved, so the
/// fallback decision is made exactly once per spec regardless of which
/// layer set `cli = Codex`, and BEFORE the spec is queued for spawn — a
/// worker whose spec is already rewritten to `claude` here never goes
/// through worktree/agent-registration setup for the wrong harness.
///
/// Behavior (cas-e9e9 decision via cas-a487, binding over cas-7199's
/// original "symmetric fallback" framing — see that ticket's amended AC#2):
/// - `codex` unavailable (binary absent OR `~/.codex/auth.json` absent —
///   ChatGPT login only, deliberately no `OPENAI_API_KEY` fallback) and
///   `strict == false` (default): rewrite `cli` to `Claude`, drop `model`
///   unless it already looks Claude-compatible (else `default_model`),
///   keep `effort` (shared vocabulary across harnesses). Returns one
///   human-readable notice per rewritten spec for the caller to
///   `tracing::warn!` and surface as an operator-visible banner — this
///   fallback must never be silent.
/// - Same, but `strict == true`: returns
///   [`SpecResolverError::CodexUnavailableStrict`] on the first affected
///   spec instead of rewriting anything.
/// - `cli == Claude` specs are never touched here — there is no reverse
///   fallback. A missing `claude` binary is a setup error surfaced
///   elsewhere (spawn time), not something this resolver routes around.
///
/// The two `codex_available` probes are subprocess + filesystem calls
/// (see [`crate::probe`]); this function evaluates them **at most once**
/// per call (not once per spec) and only when at least one spec actually
/// requests Codex, so specs that don't request it — the common case once a
/// host is properly set up — never pay the probe cost.
pub fn apply_codex_fallback(
    specs: &mut [WorkerSpec],
    strict: bool,
    default_model: Option<&str>,
) -> Result<Vec<String>, SpecResolverError> {
    apply_codex_fallback_with(
        specs,
        strict,
        default_model,
        WORKER_LABEL,
        crate::probe::codex_binary_present,
        crate::probe::codex_auth_present,
    )
}

/// Same fallback, applied to the single supervisor spec.
///
/// The bug is identical for a supervisor slot explicitly configured to
/// Codex, and the blast radius is worse: a worker that fails to launch
/// costs one lane, a supervisor that fails to launch costs the whole
/// session. Kept as a distinct function (rather than a boolean flag on
/// [`apply_codex_fallback`]) so the notice/error wording is unmistakably
/// about the SUPERVISOR falling back — not a generic "worker X" line that
/// could read as just another lane hiccup.
pub fn apply_codex_fallback_for_supervisor(
    spec: &mut WorkerSpec,
    strict: bool,
    default_model: Option<&str>,
) -> Result<Vec<String>, SpecResolverError> {
    apply_codex_fallback_with(
        std::slice::from_mut(spec),
        strict,
        default_model,
        SUPERVISOR_LABEL,
        crate::probe::codex_binary_present,
        crate::probe::codex_auth_present,
    )
}

const WORKER_LABEL: &str = "worker";
const SUPERVISOR_LABEL: &str = "SUPERVISOR";

/// Testable core shared by [`apply_codex_fallback`] and
/// [`apply_codex_fallback_for_supervisor`] — takes the two probes as
/// closures so tests can simulate "codex missing" / "codex present but not
/// logged in" / "codex fully available" without depending on real host
/// state, and `role_label` so the notice/error wording distinguishes a
/// worker rewrite from the (louder, higher-stakes) supervisor rewrite.
fn apply_codex_fallback_with(
    specs: &mut [WorkerSpec],
    strict: bool,
    default_model: Option<&str>,
    role_label: &str,
    binary_present: impl Fn() -> bool,
    auth_present: impl Fn() -> bool,
) -> Result<Vec<String>, SpecResolverError> {
    if !specs.iter().any(|s| s.cli == SupervisorCli::Codex) {
        return Ok(Vec::new());
    }

    let binary_ok = binary_present();
    let auth_ok = auth_present();
    if binary_ok && auth_ok {
        return Ok(Vec::new());
    }
    let reason = if !binary_ok {
        "codex binary not found on PATH"
    } else {
        "~/.codex/auth.json not found (not logged in — run `codex login`)"
    };

    let mut notices = Vec::new();
    for (slot_index, spec) in specs.iter_mut().enumerate() {
        if spec.cli != SupervisorCli::Codex {
            continue;
        }
        // Workers get "worker <name>" when the cascade already resolved a
        // name, otherwise the stable one-based spec position identifies the
        // unnamed slot honestly. The supervisor label remains the fixed,
        // shout-cased `SUPERVISOR_LABEL` alone — there is only one, a name
        // would add nothing, and it must not read as another worker line.
        let label = if role_label == SUPERVISOR_LABEL {
            SUPERVISOR_LABEL.to_string()
        } else {
            spec.name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .map_or_else(
                    || format!("{role_label} slot {}", slot_index + 1),
                    |name| format!("{role_label} {name}"),
                )
        };
        if strict {
            return Err(SpecResolverError::CodexUnavailableStrict {
                worker: label,
                reason: reason.to_string(),
            });
        }
        notices.push(format!(
            "{label}: codex unavailable ({reason}) — falling back to claude"
        ));
        spec.cli = SupervisorCli::Claude;
        if !spec
            .model
            .as_deref()
            .is_some_and(model_is_claude_compatible)
        {
            spec.model = default_model.map(str::to_string);
        }
        // effort is intentionally left untouched — shared vocabulary
        // (low/medium/high/xhigh) across harnesses per cas-a487.
    }
    Ok(notices)
}

/// Loose heuristic for "does this model name belong to the Claude family",
/// mirroring the existing `is_frontier_model` substring-match convention in
/// `cas-cli/src/mcp/tools/service/factory_ops.rs` rather than inventing a
/// stricter parser — model catalogs change too often for an exhaustive
/// allowlist to stay accurate, and a false negative here only costs an
/// unnecessary fallback to `default_model`, not a broken spawn.
fn model_is_claude_compatible(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    ["claude", "sonnet", "opus", "haiku", "fable", "mythos"]
        .iter()
        .any(|needle| m.contains(needle))
}

// ─────────────────────────────────────────────────────────────────────────────
// Private helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Read and parse a TOML config file.
///
/// Returns `Some((defaults_section, per_worker_entries, supervisor_section))` when
/// the file exists and parses successfully, or `None` when the file does not exist.
///
/// Avoids the TOCTOU race of `path.exists()` + `read_to_string` by attempting
/// the read directly and treating `NotFound` as an absent file.
fn load_config_file(
    path: &std::path::Path,
) -> Result<
    Option<(
        Option<FactoryDefaultsToml>,
        Vec<FactoryWorkerToml>,
        Option<FactorySupervisorToml>,
    )>,
    SpecResolverError,
> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(SpecResolverError::ReadConfig {
                path: path.to_path_buf(),
                source: e,
            });
        }
    };
    let config: ConfigFileToml =
        toml::from_str(&text).map_err(|e| SpecResolverError::ParseConfig {
            path: path.to_path_buf(),
            source: e,
        })?;
    let factory = config.factory.unwrap_or_default();
    Ok(Some((
        factory.defaults,
        factory.workers,
        factory.supervisor,
    )))
}

/// Apply a `[factory.defaults]` section to every spec in the vec.
fn apply_defaults_to_all(
    specs: &mut [WorkerSpec],
    d: &FactoryDefaultsToml,
) -> Result<(), SpecResolverError> {
    for spec in specs.iter_mut() {
        if let Some(ref s) = d.cli {
            spec.cli = parse_cli(s)?;
        }
        if let Some(ref m) = d.model {
            spec.model = Some(m.clone());
        }
        if let Some(ref s) = d.effort {
            spec.effort = Some(parse_effort(s)?);
        }
    }
    Ok(())
}

/// Apply one `[[factory.workers]]` TOML entry to a single spec.
fn apply_worker_toml(
    spec: &mut WorkerSpec,
    wt: &FactoryWorkerToml,
) -> Result<(), SpecResolverError> {
    if let Some(ref n) = wt.name {
        spec.name = Some(n.clone());
    }
    if let Some(ref s) = wt.cli {
        spec.cli = parse_cli(s)?;
    }
    if let Some(ref m) = wt.model {
        spec.model = Some(m.clone());
    }
    if let Some(ref s) = wt.effort {
        spec.effort = Some(parse_effort(s)?);
    }
    Ok(())
}

/// Apply a `[factory.supervisor]` TOML entry to the supervisor spec.
fn apply_supervisor_toml(
    spec: &mut WorkerSpec,
    st: &FactorySupervisorToml,
) -> Result<(), SpecResolverError> {
    if let Some(ref s) = st.cli {
        spec.cli = parse_cli(s)?;
    }
    if let Some(ref m) = st.model {
        spec.model = Some(m.clone());
    }
    if let Some(ref s) = st.effort {
        spec.effort = Some(parse_effort(s)?);
    }
    Ok(())
}

/// Apply a parsed `--worker-spec` JSON override to a single spec.
fn apply_json_spec(spec: &mut WorkerSpec, json: &WorkerSpecJson) -> Result<(), SpecResolverError> {
    if let Some(ref n) = json.name {
        spec.name = Some(n.clone());
    }
    if let Some(ref s) = json.cli {
        spec.cli = parse_cli(s)?;
    }
    if let Some(ref m) = json.model {
        spec.model = Some(m.clone());
    }
    if let Some(ref s) = json.effort {
        spec.effort = Some(parse_effort(s)?);
    }
    Ok(())
}

fn parse_cli(s: &str) -> Result<SupervisorCli, SpecResolverError> {
    s.parse::<SupervisorCli>()
        .map_err(|_| SpecResolverError::InvalidCli(s.to_string()))
}

fn parse_effort(s: &str) -> Result<Effort, SpecResolverError> {
    s.parse::<Effort>()
        .map_err(|e| SpecResolverError::InvalidEffort(s.to_string(), e))
}

#[cfg(test)]
mod codex_fallback_tests {
    //! cas-7199 / cas-a487: `apply_codex_fallback` tests. Uses the private
    //! `..._with` core (injectable probes) rather than the public
    //! `apply_codex_fallback`, which calls the REAL `codex --version` /
    //! `~/.codex/auth.json` probes — those are environment-dependent
    //! (whether this test runs on a host with codex installed and logged
    //! in) and would make these tests nondeterministic. Lives inside
    //! `src/` rather than `tests/spec_resolver.rs` for exactly that reason
    //! — the injectable core is private to this module.
    use super::*;

    fn codex_spec(name: &str) -> WorkerSpec {
        WorkerSpec {
            name: Some(name.to_string()),
            cli: SupervisorCli::Codex,
            model: None,
            effort: Some(Effort::High),
            config_dir: None,
            requester_config_dir: None,
        }
    }

    /// cas-a487 Tests spec: "spec has cli=codex, probe fails -> spec
    /// rewritten to cli=claude with warn captured".
    #[test]
    fn codex_missing_falls_back_to_claude_with_notice() {
        let mut specs = vec![codex_spec("alice")];
        let notices =
            apply_codex_fallback_with(&mut specs, false, None, WORKER_LABEL, || false, || false)
                .unwrap();
        assert_eq!(specs[0].cli, SupervisorCli::Claude);
        assert_eq!(
            specs[0].effort,
            Some(Effort::High),
            "effort must be preserved across the fallback rewrite"
        );
        assert_eq!(notices.len(), 1);
        assert!(notices[0].contains("alice"));
        assert!(notices[0].contains("claude"));
    }

    /// cas-8535: mid-session `spawn_workers` resolves one shared spec before
    /// the daemon generates a worker name. The warning must identify that
    /// unnamed slot instead of degrading to the anonymous "worker worker".
    #[test]
    fn unnamed_worker_fallback_notice_names_the_resolved_slot() {
        let mut specs = vec![WorkerSpec {
            name: None,
            cli: SupervisorCli::Codex,
            model: None,
            effort: Some(Effort::High),
            config_dir: None,
            requester_config_dir: None,
        }];

        let notices =
            apply_codex_fallback_with(&mut specs, false, None, WORKER_LABEL, || false, || false)
                .unwrap();

        assert_eq!(
            notices,
            vec![
                "worker slot 1: codex unavailable (codex binary not found on PATH) — falling back to claude"
            ]
        );
        assert_eq!(specs[0].cli, SupervisorCli::Claude);
    }

    /// Auth-only failure (binary present, no login) must still fall back,
    /// with a message that names the actual cause rather than a generic
    /// "not found".
    #[test]
    fn codex_binary_present_but_not_logged_in_falls_back_with_auth_specific_reason() {
        let mut specs = vec![codex_spec("bob")];
        let notices =
            apply_codex_fallback_with(&mut specs, false, None, WORKER_LABEL, || true, || false)
                .unwrap();
        assert_eq!(specs[0].cli, SupervisorCli::Claude);
        assert!(
            notices[0].contains("auth.json") || notices[0].contains("logged in"),
            "reason must distinguish 'not logged in' from 'not installed' — got: {}",
            notices[0]
        );
    }

    /// cas-a487 Tests spec: "strict mode -> error returned" — and no spec
    /// mutation happens on that path.
    #[test]
    fn codex_missing_in_strict_mode_errors_without_mutating_spec() {
        let mut specs = vec![codex_spec("carol")];
        let err =
            apply_codex_fallback_with(&mut specs, true, None, WORKER_LABEL, || false, || false)
                .expect_err("strict mode must error, not fall back");
        assert!(matches!(
            err,
            SpecResolverError::CodexUnavailableStrict { .. }
        ));
        assert_eq!(
            specs[0].cli,
            SupervisorCli::Codex,
            "strict-mode error path must not rewrite the spec"
        );
    }

    /// cas-8535: strict mode must report the same improved slot identifier
    /// as the fallback warning when the real spawn-path spec has no name.
    #[test]
    fn unnamed_worker_strict_error_names_the_resolved_slot() {
        let mut specs = vec![WorkerSpec {
            name: None,
            cli: SupervisorCli::Codex,
            model: None,
            effort: None,
            config_dir: None,
            requester_config_dir: None,
        }];

        let err =
            apply_codex_fallback_with(&mut specs, true, None, WORKER_LABEL, || false, || false)
                .expect_err("strict mode must identify the unnamed worker slot");

        match &err {
            SpecResolverError::CodexUnavailableStrict { worker, .. } => {
                assert_eq!(worker, "worker slot 1");
            }
            other => panic!("expected CodexUnavailableStrict, got {other:?}"),
        }
        assert!(
            err.to_string().contains("worker slot 1"),
            "rendered strict-mode error must carry the slot identifier — got: {err}"
        );
        assert_eq!(
            specs[0].cli,
            SupervisorCli::Codex,
            "must not mutate on error"
        );
    }

    /// Both probes passing must be a pure no-op: no notices, no mutation,
    /// and — implicitly, since the closures would need to be called to
    /// observe anything — a fast path when nothing needs to change.
    #[test]
    fn codex_available_is_a_no_op() {
        let mut specs = vec![codex_spec("dana")];
        let notices =
            apply_codex_fallback_with(&mut specs, false, None, WORKER_LABEL, || true, || true)
                .unwrap();
        assert!(notices.is_empty());
        assert_eq!(specs[0].cli, SupervisorCli::Codex);
    }

    /// cas-a487 Tests spec ("Reverse: spec has cli=claude, probe fails ->
    /// error always (no fallback)") — interpreted precisely for THIS
    /// function's contract: `apply_codex_fallback` only ever probes/acts on
    /// Codex specs. A Claude spec is never touched, rewritten, or errored
    /// here, regardless of codex/claude availability — a missing claude
    /// binary is a setup error surfaced elsewhere (at spawn time / the
    /// existing `resolve_cli_choice` preflight in `cas-cli`), not something
    /// this resolver-level function is responsible for detecting. This test
    /// pins "never touched", the necessary precondition for "no fallback
    /// happens here".
    #[test]
    fn claude_spec_is_never_touched_regardless_of_probe_results() {
        let mut specs = vec![WorkerSpec {
            name: Some("erin".to_string()),
            cli: SupervisorCli::Claude,
            model: Some("sonnet".to_string()),
            effort: Some(Effort::Medium),
            config_dir: None,
            requester_config_dir: None,
        }];
        let before = specs[0].clone();
        let notices =
            apply_codex_fallback_with(&mut specs, false, None, WORKER_LABEL, || false, || false)
                .unwrap();
        assert!(notices.is_empty());
        assert_eq!(specs[0], before);
    }

    /// A model that isn't Claude-compatible must be replaced by
    /// `default_model`, not carried over verbatim (e.g. a codex model name
    /// landing on a claude spawn command).
    #[test]
    fn non_claude_model_falls_back_to_default_model_on_rewrite() {
        let mut specs = vec![WorkerSpec {
            model: Some("gpt-5.6-terra".to_string()),
            ..codex_spec("frank")
        }];
        apply_codex_fallback_with(
            &mut specs,
            false,
            Some("sonnet"),
            WORKER_LABEL,
            || false,
            || false,
        )
        .unwrap();
        assert_eq!(specs[0].model.as_deref(), Some("sonnet"));
    }

    /// A model that already looks Claude-compatible must survive the
    /// rewrite unchanged, even with a different `default_model` supplied.
    #[test]
    fn claude_compatible_model_survives_rewrite() {
        let mut specs = vec![WorkerSpec {
            model: Some("claude-opus-4-5".to_string()),
            ..codex_spec("grace")
        }];
        apply_codex_fallback_with(
            &mut specs,
            false,
            Some("some-other-model"),
            WORKER_LABEL,
            || false,
            || false,
        )
        .unwrap();
        assert_eq!(specs[0].model.as_deref(), Some("claude-opus-4-5"));
    }

    /// No spec requests Codex at all: the probe closures must never be
    /// invoked (the doc comment's "at most once, and only when needed"
    /// guarantee) — proven by panicking closures instead of just asserting
    /// the (trivial) empty-notices result.
    #[test]
    fn no_codex_specs_never_calls_the_probes() {
        let mut specs = vec![WorkerSpec {
            name: Some("henry".to_string()),
            cli: SupervisorCli::Claude,
            model: None,
            effort: None,
            config_dir: None,
            requester_config_dir: None,
        }];
        let notices = apply_codex_fallback_with(
            &mut specs,
            false,
            None,
            WORKER_LABEL,
            || panic!("binary probe must not run when no spec requests codex"),
            || panic!("auth probe must not run when no spec requests codex"),
        )
        .unwrap();
        assert!(notices.is_empty());
    }

    #[test]
    fn model_is_claude_compatible_matches_known_families() {
        for m in ["sonnet", "claude-opus-4-5", "Fable-5", "MYTHOS-5", "haiku"] {
            assert!(model_is_claude_compatible(m), "{m} should match");
        }
        for m in ["gpt-5.6-terra", "o3", "grok-4.5"] {
            assert!(!model_is_claude_compatible(m), "{m} should not match");
        }
    }

    // ── apply_codex_fallback_for_supervisor (supervisor blast-radius) ──────

    /// The supervisor-facing entry point must produce a notice that names
    /// the SUPERVISOR unmistakably — not a generic "worker X" line, since a
    /// supervisor fallback changes the harness of the whole session's
    /// coordinator, a strictly bigger deal than one worker lane.
    #[test]
    fn supervisor_codex_missing_falls_back_with_supervisor_labeled_notice() {
        let mut spec = WorkerSpec {
            name: None,
            cli: SupervisorCli::Codex,
            model: None,
            effort: Some(Effort::High),
            config_dir: None,
            requester_config_dir: None,
        };
        let notices = apply_codex_fallback_with(
            std::slice::from_mut(&mut spec),
            false,
            None,
            SUPERVISOR_LABEL,
            || false,
            || false,
        )
        .unwrap();
        assert_eq!(spec.cli, SupervisorCli::Claude);
        assert_eq!(
            notices,
            vec![
                "SUPERVISOR: codex unavailable (codex binary not found on PATH) — falling back to claude"
            ],
            "supervisor banner wording must remain unchanged"
        );
        assert!(
            !notices[0].to_ascii_lowercase().contains("worker"),
            "supervisor notice must not read as a worker line — got: {}",
            notices[0]
        );
    }

    /// Strict mode on the supervisor path errors the same way as workers —
    /// same policy, just the louder wording.
    #[test]
    fn supervisor_codex_missing_in_strict_mode_errors() {
        let mut spec = WorkerSpec {
            name: None,
            cli: SupervisorCli::Codex,
            model: None,
            effort: None,
            config_dir: None,
            requester_config_dir: None,
        };
        let err = apply_codex_fallback_with(
            std::slice::from_mut(&mut spec),
            true,
            None,
            SUPERVISOR_LABEL,
            || false,
            || false,
        )
        .expect_err("strict mode must error for the supervisor too");
        match err {
            SpecResolverError::CodexUnavailableStrict { worker, .. } => {
                assert_eq!(worker, "SUPERVISOR");
            }
            other => panic!("expected CodexUnavailableStrict, got {other:?}"),
        }
        assert_eq!(spec.cli, SupervisorCli::Codex, "must not mutate on error");
    }

    /// Public wrapper smoke test — `apply_codex_fallback_for_supervisor`
    /// must be a real, callable public API (uses the REAL probes, so this
    /// only asserts it compiles and runs without panicking; behavior is
    /// covered exhaustively above via the injectable core).
    #[test]
    fn apply_codex_fallback_for_supervisor_is_callable() {
        let mut spec = WorkerSpec {
            name: None,
            cli: SupervisorCli::Claude,
            model: None,
            effort: None,
            config_dir: None,
            requester_config_dir: None,
        };
        let notices = apply_codex_fallback_for_supervisor(&mut spec, false, None).unwrap();
        assert!(notices.is_empty(), "claude spec must never be touched");
    }
}
