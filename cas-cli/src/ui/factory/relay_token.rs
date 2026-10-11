//! Commander relay token (cas-ca22). Red stub.

use std::path::{Path, PathBuf};

/// WebSocket handshake header the hub relay presents to a session daemon.
pub(crate) const RELAY_TOKEN_HEADER: &str = "x-cas-relay-token";

pub(crate) fn token_path(hub_root: &Path, session: &str) -> PathBuf {
    hub_root
        .join("relay-tokens")
        .join(format!("{session}.token"))
}

pub(crate) fn mint(_hub_root: &Path, _session: &str) -> anyhow::Result<String> {
    Ok(String::new())
}

pub(crate) fn read(_hub_root: &Path, _session: &str) -> Option<String> {
    None
}

pub(crate) fn remove(_hub_root: &Path, _session: &str) {}

pub(crate) fn matches(_presented: Option<&str>, _expected: &str) -> bool {
    true
}

pub(crate) type HandshakeRequest = tokio_tungstenite::tungstenite::handshake::client::Request;

/// The hub's upstream request to a session daemon, carrying the relay token.
pub(crate) fn upstream_request(
    port: u16,
    _token: Option<&str>,
) -> anyhow::Result<HandshakeRequest> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    Ok(format!("ws://127.0.0.1:{port}").into_client_request()?)
}

/// The relay token a WebSocket client presented in its handshake, if any.
pub(crate) fn presented_token(_request: &HandshakeRequest) -> Option<String> {
    None
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
