// Shared project-agnostic walk primitives.
// Project adapters own authentication, selectors, build reads and cleanup.
import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';

export const WALK_KIT_VERSION = '1.2.0';
const SHA = /^[0-9a-f]{7,40}$/;
const CITED = /\b[A-Za-z0-9_./-]+\.[A-Za-z0-9]+:[1-9][0-9]*\b/;
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const hash = file => createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const clean = value => String(value ?? '').replace(/[\r\n]+/g, ' ').replace(/\|/g, '\\|');

export async function requireBuild(expected, readBuild) {
  if (!expected || !Object.keys(expected).length || !Object.values(expected).every(sha => SHA.test(sha)))
    throw new Error('requireBuild: explicit served SHAs required; no fallback');
  const actual = await readBuild();
  for (const [surface, sha] of Object.entries(expected))
    if (typeof actual?.[surface] !== 'string' || !actual[surface].startsWith(sha))
      throw new Error(`HOLD: ${surface} serves ${actual?.[surface] ?? 'unknown'}, expected ${sha}`);
  return actual;
}

// readThread returns rendered {id, user, working, visible, text} bubbles.
export async function waitForReply(page, {
  readThread, before, sentAt, timeoutMs = 180_000, settleMs = 1_000,
  pollMs = 100, now = Date.now, sleep = pause,
}) {
  if (!(before instanceof Set) || !Number.isFinite(sentAt))
    throw new Error('waitForReply: supply pre-send IDs and measured sentAt');
  let seen = '', changedAt = now(), firstReplyMs = null;
  for (;;) {
    const rows = await readThread(page);
    const replies = rows.filter(row => !row.user && !row.working && row.visible === true
      && row.id && !before.has(row.id) && row.text?.trim());
    const working = rows.some(row => row.working);
    const key = JSON.stringify([replies, working]);
    if (key !== seen) { seen = key; changedAt = now(); }
    if (replies.length && firstReplyMs === null) firstReplyMs = now() - sentAt;
    if (replies.length && !working && now() - changedAt >= settleMs)
      return { replies, firstReplyMs, lastChangeMs: changedAt - sentAt, timedOut: false };
    if (now() - sentAt >= timeoutMs)
      return { replies, firstReplyMs, lastChangeMs: changedAt - sentAt, timedOut: true };
    await sleep(pollMs);
  }
}

export async function frameMessage(page, needle, file, { locateMessage, readVisibleText }) {
  const message = locateMessage(page, needle);
  await message.scrollIntoViewIfNeeded();
  const box = await message.boundingBox();
  const view = page.viewportSize();
  if (!box || !view || box.width <= 0 || box.height <= 0 || box.x >= view.width
      || box.y >= view.height || box.x + box.width <= 0 || box.y + box.height <= 0)
    throw new Error('frameMessage: message is outside the viewport');
  const visibleText = await readVisibleText(page);
  if (!visibleText.includes(needle)) throw new Error('frameMessage: named text is not visible');
  fs.mkdirSync(path.dirname(file), { recursive: true });
  await page.screenshot({ path: file });
  return { file, hash: hash(file), visibleText };
}

export async function openDraft(page, url, expected, {
  readDraft, timeoutMs = 60_000, pollMs = 100, now = Date.now, sleep = pause,
}) {
  await page.goto(url, { waitUntil: 'domcontentloaded', timeout: timeoutMs });
  const start = now();
  for (;;) {
    const draft = await readDraft(page);
    const ready = Boolean(draft?.itemId && draft.visible === true && !draft.working && draft.text?.trim());
    if (ready || now() - start >= timeoutMs)
      return { ...draft, url: page.url(), ready,
        missing: expected.filter(word => !draft?.text?.toLowerCase().includes(word.toLowerCase())) };
    await sleep(pollMs);
  }
}

export function shouldRun(step, open) {
  return !step.ifAsked || Boolean(open && (step.ifAsked === 'any' || step.ifAsked === open.kind));
}

