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

/// Prune session-start-fallback claim files older than the given age in seconds.
///
/// Claims older than the retention window can be safely removed since the
/// session is no longer active. Filesystem errors are ignored to allow best-effort
/// cleanup without blocking the caller.
pub(crate) fn prune(cas_root: &Path, older_than_secs: i64) -> Result<usize, String> {
    let directory = cas_root.join(CLAIM_DIR);
    if !directory.exists() {
        return Ok(0);
    }

    let mut removed_count = 0;
    let now = std::time::SystemTime::now();
    let cutoff = std::time::Duration::from_secs(older_than_secs as u64);

    match fs::read_dir(&directory) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("claim") {
                    continue;
                }
                if let Ok(metadata) = fs::metadata(&path) {
                    if let Ok(modified) = metadata.modified() {
                        if let Ok(age) = now.duration_since(modified) {
                            if age > cutoff {
                                if fs::remove_file(&path).is_ok() {
                                    removed_count += 1;
                                }
                            }
                        }
                    }
                }
            }
            Ok(removed_count)
        }
        Err(e) => Err(format!("Failed to read claim directory: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::{claim, prune};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn startup_context_claim_is_once_per_session() {
        let root = tempfile::tempdir().unwrap();
        assert!(claim(root.path(), "session-9568"));
        assert!(!claim(root.path(), "session-9568"));
        assert!(claim(root.path(), "session-9568-other"));
    }

    #[test]
    fn prune_removes_old_claims_only() {
        let root = tempfile::tempdir().unwrap();
        // Create a claim
        assert!(claim(root.path(), "session-test"));

        // Second attempt to claim should fail (file exists)
        assert!(!claim(root.path(), "session-test"));

        // Prune with a 0-second cutoff (everything older than now gets removed)
        let result = prune(root.path(), 0).unwrap();
        assert_eq!(result, 1);

        // After prune, the claim should succeed (file was deleted)
        assert!(claim(root.path(), "session-test"));

        // Second attempt should fail again (file was just recreated)
        assert!(!claim(root.path(), "session-test"));
    }

    #[test]
    fn prune_handles_missing_directory() {
        let root = tempfile::tempdir().unwrap();
        let result = prune(root.path(), 3600);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }
}
