//! Factory session summary types.
//!
//! `SessionSummary` is the serializable session view cas-cli's own session
//! manager produces (`crate::ui::factory::SessionManager` in cas-cli).
//!
//! # Session Types
//!
//! - **Factory**: Multi-agent sessions with supervisor and workers
//! - **Managed**: Single-agent sessions with auto-save and recording
//! - **Recording**: Playback/recording sessions for session capture

pub(crate) mod state;

#[cfg(test)]
mod state_tests;
