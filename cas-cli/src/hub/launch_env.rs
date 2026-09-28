//! Resolve the service process's CLI launch environment without reading credentials.

use std::ffi::OsString;
use std::path::PathBuf;

use cas_mux::SupervisorCli;

/// Environment to apply to the PTY command for a hub-started supervisor.
#[derive(Debug, Clone)]
pub struct LaunchEnvironment {
    pub executable: PathBuf,
    pub set: Vec<(OsString, OsString)>,
    pub remove: Vec<OsString>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum LaunchError {
    #[error("{cli} CLI is not installed or executable")]
    MissingBinary { cli: &'static str },
    #[error("{cli} profile is missing: {path}")]
    ProfileMissing { cli: &'static str, path: PathBuf },
    #[error("{cli} is not logged in for profile {profile}")]
    NotLoggedIn { cli: &'static str, profile: String },
    #[error("{cli} launch environment could not be resolved: {detail}")]
    ProbeFailed { cli: &'static str, detail: String },
}

/// `profile` is the same optional name accepted by `cas claude` / `cas codex`.
/// `None` uses the provider's machine-default profile.
pub fn resolve(
    cli: SupervisorCli,
    profile: Option<&str>,
) -> Result<LaunchEnvironment, LaunchError> {
    let _ = profile;
    Err(LaunchError::ProbeFailed { cli: match cli {
        SupervisorCli::Claude => "claude",
        SupervisorCli::Codex => "codex",
        SupervisorCli::Grok => "grok",
        SupervisorCli::OpenCode => "opencode",
    }, detail: "resolver initialization is incomplete".into() })
}
