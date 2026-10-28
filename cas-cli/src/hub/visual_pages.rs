//! Visual replies (cas-3aff): a supervisor's HTML page, served by the hub in
//! a locked-down sandbox so Commander can show it inline.
//!
//! The page is untrusted agent output. Threat model:
//! `docs/design/hub-web/visual-replies-threat-model.md`. The controls that
//! live here:
//!
//! - A page is reached only through a short-lived ticket that a
//!   DPoP-authenticated `SessionRead` request minted for one session, one page
//!   artifact and one exact Commander origin. An iframe or a new tab can't
//!   carry DPoP headers, so the ticket in the path is the capability.
//! - Every response carries [`page_csp`]: CSP `sandbox` without
//!   `allow-scripts` or `allow-same-origin` (no script, opaque origin, no
//!   popups or top navigation), no network beyond the hub's own assets and
//!   `data:`, and `frame-ancestors` naming only the minting origin.
//! - The page itself must be `text/html`; sibling artifacts it references are
//!   served only for the passive types in [`sub_asset_allowed`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// How long a minted page ticket opens the page and its assets.
pub(crate) const PAGE_TICKET_TTL: Duration = Duration::from_secs(10 * 60);

/// The page policy, shared byte for byte with the browser journey's machine
/// double so the two can't drift apart.
const PAGE_CSP_TEMPLATE: &str = include_str!("visual_page_csp.txt");

/// The CSP for a page or sub-asset framed by `frame_ancestor` (an exact,
/// already-validated origin such as `https://hub.example`).
pub(crate) fn page_csp(frame_ancestor: &str) -> String {
    PAGE_CSP_TEMPLATE
        .trim()
        .replace("{frame_ancestor}", frame_ancestor)
}

/// Whether a sibling artifact of this type may load inside a page: passive
/// media, fonts and stylesheets only. A second HTML document, script, JSON or
/// anything unknown is refused.
pub(crate) fn sub_asset_allowed(mime: &str) -> bool {
    let mime = mime
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    ["image/", "video/", "audio/", "font/"]
        .iter()
        .any(|prefix| mime.starts_with(prefix))
        || mime == "text/css"
}

/// Whether this artifact type is a page.
pub(crate) fn is_page(mime: &str) -> bool {
    mime.split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .eq_ignore_ascii_case("text/html")
}

/// What a ticket opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageTicket {
    pub session: String,
    pub page: String,
    /// The exact Commander origin allowed to frame the page.
    pub origin: String,
    pub expires: Instant,
}

/// Minted tickets, in memory only: a hub restart drops them all.
#[derive(Clone, Default)]
pub(crate) struct PageTickets {
    tickets: Arc<Mutex<HashMap<String, PageTicket>>>,
}

impl PageTickets {
    /// Mint a ticket and return it with its expiry.
    pub fn issue(&self, session: &str, page: &str, origin: &str, now: Instant) -> (String, Instant) {
        let ticket = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
        let expires = now + PAGE_TICKET_TTL;
        let mut tickets = self.tickets.lock().unwrap_or_else(|poison| poison.into_inner());
        tickets.retain(|_, held| held.expires > now);
        tickets.insert(
            ticket.clone(),
            PageTicket {
                session: session.to_owned(),
                page: page.to_owned(),
                origin: origin.to_owned(),
                expires,
            },
        );
        (ticket, expires)
    }

    /// The live ticket, or `None` when it is unknown or expired.
    pub fn resolve(&self, ticket: &str, now: Instant) -> Option<PageTicket> {
        let mut tickets = self.tickets.lock().unwrap_or_else(|poison| poison.into_inner());
        tickets.retain(|_, held| held.expires > now);
        tickets.get(ticket).cloned()
    }
}

/// One artifact's bytes and recorded type, as the page route serves them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageBytes {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Fetches one artifact of one session for the page route. Blocking: the
/// route calls it on the blocking pool. `None` means unknown or unavailable;
/// the route answers a plain 404 either way, so a ticket holder learns nothing
/// about other artifacts.
pub(crate) type PageFetch = Arc<dyn Fn(&str, &str) -> Option<PageBytes> + Send + Sync>;

/// The production fetch: the session's artifact record, its bytes from the
/// committed Cloud copy, checked against the recorded digest and size cap.
pub(crate) fn default_page_fetch() -> PageFetch {
    Arc::new(|session, artifact| fetch_committed(session, artifact).ok().flatten())
}

fn fetch_committed(session: &str, artifact: &str) -> anyhow::Result<Option<PageBytes>> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let session = crate::bridge::server::session::resolve_session_by_name(session)?;
    let root = crate::bridge::server::session::cas_root_for_session_with_fallback(&session, None)?;
    let store = cas_store::SqliteArtifactStore::open(&root)?;
    let client = crate::artifacts::cloud_client(&root);
    let Ok(signed) = crate::artifacts::signed_view(&store, client.as_ref(), artifact) else {
        return Ok(None);
    };
    // The signed URL is itself a credential: it is never logged.
    let response = ureq::get(&signed.view.url)
        .timeout(Duration::from_secs(60))
        .call()?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(cas_types::ARTIFACT_MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= cas_types::ARTIFACT_MAX_BYTES,
        "artifact exceeds the size cap"
    );
    let digest = hex::encode(Sha256::digest(&bytes));
    anyhow::ensure!(
        digest.eq_ignore_ascii_case(&signed.artifact.sha256),
        "artifact bytes do not match the recorded digest"
    );
    Ok(Some(PageBytes {
        mime: signed.artifact.mime,
        bytes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_opens_only_until_it_expires() {
        let tickets = PageTickets::default();
        let now = Instant::now();
        let (ticket, expires) = tickets.issue("factory-a", "art-page", "https://controller.example", now);
        assert_eq!(expires, now + PAGE_TICKET_TTL);
        assert_eq!(tickets.resolve(&ticket, now).unwrap().origin, "https://controller.example");
        assert_eq!(tickets.resolve(&ticket, expires), None);
        assert_eq!(tickets.resolve("not-a-ticket", now), None);
    }
}
