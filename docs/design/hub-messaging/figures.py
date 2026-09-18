"""Figure builders, CSS and JS for the hub messaging study (consumed by build.py)."""
import html, json, re
from pathlib import Path
from build import HERE, REFS, MOCK, MD, data_uri, inline

# Measured with measure-pane.py on three supervisor transcripts (see Provenance in the markdown).
WAFFLE = [('supervisor prose', 15, 'verdict'), ('your directives', 3, 'action'), ('tool traffic and injected mail', 82, 'rest')]
CHARS = [('supervisor prose', 2.4), ('your directives', 1.1), ('tool traffic and injected mail', 96.5)]

def head_meta():
    return '<meta name="description" content="Hub messaging design study: reference gallery, message taxonomy, mockups and the seam for typed supervisor→operator messages.">\n'

def waffle_svg():
    cells, k = [], 0
    order = []
    for label, n, cls in WAFFLE: order += [cls] * n
    for row in range(10):
        for col in range(10):
            cls = order[k]; k += 1
            x, y = 6 + col * 24, 6 + row * 24
            cells.append(f'<rect class="w-{cls}" x="{x}" y="{y}" width="20" height="20" rx="2"/>')
    desc = '; '.join(f'{n} of 100 are {label}' for label, n, _ in WAFFLE)
    return ('<svg class="waffle" viewBox="0 0 252 252" role="img" aria-labelledby="waffle-title waffle-desc" preserveAspectRatio="xMidYMid meet">'
            f'<title id="waffle-title">Share of pane content blocks by author, three supervisor sessions</title><desc id="waffle-desc">{desc}.</desc>' + ''.join(cells) + '</svg>')

def hero():
    legend = ''.join(f'<li><span class="sw w-{cls}" aria-hidden="true"></span><span class="n">{n}</span><span>{html.escape(label)}</span></li>' for label, n, cls in WAFFLE)
    return f'''<header class="hero" id="top">
<div class="hero-inner">
<h1 class="doc-title">{html.escape(re.search(r'^# (.+)$', MD.read_text(encoding='utf-8'), re.M).group(1))}</h1>
<p class="eyebrow">Design study · 2026-09-18 · hub conversation · for the operator</p>
<p class="verdict">Build the Ledger: the supervisor's typed turns on a ruled timeline, the pane one tap away. Desk and Brief stay live.</p>
<p class="why">Why: it is the surface the operator described — my messages and the supervisor's to me — it keeps history and elapsed time visible, and it needs one new field on an envelope that already exists. Today's mirror is six parts traffic to one part message.</p>
<figure class="hero-figure">
<div class="waffle-wrap">{waffle_svg()}
<ul class="waffle-legend" aria-hidden="true">{legend}</ul></div>
<figcaption><strong>Of every 100 things a supervisor pane prints, 15 are the supervisor's prose.</strong> 5,155 content blocks from three real cas-src supervisor sessions (2026-09-10 → 2026-09-18), classified by <code>measure-pane.py</code>; by character count prose is 2.4 %. Exact counts in the ledger below.</figcaption>
</figure>
<hr class="rule">
<p class="provenance">source: measure-pane.py over three Claude Code supervisor transcripts · captured 2026-09-18 16:05 · worktree factory/calm-stork-91 · markdown twin: <a href="2026-09-18-hub-messaging-study.md">2026-09-18-hub-messaging-study.md</a></p>
</div></header>
<main id="main">'''

def img(path: Path, alt: str, max_w=960, quality=74, cls=''):
    uri, w, h = data_uri(path, max_w, quality)
    return f'<img class="{cls}" src="{uri}" width="{w}" height="{h}" alt="{html.escape(alt, quote=True)}">'

