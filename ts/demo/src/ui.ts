import type { Operation } from "./operations.ts";
import type { Network } from "./network.ts";
import type { StartupProgress, StartupStage } from "strk20-discovery";

export const element = <T extends HTMLElement>(id: string) =>
  document.getElementById(id) as T;
const svg = (body: string, size: number) =>
  `<svg viewBox="0 0 24 24" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;
const icons = {
  mark: svg('<path d="M12 2.8 20 7.4v9.2L12 21.2 4 16.6V7.4Z" stroke-width="1.7"/><circle cx="12" cy="12" r="2.6" fill="currentColor" stroke="none"/>', 18),
  copy: svg('<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V7a2 2 0 0 1 2-2h8"/>', 14),
  download: svg('<path d="M12 4v11m0 0 4-4m-4 4-4-4M4 19h16"/>', 14),
  chevron: svg('<path d="m6 9 6 6 6-6"/>', 18),
};
export function mount(network: Network): void {
  element<HTMLDivElement>("app").innerHTML = `
    <header><a class="brand" href="https://github.com/kfastov/strk20-indexer" target="_blank" rel="noreferrer"><span class="brand-mark">${icons.mark}</span>STRK20 <span class="brand-sub">/ local discovery</span></a>
      <div class="topbar-tools"><span class="status-pill" data-state="syncing"><span class="status-dot" aria-hidden="true"></span><span id="connection" class="status">Starting</span></span>
        <select id="network" aria-label="Network"><option value="sepolia">Sepolia testnet</option><option value="mainnet">Starknet mainnet</option></select></div></header>
    <main><div class="intro"><div><span class="eyebrow">YOUR NOTES. YOUR BROWSER.</span><h1>Private transfers. <span>Local discovery.</span></h1></div>
      <p>Deposit, transfer and withdraw with locally verified notes.</p></div>
      <div class="workspace"><section class="flow card" aria-label="Transaction flow">
        <div class="flow-top"><span id="step" class="eyebrow">GET STARTED</span></div>
        <h2 id="title">Create a demo wallet</h2><p id="description"></p>
        <div id="wallet" hidden>
          <div class="balances"><div class="balance balance-private"><span>Private balance</span><strong id="private-balance">—</strong></div><div class="balance balance-public"><span>Public balance</span><strong id="public-balance">—</strong></div></div>
          <details id="wallet-details" class="wallet-details"><summary>Wallet address &amp; backup</summary>
            <div class="wallet-panel"><span class="panel-label">Demo wallet address</span>
              <div class="address-box"><a id="address" class="address" target="_blank" rel="noreferrer" title="Open in the block explorer"></a><button id="copy" class="copy-button" type="button" aria-label="Copy wallet address">${icons.copy}Copy</button></div>
              <div class="backup-row"><button id="backup" class="text-button" type="button">${icons.download}Export wallet backup</button><span>Keys are stored in this browser. Use small amounts.</span></div>
            </div>
          </details>
        </div>
        <div id="amount-row" class="field" hidden><label for="amount">Amount</label><div class="input-wrap"><input id="amount" value="0.01" inputmode="decimal" autocomplete="off" /><span class="input-suffix" aria-hidden="true">STRK</span></div></div>
        <div id="recipient-row" class="field" hidden><label id="recipient-label" for="recipient">Recipient</label><input id="recipient" placeholder="0x…" autocomplete="off" spellcheck="false" /></div>
        <div id="startup" class="startup" role="status" hidden><div class="startup-title"><strong id="startup-label"></strong><span id="startup-step"></span></div><progress id="startup-progress" class="visually-hidden" max="8" value="0" aria-label="Initialization stages"></progress><div id="startup-track" class="startup-track" aria-hidden="true">${"<i></i>".repeat(8)}</div><p id="startup-detail"></p></div>
        <p id="error" class="alert" role="alert" hidden></p>
        <div class="action-control">
          <button id="next" class="primary"><span id="button-text">Create demo wallet</span><span class="spinner" aria-hidden="true"></span></button>
          <button id="wallet-actions" class="action-toggle" type="button" popovertarget="action-menu" aria-label="Choose operation" aria-haspopup="menu" aria-expanded="false" hidden>${icons.chevron}</button>
          <div id="action-menu" popover="auto" role="menu" aria-label="Choose operation">
            <p class="menu-label">CHOOSE OPERATION</p>
            <button id="choose-shield" type="button" role="menuitemradio" aria-checked="false"><span>Deposit<small>Move STRK into the privacy pool</small></span></button>
            <button id="choose-transfer" type="button" role="menuitemradio" aria-checked="false"><span>Private transfer<small>Send within the privacy pool</small></span></button>
            <button id="choose-withdraw" type="button" role="menuitemradio" aria-checked="false"><span>Withdraw<small>Return STRK to a public address</small></span></button>
            <button id="choose-discover" type="button" role="menuitemradio" aria-checked="false"><span>Discover<small>Refresh your verified private balance</small></span></button>
          </div>
        </div>
        <div class="secondary"><button id="import" class="text-button" type="button">Restore wallet backup</button><input id="backup-file" type="file" accept="application/json" hidden /></div>
      </section>
      <aside class="activity card" aria-label="Activity">
        <div class="section-title"><h2>Activity <span id="activity-count">0</span></h2><button id="metrics" class="text-button" type="button">Export timings ↗</button></div>
        <div id="log" tabindex="0" role="region" aria-label="Operation history"><p class="empty">No operations yet.</p></div>
      </aside></div>
      <details class="options"><summary>Compare discovery &amp; verification details</summary><div class="options-body">
        <label class="check"><input id="compare" type="checkbox" /> Compare with the official indexer</label>
        <p>This sends this demo wallet’s viewing key to the official discovery service. It sees the same transaction and block. No duplicate transaction is sent. Local discovery reuses any saved state; the reference starts without a supplied cursor. This comparison includes those cache conditions.</p>
        <div id="benchmark"></div><p id="verification">No state verified yet.</p>
        <p>Private transactions use the official hosted prover, which receives the proving inputs. Local storage is trusted in this demo. Checkpoint verification proves pool state at one block; it does not authenticate earlier write timestamps.</p>
      </div></details>
    </main><footer><span class="pipeline-chip">Public feed</span><span class="pipeline-arrow">→</span><span class="pipeline-chip">verified pool state</span><span class="pipeline-arrow">→</span><span class="pipeline-chip">private discovery in a Worker.</span></footer>`;
  element<HTMLSelectElement>("network").value = network;
  watchConnection();
  watchBalance("private-balance");
  watchBalance("public-balance");
  const toggle = element<HTMLButtonElement>("wallet-actions");
  const menu = element("action-menu");
  const choices = () => [...menu.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
  function positionMenu() {
    const rect = toggle.getBoundingClientRect();
    const width = Math.min(320, window.innerWidth - 32);
    menu.style.width = `${width}px`;
    menu.style.left = `${Math.max(16, Math.min(rect.right - width, window.innerWidth - width - 16))}px`;
    const height = menu.offsetHeight;
    menu.style.top = `${Math.max(16, rect.bottom + height + 12 <= window.innerHeight ? rect.bottom + 8 : rect.top - height - 8)}px`;
  }
  menu.addEventListener("toggle", () => {
    const open = menu.matches(":popover-open");
    toggle.setAttribute("aria-expanded", String(open));
    if (open) {
      positionMenu();
      (choices().find((button) => button.getAttribute("aria-checked") === "true") ?? choices()[0])?.focus();
    }
  });
  toggle.addEventListener("keydown", (event) => {
    if (["ArrowDown", "ArrowUp"].includes(event.key)) {
      event.preventDefault();
      menu.showPopover();
    }
  });
  menu.addEventListener("keydown", (event) => {
    const buttons = choices();
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === "ArrowDown" ? (index + 1) % buttons.length
      : event.key === "ArrowUp" ? (index + buttons.length - 1) % buttons.length
      : event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : undefined;
    if (next !== undefined) { event.preventDefault(); buttons[next]?.focus(); }
    if (event.key === "Tab") menu.hidePopover();
  });
  for (const event of ["resize", "scroll"]) window.addEventListener(event, () => {
    if (menu.matches(":popover-open")) positionMenu();
  }, { passive: true });
}
const observeText = (node: HTMLElement, apply: () => void) => {
  new MutationObserver(apply).observe(node, { childList: true, characterData: true, subtree: true });
  apply();
};
/** main.ts only writes text into #connection; the pill colour is derived from that text. */
function watchConnection(): void {
  const node = element("connection");
  const pill = node.parentElement;
  if (!pill) return;
  observeText(node, () => {
    const value = node.textContent ?? "";
    pill.dataset.state = value.startsWith("Block") ? "verified"
      : value === "Verification failed" ? "failed" : "syncing";
  });
}
/** Splits "0.01000 STRK" into an amount and a unit span; textContent stays identical for main.ts. */
function watchBalance(id: string): void {
  const node = element(id);
  observeText(node, () => {
    if (node.firstElementChild) return;
    const match = /^(\S+)( STRK)$/.exec(node.textContent ?? "");
    node.toggleAttribute("data-empty", !match);
    if (!match) return;
    const amount = document.createElement("span");
    amount.className = "amount";
    amount.textContent = match[1] ?? "";
    const unit = document.createElement("span");
    unit.className = "unit";
    unit.textContent = match[2] ?? "";
    node.replaceChildren(amount, unit);
  });
}
const startupStages: Record<StartupStage, [number, string, string]> = {
  engine: [0, "Loading discovery engine", "Preparing local discovery in your browser."],
  cache: [1, "Checking saved state", "Restoring a previous verified state when available."],
  feed: [2, "Reading public pool data", "Finding the current snapshot and recent updates."],
  checkpoint: [3, "Checking the blockchain checkpoint", "Getting the independent block header and state proof."],
  download: [4, "Loading pool state", "Downloading and unpacking public data. No viewing key is sent."],
  verify: [5, "Verifying pool state", "Checking the complete state against the blockchain checkpoint. The first visit can take tens of seconds."],
  save: [6, "Saving verified state", "Preparing the cache for your next visit."],
  ready: [7, "Preparing your wallet", "Pool state is ready. Restoring your notes and checking the public balance."],
};
export function renderStartup(progress: StartupProgress | undefined): void {
  element("startup").hidden = !progress;
  element("next").classList.toggle("initializing", !!progress);
  if (!progress) return;
  const [stage, label, detail] = startupStages[progress.stage];
  const fraction = progress.total ? (progress.completed ?? 0) / progress.total : 0;
  element("startup-label").textContent = label;
  element("startup-step").textContent = `Stage ${stage + 1} / 8`;
  const bar = element<HTMLProgressElement>("startup-progress");
  bar.value = stage + fraction;
  bar.setAttribute("aria-valuetext", label);
  [...element("startup-track").children].forEach((segment, i) => {
    segment.classList.toggle("done", i < stage);
    segment.classList.toggle("active", i === stage);
    (segment as HTMLElement).style.setProperty("--fill", i < stage ? "1" : i === stage ? String(fraction) : "0");
  });
  element("startup-detail").textContent = progress.stage === "download" && progress.total
    ? `${progress.completed ?? 0} / ${progress.total} data files loaded. ${detail}` : detail;
}
const escape = (value: string) =>
  value.replace(
    /[&<>"']/g,
    (char) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        char
      ]!,
  );
const seconds = (ms: number) => `${(ms / 1000).toFixed(2)} s`;
let ticker: number | undefined;
function stopTicker(): void {
  if (ticker !== undefined) window.clearInterval(ticker);
  ticker = undefined;
}
/** Running rows tick in place; the list is only re-rendered when an operation changes. */
function tick(): void {
  const clocks = document.querySelectorAll<HTMLElement>("#log time[data-started]");
  if (clocks.length === 0) return stopTicker();
  const now = performance.now();
  for (const clock of clocks) clock.textContent = seconds(now - Number(clock.dataset.started));
}
export function renderOperations(entries: Operation[]): void {
  const root = element<HTMLDivElement>("log");
  const open = new Set(
    [...root.querySelectorAll("details[open]")].map(
      (node) => (node as HTMLElement).dataset.id,
    ),
  );
  const longest = Math.max(0, ...entries.map((operation) => operation.elapsedMs ?? 0));
  let running = false;
  function row(operation: Operation, id: string, share?: number): string {
    const active = operation.elapsedMs === undefined;
    running ||= active;
    const state = operation.error ? "failed" : active ? "running" : "done";
    const detail = operation.detail?.startsWith("https://")
      ? `<a target="_blank" rel="noreferrer" href="${escape(operation.detail)}">View transaction ↗</a>`
      : escape(operation.detail ?? "");
    const clock = `<time${active ? ` data-started="${operation.started}"` : ""}>${seconds(operation.elapsedMs ?? performance.now() - operation.started)}</time>`;
    const bar = share === undefined ? "" : `<span class="op-bar" aria-hidden="true"><i style="width:${share.toFixed(1)}%"></i></span>`;
    return `<details class="operation ${state}" data-id="${id}" ${open.has(id) ? "open" : ""}><summary><span class="dot" aria-hidden="true"></span><span class="op-label">${escape(operation.label)}</span>${clock}${bar}</summary>
      <div class="operation-body">${detail}${operation.error ? `<p class="failure">${escape(operation.error)}</p>` : ""}${operation.children.map((child, i) => row(child, `${id}.${i}`)).join("")}</div></details>`;
  }
  element("activity-count").textContent = String(entries.length);
  const scroll = root.scrollTop;
  root.innerHTML = entries.map((operation, i) => row(operation, String(i),
    operation.elapsedMs === undefined || longest === 0 ? undefined : Math.max(2, (operation.elapsedMs / longest) * 100))).join("") || '<p class="empty">No operations yet.</p>';
  root.scrollTop = scroll;
  if (!running) stopTicker();
  else if (ticker === undefined)
    ticker = window.setInterval(tick, matchMedia("(prefers-reduced-motion: reduce)").matches ? 500 : 100);
}
export function downloadJson(value: unknown, name: string): void {
  const url = URL.createObjectURL(
    new Blob([JSON.stringify(value, null, 2)], { type: "application/json" }),
  );
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
