import type { Operation } from "./operations.ts";
import type { Network } from "./network.ts";

export const element = <T extends HTMLElement>(id: string) =>
  document.getElementById(id) as T;
export function mount(network: Network): void {
  element<HTMLDivElement>("app").innerHTML = `
    <header><a class="brand" href="https://github.com/kfastov/strk20-indexer" target="_blank" rel="noreferrer">STRK20 <span>/ local discovery</span></a>
      <select id="network" aria-label="Network"><option value="sepolia">Sepolia testnet</option><option value="mainnet">Starknet mainnet</option></select></header>
    <main><div class="intro"><span class="eyebrow">YOUR NOTES. YOUR BROWSER.</span><h1>Make a private transfer.<br>Discover it locally.</h1>
      <p>One real transaction flow, from funding to withdrawal.</p></div>
      <section class="activity" aria-label="Activity"><div class="section-title"><h2>Activity</h2><button id="metrics" class="text-button">Export timings</button></div><div id="log" aria-live="polite"></div></section>
      <section class="flow" aria-label="Transaction flow">
        <div class="flow-top"><span id="step">GET STARTED</span><span id="connection" class="status">Starting</span></div>
        <h2 id="title">Create a demo wallet</h2><p id="description"></p>
        <div id="wallet" hidden><label>Demo wallet address <button id="copy" class="text-button">Copy</button></label>
          <a id="address" class="address" target="_blank" rel="noreferrer"></a>
          <div class="balances"><div><span>Public</span><strong id="public-balance">—</strong></div><div><span>Private</span><strong id="private-balance">—</strong></div></div>
          <div class="backup-row"><button id="backup" class="text-button">Export wallet backup</button><span>Keys are stored in this browser. Use small amounts.</span></div>
        </div>
        <div id="amount-row" class="field" hidden><label for="amount">Amount · STRK</label><input id="amount" value="0.01" inputmode="decimal" autocomplete="off" /></div>
        <div id="recipient-row" class="field" hidden><label id="recipient-label" for="recipient">Recipient</label><input id="recipient" placeholder="0x…" autocomplete="off" spellcheck="false" /></div>
        <p id="error" role="alert" hidden></p><button id="next" class="primary"><span id="button-text">Create demo wallet</span><span class="spinner" aria-hidden="true"></span></button>
        <div class="secondary"><button id="import" class="text-button">Restore wallet backup</button><input id="backup-file" type="file" accept="application/json" hidden /></div>
      </section>
      <details class="options"><summary>Compare discovery &amp; verification details</summary>
        <label class="check"><input id="compare" type="checkbox" /> Compare with the official indexer</label>
        <p>This sends this demo wallet’s viewing key to the official discovery service. It sees the same transaction and block. No duplicate transaction is sent. Local discovery reuses any saved state; the reference starts without a supplied cursor. This comparison includes those cache conditions.</p>
        <div id="benchmark"></div><p id="verification">No state verified yet.</p>
        <p>Private transactions use the official hosted prover, which receives the proving inputs. Local storage is trusted in this demo. Checkpoint verification proves pool state at one block; it does not authenticate earlier write timestamps.</p>
      </details>
    </main><footer>Public feed → verified pool state → private discovery in a Worker.</footer>`;
  element<HTMLSelectElement>("network").value = network;
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
  root.innerHTML =
    entries.map((operation, i) => row(operation, String(i))).join("") ||
    '<p class="empty">Your actions will appear here.</p>';
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