// Expectations precede execution; every record atomically publishes live progress.
export function createLedger({ file, task, build, scope, surface, label = 'real-build', now = Date.now }) {
  if (!SHA.test(build ?? '')) throw new Error('createLedger: explicit build SHA required');
  if (!['real-build', 'fixture', 'eyewitness'].includes(label)) throw new Error('invalid evidence label');
  const plan = new Map(), rows = new Map(), frames = new Set(), verdicts = new Map();
  let progressAt = null;
  const write = () => {
    const list = [...rows.values()];
    const count = verdict => list.filter(row => row.verdict.startsWith(verdict)).length;
    const out = [
      `# QA evidence: ${clean(task)}`, '', 'schema: 2', `build: ${build}`,
      `scope: ${clean(scope)}`, `surface: ${clean(surface)}`, 'budget: 30 minutes',
      `progress_at: ${progressAt ?? 'unknown'}`,
      `cells=${list.length}; PASS=${count('PASS')}; FAIL=${count('FAIL')}; NOT EXERCISED=${count('NOT EXERCISED')}; NOT A ROUTE=${count('NOT A ROUTE')}`,
      `labels: ${label}=${list.length}`, 'sweep: not configured', '',
      '| id | cell | expected | observed | verdict | label | evidence path | defect task |',
      '| --- | --- | --- | --- | --- | --- | --- | --- |',
      ...list.map(row => `| ${clean(row.id)} | ${clean(row.cell)} [kit:${WALK_KIT_VERSION}; ${clean([...row.seen].join(', '))}] | ${clean(row.expected)} [source: ${clean(row.cite)}; "${clean(row.quote)}"] | ${clean(row.observed.join(' // '))} | ${clean(row.verdict)} | ${label} | ${clean(row.evidence)} | ${clean(row.defect ?? '-')} |`),
      '', '## Spec questions',
      ...[...plan.values()].filter(row => !row.cited).map(row => `- ${clean(row.cell)}: ${clean(row.expected)}`),
      '', '## Frame index', ...list.flatMap(row => row.captures.map(capture => `- ${clean(row.id)}: ${clean(capture)}`)), '',
    ];
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(`${file}.tmp`, out.join('\n'));
    fs.renameSync(`${file}.tmp`, file);
  };
  return {
    rows, write,
    expect(id, expectation) {
      if (verdicts.has(id)) throw new Error('declare expectations before the walk');
      plan.set(id, { ...expectation, cited: CITED.test(expectation.cite ?? '') && Boolean(expectation.quote?.trim()) });
    },
    record(step) {
      const expectation = plan.get(step.id);
      if (!expectation) throw new Error('declare the expectation before the walk');
      if (!step.channel || !step.view || typeof step.sent?.text !== 'string' || !step.sent?.url)
        throw new Error('record channel, view, exact instruction and URL');
      let verdict = step.verdict;
      if (!/^(PASS|FAIL|NOT A ROUTE|NOT EXERCISED: .+)$/.test(verdict)) throw new Error('invalid verdict');
      if (!expectation.cited) verdict = 'NOT EXERCISED: spec question (no cited, quoted source line)';
      const graded = verdict === 'PASS' || verdict === 'FAIL';
      if (verdict === 'PASS' && step.defect) throw new Error('PASS carries no defect');
      if (verdict === 'FAIL' && !/^cas-[a-z0-9-]+$/.test(step.defect ?? '')) throw new Error('FAIL needs its defect task');
      if (verdicts.has(step.id) && verdicts.get(step.id) !== verdict) throw new Error('conflicting verdicts for one step');
      if (verdict === 'PASS' && step.reply?.timedOut) throw new Error('reply timed out');
      if (verdict === 'PASS' && step.draft && (!step.draft.ready || step.draft.missing.length))
        throw new Error('open the delivered draft and verify its visible content');
      if (step.reply && !Number.isFinite(step.reply.firstReplyMs)
          && !(step.reply.timedOut && step.reply.firstReplyMs === null)) throw new Error('record measured reply time');
      let evidence = '-';
      if (graded) {
        const frame = step.frame;
        if (!frame || !step.claims?.length) throw new Error('graded rows need a frame and visible claims; DB-only evidence is insufficient');
        evidence = path.relative(path.dirname(file), frame.file);
        if (path.isAbsolute(evidence) || evidence.split(path.sep).includes('..')) throw new Error('capture must be inside the ledger directory');
        if (hash(frame.file) !== frame.hash) throw new Error('frame bytes changed');
        if (frames.has(frame.hash)) throw new Error('frame already cited by another step/view');
        if (step.claims.some(claim => !frame.visibleText.toLowerCase().includes(String(claim).toLowerCase())))
          throw new Error('claim does not appear in the visible frame');
      }
      const key = verdict === 'FAIL' ? `defect:${step.defect}` : `step:${step.id}`;
      const where = `${step.channel}/${step.view}`;
      const observed = graded ? `visible: ${step.claims.join('; ')}; ${step.observed ?? ''}` : step.observed ?? 'not exercised';
      const line = `${observed}; sent "${step.sent.text}" at ${step.sent.url}${step.reply ? `; reply_ms=${step.reply.firstReplyMs}` : ''}`;
      const existing = rows.get(key);
      if (graded) frames.add(step.frame.hash);
      verdicts.set(step.id, verdict);
      if (existing) {
        existing.seen.add(where); existing.observed.push(line);
        if (graded) existing.captures.push(evidence);
      } else {
        rows.set(key, { ...expectation, id: step.id, defect: verdict === 'FAIL' ? step.defect : undefined,
          verdict, observed: [line], seen: new Set([where]), evidence, captures: graded ? [evidence] : [] });
      }
      progressAt = new Date(now()).toISOString();
      write();
      return rows.get(key);
    },
  };
}

