/** Per-machine accent (Pebble, EPIC cas-cac1).
 *
 * A machine keeps one colour everywhere it appears: its rows in the rail (one
 * machine on two projects gets the same colour twice), the thread root, the
 * supervisor bubbles, the avatar, the send button and the unread pill. The
 * colour is chosen from the accent set generated into tokens.css as
 * `.machine-accent-N` (hub-web/scripts/generate-tokens.mjs), deterministically
 * from the machine id, so it is the same on every device and across reloads.
 *
 * Assignment: FNV-1a (32-bit, over UTF-16 code units) hashes the id, then a
 * jump consistent hash maps it into MACHINE_ACCENT_COUNT buckets. Jump hash,
 * not modulo, is what makes the set appendable: going from N to N+1 buckets
 * only moves ids that land in the NEW bucket, so appending a fourth accent
 * never reshuffles machines among the first three.
 */

/** Number of `.machine-accent-N` sets generated into tokens.css. Append, never reorder. */
export const MACHINE_ACCENT_COUNT = 3;

export function fnv1a32(text: string): number {
  let hash = 0x811c9dc5;
  for (let index = 0; index < text.length; index += 1) {
    hash ^= text.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash >>> 0;
}

/** Lamping & Veach jump consistent hash over a 32-bit key. */
export function jumpConsistentHash(key: number, buckets: number): number {
  if (!Number.isInteger(buckets) || buckets < 1) throw new RangeError(`Bucket count must be a positive integer, got ${buckets}`);
  let state = BigInt(key >>> 0);
  let bucket = -1n;
  let next = 0n;
  const limit = BigInt(buckets);
  while (next < limit) {
    bucket = next;
    state = (state * 2862933555777941757n + 1n) & 0xffffffffffffffffn;
    next = ((bucket + 1n) * (1n << 31n)) / ((state >> 33n) + 1n);
  }
  return Number(bucket);
}

export function machineAccentIndex(machineId: string, count = MACHINE_ACCENT_COUNT): number {
  return jumpConsistentHash(fnv1a32(machineId), count);
}

/** The class carried by a rail row and by the thread root so descendants inherit the accent. */
export function machineAccentClass(machineId: string): string {
  return `machine-accent-${machineAccentIndex(machineId)}`;
}

/** The single letter inside the machine avatar. */
export function machineMonogram(label: string): string {
  const first = label.trim().match(/\p{L}|\p{N}/u)?.[0];
  return first ? first.toLocaleUpperCase() : "?";
}
