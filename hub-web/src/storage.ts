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

async function transact<T>(storeName: "machines" | "attention", mode: IDBTransactionMode, run: (store: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  const db = await openDatabase();
  return new Promise((resolve, reject) => {
    const tx = db.transaction(storeName, mode);
    const request = run(tx.objectStore(storeName));
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
    tx.oncomplete = () => db.close();
    tx.onerror = () => reject(tx.error);
  });
}

export const catalog = {
  list: () => transact<StoredMachine[]>("machines", "readonly", (store) => store.getAll()),
  put: (machine: StoredMachine) => transact<IDBValidKey>("machines", "readwrite", (store) => store.put(machine)),
  remove: (id: string) => transact<undefined>("machines", "readwrite", (store) => store.delete(id) as IDBRequest<undefined>),
};

export const attentionStore = {
  list: () => transact<AttentionItem[]>("attention", "readonly", (store) => store.getAll()),
  put: (item: AttentionItem) => transact<IDBValidKey>("attention", "readwrite", (store) => store.put(item)),
};
