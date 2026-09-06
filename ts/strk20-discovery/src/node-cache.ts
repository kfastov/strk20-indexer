import { mkdir, readFile, rename, unlink, writeFile } from "node:fs/promises";
import { createHash, randomUUID } from "node:crypto";
import { join } from "node:path";
import type { CacheStore } from "./storage.ts";

/** Atomic replacement, with the same explicit local trust as browser storage. */
export class FileCache implements CacheStore {
  private readonly file: string;
  private readonly directory: string;
  constructor(directory: string, identity: string) {
    this.directory = directory;
    this.file = join(
      directory,
      createHash("sha256").update(identity).digest("hex") + ".state",
    );
  }
  async read(): Promise<Uint8Array | undefined> {
    try {
      return await readFile(this.file);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
      throw error;
    }
  }
  async write(bytes: Uint8Array): Promise<void> {
    await mkdir(this.directory, { recursive: true, mode: 0o700 });
    const temporary = this.file + "." + randomUUID() + ".tmp";
    try {
      await writeFile(temporary, bytes, { flag: "wx", mode: 0o600 });
      await rename(temporary, this.file);
    } finally {
      await unlink(temporary).catch((error: NodeJS.ErrnoException) => {
        if (error.code !== "ENOENT") throw error;
      });
    }
  }
  async clear(): Promise<void> {
    await unlink(this.file).catch((error: NodeJS.ErrnoException) => {
      if (error.code !== "ENOENT") throw error;
    });
  }
}
