import type { RequestRecord } from "./types.ts";

export function feedPath(path: string): boolean {
  return /^(genesis\.json|manifest\.json|head\.ndjson|live|proofs\/\d+|epochs\/\d{8}\.strk20e\.zst|snapshots\/\d{8}\.strk20s\.zst)$/.test(
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
  async get(path: string, signal?: AbortSignal): Promise<{ bytes: Uint8Array; etag: string }> {
    if (!feedPath(path) || path === "live")
      throw new Error("SCOPE_VIOLATION: non-public feed path");
    return this.request(
      `${this.base.replace(/\/$/, "")}/${path}`,
      undefined,
      path === "manifest.json" || path.startsWith("proofs/") ? "no-cache" : "default",
      signal,
      path.startsWith("snapshots/") || path.startsWith("epochs/"),
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
    download = false,
  ): Promise<{ bytes: Uint8Array; etag: string }> {
    const start = performance.now();
    // Large immutable artifacts can take longer than an RPC while still making
    // steady progress. Bound idle time as well as total time and response size.
    const control = download ? new AbortController() : undefined;
    let idle: ReturnType<typeof setTimeout> | undefined;
    let deadline: ReturnType<typeof setTimeout> | undefined;
    let received = 0;
    const fail = (reason: string) => control!.abort(new Error(
      `TRANSPORT: ${reason}: ${new URL(url).pathname} (${received} bytes received)`,
    ));
    const progress = control ? (size: number) => {
      received = size;
      clearTimeout(idle);
      idle = setTimeout(() => fail("download idle for 30s"), 30_000);
    } : undefined;
    if (progress) {
      progress(0);
      deadline = setTimeout(() => fail("download exceeded 5 minutes"), 300_000);
    }
    const timeout = control?.signal ?? AbortSignal.timeout(30_000);
    try {
      const res = await fetch(url, {
        method: body ? "POST" : "GET",
        cache,
        credentials: "omit",
        redirect: "error",
        ...(body
          ? { body, headers: { "content-type": "application/json" } }
          : {}),
        signal: signal ? AbortSignal.any([signal, timeout]) : timeout,
      });
      if (!res.ok) {
        await res.body?.cancel();
        throw new Error(`TRANSPORT: HTTP ${res.status}`);
      }
      const bytes = await readBounded(res, 32 * 1024 * 1024, progress);
      this.record({
        url,
        method: body ? "POST" : "GET",
        bytes: bytes.length,
        ms: performance.now() - start,
      });
      return { bytes, etag: res.headers.get("etag") ?? "" };
    } catch (error) {
      if (control?.signal.aborted) throw control.signal.reason;
      throw error;
    } finally {
      clearTimeout(idle);
      clearTimeout(deadline);
    }
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
  progress?: (size: number) => void,
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
      if (value.length) progress?.(size);
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
