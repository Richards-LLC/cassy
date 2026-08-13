import type { AttentionItem, PairingInstallIdentity, StoredMachine } from "./types";

const DB_NAME = "cas-commander-v1";
const DB_VERSION = 1;

function openDatabase(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains("machines")) db.createObjectStore("machines", { keyPath: "id" });
      if (!db.objectStoreNames.contains("attention")) db.createObjectStore("attention", { keyPath: "id" });
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

async function transact<T>(storeName: "machines" | "attention", mode: IDBTransactionMode, run: (store: IDBObjectStore) => IDBRequest<T>, signal?: AbortSignal): Promise<T> {
  const db = await openDatabase();
  if (signal?.aborted) {
    db.close();
    throw new DOMException("Pairing was cancelled.", "AbortError");
  }
  return new Promise((resolve, reject) => {
    const tx = db.transaction(storeName, mode);
    const request = run(tx.objectStore(storeName));
    let result: T;
    let settled = false;
    const cleanup = () => {
      signal?.removeEventListener("abort", abort);
      db.close();
    };
    const fail = (error: unknown) => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(error);
    };
    const abort = () => {
      try { tx.abort(); } catch { /* transaction already completed */ }
    };
    signal?.addEventListener("abort", abort, { once: true });
    request.onsuccess = () => { result = request.result; };
    request.onerror = () => fail(request.error);
    tx.oncomplete = () => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve(result!);
    };
    tx.onabort = () => fail(tx.error ?? new DOMException("Pairing was cancelled.", "AbortError"));
    tx.onerror = () => fail(tx.error);
  });
}

interface PairingCatalogEnvelope {
  id: string;
  pairingInstall: {
    state: "staged";
    identity: PairingInstallIdentity;
    candidate: StoredMachine;
    prior?: StoredMachine;
  };
}

type ActivePairingCatalogRecord = StoredMachine & {
  pairingInstall: {
    state: "active";
    identity: PairingInstallIdentity;
    candidate: StoredMachine;
    prior?: StoredMachine;
  };
};

export type MachineCatalogRecord = StoredMachine | PairingCatalogEnvelope | ActivePairingCatalogRecord;

export interface MachineCatalogBackend {
  list(): Promise<MachineCatalogRecord[]>;
  update(
    id: string,
    change: (current: MachineCatalogRecord | undefined) => MachineCatalogRecord | undefined,
    signal?: AbortSignal,
  ): Promise<void>;
}

function pairingEnvelope(record: MachineCatalogRecord | undefined): PairingCatalogEnvelope["pairingInstall"] | ActivePairingCatalogRecord["pairingInstall"] | undefined {
  if (!record || !("pairingInstall" in record)) return undefined;
  return record.pairingInstall;
}

function visibleMachine(record: MachineCatalogRecord | undefined): StoredMachine | undefined {
  const envelope = pairingEnvelope(record);
  if (!envelope) return record as StoredMachine | undefined;
  return envelope.state === "active" ? envelope.candidate : envelope.prior;
}

function sameInstall(record: MachineCatalogRecord | undefined, identity: PairingInstallIdentity): record is PairingCatalogEnvelope | ActivePairingCatalogRecord {
  const installed = pairingEnvelope(record)?.identity;
  return installed?.machineId === identity.machineId
    && installed.credentialId === identity.credentialId
    && installed.generation === identity.generation;
}

function stagedRecord(pairingInstall: ActivePairingCatalogRecord["pairingInstall"] | PairingCatalogEnvelope["pairingInstall"]): PairingCatalogEnvelope {
  return { id: pairingInstall.identity.machineId, pairingInstall: { ...pairingInstall, state: "staged" } };
}

export class MachineCatalog {
  constructor(private readonly backend: MachineCatalogBackend) {}

  async snapshot(): Promise<{ machines: StoredMachine[]; pendingCleanup: number }> {
    const records = await this.backend.list();
    return {
      machines: records.flatMap((record) => {
        const machine = visibleMachine(record);
        return machine ? [machine] : [];
      }),
      pendingCleanup: records.filter((record) => pairingEnvelope(record)?.state === "staged").length,
    };
  }

