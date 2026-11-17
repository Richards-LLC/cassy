//! Factory MCP Tool E2E Tests (Claude Session)
//!
//! Tests that Claude can discover and use factory and messaging MCP tools (`mcp__cas__factory`, `mcp__cas__coordination`)
//! in a real session. Verifies tool discovery, parameter passing, and response
//! interpretation.
//!
//! # Running
//! These tests require tmux (to avoid CLAUDECODE env var conflict):
//! ```bash
//! cargo test --test e2e_test factory_e2e --features claude_rs_e2e -- --ignored --nocapture
//! ```

#![cfg(feature = "claude_rs_e2e")]

mod conformance;
mod lifecycle;
mod real_factory;
