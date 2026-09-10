//! Per-session claim for the Claude custom-profile startup context fallback.
//!
//! A custom Claude profile has two possible startup delivery seams: the
//! SessionStart hook and the queued supervisor intro. The first seam to claim
//! this marker owns the context; the other omits it. `create_new` makes that
//! decision atomic across the hook process and the factory runtime.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use sha2::{Digest, Sha256};

const CLAIM_DIR: &str = "session-start-fallback-claims";

/// Whether the current hook is the Claude custom-profile supervisor path that
/// has the queued intro fallback. Other harnesses and default Claude
/// profiles have no competing startup envelope and must keep their normal
/// SessionStart context behavior.
pub(crate) fn is_custom_claude_supervisor() -> bool {
    if std::env::var("CAS_AGENT_ROLE").ok().as_deref() != Some("supervisor") {
        return false;
    }
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let configured = std::env::var("CLAUDE_CONFIG_DIR").ok();
    let config_dir = crate::ui::factory::daemon::runtime::teams::claude_config_dir_from(
        &home,
        configured.as_deref(),
    );
    config_dir != home.join(".claude")
}

/// Claim the startup context for one harness session.
///
/// Returns `true` for the first claimant and `false` for later attempts. A
/// blank session is not deduplicated, and filesystem failures fail open so a
/// best-effort startup marker can never suppress all startup context.
pub(crate) fn claim(cas_root: &Path, session_id: &str) -> bool {
    if session_id.trim().is_empty() {
        return true;
    }
    let directory = cas_root.join(CLAIM_DIR);
    if fs::create_dir_all(&directory).is_err() {
        return true;
    }
    let key = format!("{:x}", Sha256::digest(session_id.as_bytes()));
    let path = directory.join(format!("{key}.claim"));
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            let _ = writeln!(file, "session={session_id}");
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::claim;

    #[test]
    fn startup_context_claim_is_once_per_session() {
        let root = tempfile::tempdir().unwrap();
        assert!(claim(root.path(), "session-9568"));
        assert!(!claim(root.path(), "session-9568"));
        assert!(claim(root.path(), "session-9568-other"));
    }
}
