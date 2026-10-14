//! Commander relay token (cas-ca22).
//!
//! The hub is the only component that authenticates Commander operators
//! (DPoP device credentials). A session daemon must therefore accept the
//! `operator_verified` attribution only from the hub's relay connection. At
//! startup the daemon mints a random token into the hub's private directory
//! (`~/.cas/hub/relay-tokens/<session>.token`, 0600 in 0700); the hub presents
//! it in its WebSocket handshake. Every other client, including the GUI
//! socket and any WebSocket without the token, is stored unverified.

use std::path::{Path, PathBuf};

use anyhow::Context as _;

/// WebSocket handshake header the hub relay presents to a session daemon.
pub(crate) const RELAY_TOKEN_HEADER: &str = "x-cas-relay-token";

pub(crate) fn token_path(hub_root: &Path, session: &str) -> PathBuf {
    hub_root
        .join("relay-tokens")
        .join(format!("{session}.token"))
}

/// Mint a fresh 256-bit token for `session`, replacing any earlier one.
pub(crate) fn mint(hub_root: &Path, session: &str) -> anyhow::Result<String> {
    use rand::RngCore as _;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token = hex::encode(bytes);
    let path = token_path(hub_root, session);
    let dir = path.parent().context("relay token path has no parent")?;
    crate::hub::state::ensure_private_dir(dir)?;
    let temp = dir.join(format!(".{session}.{}.tmp", std::process::id()));
    {
        use std::io::Write as _;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temp)
            .with_context(|| format!("create {}", temp.display()))?;
        file.write_all(token.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&temp, &path).with_context(|| format!("publish {}", path.display()))?;
    Ok(token)
}

pub(crate) fn read(hub_root: &Path, session: &str) -> Option<String> {
    std::fs::read_to_string(token_path(hub_root, session))
        .ok()
        .map(|token| token.trim().to_owned())
        .filter(|token| !token.is_empty())
}

pub(crate) fn remove(hub_root: &Path, session: &str) {
    let _ = std::fs::remove_file(token_path(hub_root, session));
}

/// Constant-time comparison; an empty token never matches.
pub(crate) fn matches(presented: Option<&str>, expected: &str) -> bool {
    use subtle::ConstantTimeEq as _;
    match presented {
        Some(presented) if !expected.is_empty() && presented.len() == expected.len() => {
            presented.as_bytes().ct_eq(expected.as_bytes()).into()
        }
        _ => false,
    }
}

pub(crate) type HandshakeRequest = tokio_tungstenite::tungstenite::handshake::client::Request;

/// The hub's upstream request to a session daemon, carrying the relay token.
pub(crate) fn upstream_request(port: u16, token: Option<&str>) -> anyhow::Result<HandshakeRequest> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = format!("ws://127.0.0.1:{port}").into_client_request()?;
    if let Some(token) = token {
        request
            .headers_mut()
            .insert(RELAY_TOKEN_HEADER, token.parse()?);
    }
    Ok(request)
}

/// The relay token a WebSocket client presented in its handshake, if any.
pub(crate) fn presented_token<B>(
    request: &tokio_tungstenite::tungstenite::http::Request<B>,
) -> Option<String> {
    request
        .headers()
        .get(RELAY_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cas_ca22_relay_token_is_private_random_and_compared_exactly() {
        let home = tempfile::tempdir().unwrap();
        let hub_root = home.path().join(".cas/hub");
        let first = mint(&hub_root, "factory-1").unwrap();
        assert_eq!(first.len(), 64, "256 random bits, hex");
        assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(
            read(&hub_root, "factory-1").as_deref(),
            Some(first.as_str())
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&token_path(&hub_root, "factory-1")), 0o600);
            assert_eq!(mode(&hub_root.join("relay-tokens")), 0o700);
        }
        // Each daemon start mints a fresh token.
        let second = mint(&hub_root, "factory-1").unwrap();
        assert_ne!(first, second);
        assert!(matches(Some(&second), &second));
        assert!(!matches(Some(&first), &second), "a stale token is refused");
        assert!(!matches(None, &second), "no header is refused");
        assert!(!matches(Some(""), ""), "an empty token never matches");
        assert!(!matches(Some(&second[..63]), &second));
        remove(&hub_root, "factory-1");
        assert_eq!(read(&hub_root, "factory-1"), None);
    }

    #[test]
    fn cas_ca22_hub_upstream_handshake_presents_the_token() {
        let request = upstream_request(4567, Some("abc123")).unwrap();
        assert_eq!(request.uri().to_string(), "ws://127.0.0.1:4567/");
        assert_eq!(presented_token(&request).as_deref(), Some("abc123"));
        let anonymous = upstream_request(4567, None).unwrap();
        assert_eq!(presented_token(&anonymous), None);
    }
}