def today():
    items = [('today-conversation-replied-desktop-light.png', '1280 · light', 'Desktop, light: an operator turn, one typed reply, then the raw pane text.'),
             ('today-conversation-replied-phone-dark.png', '390 · dark', 'Phone, dark: the same thread; the pane text is the bulk of the column.'),
             ('today-transcript-desktop-dark.png', 'Transcript · 1280 · dark', 'The transcript view: the emulator reflowed, nothing addressed to anyone.'),
             ('today-conversation-phone-light.png', '390 · light', 'Phone, light, before any operator turn: only pane text.')]
    figs = ''.join(f'<figure class="shot">{img(REFS / f, alt, 720, 70)}<figcaption><span class="eyebrow">{html.escape(cap)}</span> {html.escape(alt)}</figcaption></figure>' for f, cap, alt in items)
    return f'<div class="shots four">{figs}</div><p class="provenance">Real renders of the checked-in fixtures (<code>hub-web/fixtures/conversations.ts</code>) through <code>capture-today.mjs</code>, worktree tip at capture time; the fixture pane text is the fixture\'s own.</p>'

def parse_refs_table():
    md = MD.read_text(encoding='utf-8')
    block = re.search(r'\n(\|[^\n]*\n)+Table: References', md).group(0)
    rows = [r for r in block.strip().splitlines() if r.startswith('|') and not re.match(r'^\|\s*:?-', r)]
    cells = [[c.strip() for c in r.strip().strip('|').split('|')] for r in rows]
    return cells[0], cells[1:]

def refs_gallery():
    _, rows = parse_refs_table()
    files = {p.name[:2]: p for p in REFS.glob('[0-9][0-9]-*.png')}
    cards = []
    for r in rows:
        rid, product, category, shows, well, badly, steal = r[:7]
        f = files.get(rid)
        if not f: raise SystemExit(f'missing capture for {rid}')
        cards.append(f'''<figure class="ref" id="ref-{rid}">
{img(f, shows, 720, 68)}
<figcaption><span class="eyebrow">{rid} · {inline(product)} · {inline(category)}</span>
<dl><dt>Does well</dt><dd>{inline(well)}</dd><dt>Does badly</dt><dd>{inline(badly)}</dd><dt class="steal">Stealable</dt><dd class="steal">{inline(steal)}</dd></dl></figcaption></figure>''')
    return '<div class="gallery">' + ''.join(cards) + '</div>'

def mockups():
    rows = [('thread', 'The thread: one directive, an answer, a receipt, coalesced statuses, a question, an answer, a blocker, the pinned ask, the working line.'),
            ('evidence', 'Evidence and the escape hatch: a five-row worker table inline on desktop with the pane open beside it; on the phone the same table as a full-width sheet.')]
    out = []
    for state, desc in rows:
        shots = ''
        for vp, mw in (('desktop', 960), ('phone', 480)):
            for scheme in ('light', 'dark'):
                f = MOCK / f'{state}-{vp}-{scheme}.png'
                alt = f'Mockup, {state} state, {"1280 desktop" if vp == "desktop" else "390 phone"}, {scheme} scheme.'
                shots += f'<figure class="shot {vp}">{img(f, alt, mw, 74)}<figcaption><span class="eyebrow">{"1280" if vp == "desktop" else "390"} · {scheme}</span></figcaption></figure>'
        out.append(f'<div class="mock-row"><p class="mock-desc"><strong>{state.capitalize()}.</strong> {html.escape(desc)}</p><div class="shots mock">{shots}</div></div>')
    return ''.join(out) + '<p class="provenance">Hand-built HTML (<code>mockups/thread.html</code>) on <code>hub-web/src/tokens.css</code>, rendered by <code>mockups/render.mjs</code> in headless Chromium at 1280 and 390 px, light and dark. Names, times and SHAs are illustrative.</p>'

