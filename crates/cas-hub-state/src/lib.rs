//! Scratch extraction for cas-a4b1; keep protocol policy in the application.

pub mod auth;
pub mod identity;
pub mod runtime;
#[doc(hidden)]
pub mod state;
#[doc(hidden)]
pub use state::ensure_private_dir;
