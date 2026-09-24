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

/**
 * Accents for a paired fleet, so two machines never share a colour while a
 * free one is left (journey F16: "atlas" and "studio" both hash to accent 2).
 * Machines are taken in id order, so the same fleet gets the same colours on
 * every device. Each keeps its hashed accent unless an earlier machine holds
 * it; then it takes the next least-used accent after it. Only a fleet larger
 * than the accent set repeats a colour.
 */
export function assignMachineAccents(machineIds: Iterable<string>, count = MACHINE_ACCENT_COUNT): Map<string, number> {
  const uses = new Array<number>(count).fill(0);
  const assigned = new Map<string, number>();
  for (const id of [...new Set(machineIds)].sort()) {
    const preferred = machineAccentIndex(id, count);
    const least = Math.min(...uses);
    let index = preferred;
    while (uses[index] !== least) index = (index + 1) % count;
    uses[index] += 1;
    assigned.set(id, index);
  }
  return assigned;
}

let fleetAccents = new Map<string, number>();

/** Set the paired fleet that machineAccentClass assigns colours within. */
export function setMachineAccentFleet(machineIds: Iterable<string>): void {
  fleetAccents = assignMachineAccents(machineIds);
}

/** The class carried by a rail row and by the thread root so descendants inherit the accent. */
export function machineAccentClass(machineId: string): string {
  return `machine-accent-${fleetAccents.get(machineId) ?? machineAccentIndex(machineId)}`;
}

/** The single letter inside the machine avatar. */
export function machineMonogram(label: string): string {
  const first = label.trim().match(/\p{L}|\p{N}/u)?.[0];
  return first ? first.toLocaleUpperCase() : "?";
}
