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

/// The logical alias every factory addresses its supervisor by, regardless of
/// which pane name that supervisor happens to have been spawned with.
pub const SUPERVISOR_ALIAS: &str = "supervisor";

/// Every queue-recipient name one factory agent answers to (cas-3bf1, GH #176).
///
/// # Why this is shared rather than local to each caller
///
/// A worker answers to exactly one name. A supervisor answers to TWO: its pane
/// name (`warm-jaguar-96`) and the logical `supervisor` alias the rest of the
/// factory addresses it by. Those are two distinct recipient keys in
/// `prompt_queue.target` AND two distinct keys in
/// `prompt_queue_recipient_seen.recipient` — so "has this recipient read it"
/// is only answerable against the FULL set.
///
/// The two readers had drifted apart, which is the bug this exists to end: the
/// turn-start hook resolved both aliases while the MCP `inbox_poll` resolved
/// only the registered pane name. The consequences were asymmetric and both
/// bad. A row addressed to `supervisor` was invisible to the supervisor's own
/// `inbox_poll` entirely (the unseen predicate matches
/// `q.target = ?alias OR q.target = 'all_workers'`, and `supervisor` was never
/// passed as an alias) — measured on the live queue at 40 of 50
/// `supervisor`-addressed rows never receipted, against 15 of 59 for the pane
/// name. And a row retired under one alias kept no receipt under the other, so
/// the transport that did not write it would surface it again.
///
/// Every reader and every receipt writer must go through this function so the
/// two can never drift again.
pub fn inbox_aliases(agent_name: &str, is_supervisor: bool) -> Vec<String> {
    let mut aliases = Vec::new();
    let trimmed = agent_name.trim();
    if !trimmed.is_empty() {
        aliases.push(trimmed.to_string());
    }
    if is_supervisor && !aliases.iter().any(|a| a == SUPERVISOR_ALIAS) {
        aliases.push(SUPERVISOR_ALIAS.to_string());
    }
    aliases
}

