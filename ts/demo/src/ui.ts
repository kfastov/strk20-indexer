import type { Operation } from "./operations.ts";
import type { Network } from "./network.ts";
import type { StartupProgress, StartupStage } from "strk20-discovery";

export const element = <T extends HTMLElement>(id: string) =>
  document.getElementById(id) as T;
export function mount(network: Network): void {
  element<HTMLDivElement>("app").innerHTML = `
    <header><a class="brand" href="https://github.com/kfastov/strk20-indexer" target="_blank" rel="noreferrer">STRK20 <span>/ local discovery</span></a>
      <select id="network" aria-label="Network"><option value="sepolia">Sepolia testnet</option><option value="mainnet">Starknet mainnet</option></select></header>
    <main><div class="intro"><span class="eyebrow">YOUR NOTES. YOUR BROWSER.</span><h1>Private transfers.<br><span>Local discovery.</span></h1>
      <p>Deposit, transfer and withdraw with locally verified notes.</p></div>
      <div class="workspace"><section class="flow" aria-label="Transaction flow">
        <div class="flow-top"><span id="step">GET STARTED</span><span id="connection" class="status">Starting</span></div>
        <h2 id="title">Create a demo wallet</h2><p id="description"></p>
        <div id="wallet" hidden>
          <div class="balances"><div><span>Public balance</span><strong id="public-balance">—</strong></div><div><span>Private balance</span><strong id="private-balance">—</strong></div></div>
          <details id="wallet-details" class="wallet-details"><summary>Wallet address &amp; backup</summary>
            <div class="wallet-address-label"><span>Demo wallet address</span><button id="copy" class="text-button" aria-label="Copy wallet address">Copy</button></div>
            <a id="address" class="address" target="_blank" rel="noreferrer"></a>
            <div class="backup-row"><button id="backup" class="text-button">Export wallet backup</button><span>Keys are stored in this browser. Use small amounts.</span></div>
          </details>
        </div>
        <div id="amount-row" class="field" hidden><label for="amount">Amount · STRK</label><input id="amount" value="0.01" inputmode="decimal" autocomplete="off" /></div>
        <div id="recipient-row" class="field" hidden><label id="recipient-label" for="recipient">Recipient</label><input id="recipient" placeholder="0x…" autocomplete="off" spellcheck="false" /></div>
        <div id="startup" class="startup" role="status" hidden><div class="startup-title"><strong id="startup-label"></strong><span id="startup-step"></span></div><progress id="startup-progress" max="8" value="0" aria-label="Initialization stages"></progress><p id="startup-detail"></p></div>
        <p id="error" role="alert" hidden></p>
        <div class="action-control">
          <button id="next" class="primary"><span id="button-text">Create demo wallet</span><span class="spinner" aria-hidden="true"></span></button>
          <button id="wallet-actions" class="action-toggle" type="button" popovertarget="action-menu" aria-label="Choose operation" aria-haspopup="menu" aria-expanded="false" hidden><svg width="18" height="18" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="m6 9 6 6 6-6" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg></button>
          <div id="action-menu" popover="auto" role="menu" aria-label="Choose operation">
            <p class="menu-label">CHOOSE OPERATION</p>
            <button id="choose-shield" type="button" role="menuitemradio" aria-checked="false"><span>Deposit<small>Move STRK into the privacy pool</small></span></button>
            <button id="choose-transfer" type="button" role="menuitemradio" aria-checked="false"><span>Private transfer<small>Send within the privacy pool</small></span></button>
            <button id="choose-withdraw" type="button" role="menuitemradio" aria-checked="false"><span>Withdraw<small>Return STRK to a public address</small></span></button>
            <button id="choose-discover" type="button" role="menuitemradio" aria-checked="false"><span>Discover<small>Refresh your verified private balance</small></span></button>
          </div>
        </div>
        <div class="secondary"><button id="import" class="text-button">Restore wallet backup</button><input id="backup-file" type="file" accept="application/json" hidden /></div>
      </section>
      <aside class="activity" aria-label="Activity">
        <div class="section-title"><h2>Activity <span id="activity-count">0</span></h2><button id="metrics" class="text-button">Export timings ↗</button></div>
        <div id="activity-current" class="activity-current" role="status" aria-live="polite"><span class="activity-label">READY WHEN YOU ARE</span><strong>Your next step starts here</strong><p>Follow each operation as it happens.</p></div>
        <div id="log" tabindex="0" role="region" aria-label="Operation history"><p class="empty">No operations yet.</p></div>
        <div class="activity-foot">Latest first · select an operation for details</div>
      </aside></div>
      <details class="options rpc-options"><summary>RPC connection · <span id="rpc-status">Default public RPC</span></summary>
        <p>Use your QuickNode endpoint for this network. The URL is saved only in this browser. Leave it empty to use the default connection.</p>
        <div class="field"><label id="rpc-network" for="rpc-url">Starknet Sepolia HTTPS URL</label><input id="rpc-url" type="password" placeholder="https://…quiknode.pro/…" autocomplete="off" spellcheck="false" /></div>
        <button id="save-rpc" class="connection-save" type="button">Save &amp; reconnect</button>
      </details>
      <details class="options"><summary>Compare discovery &amp; verification details</summary>
        <label class="check"><input id="compare" type="checkbox" /> Compare with the official indexer</label>
        <p>This sends this demo wallet’s viewing key to the official discovery service. It sees the same transaction and block. No duplicate transaction is sent. Local discovery reuses any saved state; the reference starts without a supplied cursor. This comparison includes those cache conditions.</p>
        <div id="benchmark"></div><p id="verification">No state verified yet.</p>
        <p>Private transactions use the official hosted prover, which receives the proving inputs. Local storage is trusted in this demo. Checkpoint verification proves pool state at one block; it does not authenticate earlier write timestamps.</p>
      </details>
    </main><footer>Public feed → verified pool state → private discovery in a Worker.</footer>`;
  element<HTMLSelectElement>("network").value = network;
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
  element("startup-label").textContent = label;
  element("startup-step").textContent = `Stage ${stage + 1} / 8`;
  const bar = element<HTMLProgressElement>("startup-progress");
  bar.value = stage + (progress.total ? (progress.completed ?? 0) / progress.total : 0);
  bar.setAttribute("aria-valuetext", label);
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
export function renderOperations(entries: Operation[]): void {
  const root = element<HTMLDivElement>("log");
  const open = new Set(
    [...root.querySelectorAll("details[open]")].map(
      (node) => (node as HTMLElement).dataset.id,
    ),
  );
  function row(operation: Operation, id: string): string {
    const duration =
      operation.elapsedMs === undefined
        ? "In progress"
        : `${(operation.elapsedMs / 1000).toFixed(2)} s`;
    const detail = operation.detail?.startsWith("https://")
      ? `<a target="_blank" rel="noreferrer" href="${escape(operation.detail)}">View transaction ↗</a>`
      : escape(operation.detail ?? "");
    return `<details class="operation ${operation.error ? "failed" : ""}" data-id="${id}" ${open.has(id) ? "open" : ""}><summary><span class="dot ${operation.elapsedMs === undefined ? "running" : ""}"></span><span>${escape(operation.label)}</span><time>${duration}</time></summary>
      <div class="operation-body">${detail}${operation.error ? `<p class="failure">${escape(operation.error)}</p>` : ""}${operation.children.map((child, i) => row(child, `${id}.${i}`)).join("")}</div></details>`;
  }
  const latest = entries.at(-1);
  const panel = element("activity-current");
  if (latest) {
    let current = latest;
    while (current.elapsedMs === undefined) {
      const child = [...current.children].reverse().find((entry) => entry.elapsedMs === undefined);
      if (!child) break;
      current = child;
    }
    const running = latest.elapsedMs === undefined;
    const status = latest.error ? "NEEDS ATTENTION" : running ? "IN PROGRESS" : "COMPLETED";
    const detail = running
      ? (current === latest ? "Working on this step…" : latest.label)
      : `${(latest.elapsedMs! / 1000).toFixed(2)} s · ${latest.error ? "See the details below" : "Ready for your next operation"}`;
    panel.classList.toggle("is-running", running);
    panel.classList.toggle("is-failed", !!latest.error);
    const summary = `<span class="activity-label">${status}</span><strong>${escape(current.label)}</strong><p>${escape(detail)}</p>`;
    if (panel.innerHTML !== summary) panel.innerHTML = summary;
  }
  element("activity-count").textContent = String(entries.length);
  const scroll = root.scrollTop;
  root.innerHTML = entries.map((operation, i) => ({ operation, id: String(i) })).reverse()
    .map(({ operation, id }) => row(operation, id)).join("") || '<p class="empty">No operations yet.</p>';
  root.scrollTop = scroll;
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
