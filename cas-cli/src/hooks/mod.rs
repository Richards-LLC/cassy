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
pub use context::{build_context, build_context_ai, build_plan_context};

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
pub fn handle_hook(event_name: &str, input: HookInput) -> Result<HookOutput, MemError> {
    // Resolve cas_root once at entry point using full discovery logic:
    // 1. CAS_ROOT env var (factory workers use this to share main repo's .cas)
    // 2. Git worktree detection (worktrees share main repo's .cas)
    // 3. Walk up directory tree from cwd
    //
    // IMPORTANT: We use find_cas_root() not find_cas_root_from() to preserve
    // CAS_ROOT env var priority for factory mode compatibility.
    let cas_root: Option<PathBuf> = find_cas_root().ok();

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
