use std::str::FromStr;

use cas_core::hooks::types::HookInput;
use cas_mux::SupervisorCli;
use cas_types::TaskType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationMode {
    Required,
    Bypassed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerificationPolicy {
    pub task_mode: VerificationMode,
    pub epic_mode: VerificationMode,
}

impl VerificationPolicy {
    pub fn task_required(self) -> bool {
        self.task_mode == VerificationMode::Required
    }

    pub fn epic_required(self) -> bool {
        self.epic_mode == VerificationMode::Required
    }
}

pub fn parse_harness(value: &str) -> Option<SupervisorCli> {
    SupervisorCli::from_str(value).ok()
}

pub fn worker_harness_from_env() -> SupervisorCli {
    std::env::var("CAS_FACTORY_WORKER_CLI")
        .ok()
        .and_then(|v| parse_harness(&v))
        .unwrap_or(SupervisorCli::Claude)
}

pub fn supervisor_harness_from_env() -> SupervisorCli {
    std::env::var("CAS_FACTORY_SUPERVISOR_CLI")
        .ok()
        .and_then(|v| parse_harness(&v))
        .unwrap_or(SupervisorCli::Claude)
}

pub fn is_supervisor_from_env() -> bool {
    std::env::var("CAS_AGENT_ROLE")
        .map(|r| r.eq_ignore_ascii_case("supervisor"))
        .unwrap_or(false)
}

pub fn is_worker_from_env() -> bool {
    std::env::var("CAS_AGENT_ROLE")
        .map(|r| r.eq_ignore_ascii_case("worker"))
        .unwrap_or(false)
}

/// Resolve the effective role for a hook invocation. Prefers the explicit
/// field on `HookInput` (populated by the harness at dispatch time in
/// `cli/hook.rs`). Falls back to the process env `CAS_AGENT_ROLE` when the
/// field is absent OR present-but-blank, so a deserialized payload with
/// `"agent_role": ""` doesn't suppress the env fallback — matches the old
/// `is_ok()` semantics where empty strings never counted as "role set".
fn resolve_role(input: &HookInput) -> Option<String> {
    let field = input
        .agent_role
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    field.or_else(|| {
        std::env::var("CAS_AGENT_ROLE")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    })
}

/// Prefer the role snapshotted into the `HookInput` by the harness
/// (`cli/hook.rs`) over re-reading the process env. Falls back to env when
/// the field is absent or blank — both because legacy call paths haven't
/// been updated yet and because inline constructors (e.g. tests) often
/// leave it unset.
pub fn is_supervisor(input: &HookInput) -> bool {
    resolve_role(input)
        .map(|r| r.eq_ignore_ascii_case("supervisor"))
        .unwrap_or(false)
}

/// Worker counterpart of `is_supervisor`. Same env fallback semantics.
pub fn is_worker(input: &HookInput) -> bool {
    resolve_role(input)
        .map(|r| r.eq_ignore_ascii_case("worker"))
        .unwrap_or(false)
}

/// True when the input carries *any* factory role (supervisor or worker),
/// regardless of which. Replaces the pattern
/// `std::env::var("CAS_AGENT_ROLE").is_ok()` for callers that just need to
/// know "is this a factory-spawned process?".
///
/// Matches the pre-refactor semantics: empty-string and whitespace-only
/// role values are treated as "not a factory agent", consistent with the
/// strict-value expectations documented in `HookInput::agent_role`.
pub fn is_factory_agent(input: &HookInput) -> bool {
    resolve_role(input).is_some()
}

/// Factory verification matrix.
///
/// - Subtasks: required only when worker harness supports subagents.
/// - Epics: required only when supervisor harness supports subagents.
pub fn verification_policy(supervisor: SupervisorCli, worker: SupervisorCli) -> VerificationPolicy {
    let task_mode = if worker.capabilities().supports_subagents {
        VerificationMode::Required
    } else {
        VerificationMode::Bypassed
    };

    let epic_mode = if supervisor.capabilities().supports_subagents {
        VerificationMode::Required
    } else {
        VerificationMode::Bypassed
    };

    VerificationPolicy {
        task_mode,
        epic_mode,
    }
}

pub fn verification_required_for_task_type(task_type: TaskType) -> bool {
    let policy = verification_policy(supervisor_harness_from_env(), worker_harness_from_env());
    match task_type {
        TaskType::Epic => policy.epic_required(),
        _ => policy.task_required(),
    }
}

pub fn is_worker_without_subagents_from_env() -> bool {
    is_worker_from_env() && !worker_harness_from_env().capabilities().supports_subagents
}

/// Returns the MCP coordination tool name appropriate for the current factory
/// worker's harness. Claude workers use `mcp__cas__coordination`, Codex workers
/// use `mcp__cs__coordination`.
///
/// Use this when building jail/guidance messages that include a suggested tool
/// call the worker can actually execute — the alias depends on which harness
/// is running the MCP server (Claude vs Codex).
///
/// cas-8aaf: Codex MCP servers register under the `cs` alias; Claude MCP
/// servers register under the `cas` alias. Hardcoding `mcp__cas__coordination`
/// in guidance given to Codex workers produces an instruction they cannot follow.
///
/// EPIC cas-8888 (cas-9a31, Phase 1) SILENT SITE — audited: this was a
/// boolean `== Codex` check, so Grok would have silently fallen into the
/// Claude branch (`mcp__cas__coordination`) — wrong, since Grok namespaces
/// MCP tools as `cas__<tool>` (neither Claude's `mcp__cas__` nor Codex's
/// `mcp__cs__`). Switched to an exhaustive match so a future harness
/// addition trips a compile error here too, instead of silently defaulting.
pub fn worker_coordination_tool() -> &'static str {
    match worker_harness_from_env() {
        SupervisorCli::Codex => "mcp__cs__coordination",
        SupervisorCli::Grok => "cas__coordination",
        SupervisorCli::Claude => "mcp__cas__coordination",
    }
}

