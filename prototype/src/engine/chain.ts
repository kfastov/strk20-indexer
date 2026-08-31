/**
 * A simulated Starknet + a simulated feed cutter behind it.
 *
 * This is NOT part of the engine seam. It stands in for the chain, which in
 * reality is the one thing nobody in this project controls. The mock engine
 * reads from it; `src/wallet.ts` writes to it. The real system replaces this
 * whole file with "actual Starknet".
 *
 * Clock compression: mainnet blocks land roughly every 30 s. Waiting that long
 * makes the prototype useless to click through, so the head advances every ~6 s
 * here — about 5x fast. The feed panel says so out loud, because a
 * time-to-discovery number under a compressed clock is not a latency claim.
 */

import { Latency } from './latency';
import { HEAD_BLOCK, L1_ACCEPTED, SEED_NOTES, fakeFelt } from './fixtures';
import type { EngineEvent, Felt, Identity, Note, Unsubscribe } from './types';

export type ActionKind = 'deposit' | 'send' | 'withdraw';

export const ACTION_AMOUNT: Readonly<Record<ActionKind, number>> = {
  deposit: 100,
  send: 25,
  withdraw: 10,
};

/** Mutable note record. `Note` (the engine's output type) is the frozen view. */
interface ChainNote {
  id: Felt;
  token: string;
  amount: number;
  block: number;
  nullifier: Felt;
  spent: boolean;
}

interface Pending {
  identity: 'A' | 'B';
  kind: ActionKind;
  landsAt: number;
  apply: () => void;
}

export interface Submission {
  readonly txHash: Felt;
  readonly submittedAt: number;
  /** Simulated blocks-until-inclusion, purely for the log line. */
  readonly etaBlocks: number;
}

/** Mainnet-ish head cadence, compressed. Change these two to change the feel. */
const HEAD_TICK_MS = 6_000;
const HEAD_TICK_JITTER = 0.35;

export class SimulatedChain {
  private lat: Latency;
  private notes: Record<'A' | 'B', ChainNote[]>;
  private pending: Pending[] = [];
  private listeners = new Set<(ev: EngineEvent) => void>();
  private timer: number | null = null;
  private seq = 0;

  head = HEAD_BLOCK;
  l1Accepted = L1_ACCEPTED;

  constructor(seed: number) {
    this.lat = new Latency(seed ^ 0x9e37);
    this.notes = { A: [], B: [] };
    for (const id of ['A', 'B'] as const) {
      for (const seed of SEED_NOTES[id]) {
        this.notes[id].push(this.mint(id, seed.amount, seed.block));
      }
    }
  }

  // -------------------------------------------------------------------------
  // lifecycle
  // -------------------------------------------------------------------------

  start(): void {
    if (this.timer !== null) return;
    this.schedule();
  }

  stop(): void {
    if (this.timer !== null) window.clearTimeout(this.timer);
    this.timer = null;
  }

  private schedule(): void {
    const ms = this.lat.draw({ centre: HEAD_TICK_MS, spread: HEAD_TICK_JITTER });
    this.timer = window.setTimeout(() => {
      this.timer = null;
      this.tick();
      this.schedule();
    }, ms);
  }

  private tick(): void {
    this.head += 1;
    if (this.head - this.l1Accepted > 8_300) this.l1Accepted += 1;
    this.drainPending();
    this.emit({ type: 'head', head: this.head, l1Accepted: this.l1Accepted });
  }

  private drainPending(): void {
    const now = performance.now();
    const due = this.pending.filter((p) => p.landsAt <= now);
    if (due.length === 0) return;
    this.pending = this.pending.filter((p) => p.landsAt > now);
    for (const p of due) p.apply();
  }

  // -------------------------------------------------------------------------
  // pokes
  // -------------------------------------------------------------------------

  onEvent(handler: (ev: EngineEvent) => void): Unsubscribe {
    this.listeners.add(handler);
    return () => this.listeners.delete(handler);
  }

  private emit(ev: EngineEvent): void {
    for (const l of this.listeners) l(ev);
  }

  // -------------------------------------------------------------------------
  // reads (what the engine's discovery would compute)
  // -------------------------------------------------------------------------

  /** Chain truth for one key. Clearing LOCAL state must never touch this. */
  notesFor(identity: Identity): readonly Note[] {
    this.drainPending();
    return this.notes[identity.id].map(
      (n): Note => ({
        id: n.id,
        token: n.token,
        amount: n.amount,
        block: n.block,
        nullifier: n.nullifier,
        spent: n.spent,
      }),
    );
  }

  balanceOf(identity: Identity): number {
    return this.notesFor(identity)
      .filter((n) => !n.spent)
      .reduce((a, n) => a + n.amount, 0);
  }

  // -------------------------------------------------------------------------
  // writes (what the WALLET does — see src/wallet.ts; not our product)
  // -------------------------------------------------------------------------

  submit(identity: Identity, kind: ActionKind): Submission {
    const amount = ACTION_AMOUNT[kind];
    const landsIn = this.lat.draw({ centre: 3_400, spread: 0.45, tailChance: 0.18, tailFactor: 2.1 });
    const id = identity.id;

    const apply = () => {
      if (kind === 'deposit') {
        this.notes[id].push(this.mint(id, amount, this.head));
        return;
      }
      const unspent = this.notes[id]
        .filter((n) => !n.spent)
        .sort((a, b) => b.amount - a.amount);
      const source = unspent[0];
      if (!source || source.amount < amount) return; // gate should have prevented this
      source.spent = true;
      const change = Math.round((source.amount - amount) * 100) / 100;
      if (change > 0) this.notes[id].push(this.mint(id, change, this.head));
    };

    this.pending.push({ identity: id, kind, landsAt: performance.now() + landsIn, apply });

    return {
      txHash: fakeFelt(`tx:${id}:${kind}:${this.seq++}`),
      submittedAt: performance.now(),
      etaBlocks: 1 + Math.floor(this.lat.between(0, 2)),
    };
  }

  private mint(id: 'A' | 'B', amount: number, block: number): ChainNote {
    const n = this.seq++;
    return {
      id: fakeFelt(`note:${id}:${n}`, 62),
      token: 'STRK',
      amount,
      block,
      nullifier: fakeFelt(`null:${id}:${n}`, 62),
      spent: false,
    };
  }
}
