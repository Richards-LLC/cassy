//! E2E test runner
//!
//! Run with: cargo test --test e2e_test

mod e2e;
mod fixtures;

#[path = "../src/test_env_guard.rs"]
mod test_env_guard;