SEAM = [
    ('1', 'Operator types', 'hub-web/src/main.ts:1867', 'submitSupervisorMessage() → deliverSupervisorMessage()', ''),
    ('2', 'One WebSocket frame', 'hub-web/src/supervisor-message.ts:8', '{ SendMessage: { client_ref, target, text, summary, urgent:false, attribution:{…null} } }', 'scope message-send (pairing-scopes.ts:6) plus a session lease (supervisor-message.ts:120)'),
    ('3', 'Hub stamps identity', 'cas-cli/src/hub/server.rs:883', '*attribution = verified_attribution(context) — operator_verified: true at :919', 'the only place the operator becomes verified'),
    ('4', 'Queue row', 'cas-cli/src/ui/factory/daemon/runtime/delivery.rs:537', 'queue.enqueue_operator_message(…) → prompt_queue (prompt_queue_store.rs:349)', ''),
    ('5', 'Header + inject', 'cas-cli/src/mcp/tools/service/agent_search_system/message.rs:97', '"[cas #N operator X@Y verified …]" then mux.inject (delivery.rs:898) or teams.write_to_inbox (:879)', ''),
    ('6', 'Supervisor answers', 'cas-cli/src/builtins/skills/cas-supervisor/references/reference.md:30', 'action=message target=operator in_reply_to=N', ''),
    ('7', 'The guard', 'cas-cli/src/mcp/tools/service/agent_search_system/message.rs:894–906', 'target="operator" requires in_reply_to; supervisor role only', 'The seam. Everything the supervisor wants to say unprompted has no lane, so the hub mirrors the pane.'),
    ('8', 'Typed envelope', 'cas-cli/src/ui/factory/protocol.rs:70', 'OperatorReplyPayload { schema_version, reply_to, message, summary, device_id, operator_label }', 'this struct grows a kind'),
    ('9', 'Routed to a device', 'crates/cas-store/src/prompt_queue_store.rs:4389', "peek_operator_replies: lower(target)='operator' AND recipient_device_id IS NOT NULL", ''),
    ('10', 'Daemon → hub → browser', 'cas-cli/src/ui/factory/protocol.rs:297', 'DaemonMessage::OperatorReply → connection.ts:987 onOperatorReply', ''),
    ('11', 'One branch in the DOM', 'hub-web/src/conversation-view.ts:82', 'event.kind === "send" ? "from-you" : "from-supervisor"', 'the branch that becomes a six-way switch'),
]

def seam_timeline():
    items = ''
    for n, title, cite, what, note in SEAM:
        decisive = ' decisive' if n == '7' else ''
        note_html = f'<p class="note">{html.escape(note)}</p>' if note else ''
        items += f'<li class="hop{decisive}"><span class="hop-n">{n}</span><div class="hop-body"><p class="hop-title">{html.escape(title)} <code class="cite">{html.escape(cite)}</code></p><p class="hop-what"><code>{html.escape(what)}</code></p>{note_html}</div></li>'
    return f'<figure class="timeline"><figcaption><strong>The message path today, as eleven file:line hops.</strong> Hops 1–5 carry an operator message down; 6–11 carry the only typed reply back. Hop 7 is the seam.</figcaption><ol class="hops">{items}</ol><p class="provenance">Every hop verified by reading the cited lines in the worktree on 2026-09-18 (see Provenance).</p></figure>'

OPTIONS = {
    'ledger': ('A · Ledger', [('thread', 'The thread: directive, answer, receipt, coalesced statuses, question, answer, blocker, the pinned ask, the working line.'), ('evidence', 'Evidence and the escape hatch: the worker table inline with the pane open beside the thread on desktop; the same table as a full-width sheet on the phone.')]),
    'desk': ('B · Desk', [('queue', 'The queue: asks, blockers and your unanswered questions across machines, oldest first; the selected ask opens with the blocker and receipt the supervisor attached, the statuses since, and a scoped answer box.'), ('item', 'Done items: a receipt opened from the Done list with the directive that led to it and its evidence table; on the phone, the ask page with its answer box.')]),
    'brief': ('C · Brief', [('now', 'The page: Now, Decisions needed, Outcomes since 09:30, Answers, Status — with your directives as margin notes on desktop and inline notes on the phone; the live phone composer stays pinned to the bottom edge.'), ('history', 'Earlier: what the page said before, with replaced statements struck through and kept ones plain.')]),
}

