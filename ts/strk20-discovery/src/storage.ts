export interface CacheStore {
  read(): Promise<Uint8Array | undefined>;
  write(bytes: Uint8Array): Promise<void>;
  clear(): Promise<void>;
}
export type CacheFactory = (identity: string) => CacheStore;

/** One atomic folded-state record. Wallet backup material uses another DB. */
export class StateCache implements CacheStore {
  private readonly name: string;
  constructor(name: string) {
    this.name = name;
  }
  private async open(): Promise<IDBDatabase> {
    return new Promise((resolve, reject) => {
      const r = indexedDB.open(this.name, 1);
      r.onupgradeneeded = () => r.result.createObjectStore("state");
      r.onsuccess = () => resolve(r.result);
      r.onerror = () => reject(r.error);
    });
  }
  async read(): Promise<Uint8Array | undefined> {
    const db = await this.open();
    try {
      return await new Promise((resolve, reject) => {
        const r = db.transaction("state").objectStore("state").get("folded");
        r.onsuccess = () => resolve(r.result as Uint8Array | undefined);
        r.onerror = () => reject(r.error);
      });
    } finally {
      db.close();
    }
  }
  async write(bytes: Uint8Array): Promise<void> {
    const db = await this.open();
    try {
      await new Promise<void>((resolve, reject) => {
        const tx = db.transaction("state", "readwrite");
        tx.objectStore("state").put(bytes, "folded");
        tx.oncomplete = () => resolve();
        tx.onabort = tx.onerror = () => reject(tx.error);
      });
    } finally {
      db.close();
    }
  }
  async clear(): Promise<void> {
    const db = await this.open();
    try {
      await new Promise<void>((resolve, reject) => {
        const tx = db.transaction("state", "readwrite");
        tx.objectStore("state").clear();
        tx.oncomplete = () => resolve();
        tx.onabort = tx.onerror = () => reject(tx.error);
      });
    } finally {
      db.close();
    }
  }
}
