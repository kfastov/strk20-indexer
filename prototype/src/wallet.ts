/**
 * The write path — deliberately NOT part of the engine seam.
 *
 * strk20-indexer has no write path and is not getting one; docs/roadmap.md says
 * so under "Deferred, with triggers": signing, key custody and a prover are
 * exactly the surface the project exists to avoid. This module stands in for
 * the pieces someone else owns:
 *
 *   - the wallet's signer
 *   - the privacy SDK's `compile_actions` / proof construction
 *   - the HOSTED prover, which also mints the FPI deposit-screening attestation
 *     (a self-hosted prover cannot, so self-hosting can do everything but shield)
 *
 * Our product is the read half of every one of these writes: the SDK cannot
 * build a spend without knowing your notes, and that is what the engine supplies
 * keylessly. Splitting this out of `engine/` is how the file layout says that.
 */

import { ACTION_AMOUNT, type ActionKind, type SimulatedChain, type Submission } from './engine/chain';
import { Latency, sleep } from './engine/latency';
import type { Identity } from './engine/types';

export interface StepDef {
  readonly id: string;
  readonly label: string;
  /** What the log says while this step runs. */
  readonly pending: string;
  /** Who actually performs it, printed in the log so the boundary is visible. */
  readonly actor: 'wallet' | 'prover' | 'chain';
}

export interface ActionDef {
  readonly kind: ActionKind;
  readonly amount: number;
  readonly steps: readonly [StepDef, StepDef];
  /** What the wallet is waiting for once submitted. */
  readonly awaiting: 'note' | 'nullifier';
}

export const ACTIONS: Readonly<Record<ActionKind, ActionDef>> = {
  deposit: {
    kind: 'deposit',
    amount: ACTION_AMOUNT.deposit,
    steps: [
      {
        id: 'approve',
        label: 'approve',
        pending: 'approving',
        actor: 'wallet',
      },
      {
        id: 'shield',
        label: 'shield',
        pending: 'proving',
        actor: 'prover',
      },
    ],
    awaiting: 'note',
  },
  send: {
    kind: 'send',
    amount: ACTION_AMOUNT.send,
    steps: [
      { id: 'select', label: 'select', pending: 'selecting', actor: 'wallet' },
      { id: 'prove', label: 'prove', pending: 'proving', actor: 'prover' },
    ],
    awaiting: 'nullifier',
  },
  withdraw: {
    kind: 'withdraw',
    amount: ACTION_AMOUNT.withdraw,
    steps: [
      { id: 'select', label: 'select', pending: 'selecting', actor: 'wallet' },
      { id: 'unshield', label: 'unshield', pending: 'proving', actor: 'prover' },
    ],
    awaiting: 'nullifier',
  },
};

export interface StepOutcome {
  readonly detail: string;
  readonly submission?: Submission;
}

export interface Wallet {
  readonly name: string;
  readonly simulated: boolean;
  /** Runs one stage of a staged action. Step 1 of each action submits. */
  runStep(kind: ActionKind, step: 0 | 1, identity: Identity, signal?: AbortSignal): Promise<StepOutcome>;
}

export class MockWallet implements Wallet {
  readonly name = 'mock-wallet (stands in for the privacy SDK + hosted prover)';
  readonly simulated = true;

  private lat: Latency;

  constructor(private chain: SimulatedChain, seed: number) {
    this.lat = new Latency(seed ^ 0x2f19);
  }

  async runStep(
    kind: ActionKind,
    step: 0 | 1,
    identity: Identity,
    signal?: AbortSignal,
  ): Promise<StepOutcome> {
    const def = ACTIONS[kind];

    if (step === 0) {
      if (kind === 'deposit') {
        await sleep(this.lat.draw({ centre: 900, spread: 0.35, tailChance: 0.15 }), signal);
        return { detail: `allowance ${def.amount.toFixed(2)} STRK` };
      }
      await sleep(this.lat.draw({ centre: 180, spread: 0.5 }), signal);
      return { detail: 'note selected' };
    }

    // Step 2 is always the expensive one: a proof, then a submission.
    const proveMs = this.lat.draw({ centre: kind === 'deposit' ? 2_100 : 1_750, spread: 0.3, tailChance: 0.2 });
    await sleep(proveMs, signal);
    const submission = this.chain.submit(identity, kind);
    return { detail: `tx ${short(submission.txHash)}`, submission };
  }
}

function short(felt: string): string {
  return felt.length > 12 ? `${felt.slice(0, 8)}…${felt.slice(-4)}` : felt;
}