def option(key):
    label, states = OPTIONS[key]
    rows = ''
    for state, desc in states:
        plates = ''
        for scheme in ('light', 'dark'):
            for vp, mw in (('phone', 440), ('desktop', 880)):
                f = MOCK / f'{key}-{state}-{vp}-{scheme}.png'
                alt = f'{label}, {state} screen, {"390 × 844 phone" if vp == "phone" else "1280 × 800 desktop"}, {scheme} scheme: {desc}'
                plates += f'<figure class="plate {vp}">{img(f, alt, mw, 72)}<figcaption class="plate-label">{"Mobile · 390" if vp == "phone" else "Desktop · 1280"} · {scheme}</figcaption></figure>'
        rows += f'<div class="plate-row"><p class="plate-desc"><span class="eyebrow">{html.escape(label)} · {html.escape(state)}</span> {html.escape(desc)}</p><div class="plates">{plates}</div></div>'
    return f'<div class="option-plates" id="plates-{key}">{rows}</div>'

FIGURES = {'hero': hero, 'today': today, 'refs-gallery': refs_gallery, 'option-ledger': lambda: option('ledger'), 'option-desk': lambda: option('desk'), 'option-brief': lambda: option('brief'), 'seam-timeline': seam_timeline, 'main-end': lambda: '</main>'}