/// Returns the MCP verification tool name appropriate for the current
/// supervisor's harness (for embedding in guidance that tells the worker what
/// to ask the supervisor to run).
///
/// Claude supervisors use `mcp__cas__verification`, Codex supervisors use
/// `mcp__cs__verification`, Grok supervisors use `cas__verification`.
///
/// EPIC cas-8888 (cas-9a31, Phase 1) SILENT SITE — audited, same fix as
/// `worker_coordination_tool` above: was a boolean `== Codex` check that
/// would have silently defaulted Grok to Claude's prefix.
pub fn supervisor_verification_tool() -> &'static str {
    match supervisor_harness_from_env() {
        SupervisorCli::Codex => "mcp__cs__verification",
        SupervisorCli::Grok => "cas__verification",
        SupervisorCli::Claude => "mcp__cas__verification",
    }
}

/// Returns *this process's own* harness — as opposed to `worker_harness_from_env`
/// (which, for a supervisor process, describes its *workers'* harness, a
/// different value entirely — see `CAS_FACTORY_WORKER_CLI`'s dual semantics
/// documented on `PtyConfig::grok`/`build_worker_config`/`build_supervisor_config`).
///
/// Use this whenever a hook handler is building "you" / "your" advisory text —
/// a reminder telling the CURRENT agent what tool call *it itself* can make
/// (e.g. "use `<prefix>coordination action=spawn_workers`"). Those sites need
/// the reader's own tool namespace, not the namespace of whichever other role
/// happens to be recorded in `CAS_FACTORY_WORKER_CLI`.
///
/// - Supervisor process → `CAS_FACTORY_SUPERVISOR_CLI` (self).
/// - Worker process → `CAS_FACTORY_WORKER_CLI` (self — each `PtyConfig::<harness>`
///   constructor unconditionally tags its own env with its own harness name
///   when spawning a worker; see cas-921f).
/// - Neither role set (solo/non-factory session) → defaults to Claude, matching
///   every other env-based harness helper in this module.
pub fn own_harness_from_env() -> SupervisorCli {
    if is_supervisor_from_env() {
        supervisor_harness_from_env()
    } else {
        worker_harness_from_env()
    }
}

