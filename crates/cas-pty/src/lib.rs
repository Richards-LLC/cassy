//! PTY management for CAS factory mode
//!
//! Provides cross-platform PTY management using portable-pty with:
//! - Async read/write operations
//! - Raw byte output (terminal parsing done by ghostty_vt)
//! - Resize support
//! - Configurations for Claude and Codex CLI agents
//!
//! # Example
//!
//! ```rust,no_run
//! use cas_pty::{Pty, PtyConfig, PtyEvent};
//! use std::path::PathBuf;
//!
//! #[tokio::main]
//! async fn main() -> cas_pty::Result<()> {
//!     // Create a PTY running bash
//!     let config = PtyConfig::default();
//!     let mut pty = Pty::spawn("my-pty", config)?;
//!
//!     // Send a command
//!     pty.send_line("echo hello").await?;
//!
//!     // Read output
//!     while let Some(event) = pty.recv().await {
//!         match event {
//!             PtyEvent::Output(data) => {
//!                 print!("{}", String::from_utf8_lossy(&data));
//!             }
//!             PtyEvent::Exited(code) => {
//!                 println!("Process exited with code: {:?}", code);
//!                 break;
//!             }
//!             PtyEvent::Error(e) => {
//!                 eprintln!("Error: {}", e);
//!                 break;
//!             }
//!         }
//!     }
//!
//!     Ok(())
//! }
//! ```

pub mod claude_trust;
mod codex_hook_warning;
pub mod codex_trust;
pub mod conformance;
mod display;
mod error;
pub mod input_queue;
pub mod opencode;
mod pty;
pub mod store_isolation;
pub mod violet_env;

pub use claude_trust::{
    CLAUDE_TRUST_FIELD, ClaudeTrustOutcome, claude_global_config_path,
    ensure_claude_project_trusted_in,
};
pub use codex_hook_warning::{CapabilityState, CodexCapabilityEvidence, detect_codex_capabilities};
pub use codex_trust::{
    CODEX_HOOK_REVIEW_FAILURE, CODEX_HOOK_REVIEW_MARKER, CODEX_PRE_TOOL_TIMEOUT_SECS,
    CODEX_TRUST_TIMEOUT_HINT, CodexHookReadiness, CodexHookTrust, CodexLaunchReadiness,
    CodexTrustOutcome, codex_home, codex_launch_readiness,
    ensure_cas_hooks_trusted, ensure_cas_hooks_trusted_in, ensure_project_trusted,
    ensure_project_trusted_in, prepare_cas_tool_hooks_in, prepare_codex_project_hooks_in,
    update_codex_config_locked,
};

pub use conformance::{
    ConformanceCheck, ConformanceEvidence, ConformanceStatus, Harness, HarnessConformanceReceipt,
    ServingIdentity, ServingRoute, codex_0149_conformance_receipt, codex_0156_conformance_receipt,
    grok_0140_conformance_receipt, grok_02114_conformance_receipt, harness_conformance_receipts,
    opencode_11823_token_plan_conformance_receipt,
};
pub use display::is_unsafe_terminal_character;
pub use error::{Error, Result};
pub use input_queue::{PTY_INPUT_BUFFER_CAP, PtyInput};
pub use pty::{
    claude_supports_chrome_flag, claude_supports_flag, expand_claude_selector, expand_codex_home,
};
pub use pty::{
    ContractRole, PROTECTED_OPERATOR_ENV, claude_supervisor_contract, claude_worker_contract,
    missing_contract_elements, rendered_contract_surface,
};
pub use pty::{
    Pty, PtyConfig, PtyEvent, TOOL_CACHE_ENV, TeamsSpawnConfig, WORKER_CREDENTIAL_WARNING_ENV,
    WORKER_ROLE_SKILLS,
};

#[cfg(test)]
#[path = "../../../cas-cli/src/test_env_guard.rs"]
mod test_env_guard;
