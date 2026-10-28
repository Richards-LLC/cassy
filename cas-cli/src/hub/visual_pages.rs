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
//! - Bytes come through the shared session artifact path (`artifact_proxy`):
//!   only committed refs attached to that session's thread, within the size
//!   cap, with Range, never forwarding a bearer. The page must be `text/html`;
//!   siblings only the passive types in [`SUB_ASSET_MIME_ALLOWLIST`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::AuthContext;

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

/// The page itself: HTML only. Passed to the shared artifact byte path
/// (`artifact_proxy`), which keeps the thread, size, Range and no-bearer rules.
pub(crate) const PAGE_MIME_ALLOWLIST: &[&str] = &["text/html"];

/// Sibling artifacts a page may load: passive media, fonts and stylesheets.
/// A second HTML document, script, JSON or anything unknown is refused.
pub(crate) const SUB_ASSET_MIME_ALLOWLIST: &[&str] =
    &["image/*", "video/*", "audio/*", "font/*", "text/css"];

/// What a ticket opens, and on whose authority: the minting device's
/// context, rechecked live on every fetch, so revoking it closes the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageTicket {
    pub session: String,
    pub page: String,
    pub context: AuthContext,
    pub expires: Instant,
}

/// Minted tickets, in memory only: a hub restart drops them all.
#[derive(Clone, Default)]
pub(crate) struct PageTickets {
    tickets: Arc<Mutex<HashMap<String, PageTicket>>>,
}

impl PageTickets {
    /// Mint a ticket and return it with its expiry.
    pub fn issue(
        &self,
        session: &str,
        page: &str,
        context: &AuthContext,
        now: Instant,
    ) -> (String, Instant) {
        let ticket = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
        let expires = now + PAGE_TICKET_TTL;
        let mut tickets = self
            .tickets
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        tickets.retain(|_, held| held.expires > now);
        tickets.insert(
            ticket.clone(),
            PageTicket {
                session: session.to_owned(),
                page: page.to_owned(),
                context: context.clone(),
                expires,
            },
        );
        (ticket, expires)
    }

    /// The live ticket, or `None` when it is unknown or expired.
    pub fn resolve(&self, ticket: &str, now: Instant) -> Option<PageTicket> {
        let mut tickets = self
            .tickets
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        tickets.retain(|_, held| held.expires > now);
        tickets.get(ticket).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_opens_only_until_it_expires() {
        let tickets = PageTickets::default();
        let now = Instant::now();
        let context = AuthContext {
            device_id: "device".into(),
            credential_id: "credential".into(),
            device_label: "Phone".into(),
            operator_label: "Daniel".into(),
            controller_origin: "https://controller.example".into(),
            scopes: Default::default(),
            request_id: "request".into(),
        };
        let (ticket, expires) = tickets.issue("factory-a", "art-page", &context, now);
        assert_eq!(expires, now + PAGE_TICKET_TTL);
        assert_eq!(tickets.resolve(&ticket, now).unwrap().context, context);
        assert_eq!(tickets.resolve(&ticket, expires), None);
        assert_eq!(tickets.resolve("not-a-ticket", now), None);
    }
}