/// The MCP tool-call prefix (`mcp__cas__`, `mcp__cs__`, or `cas__`) for
/// *this process's own* harness. See `own_harness_from_env` for why this is
/// distinct from `worker_coordination_tool`/`supervisor_verification_tool`
/// (which describe *another* role's namespace, not the reader's own).
///
/// EPIC cas-8888 (cas-fd9f): introduced to replace the ad-hoc, 2-way
/// `HookInput.source == "codex"` guess used by the hook reminder/context
/// subsystem (`cas-core::hooks::context::build_start`/`plan_mode`,
/// `cas-cli::hooks::context`) and several hardcoded-`mcp__cas__` reminder
/// sites in the PreToolUse/Stop/session-hygiene handlers. That guess had two
/// problems: (1) it was 2-way only, so Grok agents were always told
/// Claude's `mcp__cas__` prefix — a call they cannot make; (2) `source` is
/// not a harness signal at all in general (it's Claude Code's own SessionStart
/// "why did this session start" field) — CAS's Codex-manual-registration path
/// (`cas_agent_session_start`, "Codex-friendly" session bootstrap) happens to
/// hardcode `source: Some("codex")` on every call regardless of the *actual*
/// invoking harness, so relying on it is fragile by construction, not just
/// incomplete. Env-based role/harness detection is the correct signal: a
/// PreToolUse/Stop/SessionStart hook always runs *inside* the same process
/// that will read its own output, so this process's own env always describes
/// the real reader correctly.
pub fn own_tool_prefix() -> &'static str {
    own_harness_from_env().capabilities().tool_prefix
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----------------------------------------------------------------------
    // Role-helper tests (field-first, env-fallback).
    // ----------------------------------------------------------------------
    //
    // Most cases don't touch the env at all — they drive agent_role on
    // HookInput. The env-fallback tests serialize through a local mutex to
    // avoid racing with each other within this module.

    fn input_with_role(role: Option<&str>) -> HookInput {
        HookInput {
            agent_role: role.map(str::to_string),
            ..HookInput::default()
        }
    }

    use crate::test_support::TestEnvGuard;

    #[test]
    fn is_supervisor_reads_field() {
        assert!(is_supervisor(&input_with_role(Some("supervisor"))));
        assert!(is_supervisor(&input_with_role(Some("SUPERVISOR"))));
        assert!(is_supervisor(&input_with_role(Some("Supervisor"))));
        assert!(!is_supervisor(&input_with_role(Some("worker"))));
        assert!(!is_supervisor(&input_with_role(Some("other"))));
    }

    #[test]
    fn is_worker_reads_field() {
        assert!(is_worker(&input_with_role(Some("worker"))));
        assert!(is_worker(&input_with_role(Some("Worker"))));
        assert!(!is_worker(&input_with_role(Some("supervisor"))));
    }

    #[test]
    fn is_factory_agent_reads_field() {
        // Field-wins path: with any valid role on the input, no env read happens.
        assert!(is_factory_agent(&input_with_role(Some("supervisor"))));
        assert!(is_factory_agent(&input_with_role(Some("worker"))));
    }

    #[test]
    fn blank_field_and_blank_env_is_not_factory_agent() {
        // Empty/whitespace-only values were never valid roles — neither on the
        // field nor in the env. Needs env_lock because the blank-field path
        // falls through to env.
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_AGENT_ROLE", None)]);
        assert!(!is_factory_agent(&input_with_role(Some(""))));
        assert!(!is_factory_agent(&input_with_role(Some("   "))));
        assert!(!is_factory_agent(&input_with_role(Some("\t"))));
    }

    #[test]
    fn empty_field_falls_through_to_env() {
        // Regression guard for the P1 correctness fix in cas-18fe review:
        // Some("") must not suppress the env fallback.
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_AGENT_ROLE", Some("supervisor"))]);
        assert!(is_supervisor(&input_with_role(Some(""))));
        assert!(is_supervisor(&input_with_role(Some("  "))));
    }

    #[test]
    fn field_wins_over_env() {
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_AGENT_ROLE", Some("worker"))]);
        assert!(is_supervisor(&input_with_role(Some("supervisor"))));
        assert!(!is_worker(&input_with_role(Some("supervisor"))));
    }

    #[test]
    fn env_fallback_when_field_absent() {
        // agent_role: None → read CAS_AGENT_ROLE from env.
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_AGENT_ROLE", Some("worker"))]);
        assert!(is_worker(&input_with_role(None)));
        assert!(!is_supervisor(&input_with_role(None)));
        assert!(is_factory_agent(&input_with_role(None)));
    }

    #[test]
    fn env_empty_is_not_factory_agent() {
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_AGENT_ROLE", Some(""))]);
        assert!(!is_factory_agent(&input_with_role(None)));
    }

    #[test]
    fn env_absent_is_solo_user() {
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_AGENT_ROLE", None)]);
        let input = input_with_role(None);
        assert!(!is_supervisor(&input));
        assert!(!is_worker(&input));
        assert!(!is_factory_agent(&input));
    }

    // ----------------------------------------------------------------------
    // Existing matrix tests for verification_policy.
    // ----------------------------------------------------------------------

    #[test]
    fn matrix_claude_claude() {
        let p = verification_policy(SupervisorCli::Claude, SupervisorCli::Claude);
        assert!(p.task_required());
        assert!(p.epic_required());
    }

    #[test]
    fn matrix_claude_codex() {
        let p = verification_policy(SupervisorCli::Claude, SupervisorCli::Codex);
        assert!(!p.task_required());
        assert!(p.epic_required());
    }

    #[test]
    fn matrix_codex_claude() {
        let p = verification_policy(SupervisorCli::Codex, SupervisorCli::Claude);
        assert!(p.task_required());
        assert!(!p.epic_required());
    }

    #[test]
    fn matrix_codex_codex() {
        let p = verification_policy(SupervisorCli::Codex, SupervisorCli::Codex);
        assert!(!p.task_required());
        assert!(!p.epic_required());
    }

    // ----------------------------------------------------------------------
    // cas-8aaf: MCP alias helpers
    // ----------------------------------------------------------------------

    #[test]
    fn worker_coordination_tool_defaults_to_cas_when_unset() {
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_FACTORY_WORKER_CLI", None)]);
        assert_eq!(
            super::worker_coordination_tool(),
            "mcp__cas__coordination",
            "no CAS_FACTORY_WORKER_CLI set → default Claude → mcp__cas__coordination"
        );
    }

    #[test]
    fn worker_coordination_tool_returns_cas_for_claude_harness() {
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_FACTORY_WORKER_CLI", Some("claude"))]);
        assert_eq!(
            super::worker_coordination_tool(),
            "mcp__cas__coordination",
            "CAS_FACTORY_WORKER_CLI=claude → mcp__cas__coordination"
        );
    }

    #[test]
    fn worker_coordination_tool_returns_cs_for_codex_harness() {
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_FACTORY_WORKER_CLI", Some("codex"))]);
        assert_eq!(
            super::worker_coordination_tool(),
            "mcp__cs__coordination",
            "CAS_FACTORY_WORKER_CLI=codex → mcp__cs__coordination"
        );
    }

    /// EPIC cas-8888 (cas-9a31, Phase 1): the silent `== Codex` boolean check
    /// this function used to be would have silently returned Claude's
    /// `mcp__cas__coordination` for a Grok worker — wrong, since Grok
    /// namespaces tools as `cas__<tool>`.
    #[test]
    fn worker_coordination_tool_returns_cas_double_underscore_for_grok_harness() {
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_FACTORY_WORKER_CLI", Some("grok"))]);
        assert_eq!(
            super::worker_coordination_tool(),
            "cas__coordination",
            "CAS_FACTORY_WORKER_CLI=grok → cas__coordination"
        );
    }

    #[test]
    fn supervisor_verification_tool_returns_cas_when_supervisor_unset() {
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_FACTORY_SUPERVISOR_CLI", None)]);
        assert_eq!(
            super::supervisor_verification_tool(),
            "mcp__cas__verification",
            "no CAS_FACTORY_SUPERVISOR_CLI set → default Claude → mcp__cas__verification"
        );
    }

    #[test]
    fn supervisor_verification_tool_returns_cs_for_codex_supervisor() {
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_FACTORY_SUPERVISOR_CLI", Some("codex"))]);
        assert_eq!(
            super::supervisor_verification_tool(),
            "mcp__cs__verification",
            "CAS_FACTORY_SUPERVISOR_CLI=codex → mcp__cs__verification"
        );
    }

    /// EPIC cas-8888 (cas-9a31, Phase 1): same rationale as
    /// worker_coordination_tool_returns_cas_double_underscore_for_grok_harness.
    #[test]
    fn supervisor_verification_tool_returns_cas_double_underscore_for_grok_supervisor() {
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_FACTORY_SUPERVISOR_CLI", Some("grok"))]);
        assert_eq!(
            super::supervisor_verification_tool(),
            "cas__verification",
            "CAS_FACTORY_SUPERVISOR_CLI=grok → cas__verification"
        );
    }

    #[test]
    fn supervisor_verification_tool_returns_cas_for_claude_supervisor() {
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_FACTORY_SUPERVISOR_CLI", Some("claude"))]);
        assert_eq!(
            super::supervisor_verification_tool(),
            "mcp__cas__verification",
            "CAS_FACTORY_SUPERVISOR_CLI=claude → mcp__cas__verification"
        );
    }

    // ----------------------------------------------------------------------
    // EPIC cas-8888 (cas-fd9f): own_harness_from_env / own_tool_prefix
    // ----------------------------------------------------------------------

    #[test]
    fn own_tool_prefix_defaults_to_claude_when_no_role_set() {
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", None),
            ("CAS_FACTORY_WORKER_CLI", None),
            ("CAS_FACTORY_SUPERVISOR_CLI", None),
        ]);
        assert_eq!(super::own_tool_prefix(), "mcp__cas__");
    }

    #[test]
    fn own_tool_prefix_worker_reads_own_worker_cli_not_supervisor_cli() {
        // A worker process's "own" harness comes from CAS_FACTORY_WORKER_CLI
        // (self-tagged by its own PtyConfig constructor), NOT from
        // CAS_FACTORY_SUPERVISOR_CLI (which describes a different agent).
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_WORKER_CLI", Some("grok")),
            ("CAS_FACTORY_SUPERVISOR_CLI", Some("claude")),
        ]);
        assert_eq!(
            super::own_tool_prefix(),
            "cas__",
            "grok worker under a claude supervisor must see its OWN cas__ prefix"
        );
    }

    #[test]
    fn own_tool_prefix_supervisor_reads_own_supervisor_cli_not_worker_cli() {
        // A supervisor process's CAS_FACTORY_WORKER_CLI describes its WORKERS'
        // harness (a different semantic use of the same var — see cas-921f),
        // so "own" must come from CAS_FACTORY_SUPERVISOR_CLI instead.
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("supervisor")),
            ("CAS_FACTORY_WORKER_CLI", Some("codex")),
            ("CAS_FACTORY_SUPERVISOR_CLI", Some("grok")),
        ]);
        assert_eq!(
            super::own_tool_prefix(),
            "cas__",
            "grok supervisor with codex workers must see its OWN cas__ prefix, not mcp__cs__"
        );
    }

    #[test]
    fn own_tool_prefix_all_three_harnesses_as_worker() {
        let mut env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_SUPERVISOR_CLI", None),
        ]);

        env.set("CAS_FACTORY_WORKER_CLI", "claude");
        assert_eq!(super::own_tool_prefix(), "mcp__cas__");

        env.set("CAS_FACTORY_WORKER_CLI", "codex");
        assert_eq!(super::own_tool_prefix(), "mcp__cs__");

        env.set("CAS_FACTORY_WORKER_CLI", "grok");
        assert_eq!(super::own_tool_prefix(), "cas__");
    }

    #[test]
    fn own_tool_prefix_all_three_harnesses_as_supervisor() {
        let mut env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("supervisor")),
            ("CAS_FACTORY_WORKER_CLI", None),
        ]);

        env.set("CAS_FACTORY_SUPERVISOR_CLI", "claude");
        assert_eq!(super::own_tool_prefix(), "mcp__cas__");

        env.set("CAS_FACTORY_SUPERVISOR_CLI", "codex");
        assert_eq!(super::own_tool_prefix(), "mcp__cs__");

        env.set("CAS_FACTORY_SUPERVISOR_CLI", "grok");
        assert_eq!(super::own_tool_prefix(), "cas__");
    }
}
