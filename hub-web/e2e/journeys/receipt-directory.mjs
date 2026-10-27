import { mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { createHash } from 'node:crypto';

// Describe blocks and projects may share a leaf title. Keep a readable prefix,
// but key the directory by the complete test identity, including long titles.
export function receiptPartName(title, identity) {
  const prefix = title.replace(/^[A-Z]+-J[0-9]+\s*/, '').toLowerCase()
    .replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 79);
  const key = JSON.stringify([identity.project, identity.titlePath]);
  return `${prefix}-${createHash('sha256').update(key).digest('hex').slice(0, 16)}`;
}

/**
 * Claim before recording: two different tests must never share a receipt
 * directory. The same test may claim it again (a retry, or --repeat-each):
 * its earlier attempt's files are cleared first, so no stale stage capture
 * outlives the attempt that recorded it. Subdirectories (parts/) belong to
 * other tests and are left alone.
 */
export function claimReceiptDirectory(directory, title, identity) {
  mkdirSync(directory, { recursive: true });
  const owner = join(directory, 'receipt-owner.json');
  const expected = identity ? { title, project: identity.project, titlePath: identity.titlePath } : { title };
  try {
    writeFileSync(owner, JSON.stringify(expected) + '\n', { flag: 'wx' });
    return;
  } catch (error) {
    if (error.code !== 'EEXIST') throw error;
  }
  let claimed;
  try { claimed = JSON.parse(readFileSync(owner, 'utf8')); } catch { claimed = undefined; }
  // Also fence an accidental path collision: only this exact test can retry.
  // The two-argument API retains title-only ownership for legacy callers.
  if (JSON.stringify(claimed) !== JSON.stringify(expected)) {
    throw new Error(`journey receipt directory already claimed: ${directory}; give extra tests journeyPart and run with a fresh JOURNEY_RECEIPTS directory`);
  }
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (entry.isFile() && entry.name !== 'receipt-owner.json') rmSync(join(directory, entry.name), { force: true });
  }
}

// Unset is deliberately distinct from an explicitly empty mask list: ordinary
// worker/CI runs keep their screenshot options, receipt schema and timing.
export function captureMask(raw) {
  if (raw === undefined) return undefined;
  let value;
  try { value = JSON.parse(raw); } catch { throw new Error('JOURNEY_CAPTURE_MASK must be a JSON array of selectors'); }
  if (!Array.isArray(value) || value.some((s) => typeof s !== 'string' || !s.trim() || s !== s.trim())) {
    throw new Error('JOURNEY_CAPTURE_MASK must be a JSON array of selectors');
  }
  return [...new Set(value)];
}

export async function captureStageScreenshot(page, path, masks, maxHtmlBytes) {
  if (masks === undefined && maxHtmlBytes === undefined) return page.screenshot({ path });
  if (maxHtmlBytes !== undefined && Buffer.byteLength(await page.content()) > maxHtmlBytes) {
    throw new Error('capture page exceeds max_html_mb');
  }
  const locators = [];
  for (const selector of masks ?? []) {
    const locator = page.locator(selector);
    if (!await locator.count()) throw new Error('cannot apply requested capture mask');
    locators.push(locator);
  }
  // Never catch and fall back to an unmasked screenshot.
  return page.screenshot({ path, mask: locators });
}
