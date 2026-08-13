import type { AttentionItem, StoredMachine } from "./types";

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

export const catalog = {
  list: () => transact<StoredMachine[]>("machines", "readonly", (store) => store.getAll()),
  put: (machine: StoredMachine, signal?: AbortSignal) => transact<IDBValidKey>("machines", "readwrite", (store) => store.put(machine), signal),
  remove: (id: string) => transact<undefined>("machines", "readwrite", (store) => store.delete(id) as IDBRequest<undefined>),
};

export const attentionStore = {
  list: () => transact<AttentionItem[]>("attention", "readonly", (store) => store.getAll()),
  put: (item: AttentionItem) => transact<IDBValidKey>("attention", "readwrite", (store) => store.put(item)),
};
