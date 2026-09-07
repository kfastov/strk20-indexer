type Notification = { method: string; params: { subscription_id: string | number; result: any } };

/** Owns transport lifetime; callers own their cursor, catch-up and operation deadline. */
export function subscribeRpc(options: {
  url: string;
  method: string;
  params: unknown;
  onEvent: (event: Notification) => void;
  onReady?: (reconnected: boolean, current: () => boolean) => void;
  onError: (error: Error) => void;
  handshakeMs?: number;
  idleMs?: number;
}): () => void {
  let stopped = false;
  let socket: WebSocket | undefined;
  let generation = 0;
  let failures = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let reconnect: ReturnType<typeof setTimeout> | undefined;
  let connected = false;
  const stop = () => {
    stopped = true;
    generation++;
    clearTimeout(timer);
    clearTimeout(reconnect);
    socket?.close();
  };
  const fail = (error: unknown) => {
    if (stopped) return;
    stop();
    options.onError(error instanceof Error ? error : new Error(String(error)));
  };
  function connect() {
    if (stopped) return;
    const own = ++generation;
    let lost = false;
    let id: string | number | undefined;
    let openedAt = 0;
    let early: Notification[] = [];
    const current = () => !stopped && !lost && generation === own;
    const disconnect = () => {
      if (!current()) return;
      lost = true;
      clearTimeout(timer);
      socket?.close();
      // Count consecutive failures; a stable live connection earns a fresh budget.
      if (id !== undefined && performance.now() - openedAt >= 10_000) failures = 0;
      if (failures === 3) { fail(new Error("Subscription connection lost.")); return; }
      const delay = 250 * 2 ** failures++ * (0.75 + Math.random() * 0.5);
      reconnect = setTimeout(connect, delay);
    };
    const touch = () => {
      clearTimeout(timer);
      if (options.idleMs) timer = setTimeout(disconnect, options.idleMs);
    };
    try { socket = new WebSocket(options.url); }
    catch { disconnect(); return; }
    const connection = socket;
    timer = setTimeout(disconnect, options.handshakeMs ?? 5_000);
    connection.onopen = () => {
      if (!current()) { connection.close(); return; }
      connection.send(JSON.stringify({ jsonrpc: "2.0", id: 1,
        method: options.method, params: options.params }));
    };
    connection.onmessage = (message) => {
      if (!current()) return;
      try {
        const event = JSON.parse(String(message.data));
        if (event.id === 1) {
          if (event.error) throw new Error(`Subscription failed: ${event.error.message}`);
          if (typeof event.result !== "string" && typeof event.result !== "number")
            throw new Error("Invalid subscription acknowledgement.");
          if (id !== undefined) return;
          id = event.result;
          openedAt = performance.now();
          touch();
          // Some providers emit their initial notification before the ACK.
          for (const pending of early) if (pending.params.subscription_id === id) options.onEvent(pending);
          early = [];
          if (current()) options.onReady?.(connected, current);
          connected = true;
        } else if (typeof event.method === "string" && event.params) {
          if (id === undefined) {
            if (early.length === 8) throw new Error("Too many notifications before subscription ACK.");
            early.push(event);
          } else if (event.params.subscription_id === id) {
            touch();
            options.onEvent(event);
          }
        }
      } catch (error) { fail(error); }
    };
    connection.onerror = disconnect;
    connection.onclose = disconnect;
  }
  connect();
  return stop;
}
