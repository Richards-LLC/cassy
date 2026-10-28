//! Claude Code's macOS secure-storage identity, shared by auth and usage probes.

use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

/// Hash the NFC selector text, not a canonicalized path. An unscoped process
/// uses the legacy service; callers must pass `None` for the main profile.
pub(crate) fn keychain_service(account_dir: Option<&str>) -> String {
    account_dir
        .filter(|dir| !dir.trim().is_empty())
        .map(|dir| {
            let normalized: String = dir.nfc().collect();
            let hash = format!("{:x}", Sha256::digest(normalized.as_bytes()));
            format!("Claude Code-credentials-{}", &hash[..8])
        })
        .unwrap_or_else(|| "Claude Code-credentials".to_string())
}
