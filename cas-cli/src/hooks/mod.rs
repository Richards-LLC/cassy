//! Claude Code hook handling
//!
//! Processes hook events from Claude Code via stdin/stdout JSON protocol.
//!
//! # Architecture
//!
//! This module re-exports core types from `cas-core::hooks` and provides
//! CLI-specific wrappers that handle store opening and configuration loading.
//!
//! # Supported Hooks
//!
//! - **SessionStart**: Injects relevant context at session start
//! - **SessionEnd**: Marks observations for extraction
//! - **Stop**: Generates session summary when agent finishes
//! - **SubagentStop**: Cleans up subagent leases when subagent finishes
//! - **PostToolUse**: Captures interesting tool interactions as observations
//! - **UserPromptSubmit**: Optional prompt capture (currently passthrough)
//!
//! # Usage
//!
//! ```bash
//! # Handle a hook event (reads JSON from stdin)
//! cas hook SessionStart
//! ```

mod context;
pub(crate) mod delivery_provenance;
pub(crate) mod handlers;
pub mod scorer;
pub mod transcript;
mod types;

// Re-export types from cas-core
pub use cas_core::hooks::{
    // Context scoring
    BasicContextScorer,
    ContextItem,
    ContextItemType,
    ContextQuery,
    ContextScorer,
    ContextStats,
    ContextStores,
    // Config trait
    DefaultHooksConfig,
    // Types
    HookInput,
    HookOutput,
    HookSpecificOutput,
    HooksConfig,
    PlanModeConfig,
    // Caching
    RuleMatchCache,
    SurfacedItemCallback,
    // Context building (with stores)
    build_context_with_stores,
    build_plan_context_with_stores,
    // Utilities
    estimate_tokens,
    rule_matches_path,
    token_display,
    truncate,
};

// Re-export CLI scorers
pub use scorer::HybridContextScorer;

// Re-export transcript functions from cas-core
pub use cas_core::hooks::transcript::{
    ContentBlock, TranscriptEntry, TranscriptMessage, check_promise_in_transcript,
    get_last_assistant_text, get_recent_assistant_messages,
};

// Re-export CLI-specific wrappers
pub use context::{
    build_context, build_context_ai, build_context_with_token_budget, build_plan_context,
};

// Re-export handlers
pub use handlers::{
    get_session_files, handle_message_display, handle_notification, handle_permission_request,
    handle_post_tool_use, handle_pre_compact, handle_pre_tool_use, handle_session_end,
    handle_session_start, handle_stop, handle_subagent_start, handle_subagent_stop,
    handle_user_prompt_submit, handle_verifier_spawn_cleanup,
};

use std::path::PathBuf;

use crate::error::MemError;
use crate::store::find_cas_root;

/// Process-wide mutex for tests that mutate `CAS_AGENT_ROLE` or any other env
/// var read by CAS hook handlers.
///
/// All test modules that call `std::env::set_var("CAS_AGENT_ROLE", …)` (or any
/// `CAS_*` var) must hold this guard for the duration of the test.  Per-module
/// static mutexes silently fail to coordinate with each other, so two tests in
/// different modules (e.g. `close_ops`, `pre_tool`, `handlers_tests`) can race
/// on the same env var.  A single shared lock eliminates the race.
///
/// Poison-tolerant: if a test panics while holding the lock, the next test
/// recovers the guard via `into_inner()` instead of propagating the poison.
///
/// Usage: `let _g = crate::hooks::test_env_lock();`
#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::test_env_guard::test_env_lock()
}

/// Route a hook event to its handler
///
/// This is the single entry point that resolves cas_root once and passes it
/// to all handlers, eliminating redundant find_cas_root() calls.
pub fn handle_hook(event_name: &str, mut input: HookInput) -> Result<HookOutput, MemError> {
    // CAS-owned headless model calls are implementation details, not user
    // sessions. Their prompts must never be captured as memory and their
    // short-lived harnesses must never register as factory workers.
    if crate::internal_llm::is_internal_invocation() {
        return Ok(HookOutput::empty());
    }

    // Resolve cas_root once at entry point using full discovery logic:
    // 1. CAS_ROOT env var (factory workers use this to share main repo's .cas)
    // 2. Git worktree detection (worktrees share main repo's .cas)
    // 3. Walk up directory tree from cwd
    //
    // IMPORTANT: We use find_cas_root() not find_cas_root_from() to preserve
    // CAS_ROOT env var priority for factory mode compatibility.
    let cas_root: Option<PathBuf> = find_cas_root().ok();

    // The harness submits a raw prompt string. The delivery authority records
    // a one-shot typed envelope keyed to that exact string before it injects
    // machine traffic. Consume the envelope before prompt capture; do not
    // recover provenance by parsing its human-facing rendered text.
    if event_name == "UserPromptSubmit" && input.machine_prompt_provenance.is_none() {
        input.machine_prompt_provenance = cas_root
            .as_deref()
            .and_then(|root| {
                input
                    .submitted_prompt()
                    .and_then(|prompt| {
                        std::env::var("CAS_AGENT_NAME")
                            .ok()
                            .and_then(|recipient| {
                                delivery_provenance::consume(root, &recipient, prompt)
                            })
                    })
            });
    }

    match event_name {
        "SessionStart" => handle_session_start(&input, cas_root.as_deref()),
        "SessionEnd" => handle_session_end(&input, cas_root.as_deref()),
        "Stop" => handle_stop(&input, cas_root.as_deref()),
        "SubagentStart" => handle_subagent_start(&input, cas_root.as_deref()),
        "SubagentStop" => handle_subagent_stop(&input, cas_root.as_deref()),
        "PostToolUse" => handle_post_tool_use(&input, cas_root.as_deref()),
        "PostToolUseFailure" | "PermissionDenied" => {
            handle_verifier_spawn_cleanup(&input, cas_root.as_deref())
        }
        "PreToolUse" => handle_pre_tool_use(&input, cas_root.as_deref()),
        "UserPromptSubmit" => handle_user_prompt_submit(&input, cas_root.as_deref()),
        "PermissionRequest" => handle_permission_request(&input, cas_root.as_deref()),
        "Notification" => handle_notification(&input, cas_root.as_deref()),
        "PreCompact" => handle_pre_compact(&input, cas_root.as_deref()),
        "MessageDisplay" => handle_message_display(&input, cas_root.as_deref()),
        _ => {
            // Unknown hook, just pass through
            Ok(HookOutput::empty())
        }
    }
}