  async recoverPending(): Promise<{ machines: StoredMachine[]; pendingCleanup: number }> {
    const records = await this.backend.list();
    for (const record of records) {
      const envelope = pairingEnvelope(record);
      if (envelope?.state !== "staged") continue;
      try {
        await this.rollback(envelope.identity);
      } catch {
        // The staged envelope remains durable and invisible until a later recovery succeeds.
      }
    }
    return this.snapshot();
  }

  async stage(machine: StoredMachine, identity: PairingInstallIdentity, signal?: AbortSignal): Promise<void> {
    if (machine.id !== identity.machineId || machine.credentialId !== identity.credentialId) {
      throw new Error("Pairing install identity does not match the credential.");
    }
    await this.backend.update(machine.id, (current) => ({
      id: machine.id,
      pairingInstall: {
        state: "staged",
        identity,
        candidate: machine,
        ...(visibleMachine(current) ? { prior: visibleMachine(current) } : {}),
      },
    }), signal);
  }

  async activate(identity: PairingInstallIdentity, signal?: AbortSignal): Promise<boolean> {
    let activated = false;
    await this.backend.update(identity.machineId, (current) => {
      if (!sameInstall(current, identity) || current.pairingInstall.state !== "staged") return current;
      activated = true;
      return {
        ...current.pairingInstall.candidate,
        pairingInstall: { ...current.pairingInstall, state: "active" },
      };
    }, signal);
    return activated;
  }

  async rollback(identity: PairingInstallIdentity): Promise<boolean> {
    let blocked = false;
    try {
      await this.backend.update(identity.machineId, (current) => {
        if (!sameInstall(current, identity)) return current;
        blocked = true;
        return current.pairingInstall.state === "active" ? stagedRecord(current.pairingInstall) : current;
      });
    } catch (error) {
      // A transient failed active→staged write must not leave a cancelled, active credential
      // visible on the next boot. Persist the invisible staged form before reporting cleanup
      // as incomplete; later recovery restores the exact prior row.
      await this.backend.update(identity.machineId, (current) => {
        if (!sameInstall(current, identity) || current.pairingInstall.state !== "active") return current;
        return stagedRecord(current.pairingInstall);
      });
      throw error;
    }
    if (!blocked) return false;
    let rolledBack = false;
    await this.backend.update(identity.machineId, (current) => {
      if (!sameInstall(current, identity) || current.pairingInstall.state !== "staged") return current;
      rolledBack = true;
      return current.pairingInstall.prior;
    });
    return rolledBack;
  }

  put(machine: StoredMachine, signal?: AbortSignal): Promise<void> {
    return this.backend.update(machine.id, () => machine, signal);
  }

  remove(id: string): Promise<void> {
    return this.backend.update(id, () => undefined);
  }
}

const indexedDbMachineBackend: MachineCatalogBackend = {
  list: () => transact<MachineCatalogRecord[]>("machines", "readonly", (store) => store.getAll()),
  async update(id, change, signal) {
    const db = await openDatabase();
    if (signal?.aborted) {
      db.close();
      throw new DOMException("Pairing was cancelled.", "AbortError");
    }
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction("machines", "readwrite");
      const store = tx.objectStore("machines");
      let settled = false;
      const cleanup = () => {
        signal?.removeEventListener("abort", abort);
        db.close();
      };
      const fail = (error: unknown) => {
        if (settled) return;
        settled = true;
        cleanup();
        reject(error);
      };
      const abort = () => {
        try { tx.abort(); } catch { /* transaction already completed */ }
      };
      signal?.addEventListener("abort", abort, { once: true });
      const read = store.get(id);
      read.onsuccess = () => {
        const next = change(read.result as MachineCatalogRecord | undefined);
        if (next) store.put(next);
        else store.delete(id);
      };
      read.onerror = () => fail(read.error);
      tx.oncomplete = () => {
        if (settled) return;
        settled = true;
        cleanup();
        resolve();
      };
      tx.onabort = () => fail(tx.error ?? new DOMException("Pairing was cancelled.", "AbortError"));
      tx.onerror = () => fail(tx.error);
    });
  },
};

export const catalog = new MachineCatalog(indexedDbMachineBackend);

export const attentionStore = {
  list: () => transact<AttentionItem[]>("attention", "readonly", (store) => store.getAll()),
  put: (item: AttentionItem) => transact<IDBValidKey>("attention", "readwrite", (store) => store.put(item)),
};
