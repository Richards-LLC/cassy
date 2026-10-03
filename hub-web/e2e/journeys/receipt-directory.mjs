import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

/** Claim before recording: two tests must never share a receipt directory. */
export function claimReceiptDirectory(directory, title) {
  mkdirSync(directory, { recursive: true });
  try {
    writeFileSync(join(directory, 'receipt-owner.json'), JSON.stringify({ title }) + '\n', { flag: 'wx' });
  } catch (error) {
    if (error.code !== 'EEXIST') throw error;
    throw new Error(`journey receipt directory already claimed: ${directory}; give extra tests journeyPart and run with a fresh JOURNEY_RECEIPTS directory`);
  }
}
