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
 * jump consistent hash maps it into the MACHINE_ACCENT_BASE preferred buckets;
 * a paired fleet then resolves collisions and records each machine's accent
 * (assignMachineAccents, setMachineAccentFleet). Jump hash,
 * not modulo, is what makes the set appendable: going from N to N+1 buckets
 * only moves ids that land in the NEW bucket, so appending a fourth accent
 * never reshuffles machines among the first three.
 */

/** Number of `.machine-accent-N` sets generated into tokens.css. Append, never reorder. */
export const MACHINE_ACCENT_COUNT = 5;
/**
 * The first sets, the ones a machine's id hashes to (cas-50a7). Sets past
 * these (teal, rose) are handed out only when every base set is in use, so a
 * fleet of up to three looks exactly as it did before they were appended.
 */
export const MACHINE_ACCENT_BASE = 3;

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

export function machineAccentIndex(machineId: string, count = MACHINE_ACCENT_BASE): number {
  return jumpConsistentHash(fnv1a32(machineId), count);
}

/**
 * The next accent for a machine: the least-used one, preferring its hashed
 * accent, then the other base sets after it, then the appended sets. A fleet
 * no larger than the base set therefore gets exactly the colours it always
 * had; a fourth and fifth machine get teal and rose instead of a repeat.
 */
function pickAccent(preferred: number, uses: readonly number[], count: number): number {
  const base = Math.min(MACHINE_ACCENT_BASE, count);
  const order = [...Array.from({ length: base }, (_, step) => (preferred + step) % base), ...Array.from({ length: count - base }, (_, step) => base + step)];
  const least = Math.min(...uses);
  return order.find((index) => uses[index] === least) ?? preferred;
}

/**
 * Accents for a paired fleet, so two machines never share a colour while a
 * free one is left (journey F16: "atlas" and "studio" both hash to accent 2).
 *
 * `stored` is what each machine was given when it was first seen (cas-50a7):
 * a machine keeps that accent whatever else is paired or removed, so pairing a
 * new machine never re-colours the ones already there. Machines without one
 * are taken in id order (the same fleet then gets the same colours on every
 * device) and get their hashed accent unless another machine holds it.
 */
export function assignMachineAccents(machineIds: Iterable<string>, count = MACHINE_ACCENT_COUNT, stored: ReadonlyMap<string, number> = new Map()): Map<string, number> {
  const uses = new Array<number>(count).fill(0);
  const assigned = new Map<string, number>();
  const fleet = [...new Set(machineIds)].sort();
  // Stored accents first; one that another fleet member already keeps (a
  // machine removed and re-paired after its colour was reused) is reassigned.
  for (const id of fleet) {
    const index = stored.get(id);
    if (index === undefined || !Number.isInteger(index) || index < 0 || index >= count || uses[index] > 0) continue;
    uses[index] += 1;
    assigned.set(id, index);
  }
  for (const id of fleet) {
    if (assigned.has(id)) continue;
    const index = pickAccent(machineAccentIndex(id, Math.min(MACHINE_ACCENT_BASE, count)), uses, count);
    uses[index] += 1;
    assigned.set(id, index);
  }
  return assigned;
}

/** Where each machine's accent is kept between visits (the browser's localStorage). */
export interface MachineAccentStore {
  load(): Map<string, number>;
  save(accents: ReadonlyMap<string, number>): void;
}

export const MACHINE_ACCENT_STORAGE_KEY = "cas.commander.machine-accents.v1";

/** A store over a Storage; unreadable or denied storage degrades to this page only. */
export function storageAccentStore(storage: Pick<Storage, "getItem" | "setItem"> | undefined): MachineAccentStore {
  // Read once: render() asks on every pass, and only this page writes it.
  let cache: Map<string, number> | undefined;
  const read = (): Map<string, number> => {
    try {
      const parsed: unknown = JSON.parse(storage?.getItem(MACHINE_ACCENT_STORAGE_KEY) ?? "{}");
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return new Map();
      return new Map(Object.entries(parsed as Record<string, unknown>).filter((entry): entry is [string, number] => Number.isInteger(entry[1])));
    } catch {
      return new Map();
    }
  };
  return {
    load() {
      cache ??= read();
      return new Map(cache);
    },
    save(accents) {
      cache = new Map(accents);
      try { storage?.setItem(MACHINE_ACCENT_STORAGE_KEY, JSON.stringify(Object.fromEntries(accents))); } catch { /* Kept for this page only. */ }
    },
  };
}

let fleetAccents = new Map<string, number>();

/**
 * Set the paired fleet that machineAccentClass assigns colours within. With a
 * store, a machine's accent is recorded the first time it is seen (right after
 * it pairs) and kept from then on; machines no longer paired keep their entry,
 * so re-pairing one brings its colour back when it is free.
 */
export function setMachineAccentFleet(machineIds: Iterable<string>, store?: MachineAccentStore): void {
  const stored = store?.load() ?? new Map<string, number>();
  fleetAccents = assignMachineAccents(machineIds, MACHINE_ACCENT_COUNT, stored);
  if (!store) return;
  let changed = false;
  for (const [id, index] of fleetAccents) if (stored.get(id) !== index) { stored.set(id, index); changed = true; }
  if (changed) store.save(stored);
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
