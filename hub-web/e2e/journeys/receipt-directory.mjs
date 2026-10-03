import { mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

/**
 * Claim before recording: two different tests must never share a receipt
 * directory. The same test may claim it again (a retry, or --repeat-each):
 * its earlier attempt's files are cleared first, so no stale stage capture
 * outlives the attempt that recorded it. Subdirectories (parts/) belong to
 * other tests and are left alone.
 */
export function claimReceiptDirectory(directory, title) {
  mkdirSync(directory, { recursive: true });
  const owner = join(directory, 'receipt-owner.json');
  try {
    writeFileSync(owner, JSON.stringify({ title }) + '\n', { flag: 'wx' });
    return;
  } catch (error) {
    if (error.code !== 'EEXIST') throw error;
  }
  let claimed;
  try { claimed = JSON.parse(readFileSync(owner, 'utf8')).title; } catch { claimed = undefined; }
  if (claimed !== title) {
    throw new Error(`journey receipt directory already claimed: ${directory}; give extra tests journeyPart and run with a fresh JOURNEY_RECEIPTS directory`);
  }
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (entry.isFile() && entry.name !== 'receipt-owner.json') rmSync(join(directory, entry.name), { force: true });
  }
}