#[cfg(test)]
mod internal_llm_tests {
    use super::*;
    use crate::test_support::TestEnvGuard;

    #[test]
    fn internal_model_hooks_create_neither_agents_nor_prompt_memories() {
        let project = tempfile::tempdir().expect("project");
        let cas_root = crate::store::init_cas_dir(project.path()).expect("cas root");
        let mut env = TestEnvGuard::with_optional_vars(&[
            ("CAS_ROOT", Some(cas_root.to_str().expect("utf8 cas root"))),
            (crate::internal_llm::INTERNAL_LLM_ENV, Some("1")),
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_AGENT_NAME", Some("parent-worker")),
            ("CAS_SESSION_ID", Some("parent-session")),
            ("CAS_FACTORY_MODE", Some("1")),
        ]);

        let analyzer = "Analyze this user prompt and determine if it expresses a coding preference or rule that should be remembered.";
        let internal_session = HookInput {
            session_id: "nested-child-session".into(),
            cwd: project.path().to_string_lossy().into_owned(),
            hook_event_name: "SessionStart".into(),
            source: Some("startup".into()),
            ..HookInput::default()
        };
        handle_hook("SessionStart", internal_session).expect("internal session start");
        handle_hook(
            "UserPromptSubmit",
            HookInput {
                session_id: "nested-child-session".into(),
                cwd: project.path().to_string_lossy().into_owned(),
                hook_event_name: "UserPromptSubmit".into(),
                user_prompt: Some(analyzer.into()),
                ..HookInput::default()
            },
        )
        .expect("internal prompt hook");

        let agents = crate::store::open_agent_store(&cas_root).expect("agent store");
        assert!(agents.list(None).expect("agents").is_empty());
        let memories = crate::store::open_store(&cas_root).expect("memory store");
        assert!(memories.list().expect("memories").is_empty());
        let events = crate::store::open_event_store(&cas_root).expect("event store");
        assert!(
            events
                .list_recent(20)
                .expect("internal activity")
                .is_empty(),
            "nested SessionStart/UserPromptSubmit must emit neither registration nor memory activity"
        );

        // The marker is the boundary, not the prompt shape: a real user turn
        // still follows the ordinary memory path.
        env.remove(crate::internal_llm::INTERNAL_LLM_ENV);
        env.remove("CAS_AGENT_ROLE");
        env.remove("CAS_AGENT_NAME");
        env.remove("CAS_SESSION_ID");
        env.remove("CAS_FACTORY_MODE");
        handle_hook(
            "UserPromptSubmit",
            HookInput {
                session_id: "real-user-session".into(),
                cwd: project.path().to_string_lossy().into_owned(),
                hook_event_name: "UserPromptSubmit".into(),
                user_prompt: Some(
                    "Please implement durable user memory capture for this project.".into(),
                ),
                ..HookInput::default()
            },
        )
        .expect("real prompt hook");
        let stored = memories.list().expect("stored memories");
        assert_eq!(stored.len(), 1);
        assert!(stored[0].content.contains("durable user memory capture"));
        assert!(
            events
                .list_recent(20)
                .expect("real activity")
                .iter()
                .any(|event| event.summary.contains("Memory stored")),
            "a real user prompt must retain the ordinary memory/activity path"
        );
    }
}

// ── Shared test helpers ────────────────────────────────────────────────────
//
// A single process-wide mutex for tests that mutate env vars (`CAS_AGENT_ROLE`,
// `CAS_FACTORY_MODE`, `CAS_CLONE_PATH`, etc.).  All test modules that touch
// these vars must acquire this lock so they don't race with each other.
//
// Usage:
//   let _g = crate::hooks::test_env_lock();
//
// Or via a module-local wrapper (preferred for readability):
//   fn env_lock() -> std::sync::MutexGuard<'static, ()> { crate::hooks::test_env_lock() }
//
// NOTE: the canonical `test_env_lock` is defined once above (before `handle_hook`).
// A duplicate copy landed here via the cross-branch epic merge (guards A2 + surface R2
// both added it independently); the redundant definition was removed during assembly.
