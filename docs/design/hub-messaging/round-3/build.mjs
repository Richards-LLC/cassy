// Emits the round-3 Pebble state screens from one set of hand-authored
// fragments, so every state shows the same session and the same shape system.
// Run: node docs/design/hub-messaging/round-3/build.mjs
import { writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));

const M = {
  atlas: { cls: 'm-atlas', name: 'Atlas', initial: 'A' },
  studio: { cls: 'm-studio', name: 'Studio Mac', initial: 'S' },
  bench: { cls: 'm-bench', name: 'Bench', initial: 'B' },
};

const TICK = `<svg class="tick" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M2.6 8.6l3.3 3.3L13.4 4.4"/></svg>`;
const SHIELD = `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 1.6l5 2v4.1c0 3-2 5.6-5 6.7-3-1.1-5-3.7-5-6.7V3.6z"/><path d="M5.7 7.9L7.4 9.6l3.1-3.4"/></svg>`;
const ARROW = `<svg viewBox="0 0 20 20" fill="currentColor"><path d="M2.4 9.1l14.3-6.4c.7-.3 1.4.4 1.1 1.1l-6.4 14.3c-.3.7-1.4.6-1.5-.2l-.8-5.3-5.3-.8c-.8-.1-.9-1.2-.2-1.5z"/></svg>`;
const PAPERCLIP = `<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M14.8 9.1l-5 5a3.2 3.2 0 01-4.6-4.6l6-6a2.1 2.1 0 013 3l-6 6a1 1 0 01-1.4-1.4l5.3-5.3"/></svg>`;
const PLUS = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>`;

const page = (title, body, klass = '') => `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${title}</title>
<link rel="stylesheet" href="../../../../hub-web/src/tokens.css">
<link rel="stylesheet" href="pebble.css">
</head>
<body${klass ? ` class="${klass}"` : ''}>
${body}
</body>
</html>
`;

// ---- conversation list ---------------------------------------------------

const ROWS = [
  { m: 'atlas', proj: 'cas-src', when: '09:58', hot: true, flag: true, bold: true,
    prev: 'Fix the warning in-train, or ship allowlisted?' },
  { m: 'studio', proj: 'gabber-studio', count: '2', bold: true,
    prev: 'Pass two is green — every pack in one place.' },
  { m: 'atlas', proj: 'petra-stella-cloud', when: 'Tue', prev: 'Preview is up for the alias merge.' },
  { m: 'studio', proj: 'openclaw', when: 'Tue', prev: 'Rebased and pushed; nothing waiting.' },
  { m: 'bench', proj: 'mecha-cassy', when: 'Mon', prev: 'Posted both threads to the channel.' },
  { m: 'bench', proj: 'cas-hub-static', when: 'Mon', prev: 'Nothing waiting on you.' },
];

const row = (r, on) => {
  const m = M[r.m];
  const right = r.count ? `<span class="pipcount">${r.count}</span>`
    : `<span class="when${r.hot ? ' hot' : ''}">${r.when}</span>`;
  const tail = r.flag ? `<span class="flag"></span>` : '';
  return `      <a class="row ${m.cls}${on ? ' on' : ''}" href="#">
        <span class="mono">${m.initial}</span>
        <span class="who"><b>${m.name}</b><span class="sep"></span><span class="proj">${r.proj}</span></span>
        ${right}
        <span class="prev${r.bold ? ' bold' : ''}">${r.prev}</span>
        ${tail}
      </a>`;
};

const rail = (onIndex) => `    <aside class="rail">
      <h1>Conversations</h1>
