import type { Page } from "@playwright/test";

/** Read committed real-browser storage as evidence, never manufacture app state. */
export async function journalRows(page: Page, store: "sends" | "replies") {
  return page.evaluate((store) => new Promise<Array<{ scope: { hub: string; session: string; device: string }; send?: { id: string; text: string; state: string }; reply?: { notification_id: number; message: string } }>>((resolve, reject) => {
    const request = indexedDB.open("cas-commander-delivery-v1", 1);
    request.onupgradeneeded = () => request.transaction?.abort();
    request.onerror = () => reject(request.error);
    request.onsuccess = () => {
      const db = request.result;
      const tx = db.transaction(store, "readonly");
      const read = tx.objectStore(store).getAll();
      tx.oncomplete = () => { db.close(); resolve(read.result); };
      tx.onabort = () => { db.close(); reject(tx.error); };
    };
  }), store);
}
