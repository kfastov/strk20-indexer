import type { RequestRecord } from "./types.ts";

export function feedPath(path: string): boolean {
  return /^(genesis\.json|manifest\.json|head\.ndjson|live|epochs\/\d{8}\.strk20e\.zst|snapshots\/\d{8}\.strk20s\.zst)$/.test(
    path,
  );
}
export class PublicTransport {
  readonly base: string;
  private readonly record: (r: RequestRecord) => void;
  constructor(base: string, record: (r: RequestRecord) => void) {
    this.base = base;
    this.record = record;
    const url = new URL(base);
    if (url.search || url.hash || url.username || url.password)
      throw new Error(
        "CONFIG_INVALID: feed URL must not contain credentials or query parameters",
      );
  }
  async get(path: string): Promise<{ bytes: Uint8Array; etag: string }> {
    if (!feedPath(path) || path === "live")
      throw new Error("SCOPE_VIOLATION: non-public feed path");
    return this.request(
      `${this.base.replace(/\/$/, "")}/${path}`,
      undefined,
      path === "manifest.json" ? "no-cache" : "default",
    );
  }
  async rpc(
    url: string,
    method:
      | "starknet_chainId"
      | "starknet_getBlockWithTxHashes"
      | "starknet_getStorageProof",
    params: unknown[],
    signal?: AbortSignal,
  ): Promise<unknown> {
    const result = await this.request(
      url,
      JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
      "default",
      signal,
    );
    const json = JSON.parse(new TextDecoder().decode(result.bytes)) as {
      result?: unknown;
      error?: { code: number; message: string };
    };
    if (json.error)
      throw new Error(
        `RPC_UNAVAILABLE: ${json.error.code} ${json.error.message} [${method}]`,
      );
    if (json.result === undefined)
      throw new Error("RPC_UNAVAILABLE: missing result");
    return json.result;
  }
  private async request(
    url: string,
    body?: string,
    cache: RequestCache = "default",
    signal?: AbortSignal,
  ): Promise<{ bytes: Uint8Array; etag: string }> {
    const start = performance.now();
    const res = await fetch(url, {
      method: body ? "POST" : "GET",
      cache,
      credentials: "omit",
      redirect: "error",
      ...(body
        ? { body, headers: { "content-type": "application/json" } }
        : {}),
      signal: signal ? AbortSignal.any([signal, AbortSignal.timeout(30_000)]) : AbortSignal.timeout(30_000),
    });
    if (!res.ok) throw new Error(`TRANSPORT: HTTP ${res.status}`);
    const bytes = await readBounded(res, 32 * 1024 * 1024);
    this.record({
      url,
      method: body ? "POST" : "GET",
      bytes: bytes.length,
      ms: performance.now() - start,
    });
    return { bytes, etag: res.headers.get("etag") ?? "" };
  }
  async live(
    onEvent: (name: string, data: string) => void,
    signal: AbortSignal,
  ): Promise<void> {
    const url = `${this.base.replace(/\/$/, "")}/live`;
    const started = performance.now();
    const idle = new AbortController();
    let timer = setTimeout(() => idle.abort(), 30_000);
    const res = await fetch(url, {
      credentials: "omit",
      redirect: "error",
      signal: AbortSignal.any([signal, idle.signal]),
      headers: { Accept: "text/event-stream" },
    }).catch((error: unknown) => {
      clearTimeout(timer);
      throw error;
    });
    if (!res.ok || !res.body) {
      clearTimeout(timer);
      await res.body?.cancel();
      throw new Error(`TRANSPORT: SSE HTTP ${res.status}`);
    }
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "",
      count = 0;
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        clearTimeout(timer);
        timer = setTimeout(() => idle.abort(), 30_000);
        count += value.length;
        buffer = (buffer + decoder.decode(value, { stream: true })).replace(
          /\r\n/g,
          "\n",
        );
        if (buffer.length > 6 * 1024 * 1024)
          throw new Error("SSE message exceeds limit");
        let end: number;
        while ((end = buffer.indexOf("\n\n")) >= 0) {
          const frame = buffer.slice(0, end);
          buffer = buffer.slice(end + 2);
          const lines = frame.split("\n");
          const name =
            lines
              .find((l) => l.startsWith("event:"))
              ?.slice(6)
              .trim() ?? "message";
          const data = lines
            .filter((l) => l.startsWith("data:"))
            .map((l) => l.slice(5).replace(/^ /, ""))
            .join("\n");
          if (data) onEvent(name, data);
        }
      }
    } finally {
      clearTimeout(timer);
      await reader.cancel().catch(() => {});
      this.record({
        url,
        method: "GET",
        bytes: count,
        ms: performance.now() - started,
      });
    }
  }
}
async function readBounded(
  response: Response,
  limit: number,
): Promise<Uint8Array> {
  if (!response.body) throw new Error("TRANSPORT: empty response");
  const reader = response.body.getReader();
  const parts: Uint8Array[] = [];
  let size = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.length;
      if (size > limit) throw new Error("TRANSPORT: response too large");
      parts.push(value);
    }
  } finally {
    await reader.cancel().catch(() => {});
  }
  const result = new Uint8Array(size);
  let offset = 0;
  for (const p of parts) {
    result.set(p, offset);
    offset += p.length;
  }
  return result;
}