${ROWS.map((r, i) => row(r, i === onIndex)).join('\n')}
    </aside>`;

const fab = `    <div class="fab">${PLUS}</div>`;

// ---- thread chrome -------------------------------------------------------

const thead = (mKey, proj) => {
  const m = M[mKey];
  return `      <header class="thead">
        <span class="mono">${m.initial}</span>
        <div class="id"><b>${m.name}</b><span>${proj}</span></div>
      </header>`;
};

const composer = (mKey) => `      <div class="composer">
        <div class="clip">${PAPERCLIP}</div>
        <div class="field">Message ${M[mKey].name}</div>
        <div class="send">${ARROW}</div>
      </div>`;

// ---- message parts -------------------------------------------------------

const you = (text, time) => `        <div class="turn you">
          <div class="bub"><p>${text}</p></div>${time ? `\n          <time>${time}</time>` : ''}
        </div>`;

const sup = (bubbles, time) => `        <div class="turn">
${bubbles.map((b) => `          ${b}`).join('\n')}${time ? `\n          <time>${time}</time>` : ''}
        </div>`;

const plain = (text) => `<div class="bub"><p>${text}</p></div>`;
const receipt = (text) => `<div class="bub receipt">${TICK}<p>${text}</p></div>`;

const coalesce = (text) => `        <div class="coalesce">${text}</div>`;

const working = `        <div class="turn">
          <div class="working"><span class="dots"><i></i><i></i><i></i></span>working</div>
        </div>`;

const ASK_TEXT = 'Gate run 33512 failed on that one warning. Fix it in-train — one worker, about ten minutes — or ship 3.26.0 with it allowlisted?';
const BLK_TEXT = 'The release gate went red. The train is held; nothing was tagged or pushed to main.';
const BLK_EVIDENCE = 'attention.rs:212 · needless_borrow';

// attention object, treatment A: one fused stepped object
const askA = `<div class="obj t-a">
            <div class="obj-body"><p>${ASK_TEXT}</p></div>
            <div class="obj-foot"><span class="chip">Fix in-train</span><span class="chip">Ship with allowlist</span></div>
          </div>`;
const blkA = `<div class="obj t-a blk">
            <div class="obj-body"><p>${BLK_TEXT}</p></div>
            <div class="obj-foot"><code class="window">${BLK_EVIDENCE}</code></div>
          </div>`;

// attention object, treatment B: flatter slab with a real pointed tail
const askB = `<div class="slabobj ask">
            <p>${ASK_TEXT}</p>
            <div class="picks"><span class="pick">Fix in-train</span><span class="pick">Ship with allowlist</span></div>
          </div>`;
const blkB = `<div class="slabobj blk">
            <p>${BLK_TEXT}</p>
            <code class="wireline">${BLK_EVIDENCE}</code>
          </div>`;

// attachment object, treatment A: a dog-eared sheet
const sheetA = `<div class="sheet">
            <span class="plate">PDF</span>
            <span>
              <span class="fname">3.26.0 release brief.pdf</span>
              <span class="fsub">1.4 MB<span class="verified">${SHIELD}hash verified</span></span>
            </span>
          </div>`;

// attachment object, treatment B: a two-tone plate slab
const slabB = `<div class="slab">
            <span class="slab-plate">PDF</span>
            <span class="slab-main">
              <span class="fname">3.26.0 release brief.pdf</span>
              <span class="fsub">1.4 MB<span class="verified">${SHIELD}hash verified</span></span>
            </span>
          </div>`;

// ---- the Atlas session ---------------------------------------------------

const atlasThread = (t) => [
  `        <p class="day">Today</p>`,
  you('Merge the two green lanes, then cut 3.26.0 once the gate is green.', '09:41'),
  sup([
    plain('On it. Both lanes are green on their own CI — the gate starts after the second merge lands.'),
    receipt('Both lanes are on the epic branch. The release gate is running.'),
  ], '09:47'),
  coalesce('3 more updates · gate 11 of 14 targets green'),
  sup([t === 'a' ? sheetA : slabB], '09:52'),
  you('Did the tokens drift test move?', '09:56'),
  sup([plain("No — unchanged since 3.25.3. The gate's only new failure is one lint warning.")]),
  sup([t === 'a' ? blkA : blkB]),
  sup([t === 'a' ? askA : askB], '09:58'),
  working,
].join('\n');

const atlasTail = (t) => [
  coalesce('3 more updates · gate 11 of 14 targets green'),
  sup([t === 'a' ? sheetA : slabB], '09:52'),
  you('Did the tokens drift test move?', '09:56'),
  sup([plain("No — unchanged since 3.25.3. The gate's only new failure is one lint warning.")]),
  sup([t === 'a' ? askA : askB], '09:58'),
  working,
].join('\n');

// ---- evidence table ------------------------------------------------------

const EVI = [
  ['core', '412', 'pass'], ['ui', '388', 'pass'], ['net', '211', 'pass'],
  ['store', '174', 'pass'], ['hooks', '96', 'pass'], ['mcp', '143', 'pass'],
  ['hub', '260', 'pass'], ['cli', '318', '1 flake'],
];
const eviBubble = `<div class="bub">
            <p>Yes — pass two is green. Every pack:</p>
            <div class="evi">
              <div class="evi-row evi-head"><span>pack</span><span>cases</span><span>result</span></div>