// One job per channel/viewport; each owns its fixture identity and browser state.
export async function runSurfaceLanes({
  surfaces, outputDir, profileRoot, createPersona, disposePersona, run, maxParallel = 3,
}) {
  if (!Array.isArray(surfaces) || !surfaces.length || !outputDir || !profileRoot
      || ![createPersona, disposePersona, run].every(fn => typeof fn === 'function')
      || !Number.isInteger(maxParallel) || maxParallel < 1)
    throw new Error('surface lanes need surfaces, separate evidence/profile roots and persona/run adapters');
  const output = path.resolve(outputDir), profiles = path.resolve(profileRoot);
  const nested = (a, b) => { const rel = path.relative(a, b); return !rel || (!rel.startsWith(`..${path.sep}`) && rel !== '..' && !path.isAbsolute(rel)); };
  if (nested(output, profiles) || nested(profiles, output)) throw new Error('keep browser profiles outside evidence');
  const ids = new Set();
  const lanes = surfaces.map(surface => {
    const channel = String(surface.channel ?? '').toLowerCase();
    if (!/^[a-z][a-z0-9-]*$/.test(channel)
        || (surface.width !== undefined && (!Number.isInteger(surface.width) || surface.width <= 0)))
      throw new Error('surface lane needs a channel and optional positive viewport width');
    const id = channel + (surface.width === undefined ? '' : `-${surface.width}`);
    if (ids.has(id)) throw new Error(`duplicate surface lane: ${id}`);
    ids.add(id);
    return { ...surface, id, ledgerPath: path.join(output, id, 'LEDGER.md') };
  });
  const parallel = async (items, job) => {
    let cursor = 0;
    const results = [], errors = [];
    await Promise.all(Array.from({ length: Math.min(maxParallel, items.length) }, async () => {
      for (;;) {
        const index = cursor++;
        if (index >= items.length) return;
        try { results[index] = await job(items[index]); } catch (error) { errors.push(error); }
      }
    }));
    if (errors.length) throw new AggregateError(errors, errors.map(error => error.message).join('; '));
    return results;
  };
  const created = new Map(), directories = [];
  let result, failure;
  try {
    fs.mkdirSync(profiles, { recursive: true, mode: 0o700 });
    for (const lane of lanes) {
      lane.browserProfileDir = fs.mkdtempSync(path.join(profiles, `${lane.id}-`));
      directories.push(lane.browserProfileDir);
    }
    const seen = new Set();
    await parallel(lanes, async lane => {
      lane.persona = await createPersona(lane);
      if (!lane.persona?.id) throw new Error('each surface needs a distinct persona id');
      const id = String(lane.persona.id);
      if (!created.has(id)) created.set(id, lane.persona);
      if (seen.has(id)) throw new Error('each surface needs a distinct persona; shared login refused');
      seen.add(id);
    });
    result = await parallel(lanes, run);
  } catch (error) { failure = error; }
  try { await parallel([...created.values()], disposePersona); }
  catch (error) { failure ??= error; }
  finally { for (const directory of directories) fs.rmSync(directory, { recursive: true, force: true }); }
  if (failure) throw failure;
  return result;
}
