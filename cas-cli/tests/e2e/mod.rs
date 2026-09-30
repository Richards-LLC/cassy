//! End-to-end workflow tests for CAS
//!
//! Tests complete user workflows from start to finish using real CAS instances.
//!
//! Note: Many tests were removed when CLI commands were consolidated into MCP-only.
//! Remaining tests cover cloud sync, factory mode and hooks.

mod cloud_sync;
mod factory_e2e;
mod factory_tui_headful;
mod hook_e2e;