CSS = r'''
:root { color-scheme: light dark;
  --bg:#F7F4EE; --surface:#FFFFFF; --surface-hero:#F1E7DD; --line:#DAD3C7; --line-strong:#8F8371; --ink:#1B1D24; --ink-muted:#5A5F6E;
  --verdict:#2E3A9F; --verdict-soft:#DDE1F7; --evidence:#5A5F6E; --action:#2E3A9F; --good:#226845; --warning:#7F5504; --danger:#B3261E;
  --good-tint:rgba(34,104,69,0.10); --warning-tint:rgba(127,85,4,0.10); --danger-tint:rgba(179,38,30,0.10); --focus:#2E3A9F;
  --display:"Iowan Old Style","Palatino Linotype",Palatino,"Book Antiqua",Georgia,"Times New Roman",serif;
  --body:Inter,ui-sans-serif,system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;
  --mono:"JetBrains Mono","IBM Plex Mono",ui-monospace,SFMono-Regular,Menlo,Consolas,monospace; }
@media (prefers-color-scheme: dark) { :root {
  --bg:#12141A; --surface:#191C24; --surface-hero:#241E1E; --line:#2B3040; --line-strong:#6B7390; --ink:#E9E6E0; --ink-muted:#A3A7B4;
  --verdict:#A9B3FF; --verdict-soft:rgba(169,179,255,0.16); --evidence:#A3A7B4; --action:#A9B3FF; --good:#5FC492; --warning:#E2B14D; --danger:#EF7B72;
  --good-tint:rgba(95,196,146,0.14); --warning-tint:rgba(226,177,77,0.14); --danger-tint:rgba(239,123,114,0.14); --focus:#A9B3FF; } }
* { box-sizing: border-box; }
html { background: var(--bg); color: var(--ink); }
body { margin: 0; font: 400 17px/26px var(--body); background: var(--bg); color: var(--ink); }
.skip { position: absolute; left: -999px; top: 8px; background: var(--surface); color: var(--ink); padding: 8px 12px; }
.skip:focus { left: 8px; z-index: 9; }
a { color: var(--action); } a:focus-visible, button:focus-visible, summary:focus-visible, img:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
main { max-width: 1120px; margin: 0 auto; padding: 0 24px 96px; }
h1, h2, h3 { font-family: var(--body); font-weight: 600; letter-spacing: -0.01em; color: var(--ink); }
h1 { font: 400 34px/38px var(--display); margin: 0 0 8px; }
h2 { font-size: 27px; line-height: 32px; margin: 64px 0 16px; padding-top: 24px; border-top: 1px solid var(--line-strong); }
h3 { font-size: 21px; line-height: 30px; margin: 40px 0 8px; }
p, li { max-width: 68ch; }
p { margin: 0 0 16px; }
ul, ol { padding-left: 24px; margin: 0 0 16px; }
li { margin: 0 0 6px; }
code { font: 400 15px/22px var(--mono); background: var(--surface); color: var(--ink); padding: 0 4px; border-radius: 4px; overflow-wrap: anywhere; }
pre { background: var(--surface); color: var(--ink); border: 1px solid var(--line); padding: 12px 16px; overflow-x: auto; border-radius: 8px; max-width: 100%; }
pre code { background: transparent; padding: 0; white-space: pre; }
blockquote { margin: 0 0 16px; padding: 12px 24px; background: var(--surface-hero); color: var(--ink); border-left: 3px solid var(--verdict); }
blockquote p { font: italic 400 21px/30px var(--display); margin: 0; }
.eyebrow { font: 600 12px/16px var(--mono); letter-spacing: 0.08em; text-transform: uppercase; color: var(--ink-muted); }
.provenance { font: 400 13px/18px var(--mono); color: var(--ink-muted); max-width: none; }
/* hero */
.hero { background: var(--surface-hero); color: var(--ink); padding: 48px 24px 24px; }
.hero-inner { max-width: 1120px; margin: 0 auto; display: grid; grid-template-columns: minmax(0, 1fr) 420px; grid-template-areas: "title title" "eyebrow figure" "verdict figure" "why figure" "rule rule" "prov prov"; column-gap: 48px; row-gap: 16px; align-items: start; }
.hero .eyebrow { grid-area: eyebrow; margin: 0; align-self: end; }
.hero .doc-title { grid-area: title; font: 400 21px/30px var(--display); color: var(--ink-muted); margin: 0; }
.hero .verdict { grid-area: verdict; font: 400 clamp(28px, 3.4vw, 42px)/1.08 var(--display); letter-spacing: -0.015em; max-width: 22ch; margin: 0; }
.hero-figure { margin: 0; grid-area: figure; }
.hero .why { grid-area: why; font: 400 15px/22px var(--body); color: var(--ink); max-width: 60ch; margin: 0; }
.hero .why::first-line { font-weight: 600; }
.hero .rule { grid-area: rule; border: 0; height: 3px; background: var(--verdict); margin: 8px 0 0; }
.hero .provenance { grid-area: prov; margin: 0; }
.waffle-wrap { display: grid; grid-template-columns: 200px minmax(0,1fr); gap: 16px; align-items: center; }
.waffle { width: 200px; height: 200px; display: block; }
.w-verdict { fill: var(--verdict); } .w-action { fill: none; stroke: var(--action); stroke-width: 2; } .w-rest { fill: var(--line); }
.sw { display: inline-block; width: 12px; height: 12px; border-radius: 2px; margin-right: 8px; vertical-align: -1px; }
.sw.w-verdict { background: var(--verdict); } .sw.w-action { background: transparent; border: 2px solid var(--action); } .sw.w-rest { background: var(--line); }
.waffle-legend { list-style: none; padding: 0; margin: 0; font: 400 14px/20px var(--body); color: var(--ink); }
.waffle-legend li { margin: 0 0 8px; display: flex; align-items: baseline; gap: 4px; }
.waffle-legend .n { font: 500 15px/20px var(--mono); font-variant-numeric: tabular-nums; min-width: 2.5ch; text-align: right; margin-right: 4px; }
.hero-figure figcaption { font: 400 14px/20px var(--body); color: var(--ink-muted); margin-top: 12px; }
.hero-figure figcaption strong { color: var(--ink); font-weight: 600; }
/* tables */
.table-scroll { overflow-x: auto; margin: 0 0 24px; max-width: 100%; }
table { border-collapse: collapse; width: 100%; font: 400 15px/22px var(--body); }
caption { text-align: left; caption-side: top; padding: 0 0 8px; color: var(--ink-muted); font: 400 14px/20px var(--body); }
th { text-align: left; font: 600 12px/16px var(--mono); letter-spacing: 0.08em; text-transform: uppercase; color: var(--ink-muted); padding: 8px 12px 8px 0; border-bottom: 1px solid var(--line-strong); vertical-align: bottom; }
td { padding: 8px 12px 8px 0; border-bottom: 1px solid var(--line); vertical-align: top; }
td code { font-size: 14px; }
tr.decisive td { background: var(--verdict-soft); }
tr.sum td { font-weight: 600; border-top: 1px solid var(--line-strong); }
/* shots and gallery */
.shots { display: grid; gap: 24px; margin: 0 0 16px; }
.shots.four { grid-template-columns: repeat(4, minmax(0,1fr)); }
.shots.mock { grid-template-columns: 2fr 2fr 1fr 1fr; align-items: start; }
.shot { margin: 0; min-width: 0; }
.shot img { width: 100%; height: auto; display: block; border: 1px solid var(--line); border-radius: 8px; background: var(--surface); }
.shot figcaption, .ref figcaption { font: 400 14px/20px var(--body); color: var(--ink-muted); margin-top: 8px; }
.mock-row { margin: 0 0 40px; } .mock-desc { max-width: none; }
.gallery { display: grid; grid-template-columns: repeat(2, minmax(0,1fr)); gap: 32px 24px; margin: 0 0 24px; }
.ref { margin: 0; min-width: 0; border-top: 1px solid var(--line-strong); padding-top: 12px; }
.ref img { width: 100%; height: auto; display: block; border: 1px solid var(--line); border-radius: 8px; background: var(--surface); }
.ref figcaption .eyebrow { display: block; margin: 0 0 8px; color: var(--ink); }
.ref dl { margin: 0; display: grid; grid-template-columns: 9ch minmax(0,1fr); gap: 4px 12px; }
.ref dt { font: 600 12px/20px var(--mono); letter-spacing: 0.08em; text-transform: uppercase; color: var(--ink-muted); }
.ref dd { margin: 0; color: var(--ink); }
.ref dt.steal, .ref dd.steal { color: var(--verdict); }
/* option plates (hub-mobile plate format: paired phone + desktop) */
.plate-row { margin: 0 0 32px; }
.plate-desc { max-width: none; margin: 0 0 12px; }
.plate-desc .eyebrow { display: inline-block; margin-right: 8px; color: var(--ink); }
.plates { display: grid; grid-template-columns: 1fr 2.2fr 1fr 2.2fr; gap: 16px; align-items: start; }
.plate { margin: 0; min-width: 0; }
.plate img { width: 100%; height: auto; display: block; border: 1px solid var(--line); border-radius: 8px; background: var(--surface); }
.plate-label { font: 600 12px/16px var(--mono); letter-spacing: 0.08em; text-transform: uppercase; color: var(--ink-muted); margin-top: 8px; }
/* timeline */
.timeline { margin: 0 0 32px; }
.timeline figcaption { margin: 0 0 16px; color: var(--ink-muted); font: 400 15px/22px var(--body); }
.timeline figcaption strong { color: var(--ink); }
.hops { list-style: none; margin: 0; padding: 0 0 0 56px; border-left: 2px solid var(--line-strong); }
.hop { position: relative; margin: 0 0 20px; }
.hop-n { position: absolute; left: -56px; top: 0; width: 40px; text-align: right; font: 500 15px/22px var(--mono); color: var(--ink-muted); }
.hop-n::after { content: ""; position: absolute; right: -15px; top: 7px; width: 8px; height: 8px; border-radius: 999px; background: var(--line-strong); }
.hop.decisive .hop-n { color: var(--verdict); }
.hop.decisive .hop-n::after { background: var(--verdict); width: 12px; height: 12px; right: -17px; top: 5px; }
.hop-title { margin: 0 0 4px; font-weight: 600; max-width: none; }
.hop-title .cite { font-weight: 400; margin-left: 8px; }
.hop-what { margin: 0; max-width: none; }
.hop-what code { font-size: 14px; }
.hop.decisive .hop-body { background: var(--verdict-soft); color: var(--ink); border-left: 3px solid var(--verdict); padding: 12px 16px; margin-left: -16px; border-radius: 0 8px 8px 0; }
.note { font: italic 400 15px/22px var(--display); color: var(--ink); margin: 8px 0 0; max-width: 60ch; }
details { margin: 0 0 24px; } summary { cursor: pointer; font-weight: 600; margin-bottom: 12px; }
dialog { border: 0; padding: 0; background: var(--surface); color: var(--ink); max-width: min(96vw, 1400px); border-radius: 8px; box-shadow: 0 24px 80px rgba(18,20,26,0.40); }
dialog::backdrop { background: rgba(18,20,26,0.72); }
dialog img { max-width: 96vw; max-height: 90vh; width: auto; height: auto; display: block; }
dialog button { position: absolute; top: 8px; right: 8px; height: 40px; padding: 0 16px; font: inherit; background: var(--surface); color: var(--ink); border: 1px solid var(--line-strong); border-radius: 4px; }
footer { max-width: 1120px; margin: 0 auto; padding: 24px; border-top: 1px solid var(--line-strong); }
@media (max-width: 1079px) { .shots.four { grid-template-columns: repeat(2, minmax(0,1fr)); } .shots.mock { grid-template-columns: repeat(2, minmax(0,1fr)); } .plates { grid-template-columns: 1fr 2.2fr; } }
@media (max-width: 819px) {
  .hero { padding: 32px 16px 16px; }
  .hero-inner { grid-template-columns: minmax(0,1fr); grid-template-areas: "title" "eyebrow" "verdict" "figure" "why" "rule" "prov"; }
  .hero .why { font-size: 14px; line-height: 20px; }
  .hero .verdict { font-size: clamp(24px, 6.4vw, 32px); }
  .waffle-wrap { grid-template-columns: 150px minmax(0,1fr); gap: 12px; }
  .waffle { width: 150px; height: 150px; }
  main { padding: 0 16px 64px; }
  h2 { font-size: 24px; line-height: 30px; margin-top: 48px; }
  .shots.four, .shots.mock, .gallery, .plates { grid-template-columns: minmax(0,1fr); }
  .hops { padding-left: 44px; } .hop-n { left: -44px; width: 30px; }
  .hop.decisive .hop-body { margin-left: 0; }
  .ref dl { grid-template-columns: minmax(0,1fr); }
}
@media (prefers-reduced-motion: reduce) { * { transition: none !important; animation: none !important; } }
@media print {
  :root { --bg:#FFFFFF; --surface:#FFFFFF; --surface-hero:#FFFFFF; --line:#DAD3C7; --line-strong:#8F8371; --ink:#1B1D24; --ink-muted:#5A5F6E; --verdict:#2E3A9F; --verdict-soft:#DDE1F7; --action:#2E3A9F; }
  body { font-size: 12px; line-height: 17px; }
  .hero { padding: 16px 0; } .hero-inner { grid-template-columns: minmax(0,1fr); grid-template-areas: "title" "eyebrow" "verdict" "why" "figure" "rule" "prov"; }
  .hero .verdict { font-size: 28px; }
  details { display: block; } details > * { display: block; } details:not([open]) > summary ~ * { display: block; }
  a[href^="http"]::after { content: " (" attr(href) ")"; font-size: 10px; color: var(--ink-muted); }
  .shot, .ref, .hop, tr, figure { break-inside: avoid; }
  thead { display: table-header-group; }
  .shots.four, .shots.mock, .gallery { grid-template-columns: repeat(2, minmax(0,1fr)); }
  .plates { grid-template-columns: 1fr 2.2fr; }
  dialog, .skip { display: none; }
  .table-scroll { overflow: visible; }
}
'''

JS = r'''
(function () {
  // Progressive enhancement only: click any capture to view it large; Escape closes.
  var dlg = document.createElement('dialog'); dlg.setAttribute('aria-label', 'Enlarged capture');
  var im = document.createElement('img'); var btn = document.createElement('button'); btn.type = 'button'; btn.textContent = 'Close';
  dlg.append(im, btn); document.body.append(dlg);
  btn.addEventListener('click', function () { dlg.close(); });
  dlg.addEventListener('click', function (e) { if (e.target === dlg) dlg.close(); });
  document.querySelectorAll('.shot img, .ref img, .plate img').forEach(function (el) {
    el.tabIndex = 0; el.style.cursor = 'zoom-in';
    function open() { im.src = el.src; im.alt = el.alt; dlg.showModal(); }
    el.addEventListener('click', open);
    el.addEventListener('keydown', function (e) { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); open(); } });
  });
  window.addEventListener('beforeprint', function () { document.querySelectorAll('details').forEach(function (d) { d.open = true; }); });
})();
'''