${EVI.map(([p, c, r]) => `              <div class="evi-row"><span>${p}</span><span>${c}</span><span class="${r === 'pass' ? 'pass' : 'flake'}">${r}</span></div>`).join('\n')}
            </div>
          </div>`;

// ---- pages ---------------------------------------------------------------

const threadPage = (t) => page(`Pebble — thread ${t.toUpperCase()}`, `  <div class="app show-thread m-atlas">
${rail(0)}
    <main class="thread">
${thead('atlas', 'cas-src')}
      <div class="msgs">
${atlasThread(t)}
      </div>
${composer('atlas')}
    </main>
  </div>`);

const listPage = page('Pebble — conversation list', `  <div class="app show-list m-atlas">
${rail(0)}
    <main class="thread">
${thead('atlas', 'cas-src')}
      <div class="msgs">
${atlasTail('a')}
      </div>
${composer('atlas')}
    </main>
  </div>
${fab}`);

const evidencePage = page('Pebble — long evidence', `  <div class="app show-thread m-studio">
${rail(1)}
    <main class="thread">
${thead('studio', 'gabber-studio')}
      <div class="msgs">
        <p class="day">Today</p>
${you('Did pass two clear the flake?', '09:28')}
${sup([eviBubble, receipt('Tagged gabber-studio v2.4.1 and pushed.')], '09:31')}
${working}
      </div>
${composer('studio')}
    </main>
  </div>`);

const emptyPage = page('Pebble — nothing waiting', `  <div class="app show-thread m-bench">
${rail(5)}
    <main class="thread">
${thead('bench', 'cas-hub-static')}
      <div class="empty">
        <span class="mono">B</span>
        <b>Bench</b>
        <span class="proj2">cas-hub-static</span>
        <p class="said">Nothing waiting on you. Bench will write here when it needs a decision.</p>
        <div class="quiet">Promoted the hub to production on Monday.</div>
      </div>
${composer('bench')}
    </main>
  </div>`);

const one = (cap, html) => `        <div class="one">
          ${html}
          <span class="cap">${cap}</span>
        </div>`;

const pairsPage = page('Pebble — A/B pairs', `  <div class="pairs m-atlas">

    <section class="pair">
      <h2>Ask — silhouette</h2>
      <div class="side">
${one('A · fused tray: body steps down into a deeper tray that holds the replies, one outline', askA)}
${one('B · tailed slab: flatter corners and a real pointed tail, replies inside the field', askB)}
${one('for scale · an ordinary calm pebble', plain('Both lanes are on the epic branch.'))}
      </div>
    </section>

    <section class="pair">
      <h2>Blocker — silhouette</h2>
      <div class="side">
${one('A · fused object with an inset window for the evidence', blkA)}
${one('B · tailed slab with the evidence as an outlined line', blkB)}
      </div>
    </section>

    <section class="pair">
      <h2>Attachment — object</h2>
      <div class="side">
${one('A · sheet: its own surface, dog-eared corner, no bubble around it', sheetA)}
${one('B · plate slab: the silhouette itself splits into plate and surface', slabB)}
      </div>
    </section>

  </div>`);

const FILES = {
  'thread-a.html': threadPage('a'),
  'thread-b.html': threadPage('b'),
  'list.html': listPage,
  'evidence.html': evidencePage,
  'empty.html': emptyPage,
  'pairs.html': pairsPage,
};

for (const [name, html] of Object.entries(FILES)) {
  await writeFile(resolve(here, name), html, 'utf8');
  console.log(`wrote ${name} (${html.length} bytes)`);
}