/// Write the surfacing receipt for every alias this recipient answers to
/// (cas-3bf1, GH #176).
///
/// A drain writes a receipt only under the alias it queried. That is complete
/// for a worker (one alias) and incomplete for a supervisor (two), because
/// `prompt_queue_recipient_seen` is keyed on the literal recipient string — so
/// a row retired under `warm-jaguar-96` stays unread under `supervisor`, and
/// whichever reader did not write it surfaces the row again on a later turn.
/// The in-turn duplicate guards in the two readers only cover duplicates
/// WITHIN one turn; nothing covered the cross-turn case.
///
/// `all_workers` broadcasts are the sharpest case — they are exempt from every
/// row-level ack filter, so this table is the ONLY thing that can ever retire
/// one — but the live queue shows the same stranding on directed rows.
///
/// Best-effort, matching the receipt writes on the daemon side: a receipt that
/// fails to persist costs a redelivery, which is recoverable, whereas failing
/// the surfacing would cost the message, which is not.
pub fn mirror_receipts_across_aliases(
    queue: &dyn cas_store::PromptQueueStore,
    rows: &[cas_store::QueuedPrompt],
    aliases: &[String],
) {
    if aliases.len() < 2 {
        return;
    }
    for row in rows {
        for alias in aliases {
            if let Err(error) = queue.record_recipient_surfaced(
                row.id,
                alias,
                cas_store::SurfacingSource::TransportDelivered,
            ) {
                tracing::debug!(
                    target: "cas::coordination",
                    message_id = row.id,
                    %alias,
                    %error,
                    "cas-3bf1: could not mirror the surfacing receipt across the alias set"
                );
            }
        }
    }
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
mod alias_receipt_tests {
    use super::*;
    use cas_store::{PromptQueueStore, SqlitePromptQueueStore};

    fn store() -> (tempfile::TempDir, SqlitePromptQueueStore) {
        let temp = tempfile::TempDir::new().unwrap();
        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        (temp, store)
    }

    /// A worker answers to one name; a supervisor to its pane name AND the
    /// logical alias the whole factory addresses it by.
    #[test]
    fn a_supervisor_answers_to_both_its_pane_name_and_the_logical_alias() {
        assert_eq!(inbox_aliases("brave-fox-53", false), vec!["brave-fox-53"]);
        assert_eq!(
            inbox_aliases("warm-jaguar-96", true),
            vec!["warm-jaguar-96", "supervisor"]
        );
        // A supervisor literally NAMED `supervisor` must not get it twice —
        // duplicate aliases would double-poll and double-render every row.
        assert_eq!(inbox_aliases("supervisor", true), vec!["supervisor"]);
        assert!(inbox_aliases("   ", false).is_empty());
    }

    /// cas-3bf1 (GH #176) AC1 — THE REPRODUCTION.
    ///
    /// A broadcast surfaced under alias A must not re-inject when the other
    /// reader polls under alias B. `all_workers` rows are exempt from every
    /// row-level ack filter, so the receipt table is the only thing that can
    /// retire one — and before the fix the receipt landed under exactly one
    /// alias.
    #[test]
    fn a_broadcast_surfaced_under_one_alias_does_not_re_inject_under_the_other() {
        let (_temp, store) = store();
        let id = store.enqueue("director", "all_workers", "all hands").unwrap();
        let aliases = inbox_aliases("warm-jaguar-96", true);

        // The pane-alias reader surfaces it and retires the whole identity.
        let surfaced = store
            .poll_unseen_for_recipient("warm-jaguar-96", None, 10)
            .unwrap();
        assert_eq!(surfaced.len(), 1, "precondition: the broadcast is unread");
        mirror_receipts_across_aliases(&store, &surfaced, &aliases);

        assert!(
            store
                .poll_unseen_for_recipient("supervisor", None, 10)
                .unwrap()
                .is_empty(),
            "a broadcast already shown to this supervisor must not come back \
             under its other alias — that is the cross-turn re-injection"
        );
        assert_eq!(id, surfaced[0].id);
    }

    /// The reverse direction, which needs no SURFACE_LIMIT interaction: a row
    /// retired by the `supervisor`-alias reader must not re-inject for the
    /// pane-alias reader.
    #[test]
    fn the_alias_retirement_is_symmetric() {
        let (_temp, store) = store();
        store.enqueue("director", "all_workers", "all hands").unwrap();
        let aliases = inbox_aliases("warm-jaguar-96", true);

        let surfaced = store
            .poll_unseen_for_recipient("supervisor", None, 10)
            .unwrap();
        mirror_receipts_across_aliases(&store, &surfaced, &aliases);

        assert!(
            store
                .poll_unseen_for_recipient("warm-jaguar-96", None, 10)
                .unwrap()
                .is_empty(),
            "retirement must not depend on which alias happened to fetch it"
        );
    }

    /// AC2 — directed-message behaviour is unchanged, and one agent's identity
    /// must never retire another agent's mail. This is the guard that keeps the
    /// fix from becoming a message-loss bug.
    #[test]
    fn mirroring_never_reaches_beyond_this_recipients_identity() {
        let (_temp, store) = store();
        let mine = store.enqueue("supervisor", "worker-a", "yours").unwrap();
        store.enqueue("supervisor", "worker-b", "theirs").unwrap();

        let surfaced = store
            .poll_unseen_for_recipient("worker-a", None, 10)
            .unwrap();
        assert_eq!(surfaced.len(), 1);
        assert_eq!(surfaced[0].id, mine);
        // A worker has ONE alias, so mirroring is a no-op by construction.
        mirror_receipts_across_aliases(&store, &surfaced, &inbox_aliases("worker-a", false));

        assert_eq!(
            store
                .poll_unseen_for_recipient("worker-b", None, 10)
                .unwrap()
                .len(),
            1,
            "worker-b's directed mail must be untouched by worker-a's drain"
        );
    }

    /// The visibility half of the bug, measured on the live queue at 40 of 50
    /// `supervisor`-addressed rows never receipted: a row addressed to the
    /// logical alias was unreachable from a reader that only knew the pane
    /// name, because the predicate matches `q.target = ?alias OR 'all_workers'`.
    #[test]
    fn a_supervisor_addressed_row_is_reachable_from_the_alias_set() {
        let (_temp, store) = store();
        let id = store
            .enqueue("worker-1", "supervisor", "merge request")
            .unwrap();

        assert!(
            store
                .poll_unseen_for_recipient("warm-jaguar-96", None, 10)
                .unwrap()
                .is_empty(),
            "precondition: the pane name alone cannot see supervisor-addressed mail"
        );

        let found: Vec<i64> = inbox_aliases("warm-jaguar-96", true)
            .iter()
            .flat_map(|alias| store.poll_unseen_for_recipient(alias, None, 10).unwrap())
            .map(|row| row.id)
            .collect();
        assert_eq!(
            found,
            vec![id],
            "polling the full alias set must reach supervisor-addressed mail"
        );
    }
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
