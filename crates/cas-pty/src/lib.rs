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

pub mod codex_trust;
pub mod conformance;
pub mod opencode;
mod error;
mod pty;

pub use codex_trust::{
    CODEX_TRUST_TIMEOUT_HINT, CodexTrustOutcome, codex_home, ensure_cas_hooks_trusted,
    ensure_cas_hooks_trusted_in, ensure_project_trusted, ensure_project_trusted_in,
    update_codex_config_locked,
};

pub use conformance::{
    ConformanceCheck, ConformanceEvidence, ConformanceStatus, Harness, HarnessConformanceReceipt,
    codex_0146_conformance_receipt, grok_02114_conformance_receipt, harness_conformance_receipts,
};
pub use error::{Error, Result};
pub use pty::{
    ContractRole, claude_supervisor_contract, claude_worker_contract, missing_contract_elements,
    rendered_contract_surface,
};
pub use pty::{Pty, PtyConfig, PtyEvent, TeamsSpawnConfig};
