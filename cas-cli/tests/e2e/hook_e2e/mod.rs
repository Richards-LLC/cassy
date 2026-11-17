//! CAS Hook E2E tests using real Claude API calls
//!
//! These tests verify that CAS hooks work correctly when integrated with Claude Code.
//! They require:
//! - Valid Claude API credentials (or local Claude instance)
//! - CAS CLI installed and accessible
//!
//! Run with: cargo test --test e2e_test hook_e2e -- --ignored --nocapture

#![cfg(feature = "claude_rs_e2e")]

use crate::fixtures::{HOOK_TEST_SESSION_ID, HookTestEnv};
use claude_rs::{Message, QueryOptions, session};

// =============================================================================
// SessionStart Hook Tests
// =============================================================================

mod direct_hook;
mod exit_blockers;
mod jail_core;
mod jail_edge_cases;
mod mcp_integration;

/// Stop-hook decisions are machine protocol; an empty response permits stopping.
fn stop_hook_blocks(output: &str) -> bool {
    if output.trim().is_empty() {
        return false;
    }
    let decision: serde_json::Value = serde_json::from_str(output)
        .expect("nonempty Stop-hook output must be valid JSON");
    decision["decision"] == "block"
}
